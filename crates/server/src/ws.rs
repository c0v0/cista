//! `/ws`：首帧 JSON 握手（30 秒内），之后是 y-sync 线协议二进制帧（见 `wire.rs`）。
//!
//! - 分享模式：`{"v":1,"mode":"share","token":"…","object":"<workspace id>"}`
//! - 所有者模式：`{"v":1,"mode":"owner","device":"…","object":"…","nonce":"…","ts":…,"sig":"…"}`
//!   签名素材见 `cista_core::auth::RequestMeta`：method=`WS`、path=`/ws`、body=原始握手 JSON。
//!
//! 只读（`note=ro`）连接照常收 sync/awareness，但其 `update` 在服务端被丢弃（ADR 0006）。

use std::time::Duration;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use cista_core::auth::{self, RequestMeta};
use cista_core::share::{can_read_notes, can_write_notes, ShareKind};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::error::AppError;
use crate::public::{ensure_public, load_link, load_workspace};
use crate::state::{now, AppState};
use crate::wire::{self, WireMessage};
use crate::yjs::{apply_client_update, WorkspaceDoc};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct Handshake {
    v: u32,
    mode: String,
    object: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    sig: Option<String>,
}

struct Session {
    workspace_id: String,
    can_write: bool,
}

enum Outcome {
    /// 只回给当前连接（SyncStep2）。
    Reply(Vec<u8>),
    /// 发给同工作区的其他连接（增量 update / awareness）。
    Broadcast(Vec<u8>),
    /// 丢弃（只读连接的 update 等）。
    None,
}

pub async fn handler(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| async move {
        if let Err(error) = run_session(state, socket).await {
            tracing::info!(error = ?error, "ws session closed");
        }
    })
}

