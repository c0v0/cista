//! 私有 blob 上传（密文）。服务端按内容寻址，重复上传天然幂等。
//!
//! Phase 1 只做一次性上传；Phase 2 的分享上传改成流式 + 硬截断 + 配额预留。

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Extension;
use axum::Json;
use cista_core::protocol::BlobRef;

use crate::auth::SpooledUpload;
use crate::error::AppError;
use crate::state::{now, AppState};

/// `POST /api/files`
///
/// 中间件已把请求体流式写入临时文件并完成摘要验签；这里只做去重、配额与原子落位。
pub async fn upload(
    State(state): State<AppState>,
    Extension(spool): Extension<SpooledUpload>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<BlobRef>), AppError> {
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();
    if idempotency_key.is_empty() {
        return Err(AppError::BadRequest("missing_idempotency_key"));
    }

    let size = spool.size;
    if size == 0 {
        return Err(AppError::BadRequest("empty_blob"));
    }
    let hash = spool.hash.clone();
    let dir = state.data_dir().join("blobs").join("private");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(&hash);

    if tokio::fs::try_exists(&path).await? {
        // 同内容已存在：不重复写盘、也不重复记账。
        tracing::debug!(hash = %hash, "blob already present");
        return Ok((StatusCode::CREATED, Json(BlobRef { hash, size })));
    }

    // 私有 blob 行在私有库，实例配额在共享库；两库不同事务，失败时补偿。
    let mut tx = state.private.pool().begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO blobs (hash, scope, size, refcount, created_at)
         VALUES (?, 'private', ?, 1, ?)
         ON CONFLICT(hash, scope) DO NOTHING",
    )
    .bind(&hash)
    .bind(i64::try_from(size).unwrap_or(i64::MAX))
    .bind(now())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        // 并发同内容上传：另一个请求已记账，临时文件由中间件清理。
        return Ok((StatusCode::CREATED, Json(BlobRef { hash, size })));
    }

    let mut quota_tx = state.shared.pool().begin().await?;
    if let Err(error) = crate::quota::reserve_instance(&mut quota_tx, size, state.quotas()).await {
        drop(quota_tx);
        drop(tx);
        return Err(error);
    }
    if let Err(error) = tokio::fs::rename(&spool.path, &path).await {
        drop(quota_tx);
        drop(tx);
        return Err(error.into());
    }
    if let Err(error) = quota_tx.commit().await {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error.into());
    }
    if let Err(error) = tx.commit().await {
        // 私有行没提交成功：删掉本体并尽力回滚实例配额。
        let _ = tokio::fs::remove_file(&path).await;
        if let Ok(mut rollback) = state.shared.pool().begin().await {
            let _ = crate::quota::release_instance(&mut rollback, size).await;
            let _ = rollback.commit().await;
        }
        return Err(error.into());
    }

    Ok((StatusCode::CREATED, Json(BlobRef { hash, size })))
}

/// `GET /api/files/{hash}`：取回私有 blob（客户端解密）。按内容寻址，鉴权走 B+。
pub async fn download(
    State(state): State<AppState>,
    axum::extract::Path(hash): axum::extract::Path<String>,
) -> Result<axum::response::Response, AppError> {
    if hash.len() != 64 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(AppError::NotFound);
    }
    let known: Option<i64> =
        sqlx::query_scalar("SELECT size FROM blobs WHERE hash = ? AND scope = 'private' LIMIT 1")
            .bind(&hash)
            .fetch_optional(state.private.pool())
            .await?;
    if known.is_none() {
        return Err(AppError::NotFound);
    }
    let bytes = tokio::fs::read(state.data_dir().join("blobs").join("private").join(&hash))
        .await
        .map_err(|_| AppError::NotFound)?;
    use axum::response::IntoResponse;
    Ok((
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "application/octet-stream"),
            (axum::http::header::CONTENT_DISPOSITION, "attachment"),
        ],
        bytes,
    )
        .into_response())
}
