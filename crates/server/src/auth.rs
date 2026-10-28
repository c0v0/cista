//! B+ 请求签名校验（服务端）。
//!
//! 校验顺序：头齐备 → 设备存在且未吊销 → 时间窗 → 验签 → nonce 防重放。
//! 失败一律 401，不区分原因，也不把失败细节写进响应。

use axum::body::{to_bytes, Body};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Method, Uri};
use axum::middleware::Next;
use axum::response::Response;
use cista_core::auth::{self, RequestMeta};
use std::net::SocketAddr;
use subtle::ConstantTimeEq;

use crate::error::AppError;
use crate::state::{now, AppState};

pub const H_DEVICE: &str = "x-device-id";
pub const H_NONCE: &str = "x-nonce";
pub const H_TIMESTAMP: &str = "x-timestamp";
pub const H_SIGNATURE: &str = "x-signature";

/// 已认证请求体的默认上限（笔记加密信封的上限）。
/// 更小的路由用 [`request_body_limit`] 收紧；这样认证前不会无条件缓冲 8 MiB。
pub const MAX_SIGNED_BODY: usize = crate::MAX_NOTE_BODY;

/// 按路径给出认证前允许缓冲的最大请求体。
pub fn request_body_limit(path: &str) -> usize {
    match path {
        "/api/devices/enroll" => 64 * 1024,
        "/api/notes" | "/api/workspaces" => MAX_SIGNED_BODY,
        _ => MAX_SIGNED_BODY,
    }
}

#[derive(Debug, Clone)]
pub struct SignedDevice {
    pub id: String,
    #[allow(dead_code)] // Phase 2 校验分享对象归属时会用到
    pub pubkey: [u8; 32],
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// 常数时间比较两个等长摘要，避免 token 校验的前缀时间差。
pub fn token_matches(expected: &str, provided: &str) -> bool {
    use sha2::{Digest, Sha256};
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(provided.as_bytes());
    bool::from(a.ct_eq(&b))
}

/// 校验签名头；成功返回设备身份。
pub async fn verify_headers(
    state: &AppState,
    headers: &HeaderMap,
    method: &Method,
    uri: &Uri,
    body: &[u8],
) -> Result<SignedDevice, AppError> {
    let device_id = header(headers, H_DEVICE).ok_or(AppError::Unauthorized)?;
    let nonce_hex = header(headers, H_NONCE).ok_or(AppError::Unauthorized)?;
    let timestamp = header(headers, H_TIMESTAMP)
        .and_then(|raw| raw.parse::<i64>().ok())
        .ok_or(AppError::Unauthorized)?;
    let signature = header(headers, H_SIGNATURE).ok_or(AppError::Unauthorized)?;

    let nonce = hex::decode(nonce_hex).map_err(|_| AppError::Unauthorized)?;
    auth::validate_nonce(&nonce).map_err(|_| AppError::Unauthorized)?;

    let row: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT pubkey FROM devices WHERE id = ? AND revoked_at IS NULL")
            .bind(device_id)
            .fetch_optional(state.private.pool())
            .await?;
    let (pubkey_bytes,) = row.ok_or(AppError::Unauthorized)?;
    let pubkey: [u8; 32] = pubkey_bytes
        .as_slice()
        .try_into()
        .map_err(|_| AppError::Unauthorized)?;

    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or_else(|| uri.path());
    let meta = RequestMeta::new(method.as_str(), path_and_query, body, &nonce, timestamp);
    let current_time = now();
    auth::verify(&pubkey, &meta, signature, current_time).map_err(|_| AppError::Unauthorized)?;

    // 验签通过后才消费 nonce：伪造请求不能占用缓存条目。
    let fresh = state
        .replay
        .lock()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("replay cache poisoned")))?
        .check_and_record(device_id, &nonce, current_time);
    if !fresh {
        return Err(AppError::Unauthorized);
    }

    Ok(SignedDevice {
        id: device_id.to_string(),
        pubkey,
    })
}

/// 中间件：所有 `/api/*` 所有者端点都必须过这一层。
pub async fn require_signed(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    // 认证前先做便宜的检查，避免为超大/无凭据请求分配内存。
    let limit = request_body_limit(request.uri().path());
    if let Some(len) = request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.parse::<usize>().ok())
    {
        if len > limit {
            return Err(AppError::PayloadTooLarge);
        }
    }
    if header(request.headers(), H_DEVICE).is_none()
        || header(request.headers(), H_NONCE).is_none()
        || header(request.headers(), H_SIGNATURE).is_none()
        || header(request.headers(), H_TIMESTAMP).is_none()
    {
        return Err(AppError::Unauthorized);
    }

    let (parts, body) = request.into_parts();

    // 全局并发预算：每次认证最多缓冲 `limit` 字节，15 个并发就已经接近内存上限。
    let permit = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.body_budget.clone().acquire_owned(),
    )
    .await
    .map_err(|_| AppError::RateLimited {
        retry_after_secs: 1,
    })?
    .map_err(|_| AppError::Internal(anyhow::anyhow!("body budget closed")))?;

    let bytes = match to_bytes(body, limit).await {
        Ok(bytes) => bytes,
        Err(_) => {
            drop(permit);
            return Err(AppError::PayloadTooLarge);
        }
    };

    // 连续失败会指数退避：伪造签名/重放不能无限重试。
    let now_ts = now();
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let failure_key = failure_key(&peer, &parts.headers);
    if let Some(retry_after_secs) = state.failures.locked_for(&failure_key, now_ts) {
        return Err(AppError::RateLimited { retry_after_secs });
    }
    let device =
        match verify_headers(&state, &parts.headers, &parts.method, &parts.uri, &bytes).await {
            Ok(device) => {
                state.failures.clear(&failure_key);
                device
            }
            Err(error) => {
                state.failures.record(&failure_key, now_ts);
                return Err(error);
            }
        };
    drop(permit);

    // 验签通过后按设备限速：伪造设备 id 的请求进不了桶。
    state
        .rate
        .check(&format!("dev:{}", device.id), now())
        .map_err(|limited| AppError::RateLimited {
            retry_after_secs: limited.retry_after_secs,
        })?;

    let mut request = Request::from_parts(parts, Body::from(bytes));
    request.extensions_mut().insert(device);
    Ok(next.run(request).await)
}

/// 失败计数的 key：`(peer, 声称的设备 id)`。
///
/// 只看 `x-device-id` 会让知道某个设备 id 的人用伪造请求把该设备锁死；
/// 加上对端地址后，攻击者只能锁住自己。
fn failure_key(peer: &str, headers: &HeaderMap) -> String {
    let claimed = headers
        .get(H_DEVICE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 64 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
        .unwrap_or("anonymous");
    format!("{peer}:{claimed}")
}
