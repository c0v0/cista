//! 分享面验收：权限、两步下载、410、隔离、配额、文件名与 MIME
//! （`docs/specs/phase2-sharing.md` §6）。

mod common;

use axum::http::StatusCode;
use cista_core::protocol::{
    ConfirmResponse, LinkInfo, PublicFileEntry, PublicFileList, PublicMeta,
};
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

    let (status, downloaded) = app
        .send(app.public(
            "GET",
            "/public/files/blob",
            &token,
            b"",
            &[("x-share-ticket", confirm.ticket.as_str())],
        ))
        .await;
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
async fn concurrent_downloads_consume_exactly_once() {
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

    // confirm 不再消费次数；两次 confirm 各自拿到一张单次票据。
    let mut tickets = Vec::new();
    for _ in 0..2 {
        let (status, body) = app
            .send(app.public("POST", "/public/files/confirm", &token, b"", &[]))
            .await;
        assert_eq!(status, StatusCode::OK);
        tickets.push(TestApp::json::<ConfirmResponse>(&body).ticket);
    }

    let first = app.public(
        "GET",
        "/public/files/blob",
        &token,
        b"",
        &[("x-share-ticket", tickets[0].as_str())],
    );
    let second = app.public(
        "GET",
        "/public/files/blob",
        &token,
        b"",
        &[("x-share-ticket", tickets[1].as_str())],
    );
    let (a, b) = tokio::join!(app.send(first), app.send(second));
    let successes = [a.0, b.0]
        .iter()
        .filter(|status| **status == StatusCode::OK)
        .count();
    assert_eq!(successes, 1, "并发下载必须只有一次成功消费链接次数");
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

#[tokio::test]
async fn workspace_file_list_is_paginated_and_capped() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "分页工作区").await;
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

    for name in ["a.txt", "b.txt", "c.txt"] {
        let (status, body) = upload(&app, &token, name, "text/plain", b"x", None).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&body)
        );
    }

    let (status, body) = app
        .send(app.public("GET", "/public/workspaces/files?limit=2", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::OK);
    let first: PublicFileList = TestApp::json(&body);
    assert_eq!(first.files.len(), 2, "一页最多 limit 条");
    assert_eq!(first.next_offset, Some(2));

    let (status, body) = app
        .send(app.public(
            "GET",
            "/public/workspaces/files?limit=2&offset=2",
            &token,
            b"",
            &[],
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    let second: PublicFileList = TestApp::json(&body);
    assert_eq!(second.files.len(), 1);
    assert_eq!(second.next_offset, None, "最后一页不带 next_offset");

    let mut names: Vec<String> = first
        .files
        .iter()
        .chain(second.files.iter())
        .map(|entry| entry.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, vec!["a.txt", "b.txt", "c.txt"]);

    // 上限钳制：limit 超过 PAGE_MAX 不报错，也不会返回超过总数的条目。
    let (status, body) = app
        .send(app.public(
            "GET",
            "/public/workspaces/files?limit=100000",
            &token,
            b"",
            &[],
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(TestApp::json::<PublicFileList>(&body).files.len(), 3);
}

#[tokio::test]
async fn upload_streams_and_rejects_oversize_without_leftovers() {
    let app = TestApp::with_config(|cfg| cfg.max_file_bytes = 1024).await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "流式上传").await;
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

    let big = vec![7u8; 4096];
    let (status, _) = upload(&app, &token, "big.txt", "text/plain", &big, None).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    // 临时文件必须清理，且不留下任何行/本体。
    let probes = app.state.shared.blob_path("probe");
    let blobs_dir = probes.parent().expect("blobs dir");
    let mut entries = tokio::fs::read_dir(blobs_dir).await.expect("read dir");
    while let Some(entry) = entries.next_entry().await.expect("entry") {
        let name = entry.file_name().to_string_lossy().to_string();
        assert!(!name.starts_with(".upload-"), "临时文件未清理: {name}");
    }
    let (status, body) = app
        .send(app.public("GET", "/public/workspaces/files", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        TestApp::json::<PublicFileList>(&body).files.is_empty(),
        "被拒的上传不能留下文件行"
    );

    // 上限内的小文件仍然可用（流式路径正常收尾）。
    let (status, body) = upload(&app, &token, "ok.txt", "text/plain", b"hi", None).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn shared_blobs_are_encrypted_at_rest_and_decrypt_on_download() {
    use sha2::{Digest, Sha256};

    let app = TestApp::with_config(|cfg| cfg.static_key = Some([9u8; 32])).await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "静态加密").await;
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

    let marker = b"plaintext-marker-blob-9f";
    let (status, body) = upload(&app, &token, "secret.txt", "text/plain", marker, None).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let entry: PublicFileEntry = TestApp::json(&body);

    let hash = hex::encode(Sha256::digest(marker));
    let stored = tokio::fs::read(app.state.shared.blob_path(&hash))
        .await
        .expect("blob file");
    assert!(
        !stored.windows(marker.len()).any(|window| window == marker),
        "共享 blob 落盘不得包含明文"
    );
    assert!(stored.starts_with(b"CB1\0"), "必须带分块密文魔数");
    assert_eq!(
        stored.len(),
        4 + 24 + 4 + marker.len() + 16,
        "魔数 + nonce + 长度 + 密文 + tag"
    );

    let (status, downloaded) = app
        .send(app.public(
            "GET",
            &format!("/public/workspaces/files/{}/blob", entry.id),
            &token,
            b"",
            &[],
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(downloaded, marker, "下载必须解回原文");
}

#[tokio::test]
async fn ticket_is_single_use() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let blob = app.upload_private_blob(&device, b"retry-bytes").await;
    let (request, token) = link_request(ShareKind::File, &blob.hash, NoteMode::Off, FilesMode::Ro);
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );

    let (status, body) = app
        .send(app.public("POST", "/public/files/confirm", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::OK);
    let confirm: ConfirmResponse = TestApp::json(&body);

    let (status, downloaded) = app
        .send(app.public(
            "GET",
            "/public/files/blob",
            &token,
            b"",
            &[("x-share-ticket", confirm.ticket.as_str())],
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(downloaded, b"retry-bytes");

    // 同一票据第二次必须被拒绝；需要重试时重新 confirm 换新票据。
    let (status, _) = app
        .send(app.public(
            "GET",
            "/public/files/blob",
            &token,
            b"",
            &[("x-share-ticket", confirm.ticket.as_str())],
        ))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn quota_rejection_leaves_no_orphan_blob() {
    // 工作区配额 64 KiB；上传 128 KiB：文件本身在 max_file_bytes 之内，但预留失败。
    let app = TestApp::with_config(|cfg| {
        cfg.workspace_quota_bytes = 64 * 1024;
        cfg.instance_quota_bytes = 1024 * 1024;
        cfg.link_quota_bytes = 1024 * 1024;
    })
    .await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "quota").await;
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

    let payload = vec![7u8; 128 * 1024];

    let (status, body) = upload(&app, &token, "big.bin", "text/plain", &payload, None).await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let (status, body) = list_files(&app, &token).await;
    assert_eq!(status, StatusCode::OK);
    let page: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(page["files"].as_array().map(Vec::len), Some(0));

    let mut entries = tokio::fs::read_dir(app.state.shared.blobs_dir())
        .await
        .expect("blobs dir");
    assert!(
        entries.next_entry().await.expect("entry").is_none(),
        "quota 失败后不应留下共享 blob"
    );
}

#[tokio::test]
async fn outstanding_tickets_are_capped_per_link() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let blob = app.upload_private_blob(&device, b"ticket-cap").await;
    let (request, token) = link_request(ShareKind::File, &blob.hash, NoteMode::Off, FilesMode::Ro);
    assert_eq!(
        app.create_link(&device, request).await.0,
        StatusCode::CREATED
    );

    for _ in 0..12 {
        let (status, _) = app
            .send(app.public("POST", "/public/files/confirm", &token, b"", &[]))
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blob_tickets")
        .fetch_one(app.state.shared.pool())
        .await
        .expect("count tickets");
    assert!(count <= 8, "tickets={count}");
}

#[tokio::test]
async fn file_link_rejects_files_off_scope() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let blob = app.upload_private_blob(&device, b"scope-check").await;
    let (request, _token) =
        link_request(ShareKind::File, &blob.hash, NoteMode::Off, FilesMode::Off);
    let (status, body) = app.create_link(&device, request).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn deleting_one_of_two_same_content_files_keeps_link_quota_for_the_other() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "配额").await;
    let (request, token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Off,
        FilesMode::Rwm,
    );
    let (status, body) = app.create_link(&device, request).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let link: LinkInfo = TestApp::json(&body);

    let content = b"same-content";
    let (status, body) = upload(&app, &token, "a.txt", "text/plain", content, None).await;
    assert_eq!(status, StatusCode::CREATED);
    let first: PublicFileEntry = TestApp::json(&body);

    let rename_uri = format!("/public/workspaces/files/{}", first.id);
    let (status, _) = app
        .send(app.public(
            "PATCH",
            &rename_uri,
            &token,
            br#"{"name":"b.txt"}"#,
            &[("content-type", "application/json")],
        ))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = upload(&app, &token, "a.txt", "text/plain", content, None).await;
    assert_eq!(status, StatusCode::CREATED);
    let second: PublicFileEntry = TestApp::json(&body);

    let delete_uri = format!("/public/workspaces/files/{}", second.id);
    let (status, _) = app
        .send(app.public("DELETE", &delete_uri, &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let uploads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM uploads WHERE link_id = ?")
        .bind(&link.id)
        .fetch_one(app.state.shared.pool())
        .await
        .expect("count uploads");
    assert_eq!(uploads, 1, "删除第二个文件不得清掉第一个文件的配额行");
}

#[tokio::test]
async fn base64_headers_support_unicode_passphrase_and_filename() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;

    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "Unicode").await;
    let (mut request, token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Off,
        FilesMode::Rwm,
    );
    request.passphrase = Some("中文口令".to_string());
    let (status, body) = app.create_link(&device, request).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let name = STANDARD.encode("中文.txt".as_bytes());
    let passphrase = STANDARD.encode("中文口令".as_bytes());
    let (status, body) = app
        .send(app.public(
            "POST",
            "/public/workspaces/files/upload",
            &token,
            b"data",
            &[
                ("x-file-name-b64", name.as_str()),
                ("x-share-passphrase-b64", passphrase.as_str()),
                ("content-type", "text/plain"),
            ],
        ))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let entry: PublicFileEntry = TestApp::json(&body);
    assert_eq!(entry.name, "中文.txt");
}
