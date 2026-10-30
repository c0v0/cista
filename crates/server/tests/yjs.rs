//! Phase 3 验收（存储层）：收敛、离线并发、只读丢弃、重启恢复、快照清理
//! （`docs/specs/phase3-yjs.md` §3）。

mod common;

use axum::http::StatusCode;
use cista_core::auth::DeviceSecret;
use cista_core::protocol::WorkspaceInfo;
use cista_server::yjs::{apply_client_update, snapshot, SNAPSHOT_EVERY_UPDATES};
use common::TestApp;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

const ROOT: &str = "note";

fn client_doc() -> (Doc, yrs::TextRef) {
    let doc = Doc::new();
    let text = doc.get_or_insert_text(ROOT);
    (doc, text)
}

fn insert(text: &yrs::TextRef, doc: &Doc, index: u32, value: &str) {
    let mut txn = doc.transact_mut();
    text.insert(&mut txn, index, value);
}

fn full_update(doc: &Doc) -> Vec<u8> {
    doc.transact()
        .encode_state_as_update_v1(&StateVector::default())
}

async fn setup() -> (TestApp, DeviceSecret, WorkspaceInfo) {
    let app = TestApp::new().await;
    let device = app.enroll("desktop").await;
    let workspace = app.create_workspace(&device, "协作").await;
    (app, device, workspace)
}

#[tokio::test]
async fn updates_persist_and_survive_restart() {
    let (app, _device, workspace) = setup().await;
    let server = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("load doc");

    let (client, text) = client_doc();
    insert(&text, &client, 0, "hello");
    let update = full_update(&client);

    let accepted = apply_client_update(&app.state, &server, &update, true)
        .await
        .expect("apply");
    assert!(accepted.is_some());
    assert_eq!(server.text(), "hello");

    // 模拟进程重启：卸载后从快照 + 尾部重建。
    app.state.documents.unload(&workspace.id);
    let reloaded = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("reload");
    assert_eq!(reloaded.text(), "hello");
    assert_eq!(reloaded.state_vector(), server.state_vector());
}

#[tokio::test]
async fn two_clients_converge_through_server() {
    let (app, _device, workspace) = setup().await;
    let server = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("load doc");

    let (a, text_a) = client_doc();
    let (b, text_b) = client_doc();

    insert(&text_a, &a, 0, "AAA");
    insert(&text_b, &b, 0, "BBB");

    let ua = full_update(&a);
    let ub = full_update(&b);
    assert!(apply_client_update(&app.state, &server, &ua, true)
        .await
        .expect("apply a")
        .is_some());
    assert!(apply_client_update(&app.state, &server, &ub, true)
        .await
        .expect("apply b")
        .is_some());

    // 双方从服务端补齐缺失部分。
    let diff_a = server
        .diff_since(&a.transact().state_vector().encode_v1())
        .expect("diff a");
    let diff_b = server
        .diff_since(&b.transact().state_vector().encode_v1())
        .expect("diff b");
    a.transact_mut()
        .apply_update(Update::decode_v1(&diff_a).expect("decode a"))
        .expect("apply a");
    b.transact_mut()
        .apply_update(Update::decode_v1(&diff_b).expect("decode b"))
        .expect("apply b");

    let text_of_a = text_a.get_string(&a.transact());
    let text_of_b = text_b.get_string(&b.transact());
    assert_eq!(text_of_a, text_of_b, "双客户端必须收敛");
    assert_eq!(text_of_a, server.text());
    assert!(text_of_a.contains("AAA") && text_of_a.contains("BBB"));
}

#[tokio::test]
async fn readonly_updates_are_dropped() {
    let (app, _device, workspace) = setup().await;
    let server = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("load doc");

    let (client, text) = client_doc();
    insert(&text, &client, 0, "should-not-land");
    let update = full_update(&client);

    let accepted = apply_client_update(&app.state, &server, &update, false)
        .await
        .expect("ro apply");
    assert!(accepted.is_none(), "只读连接的 update 必须被丢弃");
    assert_eq!(server.text(), "");
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM doc_updates WHERE workspace_id = ?")
        .bind(&workspace.id)
        .fetch_one(app.state.shared.pool())
        .await
        .expect("count updates");
    assert_eq!(rows, 0, "被丢弃的 update 不能落库");
}

