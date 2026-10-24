//! 集成测试脚手架：临时目录 + 真实 SQLite（私有库/共享库）+ 真实 axum 路由。

#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use cista_core::auth::{DeviceSecret, RequestMeta};
use cista_core::crypto::{seal_note, Key, NotePlaintext, SealedNote};
use cista_core::protocol::{
    BlobRef, CreateLinkRequest, CreateWorkspaceRequest, EnrollResponse, NoteRecord, NotesPage,
    PutNoteRequest, WorkspaceInfo, WorkspaceOrigin,
};
use cista_core::share::{new_share_token, FilesMode, NoteMode, ShareKind};
use cista_server::config::Config;
use cista_server::db::{PrivateStore, SharedStore};
use cista_server::state::AppState;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

pub const BOOTSTRAP: &str = "test-bootstrap-token";

pub struct TestApp {
    pub state: AppState,
    pub router: Router,
    pub dir: TempDir,
}

impl TestApp {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    /// 自定义配置（限速/配额边界测试）。
    pub async fn with_config(apply: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = Config::for_tests(dir.path().to_path_buf());
        config.bootstrap_token = Some(BOOTSTRAP.to_string());
        apply(&mut config);
        let private = PrivateStore::connect(&dir.path().join("private.db"))
            .await
            .expect("open private db");
        let shared = SharedStore::connect(
            &dir.path().join("shared.db"),
            dir.path().join("blobs").join("shared"),
        )
        .await
        .expect("open shared db");
        let state = AppState::new(config, private, shared);
        let router = cista_server::app(state.clone());
        Self { state, router, dir }
    }

    pub async fn send(&self, request: Request<Body>) -> (StatusCode, Vec<u8>) {
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes()
            .to_vec();
        (status, bytes)
    }

    /// 用 bootstrap token 入网一台设备。
    pub async fn enroll(&self, name: &str) -> DeviceSecret {
        let secret = DeviceSecret::generate().expect("device key");
        let body = serde_json::json!({
            "name": name,
            "pubkey": hex::encode(secret.public_key()),
        })
        .to_string();
        let request = Request::builder()
            .method("POST")
            .uri("/api/devices/enroll")
            .header("authorization", format!("Bootstrap {BOOTSTRAP}"))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .expect("enroll request");
        let (status, bytes) = self.send(request).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "enroll failed: {}",
            String::from_utf8_lossy(&bytes)
        );
        let response: EnrollResponse = serde_json::from_slice(&bytes).expect("enroll json");
        assert_eq!(response.device_id, secret.device_id());
        secret
    }

    /// 用当前时间 + 新 nonce 构造签名请求。
    pub fn signed(
        &self,
        secret: &DeviceSecret,
        method: &str,
        uri: &str,
        body: &[u8],
    ) -> Request<Body> {
        self.signed_with(secret, method, uri, body, &[])
    }

    /// 同上，附加额外请求头（如 `If-Match`）。
    pub fn signed_with(
        &self,
        secret: &DeviceSecret,
        method: &str,
        uri: &str,
        body: &[u8],
        extra_headers: &[(&str, &str)],
    ) -> Request<Body> {
        let nonce = fresh_nonce();
        signed_request_with_headers(secret, method, uri, body, &nonce, now_ts(), extra_headers)
    }

    /// 公网分享请求：token 只放在 `Authorization: Share` 头里。
    pub fn public(
        &self,
        method: &str,
        uri: &str,
        token: &str,
        body: &[u8],
        headers: &[(&str, &str)],
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Share {token}"));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(Body::from(body.to_vec())).expect("request")
    }

    pub async fn create_workspace(&self, device: &DeviceSecret, title: &str) -> WorkspaceInfo {
        let body = serde_json::to_vec(&CreateWorkspaceRequest {
            title: title.to_string(),
            origin: WorkspaceOrigin::Blank,
            source_note_id: None,
            expires_at: None,
        })
        .expect("json");
        let (status, bytes) = self
            .send(self.signed(device, "POST", "/api/workspaces", &body))
            .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "workspace creation failed: {}",
            String::from_utf8_lossy(&bytes)
        );
        Self::json(&bytes)
    }

    pub async fn create_link(
        &self,
        device: &DeviceSecret,
        request: CreateLinkRequest,
    ) -> (StatusCode, Vec<u8>) {
        let body = serde_json::to_vec(&request).expect("json");
        self.send(self.signed(device, "POST", "/api/share", &body))
            .await
    }

    pub async fn upload_private_blob(&self, device: &DeviceSecret, bytes: &[u8]) -> BlobRef {
        let (status, body) = self
            .send(self.signed_with(
                device,
                "POST",
                "/api/files",
                bytes,
                &[("idempotency-key", "test-key")],
            ))
            .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "blob upload failed: {}",
            String::from_utf8_lossy(&body)
        );
        Self::json(&body)
    }

    pub fn json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> T {
        serde_json::from_slice(bytes).expect("json body")
    }
}

pub fn now_ts() -> i64 {
    cista_server::state::now()
}

pub fn fresh_nonce() -> [u8; 16] {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).expect("nonce");
    nonce
}

pub fn sign_at(
    secret: &DeviceSecret,
    method: &str,
    uri: &str,
    body: &[u8],
    nonce: &[u8],
    ts: i64,
) -> String {
    let meta = RequestMeta::new(method, uri, body, nonce, ts);
    secret.sign(&meta)
}

pub fn signed_request(
    secret: &DeviceSecret,
    method: &str,
    uri: &str,
    body: &[u8],
    nonce: &[u8],
    ts: i64,
) -> Request<Body> {
    signed_request_with_headers(secret, method, uri, body, nonce, ts, &[])
}

pub fn signed_request_with_headers(
    secret: &DeviceSecret,
    method: &str,
    uri: &str,
    body: &[u8],
    nonce: &[u8],
    ts: i64,
    extra_headers: &[(&str, &str)],
) -> Request<Body> {
    let signature = sign_at(secret, method, uri, body, nonce, ts);
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-device-id", secret.device_id())
        .header("x-nonce", hex::encode(nonce))
        .header("x-timestamp", ts.to_string())
        .header("x-signature", signature);
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(body.to_vec())).expect("request")
}

/// 客户端侧封装一篇笔记（CK 随机、AAD 绑定 id + rev）。
pub fn sealed_note(mk: &Key, id: &str, rev: u64, title: &str, markdown: &str) -> SealedNote {
    let ck = Key::generate().expect("ck");
    seal_note(
        mk,
        id,
        rev,
        &ck,
        &NotePlaintext {
            title: title.into(),
            markdown: markdown.into(),
        },
    )
    .expect("seal note")
}

pub fn put_body(sealed: SealedNote) -> Vec<u8> {
    serde_json::to_vec(&PutNoteRequest { sealed }).expect("put body")
}

pub fn note_record(bytes: &[u8]) -> NoteRecord {
    TestApp::json(bytes)
}

pub fn notes_page(bytes: &[u8]) -> NotesPage {
    TestApp::json(bytes)
}

/// 共享链接请求 + 对应 token（token 只出现在客户端）。
pub fn link_request(
    kind: ShareKind,
    object_id: &str,
    note_mode: NoteMode,
    files_mode: FilesMode,
) -> (CreateLinkRequest, String) {
    let token = new_share_token().expect("token");
    (
        CreateLinkRequest {
            kind,
            object_id: object_id.to_string(),
            token: token.clone(),
            title: Some("测试分享".to_string()),
            note_mode,
            files_mode,
            expires_at: None,
            max_uses: None,
            passphrase: None,
        },
        token,
    )
}
