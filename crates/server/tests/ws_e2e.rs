//! Phase 3 端到端：真实 WebSocket 客户端（tokio-tungstenite）。
//!
//! 覆盖：首帧 JSON 握手 → 服务端 SyncStep1 → 双端 update 收敛 →
//! 只读（`note=ro`）连接的 update 被丢弃且不影响可写连接。

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use axum::http::StatusCode;
use cista_core::share::{FilesMode, NoteMode, ShareKind};
use cista_server::wire::{self, WireMessage};
use cista_server::yjs::ROOT_TEXT;
use common::{link_request, TestApp};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message as TMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use yrs::updates::decoder::Decode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn recv_frame(ws: &mut Ws) -> Vec<u8> {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("frame timeout")
            .expect("stream ended")
            .expect("frame error");
        match message {
            TMessage::Binary(bytes) => return bytes.to_vec(),
            TMessage::Close(_) => panic!("connection closed"),
            _ => continue,
        }
    }
}

async fn connect(addr: SocketAddr, token: &str, workspace: &str) -> Ws {
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("connect");
    let handshake = json!({ "v": 1, "mode": "share", "token": token, "object": workspace });
    ws.send(TMessage::Text(handshake.to_string().into()))
        .await
        .expect("handshake");
    let frame = recv_frame(&mut ws).await;
    assert!(
        matches!(wire::parse(&frame), Ok(WireMessage::SyncStep1(_))),
        "握手后的第一帧必须是 SyncStep1"
    );
    ws
}

fn apply(doc: &Doc, bytes: &[u8]) {
    doc.transact_mut()
        .apply_update(Update::decode_v1(bytes).expect("decode update"))
        .expect("apply update");
}

/// 收到的是完整线帧（tag + 子类型 + 载荷），先剥壳再交给 yrs。
fn apply_frame(doc: &Doc, frame: &[u8]) {
    let payload = match wire::parse(frame).expect("parse frame") {
        WireMessage::Update(bytes) | WireMessage::SyncStep2(bytes) => bytes,
        other => panic!("unexpected frame: {other:?}"),
    };
    apply(doc, &payload);
}

fn full_update(doc: &Doc) -> Vec<u8> {
    doc.transact()
        .encode_state_as_update_v1(&StateVector::default())
}

fn text_of(doc: &Doc) -> String {
    doc.get_or_insert_text(ROOT_TEXT)
        .get_string(&doc.transact())
}

fn insert(doc: &Doc, offset: u32, value: &str) {
    doc.get_or_insert_text(ROOT_TEXT)
        .insert(&mut doc.transact_mut(), offset, value);
}

#[tokio::test]
async fn ws_sync_converges_and_drops_readonly_updates() {
    let app = TestApp::new().await;
    let device = app.enroll("ws-device").await;
    let workspace = app.create_workspace(&device, "WS 工作区").await;

    let (request, rw_token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Rw,
        FilesMode::Ro,
    );
    let (status, bytes) = app.create_link(&device, request).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let (request, ro_token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Ro,
        FilesMode::Ro,
    );
    let (status, _) = app.create_link(&device, request).await;
    assert_eq!(status, StatusCode::CREATED);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let router = app.router.clone();
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });

    let mut a = connect(addr, &rw_token, &workspace.id).await;
    let mut b = connect(addr, &rw_token, &workspace.id).await;

    let doc_a = Doc::new();
    let doc_b = Doc::new();

    // A 写入 → 服务端落库并广播（发送者也会收到自己的 echo）。
    insert(&doc_a, 0, "AAA");
    a.send(TMessage::Binary(wire::update(&full_update(&doc_a)).into()))
        .await
        .expect("send a");
    apply_frame(&doc_a, &recv_frame(&mut a).await);
    apply_frame(&doc_b, &recv_frame(&mut b).await);
    assert_eq!(text_of(&doc_b), "AAA", "B 必须收到 A 的 update");

    // B 追加 → A 收敛。
    insert(&doc_b, 3, "BBB");
    b.send(TMessage::Binary(wire::update(&full_update(&doc_b)).into()))
        .await
        .expect("send b");
    apply_frame(&doc_b, &recv_frame(&mut b).await);
    apply_frame(&doc_a, &recv_frame(&mut a).await);

    assert_eq!(text_of(&doc_a), "AAABBB");
    assert_eq!(text_of(&doc_b), "AAABBB");
    let server_doc = app
        .state
        .documents
        .get_or_load(&app.state, &workspace.id)
        .await
        .expect("server doc");
    assert_eq!(server_doc.text(), "AAABBB", "服务端文档必须收敛");

    // 只读连接：握手成功、能收 sync，但 update 被丢弃。
    let mut c = connect(addr, &ro_token, &workspace.id).await;
    let doc_c = Doc::new();
    insert(&doc_c, 0, "ZZZ");
    c.send(TMessage::Binary(wire::update(&full_update(&doc_c)).into()))
        .await
        .expect("send ro");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        server_doc.text(),
        "AAABBB",
        "只读连接的 update 必须被服务端丢弃"
    );

    // 只读连接不影响可写连接：A 继续能写并被 B 看到。
    insert(&doc_a, 6, "!");
    a.send(TMessage::Binary(wire::update(&full_update(&doc_a)).into()))
        .await
        .expect("send a again");
    apply_frame(&doc_a, &recv_frame(&mut a).await);
    apply_frame(&doc_b, &recv_frame(&mut b).await);
    assert_eq!(text_of(&doc_b), "AAABBB!");
    assert_eq!(server_doc.text(), "AAABBB!");
}
