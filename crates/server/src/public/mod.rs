//! 公网分享路由的共用逻辑：token 提取、链接状态机、口令、文件响应头。
//!
//! 不变量（RULES/`design.md` §3.2）：分享 token 不进日志/query/Referer。
//! 因此路由路径里**没有 token**，一律从 `Authorization: Share <token>`
//! （或 `X-Share-Token`）头读取；链接本身用 fragment `#/s/<token>`、`#/w/<token>`。

pub mod files;
pub mod workspaces;

use argon2::password_hash::{PasswordHash, PasswordVerifier};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cista_core::share::{hash_token, is_valid_token_shape, ShareKind, MAX_PASSPHRASE_BYTES};
use std::path::Path;

use crate::error::AppError;
use crate::state::{now, AppState};

/// 头里携带的分享 token（明文只在这个值里，不进 URL）。
pub struct ShareToken(pub String);

impl FromRequestParts<AppState> for ShareToken {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let headers = &parts.headers;
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Share "))
            .map(|value| value.trim().to_string())
            .or_else(|| {
                headers
                    .get("x-share-token")
                    .and_then(|value| value.to_str().ok())
                    .map(|value| value.trim().to_string())
            })
            .filter(|value| !value.is_empty())
            .ok_or(AppError::Unauthorized)?;
        Ok(Self(token))
    }
}

/// 从链接行读出的授权上下文（不含明文 token）。
#[derive(Debug, Clone)]
pub struct Link {
    pub id: String,
    pub kind: ShareKind,
    pub object_id: String,
    pub title: String,
    pub note_scopes: u32,
    pub file_scopes: u32,
    pub expires_at: Option<i64>,
    pub max_uses: Option<u32>,
    pub used_count: u32,
    pub passphrase_hash: Option<String>,
    pub revoked_at: Option<i64>,
}

type LinkRow = (
    String,
    String,
    String,
    String,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    i64,
    Option<String>,
    Option<i64>,
);

async fn fetch_link(state: &AppState, token: &str, expected: ShareKind) -> Result<Link, AppError> {
    if !is_valid_token_shape(token) {
        return Err(AppError::NotFound);
    }
    let token_hash = hash_token(token);
    let row: Option<LinkRow> = sqlx::query_as(
        "SELECT id, kind, object_id, title, note_scopes, file_scopes,
                expires_at, max_uses, used_count, passphrase_hash, revoked_at
         FROM share_links WHERE token_hash = ?",
    )
    .bind(&token_hash)
    .fetch_optional(state.shared.pool())
    .await?;

    let (
        id,
        kind,
        object_id,
        title,
        note_scopes,
        file_scopes,
        expires_at,
        max_uses,
        used_count,
        passphrase_hash,
        revoked_at,
    ) = row.ok_or(AppError::NotFound)?;

    let kind = match kind.as_str() {
        "file" => ShareKind::File,
        "workspace" => ShareKind::Workspace,
        _ => return Err(AppError::NotFound),
    };
    if kind != expected {
        return Err(AppError::NotFound);
    }

    Ok(Link {
        id,
        kind,
        object_id,
        title,
        note_scopes: u32::try_from(note_scopes).unwrap_or(0),
        file_scopes: u32::try_from(file_scopes).unwrap_or(0),
        expires_at,
        max_uses: max_uses.and_then(|value| u32::try_from(value).ok()),
        used_count: u32::try_from(used_count).unwrap_or(0),
        passphrase_hash,
        revoked_at,
    })
}

fn ensure_link_usable(link: &Link, check_uses: bool) -> Result<(), AppError> {
    if link.revoked_at.is_some() {
        return Err(AppError::Gone);
    }
    if let Some(expires_at) = link.expires_at {
        if expires_at <= now() {
            return Err(AppError::Gone);
        }
    }
    if check_uses {
        if let Some(max_uses) = link.max_uses {
            if link.used_count >= max_uses {
                return Err(AppError::Gone);
            }
        }
    }
    Ok(())
}

/// 每个链接一个桶（`design.md` §2.3）：公网没有可信设备身份，key 用链接 id。
///
/// 只在 token 命中后才建桶；乱猜 token 不会撑大 map。
fn limit_link(state: &AppState, link: &Link) -> Result<(), AppError> {
    state
        .rate
        .check(&format!("share:{}", link.id), now())
        .map_err(|limited| AppError::RateLimited {
            retry_after_secs: limited.retry_after_secs,
        })
}

