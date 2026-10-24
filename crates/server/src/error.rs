//! 统一错误类型与响应。
//!
//! 规则：不泄漏对象是否存在（404 与 403 同形）、不回显内部细节、不记正文/token。

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use cista_core::protocol::ApiError;

#[derive(Debug)]
pub enum AppError {
    Unauthorized,
    Forbidden,
    NotFound,
    BadRequest(&'static str),
    /// 已存在（设备入网重复、token 冲突）。
    AlreadyExists,
    /// 笔记 rev 冲突：返回服务端当前 rev，供客户端生成冲突副本。
    RevConflict {
        rev: u64,
    },
    /// 410：链接过期/撤销/次数用尽；工作区进入过渡区后公网不可达。
    Gone,
    PayloadTooLarge,
    /// 配额超限（实例/工作区/链接）。
    Quota(&'static str),
    /// 429：令牌桶耗尽，`Retry-After` 给出等待秒数。
    RateLimited {
        retry_after_secs: u64,
    },
    Internal(anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                Json(ApiError::new("unauthorized")),
            )
                .into_response(),
            Self::Forbidden => {
                (StatusCode::FORBIDDEN, Json(ApiError::new("forbidden"))).into_response()
            }
            Self::NotFound => {
                (StatusCode::NOT_FOUND, Json(ApiError::new("not_found"))).into_response()
            }
            Self::BadRequest(message) => {
                (StatusCode::BAD_REQUEST, Json(ApiError::new(message))).into_response()
            }
            Self::AlreadyExists => {
                (StatusCode::CONFLICT, Json(ApiError::new("already_exists"))).into_response()
            }
            Self::RevConflict { rev } => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "error": "rev_conflict", "rev": rev })),
            )
                .into_response(),
            Self::Gone => (StatusCode::GONE, Json(ApiError::new("gone"))).into_response(),
            Self::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(ApiError::new("payload_too_large")),
            )
                .into_response(),
            Self::Quota(scope) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(serde_json::json!({ "error": "quota_exceeded", "scope": scope })),
            )
                .into_response(),
            Self::RateLimited { retry_after_secs } => {
                let mut response = (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(ApiError::new("rate_limited")),
                )
                    .into_response();
                if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                    response.headers_mut().insert(header::RETRY_AFTER, value);
                }
                response
            }
            Self::Internal(err) => {
                tracing::error!(error = %err, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError::new("internal")),
                )
                    .into_response()
            }
        }
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self::Internal(err.into())
    }
}
