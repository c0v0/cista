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
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::io::AsyncWriteExt;

use crate::error::AppError;
use crate::state::{now, AppState};

/// 读取请求体的最长时间；超过就 408，避免慢速连接长期占用并发预算。
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(15);

/// 可信反代对端：回环、私有网段、链路本地（Docker 网桥、Caddy 同机）。
fn is_trusted_proxy(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// 客户端地址：仅在可信反代对端时采信 `X-Forwarded-For` 的**最后一段**
/// （反代追加的真实客户端；首段可被客户端伪造）。
pub fn client_ip(peer: Option<IpAddr>, headers: &HeaderMap, trust_proxy: bool) -> Option<String> {
    if trust_proxy && peer.map(is_trusted_proxy).unwrap_or(false) {
        if let Some(last) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|raw| raw.rsplit(',').next())
            .map(str::trim)
            .filter(|candidate| candidate.parse::<IpAddr>().is_ok())
        {
            return Some(last.to_string());
        }
    }
    peer.map(|ip| ip.to_string())
}

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

    let bytes = match tokio::time::timeout(BODY_READ_TIMEOUT, to_bytes(body, limit)).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            drop(permit);
            return Err(AppError::PayloadTooLarge);
        }
        Err(_) => {
            // 慢速请求体：释放 permit 并超时，避免 16 个连接饿死所有 /api 请求。
            drop(permit);
            return Err(AppError::RequestTimeout);
        }
    };

    // 连续失败会指数退避：伪造签名/重放不能无限重试。
    let now_ts = now();
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    let client = client_ip(peer, &parts.headers, state.cfg.trust_proxy)
        .unwrap_or_else(|| "unknown".to_string());
    let failure_key = failure_key(&client, &parts.headers);
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

/// 已落盘的私有附件（中间件写入，handler 负责移动到内容寻址路径）。
#[derive(Debug, Clone)]
pub struct SpooledUpload {
    pub path: PathBuf,
    pub hash: String,
    pub size: u64,
}

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// `/api/files` 专用：流式落盘 + 摘要验签，避免 100 MiB 附件整包进内存。
pub async fn require_signed_upload(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let limit = usize::try_from(state.cfg.max_file_bytes).unwrap_or(usize::MAX);
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
    let now_ts = now();
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    let client = client_ip(peer, &parts.headers, state.cfg.trust_proxy)
        .unwrap_or_else(|| "unknown".to_string());
    let failure_key = failure_key(&client, &parts.headers);
    if let Some(retry_after_secs) = state.failures.locked_for(&failure_key, now_ts) {
        return Err(AppError::RateLimited { retry_after_secs });
    }

    let permit = tokio::time::timeout(
        Duration::from_secs(2),
        state.upload_budget.clone().acquire_owned(),
    )
    .await
    .map_err(|_| AppError::RateLimited {
        retry_after_secs: 1,
    })?
    .map_err(|_| AppError::Internal(anyhow::anyhow!("upload budget closed")))?;

    let dir = state.data_dir().join("blobs").join("private");
    tokio::fs::create_dir_all(&dir).await?;
    let temp = dir.join(format!(".upload-{}.tmp", crate::ids::new_id("up")));
    let spooled = tokio::time::timeout(UPLOAD_TIMEOUT, spool_body(body, &temp, limit)).await;
    let (size, digest) = match spooled {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            let _ = tokio::fs::remove_file(&temp).await;
            drop(permit);
            return Err(error);
        }
        Err(_) => {
            let _ = tokio::fs::remove_file(&temp).await;
            drop(permit);
            return Err(AppError::RequestTimeout);
        }
    };
    drop(permit);

    let device = match verify_digest_headers(
        &state,
        &parts.headers,
        &parts.method,
        &parts.uri,
        digest,
        now_ts,
    )
    .await
    {
        Ok(device) => {
            state.failures.clear(&failure_key);
            device
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp).await;
            if matches!(error, AppError::Unauthorized) {
                state.failures.record(&failure_key, now_ts);
            }
            return Err(error);
        }
    };

    let mut request = Request::from_parts(parts, Body::empty());
    request.extensions_mut().insert(device);
    request.extensions_mut().insert(SpooledUpload {
        path: temp.clone(),
        hash: hex::encode(digest),
        size,
    });
    let response = next.run(request).await;
    // handler 成功时会把临时文件 rename 走；失败路径由这里兜底清理。
    let _ = tokio::fs::remove_file(&temp).await;
    Ok(response)
}

async fn spool_body(body: Body, path: &Path, limit: usize) -> Result<(u64, [u8; 32]), AppError> {
    let mut file = tokio::fs::File::create(path).await?;
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AppError::BadRequest("broken_body"))?;
        size = size.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if size > limit as u64 {
            return Err(AppError::PayloadTooLarge);
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    let digest: [u8; 32] = hasher.finalize().into();
    Ok((size, digest))
}

async fn verify_digest_headers(
    state: &AppState,
    headers: &HeaderMap,
    method: &Method,
    uri: &Uri,
    body_sha256: [u8; 32],
    current_time: i64,
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
    let meta = RequestMeta::with_body_sha256(
        method.as_str(),
        path_and_query,
        body_sha256,
        &nonce,
        timestamp,
    );
    auth::verify(&pubkey, &meta, signature, current_time).map_err(|_| AppError::Unauthorized)?;
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
