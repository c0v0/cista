//! 分享面验收：权限、两步下载、410、隔离、配额、文件名与 MIME
//! （`docs/specs/phase2-sharing.md` §6）。

mod common;

use axum::http::StatusCode;
use cista_core::protocol::{ConfirmResponse, PublicFileEntry, PublicFileList, PublicMeta};
use cista_core::share::{new_share_token, FilesMode, NoteMode, ShareKind};
use common::{link_request, now_ts, TestApp};
use tower::ServiceExt;

async fn upload(
    app: &TestApp,
    token: &str,
    name: &str,
    mime: &str,
    bytes: &[u8],
    passphrase: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut headers = vec![("x-file-name", name), ("content-type", mime)];
    if let Some(passphrase) = passphrase {
        headers.push(("x-share-passphrase", passphrase));
    }
    app.send(app.public(
        "POST",
        "/public/workspaces/files/upload",
        token,
        bytes,
        &headers,
    ))
    .await
}

async fn list_files(app: &TestApp, token: &str) -> (StatusCode, Vec<u8>) {
    app.send(app.public("GET", "/public/workspaces/files", token, b"", &[]))
        .await
}

#[tokio::test]
async fn workspace_link_meta_and_forged_token() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "共享工作区").await;

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

    let (status, body) = app
        .send(app.public("GET", "/public/workspaces/meta", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::OK);
    let meta: PublicMeta = TestApp::json(&body);
    assert_eq!(meta.title, "共享工作区");
    assert_eq!(meta.kind, ShareKind::Workspace);
    assert_eq!(meta.used_count, 0);

    let forged = new_share_token().expect("token");
    let (status, _) = app
        .send(app.public("GET", "/public/workspaces/meta", &forged, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn read_only_link_cannot_upload() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "只读").await;
    let (request, token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Ro,
        FilesMode::Ro,
    );
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );

    let (status, _) = upload(&app, &token, "a.txt", "text/plain", b"hello", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = list_files(&app, &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(TestApp::json::<PublicFileList>(&body).files.is_empty());
}

#[tokio::test]
async fn rw_link_uploads_and_downloads_with_attachment_headers() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "可写").await;
    let (request, token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Off,
        FilesMode::Rw,
    );
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );

    let (status, body) = upload(
        &app,
        &token,
        "report.pdf",
        "application/pdf",
        b"%PDF-1.4 shared",
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let entry: PublicFileEntry = TestApp::json(&body);
    assert_eq!(entry.name, "report.pdf");

    let (status, body) = list_files(&app, &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(TestApp::json::<PublicFileList>(&body).files.len(), 1);

    let uri = format!("/public/workspaces/files/{}/blob", entry.id);
    let response = app
        .router
        .clone()
        .oneshot(app.public("GET", &uri, &token, b"", &[]))
        .await
        .expect("download response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    let disposition = response
        .headers()
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(disposition.starts_with("attachment"), "{disposition}");
}

#[tokio::test]
async fn drop_link_hides_listing_and_requires_passphrase() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "收件箱").await;

    let (mut request, token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Off,
        FilesMode::Drop,
    );
    request.passphrase = Some("lettre".into());
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );

    // 口令校验先于权限位：带正确口令才能看到「drop 链接不允许列目录」的 403。
    let (status, _) = app
        .send(app.public(
            "GET",
            "/public/workspaces/files",
            &token,
            b"",
            &[("x-share-passphrase", "lettre")],
        ))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = upload(&app, &token, "a.txt", "text/plain", b"hi", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = upload(&app, &token, "a.txt", "text/plain", b"hi", Some("wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = upload(&app, &token, "a.txt", "text/plain", b"hi", Some("lettre")).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn drop_link_without_passphrase_is_rejected() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "收件箱2").await;
    let (request, _token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Off,
        FilesMode::Drop,
    );
    let (status, body) = app.create_link(&device, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).expect("json")["error"],
        "passphrase_required"
    );
}

#[tokio::test]
async fn single_file_link_is_two_step_and_capped_to_one_use() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let blob = app.upload_private_blob(&device, b"secret-bytes").await;

    let (mut request, token) =
        link_request(ShareKind::File, &blob.hash, NoteMode::Off, FilesMode::Ro);
    request.title = Some("secret.txt".into());
    request.max_uses = Some(1);
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );

    for _ in 0..2 {
        let (status, body) = app
            .send(app.public("GET", "/public/files/meta", &token, b"", &[]))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(TestApp::json::<PublicMeta>(&body).used_count, 0);
    }

    let (status, body) = app
        .send(app.public("POST", "/public/files/confirm", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::OK);
    let confirm: ConfirmResponse = TestApp::json(&body);

    let uri = format!("/public/files/blob?ticket={}", confirm.ticket);
    let (status, downloaded) = app.send(app.public("GET", &uri, &token, b"", &[])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(downloaded.as_slice(), b"secret-bytes");

    let (status, _) = app
        .send(app.public("GET", "/public/files/meta", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::GONE);
    let (status, _) = app
        .send(app.public("POST", "/public/files/confirm", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::GONE);
}

#[tokio::test]
async fn concurrent_confirm_consumes_exactly_once() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let blob = app.upload_private_blob(&device, b"race").await;

    let (mut request, token) =
        link_request(ShareKind::File, &blob.hash, NoteMode::Off, FilesMode::Ro);
    request.max_uses = Some(1);
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );

    let first = app.public("POST", "/public/files/confirm", &token, b"", &[]);
    let second = app.public("POST", "/public/files/confirm", &token, b"", &[]);
    let (a, b) = tokio::join!(app.send(first), app.send(second));
    let successes = [a.0, b.0]
        .iter()
        .filter(|status| **status == StatusCode::OK)
        .count();
    assert_eq!(successes, 1, "并发 confirm 必须只有一次成功");
}

#[tokio::test]
async fn revoked_and_expired_links_are_gone() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let blob = app.upload_private_blob(&device, b"data").await;

    let (mut request, token) =
        link_request(ShareKind::File, &blob.hash, NoteMode::Off, FilesMode::Ro);
    request.expires_at = Some(now_ts() - 10);
    let (status, bytes) = app.create_link(&device, request).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let (status, _) = app
        .send(app.public("GET", "/public/files/meta", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::GONE);

    let (request, token) = link_request(ShareKind::File, &blob.hash, NoteMode::Off, FilesMode::Ro);
    let (status, bytes) = app.create_link(&device, request).await;
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = TestApp::json(&bytes);
    let link_id = created["id"].as_str().expect("link id").to_string();
    let uri = format!("/api/share/{link_id}");
    let (status, _) = app.send(app.signed(&device, "DELETE", &uri, b"")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = app
        .send(app.public("GET", "/public/files/meta", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::GONE);
}

#[tokio::test]
async fn share_token_cannot_reach_private_api() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "隔离").await;
    let (request, token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Rw,
        FilesMode::Rw,
    );
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );

    let (status, _) = app
        .send(app.public("GET", "/api/notes", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = app
        .send(app.signed(&device, "GET", "/public/workspaces/meta", b""))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn upload_rejects_html_oversize_and_sanitizes_names() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "上传").await;
    let (request, token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Off,
        FilesMode::Rwm,
    );
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );

    let (status, _) = upload(&app, &token, "evil.html", "text/html", b"<script>", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = upload(&app, &token, "evil.svg", "image/svg+xml", b"<svg/>", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let big = vec![0u8; 2 * 1024 * 1024];
    let (status, _) = upload(&app, &token, "big.bin", "text/plain", &big, None).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let shared_dir = app.dir.path().join("blobs").join("shared");
    let entries = std::fs::read_dir(&shared_dir)
        .map(|dir| dir.count())
        .unwrap_or(0);
    assert_eq!(entries, 0, "超限上传不应落盘");

    let (status, body) = upload(&app, &token, "../../etc/passwd", "text/plain", b"v1", None).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(TestApp::json::<PublicFileEntry>(&body).name, "passwd");

    let (status, body) = upload(&app, &token, "passwd", "text/plain", b"v2", None).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(TestApp::json::<PublicFileEntry>(&body).name, "passwd-2");
}

#[tokio::test]
async fn workspaces_are_isolated_from_each_other() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let first = app.create_workspace(&device, "A").await;
    let second = app.create_workspace(&device, "B").await;

    let (request, token_a) = link_request(
        ShareKind::Workspace,
        &first.id,
        NoteMode::Off,
        FilesMode::Rw,
    );
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );
    let (request, token_b) = link_request(
        ShareKind::Workspace,
        &second.id,
        NoteMode::Off,
        FilesMode::Ro,
    );
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );

    let (status, body) = upload(&app, &token_a, "only-a.txt", "text/plain", b"A", None).await;
    assert_eq!(status, StatusCode::CREATED);
    let entry: PublicFileEntry = TestApp::json(&body);

    let (status, body) = list_files(&app, &token_b).await;
    assert_eq!(status, StatusCode::OK);
    assert!(TestApp::json::<PublicFileList>(&body).files.is_empty());

    let uri = format!("/public/workspaces/files/{}/blob", entry.id);
    let (status, _) = app.send(app.public("GET", &uri, &token_b, b"", &[])).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn tokens_never_appear_in_responses() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "标题").await;
    let (request, token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Ro,
        FilesMode::Ro,
    );
    let (status, bytes) = app.create_link(&device, request).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(!String::from_utf8_lossy(&bytes).contains(&token));

    let (status, body) = app
        .send(app.public("GET", "/public/workspaces/meta", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&body).contains(&token));

    let (status, body) = app
        .send(app.signed(&device, "GET", "/api/share", b""))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&body).contains(&token));
}
