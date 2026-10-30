//! `/ws`：首帧 JSON 握手（30 秒内），之后是 y-sync 线协议二进制帧（见 `wire.rs`）。
//!
//! - 分享模式：`{"v":1,"mode":"share","token":"…","object":"<workspace id>"}`
//! - 所有者模式：`{"v":1,"mode":"owner","device":"…","object":"…","nonce":"…","ts":…,"sig":"…"}`
//!   签名素材见 `cista_core::auth::RequestMeta`：method=`WS`、path=`/ws`、body=原始握手 JSON。
//!
//! 只读（`note=ro`）连接照常收 sync/awareness，但其 `update` 在服务端被丢弃（ADR 0006）。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use cista_core::auth::{self, RequestMeta};
use cista_core::share::{can_read_notes, can_write_notes, ShareKind};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::error::AppError;
use crate::public::{ensure_public, load_link, load_workspace, revalidate_link};
use crate::state::{now, AppState};
use crate::wire::{self, WireMessage};
use crate::yjs::{apply_client_update, Documents, WorkspaceDoc};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 握手 JSON 上限：正常请求只有几百字节。
pub const MAX_HANDSHAKE_BYTES: usize = 64 * 1024;
/// 单条 y-sync 消息上限，与 `yjs::MAX_UPDATE_BYTES` 一致。
pub const MAX_WS_MESSAGE: usize = crate::yjs::MAX_UPDATE_BYTES;
/// awareness 只转发光标，超过这个大小一律拒绝。
pub const MAX_AWARENESS_BYTES: usize = 64 * 1024;

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
    /// 分享模式下的口令（与文件端点同一个 `x-share-passphrase` 语义）。
    #[serde(default)]
    passphrase: Option<String>,
}

/// 长连接期间复核身份所需的凭据；token 只在内存里。
enum SessionAuth {
    Owner { device: String },
    Share { token: String, link_id: String },
}

struct Session {
    workspace_id: String,
    can_write: bool,
    frames: u32,
    window_start: i64,
    auth: SessionAuth,
}

/// WS 会话结束时释放活跃计数，允许文档被空闲回收。
struct ActiveSession {
    documents: Arc<Documents>,
    workspace_id: String,
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        self.documents.release(&self.workspace_id);
    }
}

enum Outcome {
    /// 只回给当前连接（SyncStep2）。
    Reply(Vec<u8>),
    /// 发给同工作区的其他连接（增量 update / awareness）；`Arc` 避免为每个订阅者复制。
    Broadcast(Arc<[u8]>),
    /// 丢弃（只读连接的 update 等）。
    None,
}

pub async fn handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&state, &headers) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    let Ok(handshake_permit) = state.ws_handshake_budget.clone().try_acquire_owned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "too many websocket handshakes",
        )
            .into_response();
    };
    // 握手消息很小；之后的 y-sync 帧仍受 `MAX_WS_MESSAGE`（= 单 update 上限）约束。
    let upgrade = upgrade
        .max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE);
    upgrade.on_upgrade(move |socket| async move {
        if let Err(error) = run_session(state, socket, handshake_permit).await {
            tracing::info!(error = ?error, "ws session closed");
        }
    })
}

/// 浏览器必须同源或命中 `CISTA_CORS_ORIGIN`；原生客户端通常不带 Origin。
fn origin_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return true;
    };
    let origin = origin.trim_end_matches('/');
    if let Some(configured) = state.cfg.cors_origin.as_deref() {
        return origin.eq_ignore_ascii_case(configured.trim_end_matches('/'));
    }
    if origin.eq_ignore_ascii_case("tauri://localhost")
        || origin.eq_ignore_ascii_case("http://tauri.localhost")
        || origin.eq_ignore_ascii_case("https://tauri.localhost")
    {
        return true;
    }
    let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .map(|rest| rest.eq_ignore_ascii_case(host.trim()))
        .unwrap_or(false)
}