/// 解析并校验 token：非法/不存在统一 404；撤销/过期/用尽统一 410。
pub async fn load_link(
    state: &AppState,
    token: &str,
    expected: ShareKind,
) -> Result<Link, AppError> {
    let link = fetch_link(state, token, expected).await?;
    limit_link(state, &link)?;
    ensure_link_usable(&link, true)?;
    Ok(link)
}

/// 票据下载用：允许「次数已用尽」，但仍然拒绝撤销/过期。
///
/// 原因：`confirm` 已经原子消费了唯一一次使用；随后凭票据下载不能再被用量检查拦住
/// （`design.md` §3.2「返回 30–60 秒下载票据；断点重试在窗口内」）。
pub async fn load_link_for_ticket(
    state: &AppState,
    token: &str,
    expected: ShareKind,
) -> Result<Link, AppError> {
    let link = fetch_link(state, token, expected).await?;
    limit_link(state, &link)?;
    ensure_link_usable(&link, false)?;
    Ok(link)
}

/// 长连接（WS）周期性复核：不消耗限速令牌，只检查撤销/过期/次数。
pub async fn revalidate_link(
    state: &AppState,
    token: &str,
    expected: ShareKind,
) -> Result<Link, AppError> {
    let link = fetch_link(state, token, expected).await?;
    ensure_link_usable(&link, true)?;
    Ok(link)
}

