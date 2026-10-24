//! Phase 5：`cista-server gc` —— 过期链接回收、共享副本释放、过期票据清理。

mod common;

use axum::http::StatusCode;
use cista_core::protocol::LinkInfo;
use cista_core::share::{FilesMode, NoteMode, ShareKind};
use common::{link_request, now_ts, TestApp};

#[tokio::test]
async fn gc_revokes_expired_links_and_releases_blobs() {
    let app = TestApp::new().await;
    let device = app.enroll("gc-device").await;
    let blob = app.upload_private_blob(&device, b"gc-blob-content").await;

    let (mut request, _token) =
        link_request(ShareKind::File, &blob.hash, NoteMode::Ro, FilesMode::Ro);
    request.expires_at = Some(now_ts() - 10);
    let (status, bytes) = app.create_link(&device, request).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let link: LinkInfo = TestApp::json(&bytes);

    // 共享副本在创建链接时已复制进来。
    let shared_ref: Option<(i64,)> =
        sqlx::query_as("SELECT refcount FROM blobs WHERE hash = ? AND scope = 'shared'")
            .bind(&blob.hash)
            .fetch_optional(app.state.shared.pool())
            .await
            .expect("shared blob");
    assert!(shared_ref.is_some(), "创建文件链接应复制共享副本");

    // 一条已过期的下载票据，验证 gc 会清掉。
    sqlx::query(
        "INSERT INTO blob_tickets (ticket_hash, link_id, file_id, blob_hash, expires_at, created_at)
         VALUES (?, ?, NULL, ?, ?, ?)",
    )
    .bind("expired-ticket")
    .bind(&link.id)
    .bind(&blob.hash)
    .bind(now_ts() - 5)
    .bind(now_ts() - 100)
    .execute(app.state.shared.pool())
    .await
    .expect("seed ticket");

    let report = cista_server::gc::run(&app.state.cfg).await.expect("gc");
    assert_eq!(report.expired_links, 1, "{report}");
    assert_eq!(report.released_blobs, 1, "{report}");
    assert_eq!(report.expired_tickets, 1, "{report}");

    let revoked_at: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM share_links WHERE id = ?")
            .bind(&link.id)
            .fetch_one(app.state.shared.pool())
            .await
            .expect("link");
    assert!(revoked_at.is_some(), "过期链接必须被标记撤销");

    let shared_ref: Option<(i64,)> =
        sqlx::query_as("SELECT refcount FROM blobs WHERE hash = ? AND scope = 'shared'")
            .bind(&blob.hash)
            .fetch_optional(app.state.shared.pool())
            .await
            .expect("shared blob");
    assert!(shared_ref.is_none(), "引用归零后必须删除共享 blob 元数据");

    let tickets: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blob_tickets")
        .fetch_one(app.state.shared.pool())
        .await
        .expect("tickets");
    assert_eq!(tickets, 0, "过期票据必须被清理");

    // 再跑一次是幂等的。
    let report = cista_server::gc::run(&app.state.cfg).await.expect("gc 2");
    assert_eq!(report.expired_links, 0, "{report}");
}