async fn run_session(state: AppState, mut socket: WebSocket) -> Result<(), AppError> {
    let raw = read_handshake(&mut socket).await?;
    let session = authenticate(&state, &raw).await?;
    let doc = state
        .documents
        .get_or_load(&state, &session.workspace_id)
        .await?;

    let opening = wire::sync_step1(&doc.state_vector());
    let (mut sink, mut stream) = socket.split();
    sink.send(WsMessage::Binary(opening.into()))
        .await
        .map_err(|_| AppError::Internal(anyhow::anyhow!("websocket send failed")))?;

    let mut broadcasts = state.ws_events.subscribe(&session.workspace_id);
    tracing::info!(
        workspace = %session.workspace_id,
        can_write = session.can_write,
        "ws session opened"
    );

    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(WsMessage::Binary(bytes))) => {
                        match handle_frame(&state, &doc, &session, &bytes).await? {
                            Outcome::Reply(frame) => {
                                if sink.send(WsMessage::Binary(frame.into())).await.is_err() {
                                    break;
                                }
                            }
                            Outcome::Broadcast(frame) => {
                                state.ws_events.publish(&session.workspace_id, frame);
                            }
                            Outcome::None => {}
                        }
                    }
                    Some(Ok(WsMessage::Ping(payload))) => {
                        if sink.send(WsMessage::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => break,
                }
            }
            broadcast = broadcasts.recv() => {
                match broadcast {
                    Ok(frame) => {
                        if sink.send(WsMessage::Binary(frame.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    // 断开时把未快照的尾部合并进快照，避免尾部无限增长。
    if let Err(error) = crate::yjs::snapshot(&state, &doc).await {
        tracing::warn!(workspace = %session.workspace_id, error = ?error, "ws snapshot on close failed");
    }
    Ok(())
}

async fn read_handshake(socket: &mut WebSocket) -> Result<Vec<u8>, AppError> {
    let frame = tokio::time::timeout(HANDSHAKE_TIMEOUT, socket.recv())
        .await
        .map_err(|_| AppError::Unauthorized)?;
    match frame {
        Some(Ok(WsMessage::Text(text))) => Ok(text.as_str().as_bytes().to_vec()),
        Some(Ok(WsMessage::Binary(bytes))) => Ok(bytes.to_vec()),
        _ => Err(AppError::Unauthorized),
    }
}

async fn authenticate(state: &AppState, raw: &[u8]) -> Result<Session, AppError> {
    let handshake: Handshake = serde_json::from_slice(raw).map_err(|_| AppError::Unauthorized)?;
    if handshake.v != 1 {
        return Err(AppError::Unauthorized);
    }

    match handshake.mode.as_str() {
        "share" => {
            let token = handshake.token.as_deref().ok_or(AppError::Unauthorized)?;
            let link = load_link(state, token, ShareKind::Workspace).await?;
            if link.object_id != handshake.object {
                return Err(AppError::Forbidden);
            }
            let workspace = load_workspace(state, &link.object_id).await?;
            ensure_public(&workspace)?;
            if !can_read_notes(link.note_scopes) {
                // `note=off` 的链接不能连正文。
                return Err(AppError::Forbidden);
            }
            Ok(Session {
                workspace_id: link.object_id,
                can_write: can_write_notes(link.note_scopes),
            })
        }
        "owner" => {
            let device = handshake.device.as_deref().ok_or(AppError::Unauthorized)?;
            let nonce_hex = handshake.nonce.as_deref().ok_or(AppError::Unauthorized)?;
            let timestamp = handshake.ts.ok_or(AppError::Unauthorized)?;
            let signature = handshake.sig.as_deref().ok_or(AppError::Unauthorized)?;

            let nonce = hex::decode(nonce_hex).map_err(|_| AppError::Unauthorized)?;
            auth::validate_nonce(&nonce).map_err(|_| AppError::Unauthorized)?;

            let row: Option<(Vec<u8>,)> =
                sqlx::query_as("SELECT pubkey FROM devices WHERE id = ? AND revoked_at IS NULL")
                    .bind(device)
                    .fetch_optional(state.private.pool())
                    .await?;
            let (pubkey_bytes,) = row.ok_or(AppError::Unauthorized)?;
            let pubkey: [u8; 32] = pubkey_bytes
                .as_slice()
                .try_into()
                .map_err(|_| AppError::Unauthorized)?;

            let meta = RequestMeta::new("WS", "/ws", raw, &nonce, timestamp);
            auth::verify(&pubkey, &meta, signature, now()).map_err(|_| AppError::Unauthorized)?;
            let fresh = state
                .replay
                .lock()
                .map_err(|_| AppError::Internal(anyhow::anyhow!("replay cache poisoned")))?
                .check_and_record(device, nonce_hex, now());
            if !fresh {
                return Err(AppError::Unauthorized);
            }

            let workspace = load_workspace(state, &handshake.object).await?;
            ensure_public(&workspace)?;
            Ok(Session {
                workspace_id: handshake.object,
                can_write: true,
            })
        }
        _ => Err(AppError::Unauthorized),
    }
}

async fn handle_frame(
    state: &AppState,
    doc: &WorkspaceDoc,
    session: &Session,
    bytes: &[u8],
) -> Result<Outcome, AppError> {
    let message = wire::parse(bytes).map_err(|_| AppError::BadRequest("invalid_ws_frame"))?;
    match message {
        WireMessage::SyncStep1(state_vector) => {
            let diff = doc.diff_since(&state_vector)?;
            Ok(Outcome::Reply(wire::sync_step2(&diff)))
        }
        WireMessage::SyncStep2(update) | WireMessage::Update(update) => {
            match apply_client_update(state, doc, &update, session.can_write).await? {
                Some(bytes) => Ok(Outcome::Broadcast(wire::update(&bytes))),
                None => Ok(Outcome::None),
            }
        }
        WireMessage::Awareness(payload) => {
            // v1 只转发光标，不落库（awareness 是临时状态）。
            Ok(Outcome::Broadcast(wire::awareness(&payload)))
        }
        WireMessage::AwarenessQuery | WireMessage::Other => Ok(Outcome::None),
    }
}