#[tokio::test]
async fn snapshot_compacts_tail_and_preserves_state() {
    let (app, _device, workspace) = setup().await;
    let server = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("load doc");

    let (client, text) = client_doc();
    // 逐条提交，触发阈值快照。
    for i in 0..SNAPSHOT_EVERY_UPDATES {
        let mut txn = client.transact_mut();
        text.insert(&mut txn, i as u32, "x");
        drop(txn);
        let update = full_update(&client);
        assert!(apply_client_update(&app.state, &server, &update, true)
            .await
            .expect("apply")
            .is_some());
    }

    let snapshots: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace_notes WHERE workspace_id = ?")
            .bind(&workspace.id)
            .fetch_one(app.state.shared.pool())
            .await
            .expect("count snapshots");
    assert_eq!(snapshots, 1, "阈值后应有快照");
    let tail: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM doc_updates WHERE workspace_id = ?")
        .bind(&workspace.id)
        .fetch_one(app.state.shared.pool())
        .await
        .expect("count tail");
    let snapshot_limit = i64::try_from(SNAPSHOT_EVERY_UPDATES).unwrap_or(i64::MAX);
    assert!(tail < snapshot_limit, "快照后尾部应被清理");

    let expected = server.text();
    app.state.documents.unload(&workspace.id);
    let reloaded = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("reload");
    assert_eq!(reloaded.text(), expected);
    assert_eq!(reloaded.text().len(), SNAPSHOT_EVERY_UPDATES as usize);
}

#[tokio::test]
async fn oversized_update_is_rejected_without_persisting() {
    let (app, _device, workspace) = setup().await;
    let server = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("load doc");

    let oversized = vec![0u8; 1024 * 1024 + 1];
    let error = apply_client_update(&app.state, &server, &oversized, true)
        .await
        .expect_err("oversize must fail");
    assert!(
        matches!(error, cista_server::error::AppError::PayloadTooLarge),
        "应当 413"
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM doc_updates WHERE workspace_id = ?")
        .bind(&workspace.id)
        .fetch_one(app.state.shared.pool())
        .await
        .expect("count updates");
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn snapshot_can_be_forced() {
    let (app, _device, workspace) = setup().await;
    let server = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("load doc");

    let (client, text) = client_doc();
    insert(&text, &client, 0, "snap");
    let update = full_update(&client);
    apply_client_update(&app.state, &server, &update, true)
        .await
        .expect("apply");

    snapshot(&app.state, &server).await.expect("snapshot");
    let state_bytes: Vec<u8> =
        sqlx::query_scalar("SELECT yjs_state FROM workspace_notes WHERE workspace_id = ?")
            .bind(&workspace.id)
            .fetch_one(app.state.shared.pool())
            .await
            .expect("snapshot row");
    assert!(!state_bytes.is_empty());
    let tail: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM doc_updates WHERE workspace_id = ?")
        .bind(&workspace.id)
        .fetch_one(app.state.shared.pool())
        .await
        .expect("count tail");
    assert_eq!(tail, 0);
}

#[tokio::test]
async fn ws_route_requires_handshake() {
    let app = TestApp::new().await;
    // 非升级请求不能建立会话（405/400），且不会创建文档。
    let (status, _) = app
        .send(app.signed(&app.enroll("desktop").await, "GET", "/ws", b""))
        .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UPGRADE_REQUIRED,
        "unexpected status: {status}"
    );
}

#[tokio::test]
async fn archived_workspace_rejects_updates_and_snapshot() {
    let (app, _device, workspace) = setup().await;
    let server = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("load doc");

    let (first, first_text) = client_doc();
    insert(&first_text, &first, 0, "before-archive");
    apply_client_update(&app.state, &server, &full_update(&first), true)
        .await
        .expect("apply before archive");

    sqlx::query("UPDATE workspaces SET state = 'archived' WHERE id = ?")
        .bind(&workspace.id)
        .execute(app.state.shared.pool())
        .await
        .expect("archive workspace");

    let (second, second_text) = client_doc();
    insert(&second_text, &second, 0, "after-archive");
    let error = apply_client_update(&app.state, &server, &full_update(&second), true)
        .await
        .expect_err("archived workspace must reject updates");
    assert!(
        matches!(error, cista_server::error::AppError::Gone),
        "应当 410: {error:?}"
    );

    let error = snapshot(&app.state, &server)
        .await
        .expect_err("archived workspace must not snapshot");
    assert!(matches!(error, cista_server::error::AppError::Gone));

    let tail: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM doc_updates WHERE workspace_id = ?")
        .bind(&workspace.id)
        .fetch_one(app.state.shared.pool())
        .await
        .expect("count tail");
    assert_eq!(tail, 1, "归档后的 update 不得落库");

    app.state.documents.unload(&workspace.id);
    let reloaded = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("reload");
    assert_eq!(reloaded.text(), "before-archive");
}

#[tokio::test]
async fn concurrent_snapshots_keep_latest_state() {
    let (app, _device, workspace) = setup().await;
    let server = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("load doc");
    let (client, text) = client_doc();
    insert(&text, &client, 0, "latest");
    apply_client_update(&app.state, &server, &full_update(&client), true)
        .await
        .expect("apply");

    let (a, b) = tokio::join!(snapshot(&app.state, &server), snapshot(&app.state, &server));
    a.expect("snapshot a");
    b.expect("snapshot b");

    app.state.documents.unload(&workspace.id);
    let reloaded = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("reload");
    assert_eq!(reloaded.text(), "latest");
}
