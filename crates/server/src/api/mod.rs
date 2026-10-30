//! HTTP API 模块。

pub mod archive;
pub mod devices;
pub mod files;
pub mod health;
pub mod notes;
pub mod share;
pub mod workspaces;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cista_core::protocol::ApiError;

/// 未定义路径：404，不泄漏内部信息。
pub async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(ApiError::new("not_found"))).into_response()
}
