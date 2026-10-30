//! 私有 blob 取回：签名校验 + 内容一致 + 未知哈希 404。

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;

#[tokio::test]
async fn private_blob_download_requires_signature_and_returns_bytes() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let payload = b"ciphertext-from-client";
    let blob = app.upload_private_blob(&device, payload).await;
    let uri = format!("/api/files/{}", blob.hash);

    let (status, bytes) = app.send(app.signed(&device, "GET", &uri, b"")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, payload);

    let unsigned = Request::builder()
        .method("GET")
        .uri(&uri)
        .body(Body::empty())
        .expect("request");
    let (status, _) = app.send(unsigned).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let missing = format!("/api/files/{}", "00".repeat(32));
    let (status, _) = app.send(app.signed(&device, "GET", &missing, b"")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 非法哈希形状同样 404，不泄漏内部路径。
    let malformed = "/api/files/not-a-hash";
    let (status, _) = app.send(app.signed(&device, "GET", malformed, b"")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn large_private_upload_is_streamed_and_roundtrips() {
    // 7 MiB 超过笔记路由的 6 MiB 上限：私有附件必须走流式上传而不是整包缓冲。
    let app = TestApp::with_config(|cfg| {
        cfg.max_file_bytes = 8 * 1024 * 1024;
        cfg.instance_quota_bytes = 16 * 1024 * 1024;
    })
    .await;
    let device = app.enroll("desktop").await;
    let payload = vec![7u8; 7 * 1024 * 1024];
    let blob = app.upload_private_blob(&device, &payload).await;

    let uri = format!("/api/files/{}", blob.hash);
    let (status, bytes) = app.send(app.signed(&device, "GET", &uri, b"")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes.len(), payload.len());
    assert_eq!(bytes, payload);
}