async fn run_session(
    state: AppState,
    mut socket: WebSocket,
    handshake_permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<(), AppError> {
    let raw = read_handshake(&mut socket).await?;
    let mut session = authenticate(&state, &raw).await?;
    // 只有通过握手鉴权的连接才占长连接额度；未认证连接最多占 handshake 额度。
    let session_permit =
        state
            .ws_budget
            .clone()
            .try_acquire_owned()
            .map_err(|_| AppError::RateLimited {
                retry_after_secs: 1,
            })?;
    drop(handshake_permit);
    let _session_permit = session_permit;
    let (connection_key, connection_max) = match &session.auth {
        SessionAuth::Owner { device } => (format!("ws:dev:{device}"), 16),
        SessionAuth::Share { link_id, .. } => (format!("ws:link:{link_id}"), 8),
    };
    let _connection = state
        .acquire_connection(&connection_key, connection_max)
        .ok_or(AppError::RateLimited {
            retry_after_secs: 1,
        })?;
    // 活跃会话期间禁止空闲回收（Drop 时自动 release）。
    state.documents.acquire(&session.workspace_id);
    let _active = ActiveSession {
        documents: Arc::clone(&state.documents),
        workspace_id: session.workspace_id.clone(),
    };
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
    let mut revalidate = tokio::time::interval(Duration::from_secs(10));
    revalidate.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // 跳过 interval 的立即首 tick；握手时已经完整校验过一次。
    revalidate.tick().await;
    tracing::info!(
        workspace = %session.workspace_id,
        can_write = session.can_write,
        "ws session opened"
    );

    loop {
        tokio::select! {
            _ = revalidate.tick() => {
                if !session_still_valid(&state, &mut session).await {
                    break;
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(WsMessage::Binary(bytes))) => {
                        // 每连接限速：每分钟 240 帧，超出直接断开。
                        let now_ts = now();
                        if now_ts.saturating_sub(session.window_start) >= 60 {
                            session.window_start = now_ts;
                            session.frames = 0;
                        }
                        session.frames = session.frames.saturating_add(1);
                        if session.frames > 240 {
                            break;
                        }
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
                        if sink.send(WsMessage::Binary(frame.to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                    // 慢客户端落后超过队列容量：断开让它重连并重新同步，避免无限堆积。
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    // 只在有未快照的 update 时合并；否则每次断开都写整份文档会被重连风暴放大。
    if doc.has_pending_updates() {
        if let Err(error) = crate::yjs::snapshot(&state, &doc).await {
            tracing::warn!(workspace = %session.workspace_id, error = ?error, "ws snapshot on close failed");
        }
    }
    Ok(())
}

/// 10 秒一次的撤销/过期复核；失败即断开，避免长连接绕过 HTTP 的即时鉴权。
async fn session_still_valid(state: &AppState, session: &mut Session) -> bool {
    let workspace = match load_workspace(state, &session.workspace_id).await {
        Ok(workspace) => workspace,
        Err(_) => return false,
    };
    if ensure_public(&workspace).is_err() {
        return false;
    }
    match &session.auth {
        SessionAuth::Owner { device } => {
            let row: Result<Option<(Option<i64>,)>, _> =
                sqlx::query_as("SELECT revoked_at FROM devices WHERE id = ?")
                    .bind(device)
                    .fetch_optional(state.private.pool())
                    .await;
            matches!(row, Ok(Some((None,))))
        }
        SessionAuth::Share { token, .. } => {
            match revalidate_link(state, token, ShareKind::Workspace).await {
                Ok(link) => {
                    if link.object_id != session.workspace_id || !can_read_notes(link.note_scopes) {
                        return false;
                    }
                    session.can_write = can_write_notes(link.note_scopes);
                    true
                }
                Err(_) => false,
            }
        }
    }
}

async fn read_handshake(socket: &mut WebSocket) -> Result<Vec<u8>, AppError> {
    let frame = tokio::time::timeout(HANDSHAKE_TIMEOUT, socket.recv())
        .await
        .map_err(|_| AppError::Unauthorized)?;
    match frame {
        Some(Ok(WsMessage::Text(text))) => {
            let bytes = text.as_str().as_bytes();
            if bytes.len() > MAX_HANDSHAKE_BYTES {
                return Err(AppError::PayloadTooLarge);
            }
            Ok(bytes.to_vec())
        }
        Some(Ok(WsMessage::Binary(bytes))) => {
            if bytes.len() > MAX_HANDSHAKE_BYTES {
                return Err(AppError::PayloadTooLarge);
            }
            Ok(bytes.to_vec())
        }
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
            crate::public::require_passphrase_value(state, &link, handshake.passphrase.as_deref())
                .await?;
            let workspace = load_workspace(state, &link.object_id).await?;
            ensure_public(&workspace)?;
            if !can_read_notes(link.note_scopes) {
                // `note=off` 的链接不能连正文。
                return Err(AppError::Forbidden);
            }
            Ok(Session {
                workspace_id: link.object_id,
                can_write: can_write_notes(link.note_scopes),
                frames: 0,
                window_start: now(),
                auth: SessionAuth::Share {
                    token: token.to_string(),
                    link_id: link.id.clone(),
                },
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
                .check_and_record(device, &nonce, now());
            if !fresh {
                return Err(AppError::Unauthorized);
            }

            let workspace = load_workspace(state, &handshake.object).await?;
            ensure_public(&workspace)?;
            Ok(Session {
                workspace_id: handshake.object,
                can_write: true,
                frames: 0,
                window_start: now(),
                auth: SessionAuth::Owner {
                    device: device.to_string(),
                },
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
            if diff.len() > crate::yjs::MAX_DOC_BYTES {
                return Err(AppError::PayloadTooLarge);
            }
            Ok(Outcome::Reply(wire::sync_step2(&diff)))
        }
        WireMessage::SyncStep2(update) | WireMessage::Update(update) => {
            match apply_client_update(state, doc, &update, session.can_write).await? {
                Some(bytes) => Ok(Outcome::Broadcast(Arc::from(wire::update(&bytes)))),
                None => Ok(Outcome::None),
            }
        }
        WireMessage::Awareness(payload) => {
            if payload.len() > MAX_AWARENESS_BYTES {
                return Err(AppError::PayloadTooLarge);
            }
            // v1 只转发光标，不落库（awareness 是临时状态）。
            Ok(Outcome::Broadcast(Arc::from(wire::awareness(&payload))))
        }
        WireMessage::AwarenessQuery | WireMessage::Other => Ok(Outcome::None),
    }
}
