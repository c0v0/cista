//! Phase 6：令牌桶限速 —— 设备与链接两个维度，超限返回 429 + `Retry-After`。
//!
//! 断言方式不依赖墙钟：连续请求直到出现 429（桶容量很小、补充很慢），
//! 这样在慢机器上也不会因为「测试跑得比补充快/慢」而假绿或假红。

mod common;

use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::response::Response;
use cista_core::share::{FilesMode, NoteMode, ShareKind};
use common::{link_request, TestApp};
use tower::ServiceExt;

fn retry_after_secs(response: &Response) -> u64 {
    response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .expect("429 必须带 Retry-After")
}

#[tokio::test]
async fn device_burst_returns_429_with_retry_after() {
    let app = TestApp::with_config(|cfg| {
        cfg.rate_per_minute = 6;
        cfg.rate_burst = 2;
    })
    .await;
    let device = app.enroll("限速设备").await;

    let mut limited = None;
    for _ in 0..20 {
        let response = app
            .router
            .clone()
            .oneshot(app.signed(&device, "GET", "/api/notes", b""))
            .await
            .expect("response");
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            limited = Some(response);
            break;
        }
        assert_eq!(response.status(), StatusCode::OK);
    }
    let response = limited.expect("burst=2 后必须出现 429");
    assert!(retry_after_secs(&response) >= 1);

    let body = to_bytes(response.into_body(), 4096).await.expect("body");
    let error: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(error["error"], "rate_limited");
}

#[tokio::test]
async fn link_burst_returns_429_with_retry_after() {
    let app = TestApp::with_config(|cfg| {
        cfg.rate_per_minute = 6;
        cfg.rate_burst = 3;
    })
    .await;
    let device = app.enroll("限速链接").await;
    let workspace = app.create_workspace(&device, "限速工作区").await;
    let (request, token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Ro,
        FilesMode::Ro,
    );
    let (status, bytes) = app.create_link(&device, request).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    let mut limited = None;
    for _ in 0..20 {
        let response = app
            .router
            .clone()
            .oneshot(app.public("GET", "/public/workspaces/meta", &token, b"", &[]))
            .await
            .expect("response");
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            limited = Some(response);
            break;
        }
        assert_eq!(response.status(), StatusCode::OK);
    }
    let response = limited.expect("burst=3 后必须出现 429");
    assert!(retry_after_secs(&response) >= 1);
}

#[tokio::test]
async fn per_ip_bucket_limits_unauthenticated_floods() {
    use axum::body::Body;
    use axum::http::Request;

    let app = TestApp::with_config(|cfg| {
        cfg.rate_per_minute = 60;
        cfg.rate_burst = 2;
    })
    .await;

    let anonymous = || {
        Request::builder()
            .method("GET")
            .uri("/api/notes")
            .body(Body::empty())
            .expect("request")
    };

    for _ in 0..2 {
        let (status, _) = app.send(anonymous()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = app.send(anonymous()).await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "未认证洪泛也必须按 IP 限速"
    );
}

#[tokio::test]
async fn repeated_bad_signatures_get_locked_out() {
    use axum::body::Body;
    use axum::http::Request;
    use common::now_ts;

    let app = TestApp::new().await;
    let device = app.enroll("lockout-device").await;

    let bad_request = |nonce_byte: u8| {
        Request::builder()
            .method("GET")
            .uri("/api/notes")
            .header("x-device-id", device.device_id())
            .header("x-nonce", hex::encode([nonce_byte; 16]))
            .header("x-timestamp", now_ts().to_string())
            .header("x-signature", "00".repeat(64))
            .body(Body::empty())
            .expect("request")
    };

    let mut locked = false;
    for attempt in 0..8u8 {
        let (status, _) = app.send(bad_request(attempt + 1)).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            locked = true;
            break;
        }
        assert_eq!(status, StatusCode::UNAUTHORIZED, "第 {attempt} 次应为 401");
    }
    assert!(locked, "连续伪造签名后必须进入指数退避");
}