/// 非 Latin-1 口令/文件名走 `*-b64` 头；标准 base64 解码为 UTF-8。
pub fn decode_b64_header(value: &str) -> Option<String> {
    STANDARD
        .decode(value.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// 有口令的链接：需要 `X-Share-Passphrase` 头。
pub async fn require_passphrase(
    state: &AppState,
    link: &Link,
    headers: &HeaderMap,
) -> Result<(), AppError> {
    let provided = headers
        .get("x-share-passphrase-b64")
        .and_then(|value| value.to_str().ok())
        .and_then(decode_b64_header)
        .or_else(|| {
            headers
                .get("x-share-passphrase")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        });
    require_passphrase_value(state, link, provided.as_deref()).await
}

/// 供 WS 握手持有的口令字符串复用；Argon2 在 blocking 线程池执行并限制并发。
pub async fn require_passphrase_value(
    state: &AppState,
    link: &Link,
    provided: Option<&str>,
) -> Result<(), AppError> {
    let Some(required_hash) = link.passphrase_hash.as_deref() else {
        return Ok(());
    };
    let provided = provided.unwrap_or("");
    if provided.is_empty() || provided.len() > MAX_PASSPHRASE_BYTES {
        return Err(AppError::Unauthorized);
    }
    let hash = required_hash.to_string();
    let provided = provided.to_string();
    let permit = state
        .hash_budget
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::Internal(anyhow::anyhow!("hash budget closed")))?;
    let verified = tokio::task::spawn_blocking(move || verify_passphrase(&hash, &provided))
        .await
        .map_err(|_| AppError::Internal(anyhow::anyhow!("password hashing task failed")))?;
    drop(permit);
    if verified {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

pub fn verify_passphrase(stored_hash: &str, passphrase: &str) -> bool {
    match PasswordHash::new(stored_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(passphrase.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// 分享文件的固定响应头：永不内联、禁嗅探、禁脚本。
pub fn download_headers(name: &str, mime: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    let safe_name = name.replace(['"', '\\', '\r', '\n'], "_");
    let disposition = format!("attachment; filename=\"{safe_name}\"");
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    headers
}

/// 读取共享 blob 并构造下载响应（永远 `attachment`）。
pub async fn serve_blob(
    state: &AppState,
    blob_hash: &str,
    name: &str,
) -> Result<Response, AppError> {
    let path = state.shared.blob_path(blob_hash);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let headers = download_headers(name, "application/octet-stream");
    let body = blob_body(file, state.cfg.static_key).await?;
    Ok((StatusCode::OK, headers, body).into_response())
}

/// 流式下载：每次只解密/转发一个 ≤1 MiB 的块，内存占用与文件大小无关。
async fn blob_body(
    file: tokio::fs::File,
    key: Option<[u8; 32]>,
) -> Result<axum::body::Body, AppError> {
    use cista_core::crypto::NONCE_LEN;
    use futures_util::stream::unfold;
    use tokio::io::{AsyncReadExt, BufReader};

    const PLAIN_CHUNK: usize = 256 * 1024;
    let mut reader = BufReader::new(file);
    if key.is_some() {
        let mut magic = [0u8; 4];
        if reader.read_exact(&mut magic).await.is_err() || &magic != crate::blob_crypto::MAGIC {
            return Err(AppError::Internal(anyhow::anyhow!(
                "shared blob is not encrypted (wrong CISTA_STATIC_KEY?)"
            )));
        }
    }
    let stream = unfold((reader, key, 0u32), |(mut reader, key, index)| async move {
        if key.is_none() {
            // 无静态密钥（仅测试）：直接按块转发。
            let mut buf = vec![0u8; PLAIN_CHUNK];
            return match reader.read(&mut buf).await {
                Ok(0) | Err(_) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok(buf), (reader, key, index)))
                }
            };
        }
        let key = key.expect("checked above");
        let mut head = [0u8; NONCE_LEN + 4];
        if reader.read_exact(&mut head).await.is_err() {
            return None;
        }
        let nonce = head[..NONCE_LEN].to_vec();
        let ct_len = u32::from_be_bytes(
            head[NONCE_LEN..NONCE_LEN + 4]
                .try_into()
                .unwrap_or_default(),
        );
        let ct_len = usize::try_from(ct_len).unwrap_or(usize::MAX);
        if ct_len > crate::blob_crypto::CHUNK + 16 + crate::blob_crypto::CHUNK {
            return Some((
                Err(std::io::Error::other("blob chunk too large")),
                (reader, Some(key), index),
            ));
        }
        let mut ciphertext = vec![0u8; ct_len];
        if reader.read_exact(&mut ciphertext).await.is_err() {
            return Some((
                Err(std::io::Error::other("blob frame truncated")),
                (reader, Some(key), index),
            ));
        }
        match crate::blob_crypto::open_frame(&key, index, &nonce, &ciphertext) {
            Ok(plaintext) => Some((Ok(plaintext), (reader, Some(key), index + 1))),
            Err(_) => Some((
                Err(std::io::Error::other("blob decrypt failed")),
                (reader, Some(key), index),
            )),
        }
    });
    Ok(axum::body::Body::from_stream(stream))
}

/// 共享库里的工作区行（公网只需要状态与标题）。
pub struct WorkspaceRow {
    pub title: String,
    pub state: String,
    pub expires_at: Option<i64>,
}

pub async fn load_workspace(
    state: &AppState,
    workspace_id: &str,
) -> Result<WorkspaceRow, AppError> {
    let row: Option<(String, String, Option<i64>)> =
        sqlx::query_as("SELECT title, state, expires_at FROM workspaces WHERE id = ?")
            .bind(workspace_id)
            .fetch_optional(state.shared.pool())
            .await?;
    let (title, state, expires_at) = row.ok_or(AppError::NotFound)?;
    Ok(WorkspaceRow {
        title,
        state,
        expires_at,
    })
}

/// 公网只允许 `active`；其余状态按不可达处理以免泄漏过渡区。
pub fn ensure_public(workspace: &WorkspaceRow) -> Result<(), AppError> {
    if workspace.state != "active" {
        return Err(AppError::Gone);
    }
    // 到期时间不依赖 gc：即使 timer 没跑，公网访问也立即 410。
    if let Some(expires_at) = workspace.expires_at {
        if expires_at <= crate::state::now() {
            return Err(AppError::Gone);
        }
    }
    Ok(())
}

/// 共享库里维护 blob 文件本体：不存在才写，内容寻址。
///
/// 配了 `CISTA_STATIC_KEY` 时落盘为密文（见 `blob_crypto`）；否则明文（仅开发/测试）。
pub async fn store_shared_blob(state: &AppState, hash: &str, bytes: &[u8]) -> Result<(), AppError> {
    let path = state.shared.blob_path(hash);
    if tokio::fs::try_exists(&path).await? {
        return Ok(());
    }
    let payload = match &state.cfg.static_key {
        Some(key) => crate::blob_crypto::seal_blob(key, bytes)?,
        None => bytes.to_vec(),
    };
    // 固定 `.tmp` 会让并发写同一 hash 互相踩踏；用随机后缀。
    let tmp = path.with_extension(format!("{}.tmp", crate::ids::new_id("tmp")));
    write_tmp(&tmp, &payload).await?;
    if let Err(error) = tokio::fs::rename(&tmp, &path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(error.into());
    }
    Ok(())
}

async fn write_tmp(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(path).await?;
    file.write_all(bytes).await?;
    file.flush().await?;
    Ok(())
}
