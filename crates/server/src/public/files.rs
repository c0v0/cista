//! 单文件链接：两步下载（confirm 签发一次性票据 → 凭票据下载成功时消费次数）。
//!
//! token 走 `Authorization: Share <token>` 头，不在 URL 里。

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use cista_core::protocol::{ConfirmResponse, PublicMeta};
use cista_core::share::{can_read_files, hash_token, token_fingerprint, ShareKind};

use crate::error::AppError;
use crate::public::{load_link, load_link_for_ticket, require_passphrase, serve_blob, ShareToken};
use crate::state::{now, AppState};

/// 票据有效期：足够断点重试，又不给链接泄漏无限窗口。
const TICKET_TTL_SECS: i64 = 45;

/// `GET /public/files/meta`：落地页信息，不计数。
pub async fn meta(
    State(state): State<AppState>,
    ShareToken(token): ShareToken,
) -> Result<Json<PublicMeta>, AppError> {
    let link = load_link(&state, &token, ShareKind::File).await?;
    if !can_read_files(link.file_scopes) {
        return Err(AppError::Forbidden);
    }
    Ok(Json(PublicMeta {
        kind: ShareKind::File,
        title: link.title,
        note_scopes: link.note_scopes,
        file_scopes: link.file_scopes,
        expires_at: link.expires_at,
        requires_passphrase: link.passphrase_hash.is_some(),
        max_uses: link.max_uses,
        used_count: link.used_count,
    }))
}

/// `POST /public/files/confirm`：原子消费一次使用次数并签发票据。
pub async fn confirm(
    State(state): State<AppState>,
    ShareToken(token): ShareToken,
    headers: HeaderMap,
) -> Result<Json<ConfirmResponse>, AppError> {
    let link = load_link(&state, &token, ShareKind::File).await?;
    if !can_read_files(link.file_scopes) {
        return Err(AppError::Forbidden);
    }
    require_passphrase(&state, &link, &headers).await?;

    let timestamp = now();
    let expires_at = timestamp + TICKET_TTL_SECS;

    // 次数在**下载成功**时消费（见 blob）；confirm 只检查链接仍可用，
    // 这样一次中断的下载不会白白烧掉一次性链接。

    let ticket = new_ticket()?;
    sqlx::query(
        "INSERT INTO blob_tickets (ticket_hash, link_id, file_id, blob_hash, expires_at, created_at)
         VALUES (?, ?, NULL, ?, ?, ?)",
    )
    .bind(hash_token(&ticket))
    .bind(&link.id)
    .bind(&link.object_id)
    .bind(expires_at)
    .bind(timestamp)
    .execute(state.shared.pool())
    .await?;

    // 每个链接最多保留 8 张未使用票据：重复 confirm 不会无限增长 DB。
    sqlx::query(
        "DELETE FROM blob_tickets
         WHERE link_id = ?
           AND ticket_hash NOT IN (
             SELECT ticket_hash FROM blob_tickets
             WHERE link_id = ? ORDER BY created_at DESC LIMIT 8
           )",
    )
    .bind(&link.id)
    .bind(&link.id)
    .execute(state.shared.pool())
    .await?;

    // 日志只允许出现 token 指纹，绝不出现明文 token。
    tracing::info!(
        link = %link.id,
        token = %token_fingerprint(&token),
        "file link confirmed"
    );

    Ok(Json(ConfirmResponse { ticket, expires_at }))
}

/// `GET /public/files/blob`（票据走 `X-Share-Ticket` 头，不进 URL/日志）
///
/// 注意：这里用 `load_link_for_ticket`——confirm 不再消费次数，次数在下载成功时消费。
pub async fn blob(
    State(state): State<AppState>,
    ShareToken(token): ShareToken,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let ticket = headers
        .get("x-share-ticket")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(AppError::Unauthorized)?;
    let link = load_link_for_ticket(&state, &token, ShareKind::File).await?;
    if !can_read_files(link.file_scopes) {
        return Err(AppError::Forbidden);
    }

    let row: Option<(String, i64, String)> = sqlx::query_as(
        "SELECT link_id, expires_at, blob_hash FROM blob_tickets WHERE ticket_hash = ?",
    )
    .bind(hash_token(ticket))
    .fetch_optional(state.shared.pool())
    .await?;
    let (ticket_link, ticket_expires, blob_hash) = row.ok_or(AppError::Unauthorized)?;
    if ticket_link != link.id {
        return Err(AppError::Unauthorized);
    }
    if ticket_expires <= now() {
        return Err(AppError::Gone);
    }
    // 先确认本体存在，避免一次瞬时缺失把一次性链接的次数烧掉。
    if !tokio::fs::try_exists(state.shared.blob_path(&blob_hash)).await? {
        return Err(AppError::Gone);
    }
    // 票据单次使用：并发下载只有一个请求能删掉这一行。
    let consumed = sqlx::query("DELETE FROM blob_tickets WHERE ticket_hash = ? AND expires_at > ?")
        .bind(hash_token(ticket))
        .bind(now())
        .execute(state.shared.pool())
        .await?
        .rows_affected();
    if consumed != 1 {
        return Err(AppError::Unauthorized);
    }
    // 真正下载成功才消费一次链接使用次数；条件不满足则 410。
    let used = sqlx::query(
        "UPDATE share_links SET used_count = used_count + 1
         WHERE id = ?
           AND revoked_at IS NULL
           AND (expires_at IS NULL OR expires_at > ?)
           AND (max_uses IS NULL OR used_count < max_uses)",
    )
    .bind(&link.id)
    .bind(now())
    .execute(state.shared.pool())
    .await?
    .rows_affected();
    if used != 1 {
        return Err(AppError::Gone);
    }

    let name = if link.title.is_empty() {
        "download".to_string()
    } else {
        link.title
    };
    serve_blob(&state, &blob_hash, &name).await
}

fn new_ticket() -> Result<String, AppError> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("rng unavailable")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
