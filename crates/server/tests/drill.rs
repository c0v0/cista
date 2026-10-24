//! 恢复演练配套（`scripts/restore-drill.sh` 调用）。
//!
//! - `seed`：在 `CISTA_DRILL_DATA` 里造真实数据（设备、密封笔记、文件、单文件链接），
//!   并把 MK / 分享 token / 设备种子打到 stdout。
//! - `verify`：在恢复出来的目录上解笔记、走完分享下载两步，证明备份可用。
//!
//! 两个用例默认 `#[ignore]`，只在演练时显式运行：
//! `CISTA_DRILL_DATA=… cargo test -p cista-server --test drill -- --ignored seed --nocapture`

mod common;

use std::path::{Path, PathBuf};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use cista_core::auth::DeviceSecret;
use cista_core::crypto::{open_note, Key};
use cista_core::protocol::{BlobRef, ConfirmResponse, EnrollResponse, NoteRecord};
use cista_core::share::{FilesMode, NoteMode, ShareKind};
use cista_server::config::Config;
use cista_server::db::{PrivateStore, SharedStore};
use cista_server::state::AppState;
use common::{
    fresh_nonce, link_request, now_ts, put_body, sealed_note, signed_request,
    signed_request_with_headers, TestApp, BOOTSTRAP,
};
use tower::ServiceExt;

/// 固定种子：seed 与 verify 之间无需额外传递私钥文件。
const DEVICE_SEED: [u8; 32] = [7u8; 32];

struct Drill {
    router: axum::Router,
}

impl Drill {
    async fn open(dir: &Path) -> Self {
        let mut config = Config::for_tests(dir.to_path_buf());
        config.bootstrap_token = Some(BOOTSTRAP.to_string());
        let private = PrivateStore::connect(&dir.join("private.db"))
            .await
            .expect("private db");
        let shared = SharedStore::connect(
            &dir.join("shared.db"),
            dir.join("blobs").join("shared"),
        )
        .await
        .expect("shared db");
        let state = AppState::new(config, private, shared);
        Self {
            router: cista_server::app(state),
        }
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, Vec<u8>) {
        let response = self.router.clone().oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .expect("body")
            .to_vec();
        (status, bytes)
    }

    async fn enroll(&self) -> DeviceSecret {
        let secret = DeviceSecret::from_seed(&DEVICE_SEED);
        let body = serde_json::json!({
            "name": "drill-device",
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
            "enroll: {}",
            String::from_utf8_lossy(&bytes)
        );
        let response: EnrollResponse = TestApp::json(&bytes);
        assert_eq!(response.device_id, secret.device_id());
        secret
    }

    fn public(&self, method: &str, uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Share {token}"))
            .body(Body::empty())
            .expect("public request")
    }
}

fn hex32(name: &str) -> [u8; 32] {
    let raw = hex::decode(std::env::var(name).expect(name)).expect("hex");
    raw.as_slice().try_into().expect("32 bytes")
}

#[tokio::test]
#[ignore = "restore drill: 需要 CISTA_DRILL_DATA"]
async fn seed() {
    let dir = PathBuf::from(std::env::var("CISTA_DRILL_DATA").expect("CISTA_DRILL_DATA"));
    std::fs::create_dir_all(&dir).expect("create data dir");
    let drill = Drill::open(&dir).await;
    let device = drill.enroll().await;
    let mk = Key::generate().expect("mk");

    let sealed = sealed_note(&mk, "drill-note", 1, "演练笔记", "drill-plaintext-marker");
    let (status, bytes) = drill
        .send(signed_request(
            &device,
            "PUT",
            "/api/notes/drill-note",
            &put_body(sealed),
            &fresh_nonce(),
            now_ts(),
        ))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    let (status, bytes) = drill
        .send(signed_request_with_headers(
            &device,
            "POST",
            "/api/files",
            b"drill-blob-bytes",
            &fresh_nonce(),
            now_ts(),
            &[("idempotency-key", "drill-key")],
        ))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let blob: BlobRef = TestApp::json(&bytes);

    let (request, token) = link_request(ShareKind::File, &blob.hash, NoteMode::Off, FilesMode::Ro);
    let body = serde_json::to_vec(&request).expect("link json");
    let (status, bytes) = drill
        .send(signed_request(&device, "POST", "/api/share", &body, &fresh_nonce(), now_ts()))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    println!("CISTA_DRILL_MK={}", hex::encode(mk.as_bytes()));
    println!("CISTA_DRILL_TOKEN={token}");
    println!("CISTA_DRILL_DEVICE_SEED={}", hex::encode(DEVICE_SEED));
}

#[tokio::test]
#[ignore = "restore drill: 需要 CISTA_DRILL_DATA / CISTA_DRILL_MK / CISTA_DRILL_TOKEN"]
async fn verify() {
    let dir = PathBuf::from(std::env::var("CISTA_DRILL_DATA").expect("CISTA_DRILL_DATA"));
    let mk = Key::from_bytes(hex32("CISTA_DRILL_MK"));
    let token = std::env::var("CISTA_DRILL_TOKEN").expect("CISTA_DRILL_TOKEN");
    let device = DeviceSecret::from_seed(&hex32("CISTA_DRILL_DEVICE_SEED"));
    let drill = Drill::open(&dir).await;

    // 1) 私有笔记：用 MK 解出原明文。
    let (status, bytes) = drill
        .send(signed_request(&device, "GET", "/api/notes/drill-note", b"", &fresh_nonce(), now_ts()))
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let record: NoteRecord = TestApp::json(&bytes);
    let plaintext = open_note(&mk, "drill-note", &record.sealed).expect("open note");
    assert_eq!(plaintext.markdown, "drill-plaintext-marker");
    println!("note: MK decrypt ok（{} 字）", plaintext.markdown.len());

    // 2) 分享链接：meta → confirm → blob，内容逐字节一致。
    let (status, _) = drill
        .send(drill.public("GET", "/public/files/meta", &token))
        .await;
    assert_eq!(status, StatusCode::OK, "公开 meta 失败");
    let (status, bytes) = drill
        .send(drill.public("POST", "/public/files/confirm", &token))
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let confirm: ConfirmResponse = TestApp::json(&bytes);
    let uri = format!("/public/files/blob?ticket={}", confirm.ticket);
    let (status, downloaded) = drill.send(drill.public("GET", &uri, &token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(downloaded.as_slice(), b"drill-blob-bytes");
    println!("share: upload/download ok（{} 字节）", downloaded.len());
}
