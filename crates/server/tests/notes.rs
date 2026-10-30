//! 笔记 CRUD、增量同步与「磁盘无明文」（`docs/specs/phase1-core-auth.md` §3、§4）。

mod common;

use axum::http::StatusCode;
use cista_core::crypto::Key;
use common::{note_record, notes_page, put_body, sealed_note, TestApp};

#[tokio::test]
async fn create_get_update_roundtrip() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let mk = Key::generate().expect("mk");

    let first = sealed_note(&mk, "note-1", 1, "周报", "本周内容");
    let request = app.signed(
        &device,
        "PUT",
        "/api/notes/note-1",
        &put_body(first.clone()),
    );
    let (status, bytes) = app.send(request).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let created = note_record(&bytes);
    assert_eq!(created.rev, 1);
    assert_eq!(created.sealed, first);

    let request = app.signed(&device, "GET", "/api/notes/note-1", b"");
    let (status, bytes) = app.send(request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(note_record(&bytes), created);

    let second = sealed_note(&mk, "note-1", 2, "周报 v2", "改过");
    let request = app.signed_with(
        &device,
        "PUT",
        "/api/notes/note-1",
        &put_body(second.clone()),
        &[("if-match", "1")],
    );
    let (status, bytes) = app.send(request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let updated = note_record(&bytes);
    assert_eq!(updated.rev, 2);
    assert_eq!(updated.sealed, second);
}

#[tokio::test]
async fn stale_if_match_returns_409_with_current_rev() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let mk = Key::generate().expect("mk");

    let first = sealed_note(&mk, "n", 1, "t", "m");
    let request = app.signed(&device, "PUT", "/api/notes/n", &put_body(first));
    assert_eq!(app.send(request).await.0, StatusCode::CREATED);

    let second = sealed_note(&mk, "n", 2, "t", "m2");
    let request = app.signed_with(
        &device,
        "PUT",
        "/api/notes/n",
        &put_body(second),
        &[("if-match", "1")],
    );
    assert_eq!(app.send(request).await.0, StatusCode::OK);

    // 再用旧 rev 提交 → 409 + 服务端当前 rev。
    let stale = sealed_note(&mk, "n", 2, "t", "offline edit");
    let request = app.signed_with(
        &device,
        "PUT",
        "/api/notes/n",
        &put_body(stale),
        &[("if-match", "1")],
    );
    let (status, bytes) = app.send(request).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["error"], "rev_conflict");
    assert_eq!(body["rev"], 2);
}

#[tokio::test]
async fn missing_if_match_on_existing_note_is_409() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let mk = Key::generate().expect("mk");

    let first = sealed_note(&mk, "n", 1, "t", "m");
    let request = app.signed(&device, "PUT", "/api/notes/n", &put_body(first));
    assert_eq!(app.send(request).await.0, StatusCode::CREATED);

    // 无 If-Match 的第二次写入不能被当成「创建」覆盖。
    let again = sealed_note(&mk, "n", 1, "t", "overwrite");
    let request = app.signed(&device, "PUT", "/api/notes/n", &put_body(again));
    let (status, bytes) = app.send(request).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["rev"], 1);
}

#[tokio::test]
async fn if_match_on_missing_note_is_404() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let mk = Key::generate().expect("mk");

    let note = sealed_note(&mk, "ghost", 2, "t", "m");
    let request = app.signed_with(
        &device,
        "PUT",
        "/api/notes/ghost",
        &put_body(note),
        &[("if-match", "1")],
    );
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sealed_rev_must_match_server_rev() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let mk = Key::generate().expect("mk");

    // 服务器会写 rev=1，信封却声明 rev=5 → 拒绝（AAD 绑定 rev）。
    let wrong = sealed_note(&mk, "n", 5, "t", "m");
    let request = app.signed(&device, "PUT", "/api/notes/n", &put_body(wrong));
    let (status, bytes) = app.send(request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).expect("json")["error"],
        "rev_mismatch"
    );
}

#[tokio::test]
async fn unknown_or_invalid_note_id_is_not_found() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;

    let request = app.signed(&device, "GET", "/api/notes/does-not-exist", b"");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 非法 ID（含路径字符）不进入查询，也不泄漏存在性。
    let request = app.signed(&device, "GET", "/api/notes/bad%2Fid", b"");
    let (status, _) = app.send(request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn incremental_list_uses_oplog_seq() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let mk = Key::generate().expect("mk");

    for (id, rev, title) in [("a", 1, "A"), ("b", 1, "B")] {
        let note = sealed_note(&mk, id, rev, title, "m");
        let uri = format!("/api/notes/{id}");
        let request = app.signed(&device, "PUT", &uri, &put_body(note));
        assert_eq!(app.send(request).await.0, StatusCode::CREATED);
    }

    let (status, bytes) = app
        .send(app.signed(&device, "GET", "/api/notes?since=0", b""))
        .await;
    assert_eq!(status, StatusCode::OK);
    let page = notes_page(&bytes);
    assert_eq!(page.notes.len(), 2);
    assert_eq!(page.next_since, 2);

    // 用游标再拉：没有新数据。
    let uri = format!("/api/notes?since={}", page.next_since);
    let (status, bytes) = app.send(app.signed(&device, "GET", &uri, b"")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(notes_page(&bytes).notes.is_empty());

    // 更新 a → 只返回 a，且是最新 rev。
    let note = sealed_note(&mk, "a", 2, "A2", "m");
    let request = app.signed_with(
        &device,
        "PUT",
        "/api/notes/a",
        &put_body(note),
        &[("if-match", "1")],
    );
    assert_eq!(app.send(request).await.0, StatusCode::OK);

    let uri = format!("/api/notes?since={}", page.next_since);
    let (status, bytes) = app.send(app.signed(&device, "GET", &uri, b"")).await;
    assert_eq!(status, StatusCode::OK);
    let page = notes_page(&bytes);
    assert_eq!(page.notes.len(), 1);
    assert_eq!(page.notes[0].id, "a");
    assert_eq!(page.notes[0].rev, 2);
    assert_eq!(page.next_since, 3);
}

#[tokio::test]
async fn disk_contains_no_plaintext() {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let mk = Key::generate().expect("mk");

    let marker = "TOPSECRET-MARKER-9f3a2b";
    let body_text = format!("正文里也有 {marker}");
    let note = sealed_note(&mk, "secret-note", 1, marker, &body_text);
    let sealed_json = serde_json::to_string(&note).expect("json");

    let request = app.signed(
        &device,
        "PUT",
        "/api/notes/secret-note",
        &put_body(note.clone()),
    );
    assert_eq!(app.send(request).await.0, StatusCode::CREATED);

    let mut scanned = 0usize;
    let mut ciphertext_found = false;
    for entry in std::fs::read_dir(app.dir.path()).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read db file");
        scanned += 1;
        assert!(
            !bytes
                .windows(marker.len())
                .any(|window| window == marker.as_bytes()),
            "{} 里出现了明文标记",
            path.display()
        );
        // 反向确认：密文确实落库（可能在主库，也可能还在 -wal 里）。
        if String::from_utf8_lossy(&bytes).contains(&sealed_json[..40]) {
            ciphertext_found = true;
        }
    }
    assert!(scanned > 0, "没有扫描到数据库文件");
    assert!(ciphertext_found, "密文未落库");

    // 服务端存的就是密文，能原样取回。
    let (status, bytes) = app
        .send(app.signed(&device, "GET", "/api/notes/secret-note", b""))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(note_record(&bytes).sealed, note);
}
