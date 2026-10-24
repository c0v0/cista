//! cista-server 库：路由装配 + 模块导出。
//!
//! 边界（`design.md` §2.3、§3.2）：
//! - `/healthz`：公网、无鉴权。
//! - `/api/devices/enroll`：bootstrap token 或已有设备签名（不走统一中间件）。
//! - 其余 `/api/*`：B+ 逐请求签名。
//! - `/public/*`：分享 token 只在 `Authorization: Share <token>` 头里（不进 URL）。
//! - `/ws`：首帧 JSON 握手，之后是 y-sync 标准帧。

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod gc;
pub mod ids;
pub mod public;
pub mod quota;
pub mod rate_limit;
pub mod state;
pub mod wire;
pub mod ws;
pub mod yjs;

use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::state::AppState;

/// 笔记请求体上限：5 MiB 密文 + 信封开销。
pub const MAX_NOTE_BODY: usize = 6 * 1024 * 1024;

pub fn app(state: AppState) -> Router {
    let signed: Router<AppState> = Router::new()
        .route("/api/notes", get(api::notes::list))
        .route(
            "/api/notes/{id}",
            get(api::notes::get)
                .put(api::notes::put)
                .delete(api::archive::delete_note),
        )
        .route("/api/devices", get(api::devices::list))
        .route("/api/devices/{id}/revoke", post(api::devices::revoke))
        .route("/api/files", post(api::files::upload))
        .route("/api/workspaces", post(api::workspaces::create))
        .route("/api/workspaces/{id}", get(api::workspaces::get))
        .route("/api/workspaces/{id}/restore", post(api::archive::restore))
        .route(
            "/api/workspaces/{id}/archive-complete",
            post(api::archive::archive_complete),
        )
        .route("/api/quarantine", get(api::archive::list_quarantine))
        .route("/api/share", get(api::share::list).post(api::share::create))
        .route("/api/share/{id}", delete(api::share::revoke))
        .layer(DefaultBodyLimit::max(MAX_NOTE_BODY))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_signed,
        ));

    let upload_limit = usize::try_from(state.cfg.max_file_bytes).unwrap_or(usize::MAX);

    // 注意：分享 token 永远不出现在路径/查询里，只走 header（不变量 2）。
    let public: Router<AppState> = Router::new()
        .route("/public/files/meta", get(public::files::meta))
        .route("/public/files/confirm", post(public::files::confirm))
        .route("/public/files/blob", get(public::files::blob))
        .route("/public/workspaces/meta", get(public::workspaces::meta))
        .route("/public/workspaces/files", get(public::workspaces::files))
        .route(
            "/public/workspaces/files/upload",
            post(public::workspaces::upload).layer(DefaultBodyLimit::max(upload_limit)),
        )
        .route(
            "/public/workspaces/files/{id}",
            patch(public::workspaces::rename).delete(public::workspaces::delete),
        )
        .route(
            "/public/workspaces/files/{id}/blob",
            get(public::workspaces::download),
        )
        .route("/public/workspaces/events", get(public::workspaces::events))
        .route("/ws", get(ws::handler));

    let router = Router::<AppState>::new()
        .route("/healthz", get(api::health::healthz))
        .route(
            "/api/devices/enroll",
            post(api::devices::enroll).layer(DefaultBodyLimit::max(64 * 1024)),
        )
        .merge(signed)
        .merge(public)
        .fallback(api::not_found)
        .with_state(state.clone());

    if let Some(origin) = state.cfg.cors_origin.clone() {
        match cors_layer(&origin) {
            Some(cors) => return router.layer(cors),
            None => tracing::warn!("CISTA_CORS_ORIGIN 不是合法 origin，已忽略"),
        }
    }
    router
}

/// 跨域部署（`design.md` §4.2）：只放行配置的前端 origin。
///
/// 凭据全部走显式请求头（B+ 签名 / Share token / 口令），不使用 cookie，
/// 所以不开 allow-credentials。
fn cors_layer(origin: &str) -> Option<CorsLayer> {
    use axum::http::{header, HeaderName, HeaderValue, Method};
    use std::time::Duration;

    let allowed = HeaderValue::from_str(origin).ok()?;
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::exact(allowed))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                header::IF_MATCH,
                HeaderName::from_static("idempotency-key"),
                HeaderName::from_static("x-share-passphrase"),
                HeaderName::from_static("x-file-name"),
                HeaderName::from_static("x-device-id"),
                HeaderName::from_static("x-nonce"),
                HeaderName::from_static("x-timestamp"),
                HeaderName::from_static("x-signature"),
            ])
            .expose_headers([header::RETRY_AFTER, header::ETAG])
            .max_age(Duration::from_secs(600)),
    )
}
