//! 跨域部署（`CISTA_CORS_ORIGIN`）：只放行配置的前端 origin，其它 origin 不得拿到 CORS 头。

mod common;

use axum::body::Body;
use axum::http::Request;
use common::TestApp;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:5173";

#[tokio::test]
async fn preflight_from_configured_origin_is_allowed() {
    let app = TestApp::with_config(|cfg| cfg.cors_origin = Some(ORIGIN.into())).await;
    let request = Request::builder()
        .method("OPTIONS")
        .uri("/public/workspaces/meta")
        .header("origin", ORIGIN)
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "authorization")
        .body(Body::empty())
        .expect("preflight");
    let response = app.router.clone().oneshot(request).await.expect("response");
    assert!(response.status().is_success(), "{}", response.status());
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some(ORIGIN)
    );
}

#[tokio::test]
async fn other_origins_are_never_echoed() {
    let app = TestApp::with_config(|cfg| cfg.cors_origin = Some(ORIGIN.into())).await;
    let request = Request::builder()
        .method("GET")
        .uri("/healthz")
        .header("origin", "http://evil.example")
        .body(Body::empty())
        .expect("request");
    let response = app.router.clone().oneshot(request).await.expect("response");
    // tower-http 的 `AllowOrigin::exact` 只发配置值、不回显请求 origin；
    // 浏览器按「请求 origin ≠ 允许值」拒绝，服务端不做二次判断。
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some(ORIGIN)
    );
}

#[tokio::test]
async fn without_config_no_cors_headers() {
    let app = TestApp::new().await;
    let request = Request::builder()
        .method("GET")
        .uri("/healthz")
        .header("origin", ORIGIN)
        .body(Body::empty())
        .expect("request");
    let response = app.router.clone().oneshot(request).await.expect("response");
    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}
