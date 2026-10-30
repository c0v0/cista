//! `/healthz`：供 systemd/Docker healthcheck 与外部 uptime 使用。
//!
//! 不读库、不鉴权、不返回版本/路径等内部信息（`docs/specs/smoke-healthz.md`）。

use axum::http::StatusCode;

pub async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}
