//! 私有 blob 上传（密文）。服务端按内容寻址，重复上传天然幂等。
//!
//! Phase 1 只做一次性上传；Phase 2 的分享上传改成流式 + 硬截断 + 配额预留。

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use cista_core::protocol::BlobRef;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::AppError;
use crate::state::{now, AppState};

/// `POST /api/files`
pub async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
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

    let size = u64::try_from(body.len()).unwrap_or(u64::MAX);
    if size == 0 {
        return Err(AppError::BadRequest("empty_blob"));
    }
    if size > state.cfg.max_file_bytes {
        return Err(AppError::PayloadTooLarge);
    }

    let hash = hex::encode(Sha256::digest(&body));
    let dir = state.data_dir().join("blobs").join("private");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(&hash);

    let existed = tokio::fs::try_exists(&path).await?;
    if existed {
        // 同内容已存在：不重复写盘、也不重复记账。
        tracing::debug!(hash = %hash, "blob already present");
    } else {
        let tmp = dir.join(format!(".{hash}.{}.tmp", crate::ids::new_id("tmp")));
        let mut file = tokio::fs::File::create(&tmp).await?;
        if let Err(error) = file.write_all(&body).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(error.into());
        }
        file.flush().await?;
        drop(file);
        if let Err(error) = tokio::fs::rename(&tmp, &path).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(error.into());
        }
    }

    // 与共享面共用同一份实例配额计数（shared.usage_counters.instance.total）；
    // 私有 blob 行仍在私有库里。两库不同事务，失败时补偿回滚。
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
    if inserted == 1 {
        let mut quota_tx = state.shared.pool().begin().await?;
        if let Err(error) =
            crate::quota::reserve_instance(&mut quota_tx, size, state.quotas()).await
        {
            drop(quota_tx);
            drop(tx);
            if !existed {
                let _ = tokio::fs::remove_file(&path).await;
            }
            return Err(error);
        }
        quota_tx.commit().await?;
    }
    tx.commit().await?;

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
