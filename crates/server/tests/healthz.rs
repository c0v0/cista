//! Phase 0 冒烟：`GET /healthz`（`docs/specs/smoke-healthz.md`）。

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;

#[tokio::test]
async fn healthz_returns_ok_without_auth() {
    let app = TestApp::new().await;
    let request = Request::builder()
        .uri("/healthz")
        .body(Body::empty())
        .expect("request");
    let (status, body) = app.send(request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_slice(), b"ok");
}

#[tokio::test]
async fn healthz_rejects_other_methods() {
    let app = TestApp::new().await;
    let request = Request::builder()
        .method("POST")
        .uri("/healthz")
        .body(Body::empty())
        .expect("request");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn unknown_path_is_404_without_details() {
    let app = TestApp::new().await;
    let request = Request::builder()
        .uri("/nope")
        .body(Body::empty())
        .expect("request");
    let (status, body) = app.send(request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(parsed["error"], "not_found");
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("src/"), "404 不应泄漏内部路径");
}
