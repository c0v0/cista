//! 设备入网与吊销（B+）。
//!
//! - 首台设备：`Authorization: Bootstrap <token>`（一次性，用完请从环境删除）。
//! - 新设备：由已入网设备做 B+ 签名；服务端只存公钥。
//! - 吊销：删除 `revoked_at IS NULL` 条件后立即 401。

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use cista_core::auth;
use cista_core::protocol::{DeviceInfo, EnrollRequest, EnrollResponse};

use crate::auth::SignedDevice;
use crate::error::AppError;
use crate::state::{now, AppState};

/// `POST /api/devices/enroll`
pub async fn enroll(
    State(state): State<AppState>,
    headers: HeaderMap,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let request: EnrollRequest =
        serde_json::from_slice(&body).map_err(|_| AppError::BadRequest("invalid_body"))?;
    if request.name.trim().is_empty() || request.name.len() > 64 {
        return Err(AppError::BadRequest("invalid_name"));
    }
    let pubkey = decode_pubkey(&request.pubkey)?;
    let device_id = auth::device_id(&pubkey);

    // 认证：bootstrap token（仅首台设备）或已有设备签名（新设备）。
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    let mut bootstrap = false;
    let now_ts = now();
    let failure_key = enroll_failure_key(&state, &headers);
    if let Some(retry_after_secs) = state.failures.locked_for(&failure_key, now_ts) {
        return Err(AppError::RateLimited { retry_after_secs });
    }
    let auth_result: Result<(), AppError> =
        if let Some(token) = authorization.strip_prefix("Bootstrap ") {
            match state.cfg.bootstrap_token.as_deref() {
                Some(expected) if crate::auth::token_matches(expected, token.trim()) => {
                    bootstrap = true;
                    Ok(())
                }
                _ => Err(AppError::Unauthorized),
            }
        } else {
            crate::auth::verify_headers(&state, &headers, &method, &uri, &body)
                .await
                .map(|_| ())
        };
    if let Err(error) = auth_result {
        if matches!(error, AppError::Unauthorized) {
            state.failures.record(&failure_key, now_ts);
        }
        return Err(error);
    }
    state.failures.clear(&failure_key);

    let mut tx = state.private.pool().begin().await?;
    if bootstrap {
        // 一次性消费：并发首装也只有一个能抢到这一行。
        let claimed = sqlx::query(
            "INSERT INTO meta (key, value) VALUES ('bootstrap_used', '1')
             ON CONFLICT(key) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if claimed == 0 {
            return Err(AppError::Unauthorized);
        }
    }

    let inserted = sqlx::query(
        "INSERT INTO devices (id, name, pubkey, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(&device_id)
    .bind(request.name.trim())
    .bind(pubkey.to_vec())
    .bind(now())
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if inserted == 0 {
        return Err(AppError::AlreadyExists);
    }
    tx.commit().await?;

    tracing::info!(device = %device_id, "device enrolled");
    Ok((StatusCode::CREATED, Json(EnrollResponse { device_id })))
}

/// `POST /api/devices/{id}/revoke`
pub async fn revoke(
    State(state): State<AppState>,
    Extension(device): Extension<SignedDevice>,
    Path(target): Path<String>,
) -> Result<StatusCode, AppError> {
    if target == device.id {
        return Err(AppError::Forbidden);
    }
    let affected =
        sqlx::query("UPDATE devices SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(now())
            .bind(&target)
            .execute(state.private.pool())
            .await?
            .rows_affected();

    if affected == 0 {
        return Err(AppError::NotFound);
    }
    tracing::info!(device = %target, by = %device.id, "device revoked");
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/devices`（Phase 1 只用于运维核查；无管理后台）
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<DeviceInfo>>, AppError> {
    let rows: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, name, created_at, revoked_at FROM devices ORDER BY created_at ASC",
    )
    .fetch_all(state.private.pool())
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, name, created_at, revoked_at)| DeviceInfo {
                id,
                name,
                created_at,
                revoked_at,
            })
            .collect(),
    ))
}

fn decode_pubkey(raw: &str) -> Result<[u8; 32], AppError> {
    let bytes = hex::decode(raw.trim()).map_err(|_| AppError::BadRequest("invalid_pubkey"))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| AppError::BadRequest("invalid_pubkey"))
}

/// 入网失败退避的 key：反代部署用客户端地址，直连时共用一个桶。
fn enroll_failure_key(state: &AppState, headers: &HeaderMap) -> String {
    let client = if state.cfg.trust_proxy {
        headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|raw| raw.split(',').next())
            .map(str::trim)
            .filter(|candidate| candidate.parse::<std::net::IpAddr>().is_ok())
            .unwrap_or("unknown")
    } else {
        "direct"
    };
    format!("enroll:{client}")
}
