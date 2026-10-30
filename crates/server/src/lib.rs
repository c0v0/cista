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
pub mod blob_crypto;
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

use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{header, HeaderValue};
use axum::middleware;
use axum::response::Response;
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
        .route("/api/files/{hash}", get(api::files::download))
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

    // 私有附件走流式中间件：大文件不整包进内存，验签用流式摘要。
    let uploads: Router<AppState> = Router::new()
        .route("/api/files", post(api::files::upload))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_signed_upload,
        ));

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
        .merge(uploads)
        .merge(signed)
        .merge(public)
        .fallback(api::not_found)
        .with_state(state.clone());

    // 每 IP 桶在最外层（CORS 之下）：未认证洪泛也会被限速。
    let router = router.layer(middleware::from_fn_with_state(state.clone(), ip_limit));
    // 所有 API/分享响应都可能含元数据或凭据派生内容：禁止浏览器/代理缓存。
    // 否则同一浏览器打开第二个分享链接时可能命中第一个链接的缓存响应。
    let router = router.layer(middleware::from_fn(no_store));

    if let Some(origin) = state.cfg.cors_origin.clone() {
        match cors_layer(&origin) {
            Some(cors) => return router.layer(cors),
            None => tracing::warn!("CISTA_CORS_ORIGIN 不是合法 origin，已忽略"),
        }
    }
    router
}

/// 所有路由统一 `Cache-Control: no-store`：分享元数据、票据、密文响应都不应落缓存。
async fn no_store(request: axum::extract::Request, next: middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// 每 IP 令牌桶；拿不到对端地址时落到 `ip:unknown`（单实例内共用一个桶）。
///
/// `/healthz` 不计数：探活与 uptime 监控不应挤占访问额度。
async fn ip_limit(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> Result<axum::response::Response, crate::error::AppError> {
    if request.uri().path() == "/healthz" {
        return Ok(next.run(request).await);
    }
    let peer_ip = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0.ip());
    let client = crate::auth::client_ip(
        peer_ip,
        request.headers(),
        state.cfg.trust_proxy,
        state.cfg.trust_proxy_private,
    );
    // 按路径首段分桶：/api 的洪泛不应挤占 /public 或 /ws 的额度。
    let route = request
        .uri()
        .path()
        .split('/')
        .nth(1)
        .filter(|segment| !segment.is_empty())
        .unwrap_or("root");
    let key = match client {
        Some(ip) => format!("ip:{ip}:{route}"),
        None => format!("ip:unknown:{route}"),
    };
    state
        .rate
        .check(&key, crate::state::now())
        .map_err(|limited| crate::error::AppError::RateLimited {
            retry_after_secs: limited.retry_after_secs,
        })?;
    Ok(next.run(request).await)
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
                HeaderName::from_static("x-share-passphrase-b64"),
                HeaderName::from_static("x-share-ticket"),
                HeaderName::from_static("x-share-token"),
                HeaderName::from_static("x-file-name"),
                HeaderName::from_static("x-file-name-b64"),
                HeaderName::from_static("x-device-id"),
                HeaderName::from_static("x-nonce"),
                HeaderName::from_static("x-timestamp"),
                HeaderName::from_static("x-signature"),
            ])
            .expose_headers([header::RETRY_AFTER, header::ETAG])
            .max_age(Duration::from_secs(600)),
    )
}
