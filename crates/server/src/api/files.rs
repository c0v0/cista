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

    if tokio::fs::try_exists(&path).await? {
        // 同内容已存在：不重复计 refcount。
        tracing::debug!(hash = %hash, "blob already present");
    } else {
        let tmp = dir.join(format!(".{hash}.tmp"));
        let mut file = tokio::fs::File::create(&tmp).await?;
        file.write_all(&body).await?;
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&tmp, &path).await?;
    }

    sqlx::query(
        "INSERT INTO blobs (hash, scope, size, refcount, created_at)
         VALUES (?, 'private', ?, 1, ?)
         ON CONFLICT(hash, scope) DO NOTHING",
    )
    .bind(&hash)
    .bind(i64::try_from(size).unwrap_or(i64::MAX))
    .bind(now())
    .execute(state.private.pool())
    .await?;

    Ok((StatusCode::CREATED, Json(BlobRef { hash, size })))
}
