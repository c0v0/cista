//! B+ 认证与设备生命周期（`docs/specs/phase1-core-auth.md` §2、§4）。

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cista_core::auth::{DeviceSecret, MAX_SKEW_SECS};
use common::{fresh_nonce, now_ts, signed_request, TestApp};

#[tokio::test]
async fn unsigned_private_request_is_401() {
    let app = TestApp::new().await;
    let request = Request::builder()
        .uri("/api/notes")
        .body(Body::empty())
        .expect("request");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn stale_timestamp_is_401() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let nonce = fresh_nonce();
    let stale = now_ts() - MAX_SKEW_SECS - 5;
    let request = signed_request(&device, "GET", "/api/notes", b"", &nonce, stale);
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let future = now_ts() + MAX_SKEW_SECS + 5;
    let request = signed_request(&device, "GET", "/api/notes", b"", &fresh_nonce(), future);
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn replayed_nonce_is_401() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let nonce = fresh_nonce();
    let ts = now_ts();

    let first = signed_request(&device, "GET", "/api/notes", b"", &nonce, ts);
    let (status, _) = app.send(first).await;
    assert_eq!(status, StatusCode::OK);

    // 同一 nonce + 同一时间戳重放：签名有效，但必须被拒。
    let replay = signed_request(&device, "GET", "/api/notes", b"", &nonce, ts);
    let (status, _) = app.send(replay).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoked_device_is_401_immediately() {
    let app = TestApp::new().await;
    let admin = app.enroll("admin").await;
    let doomed = app.enroll("laptop").await;

    // 吊销前可用。
    let request = app.signed(&doomed, "GET", "/api/notes", b"");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::OK);

    let uri = format!("/api/devices/{}/revoke", doomed.device_id());
    let request = app.signed(&admin, "POST", &uri, b"");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 吊销后立即失效。
    let request = app.signed(&doomed, "GET", "/api/notes", b"");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cannot_revoke_self() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let uri = format!("/api/devices/{}/revoke", device.device_id());
    let request = app.signed(&device, "POST", &uri, b"");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn revoking_unknown_device_is_404() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let request = app.signed(&device, "POST", "/api/devices/deadbeef/revoke", b"");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn enroll_requires_bootstrap_or_signature() {
    let app = TestApp::new().await;
    let new_device = DeviceSecret::generate().expect("key");
    let body = serde_json::json!({
        "name": "web",
        "pubkey": hex::encode(new_device.public_key()),
    })
    .to_string();

    let request = Request::builder()
        .method("POST")
        .uri("/api/devices/enroll")
        .header("content-type", "application/json")
        .body(Body::from(body.clone()))
        .expect("request");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let request = Request::builder()
        .method("POST")
        .uri("/api/devices/enroll")
        .header("authorization", "Bootstrap wrong-token")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("request");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn enrolled_device_can_sign_and_enroll_another() {
    let app = TestApp::new().await;
    let first = app.enroll("desktop").await;

    let second = DeviceSecret::generate().expect("key");
    let body = serde_json::json!({
        "name": "phone",
        "pubkey": hex::encode(second.public_key()),
    })
    .to_string();
    let request = signed_request(
        &first,
        "POST",
        "/api/devices/enroll",
        body.as_bytes(),
        &fresh_nonce(),
        now_ts(),
    );
    let (status, bytes) = app.send(request).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    // 新设备立即可用。
    let request = app.signed(&second, "GET", "/api/notes", b"");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn duplicate_enroll_is_409() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let body = serde_json::json!({
        "name": "desktop-again",
        "pubkey": hex::encode(device.public_key()),
    })
    .to_string();
    let request = signed_request(
        &device,
        "POST",
        "/api/devices/enroll",
        body.as_bytes(),
        &fresh_nonce(),
        now_ts(),
    );
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn tampered_body_is_401() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let nonce = fresh_nonce();
    let ts = now_ts();
    let signed_body = br#"{"sealed":{}}"#;
    let signature = common::sign_at(&device, "GET", "/api/notes", signed_body, &nonce, ts);

    // 用同一签名发不同 body。
    let request = Request::builder()
        .method("GET")
        .uri("/api/notes")
        .header("x-device-id", device.device_id())
        .header("x-nonce", hex::encode(nonce))
        .header("x-timestamp", ts.to_string())
        .header("x-signature", signature)
        .body(Body::from("tampered".as_bytes().to_vec()))
        .expect("request");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bootstrap_token_is_single_use() {
    let app = TestApp::new().await;
    let _first = app.enroll("first").await;

    // 第二次仍用 bootstrap：必须 401（token 已消费）。
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("seed");
    let secret = cista_core::auth::DeviceSecret::from_seed(&seed);
    let body = serde_json::json!({
        "name": "second",
        "pubkey": hex::encode(secret.public_key()),
    })
    .to_string();
    let request = Request::builder()
        .method("POST")
        .uri("/api/devices/enroll")
        .header("authorization", format!("Bootstrap {}", common::BOOTSTRAP))
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("enroll request");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
