//! Phase 5 验收：过渡区、restore、archive-complete、级联删除
//! （`docs/specs/phase5-archive-backup.md` §4）。

mod common;

use axum::http::StatusCode;
use cista_core::auth::DeviceSecret;
use cista_core::crypto::Key;
use cista_core::protocol::{WorkspaceInfo, WorkspaceState};
use cista_core::share::{FilesMode, NoteMode, ShareKind};
use common::{link_request, now_ts, put_body, sealed_note, TestApp};

/// 直接改库里状态，模拟「到期 → 过渡区」（v1 没有常驻调度器）。
async fn quarantine(app: &TestApp, workspace_id: &str) {
    sqlx::query("UPDATE workspaces SET state = 'quarantined', quarantined_at = ? WHERE id = ?")
        .bind(now_ts())
        .bind(workspace_id)
        .execute(app.state.shared.pool())
        .await
        .expect("quarantine workspace");
}

async fn create_workspace_with(
    app: &TestApp,
    device: &DeviceSecret,
    origin: &str,
    title: &str,
    source_note_id: Option<&str>,
) -> WorkspaceInfo {
    let body = serde_json::json!({
        "title": title,
        "origin": origin,
        "source_note_id": source_note_id,
        "expires_at": null,
    })
    .to_string();
    let (status, bytes) = app
        .send(app.signed(device, "POST", "/api/workspaces", body.as_bytes()))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    TestApp::json(&bytes)
}

async fn create_projection_workspace(
    app: &TestApp,
    device: &DeviceSecret,
    note_id: &str,
) -> WorkspaceInfo {
    create_workspace_with(app, device, "projection", "投影", Some(note_id)).await
}

async fn create_note(app: &TestApp, device: &DeviceSecret, note_id: &str, title: &str) {
    let mk = Key::generate().expect("mk");
    let sealed = sealed_note(&mk, note_id, 1, title, "正文");
    let (status, body) = app
        .send(app.signed(
            device,
            "PUT",
            &format!("/api/notes/{note_id}"),
            &put_body(sealed),
        ))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn quarantine_lists_workspace_and_restore_brings_it_back() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "将过期").await;
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

    quarantine(&app, &workspace.id).await;

    // 过渡区：公网 410，但所有者 API 可见。
    let (status, _) = app
        .send(app.public("GET", "/public/workspaces/meta", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::GONE);

    let (status, body) = app
        .send(app.signed(&device, "GET", "/api/quarantine", b""))
        .await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<WorkspaceInfo> = TestApp::json(&body);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, workspace.id);
    assert_eq!(listed[0].state, WorkspaceState::Quarantined);

    // restore → active，原链接重新可用。
    let uri = format!("/api/workspaces/{}/restore", workspace.id);
    let (status, body) = app.send(app.signed(&device, "POST", &uri, b"")).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let restored: WorkspaceInfo = TestApp::json(&body);
    assert_eq!(restored.state, WorkspaceState::Active);

    let (status, _) = app
        .send(app.public("GET", "/public/workspaces/meta", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn archive_complete_purges_shared_data_and_releases_quota() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "归档").await;
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

    // 访客上传一个文件（占用配额）。
    let (status, _) = app
        .send(app.public(
            "POST",
            "/public/workspaces/files/upload",
            &token,
            b"payload",
            &[("x-file-name", "a.txt"), ("content-type", "text/plain")],
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    quarantine(&app, &workspace.id).await;

    let files: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace_files WHERE workspace_id = ?")
            .bind(&workspace.id)
            .fetch_one(app.state.shared.pool())
            .await
            .expect("count files");
    assert_eq!(files, 1);

    let uri = format!("/api/workspaces/{}/archive-complete", workspace.id);
    let (status, body) = app.send(app.signed(&device, "POST", &uri, b"")).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let archived: WorkspaceInfo = TestApp::json(&body);
    assert_eq!(archived.state, WorkspaceState::Archived);

    // 共享数据清空、配额归零、公网 410。
    let files: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace_files WHERE workspace_id = ?")
            .bind(&workspace.id)
            .fetch_one(app.state.shared.pool())
            .await
            .expect("count files after purge");
    assert_eq!(files, 0);
    let used: i64 = sqlx::query_scalar(
        "SELECT bytes FROM usage_counters WHERE scope = 'instance' AND key = 'total'",
    )
    .fetch_one(app.state.shared.pool())
    .await
    .expect("instance usage");
    assert_eq!(used, 0, "归档后实例用量应归零");

    let (status, _) = app
        .send(app.public("GET", "/public/workspaces/meta", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::GONE);
}

#[tokio::test]
async fn deleting_source_note_without_uploads_purges_projection() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    create_note(&app, &device, "note-1", "源笔记").await;

    let workspace = create_projection_workspace(&app, &device, "note-1").await;
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

    let (status, _) = app
        .send(app.signed(&device, "DELETE", "/api/notes/note-1", b""))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = app
        .send(app.public("GET", "/public/workspaces/meta", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::GONE);
    let (status, _) = app
        .send(app.signed(&device, "GET", "/api/notes/note-1", b""))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_source_note_with_uploads_requires_force() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    create_note(&app, &device, "note-2", "有上传").await;

    let workspace = create_projection_workspace(&app, &device, "note-2").await;
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
    let (status, _) = app
        .send(app.public(
            "POST",
            "/public/workspaces/files/upload",
            &token,
            b"visitor",
            &[("x-file-name", "up.txt"), ("content-type", "text/plain")],
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    // 有上传件：阻止，409 + 三种处理路径。
    let (status, body) = app
        .send(app.signed(&device, "DELETE", "/api/notes/note-2", b""))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let conflict: serde_json::Value = TestApp::json(&body);
    assert_eq!(conflict["error"], "uploads_present");
    assert_eq!(conflict["options"].as_array().map(Vec::len), Some(3));

    // force=true：删除并级联。
    let (status, _) = app
        .send(app.signed(&device, "DELETE", "/api/notes/note-2?force=true", b""))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = app
        .send(app.public("GET", "/public/workspaces/meta", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::GONE);
}

#[tokio::test]
async fn delete_unknown_note_is_404() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let (status, _) = app
        .send(app.signed(&device, "DELETE", "/api/notes/nope", b""))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn archived_workspace_cannot_be_restored() {
    let app = TestApp::new().await;
    let device = app.enroll("archive-device").await;
    let workspace = create_workspace_with(&app, &device, "blank", "已归档", None).await;
    quarantine(&app, &workspace.id).await;

    let archive_path = format!("/api/workspaces/{}/archive-complete", workspace.id);
    let (status, body) = app
        .send(app.signed(&device, "POST", &archive_path, b""))
        .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let archived: WorkspaceInfo = TestApp::json(&body);
    assert_eq!(archived.state, WorkspaceState::Archived);

    let restore_path = format!("/api/workspaces/{}/restore", workspace.id);
    let (status, _) = app
        .send(app.signed(&device, "POST", &restore_path, b""))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "archived 是终态");
}
