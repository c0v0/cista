//! Phase 6 §2：日志脱敏。
//!
//! 用自定义 `MakeWriter` 捕获全部 tracing 输出，然后断言分享 token、口令、
//! 笔记正文标记都不出现在日志里（`docs/specs/phase6-hardening.md` §2）。

mod common;

use std::io;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use cista_core::crypto::Key;
use cista_core::share::{FilesMode, NoteMode, ShareKind};
use common::{link_request, put_body, sealed_note, TestApp};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("capture lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("capture lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter(self.0.clone())
    }
}

#[tokio::test]
async fn logs_never_contain_secrets() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("install capture subscriber");

    let app = TestApp::new().await;
    let device = app.enroll("日志设备").await;
    let workspace = app.create_workspace(&device, "日志工作区").await;

    let marker = "log-marker-never-3f9c";
    let mk = Key::generate().expect("mk");
    let note = sealed_note(&mk, "note-log", 1, "日志", marker);
    let (status, bytes) = app
        .send(app.signed(&device, "PUT", "/api/notes/note-log", &put_body(note)))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    let passphrase = "correct horse battery staple 7Q";
    let (mut request, token) = link_request(
        ShareKind::Workspace,
        &workspace.id,
        NoteMode::Ro,
        FilesMode::Ro,
    );
    request.passphrase = Some(passphrase.to_string());
    let (status, bytes) = app.create_link(&device, request).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    // 无口令 / 错口令都必须 401，且失败细节不进日志。
    let (status, _) = app
        .send(app.public("GET", "/public/workspaces/files", &token, b"", &[]))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = app
        .send(app.public(
            "GET",
            "/public/workspaces/files",
            &token,
            b"",
            &[("x-share-passphrase", "definitely-wrong-passphrase")],
        ))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let logs = String::from_utf8_lossy(&capture.0.lock().expect("capture lock")).to_string();
    assert!(!logs.is_empty(), "捕获层必须至少看到一条日志");
    for secret in [
        token.as_str(),
        passphrase,
        "definitely-wrong-passphrase",
        marker,
    ] {
        assert!(
            !logs.contains(secret),
            "日志里出现了敏感内容 {secret:?}:\n{logs}"
        );
    }
}
