//! B+ 请求签名校验（服务端）。
//!
//! 校验顺序：头齐备 → 设备存在且未吊销 → 时间窗 → 验签 → nonce 防重放。
//! 失败一律 401，不区分原因，也不把失败细节写进响应。

use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, Uri};
use axum::middleware::Next;
use axum::response::Response;
use cista_core::auth::{self, RequestMeta};

use crate::error::AppError;
use crate::state::{now, AppState};

pub const H_DEVICE: &str = "x-device-id";
pub const H_NONCE: &str = "x-nonce";
pub const H_TIMESTAMP: &str = "x-timestamp";
pub const H_SIGNATURE: &str = "x-signature";

/// 签名校验需要缓冲请求体；笔记 5 MiB 上限之外留余量。
pub const MAX_SIGNED_BODY: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SignedDevice {
    pub id: String,
    #[allow(dead_code)] // Phase 2 校验分享对象归属时会用到
    pub pubkey: [u8; 32],
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// 常数时间（等长摘要）比较 bootstrap token，避免前缀时间差。
pub fn token_matches(expected: &str, provided: &str) -> bool {
    use sha2::{Digest, Sha256};
    Sha256::digest(expected.as_bytes()) == Sha256::digest(provided.as_bytes())
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
        .check_and_record(device_id, nonce_hex, current_time);
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
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, MAX_SIGNED_BODY)
        .await
        .map_err(|_| AppError::PayloadTooLarge)?;
    let device = verify_headers(&state, &parts.headers, &parts.method, &parts.uri, &bytes).await?;

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
