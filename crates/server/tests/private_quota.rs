//! 私有 blob 计入实例配额：按内容去重只记一次，超限返回 413 + scope=instance。

mod common;

use axum::http::StatusCode;
use common::TestApp;

const MIB: usize = 1024 * 1024;

fn blob(marker: u8) -> Vec<u8> {
    vec![marker; MIB]
}

async fn instance_usage(app: &TestApp) -> i64 {
    sqlx::query_scalar(
        "SELECT bytes FROM usage_counters WHERE scope = 'instance' AND key = 'total'",
    )
    .fetch_one(app.state.shared.pool())
    .await
    .expect("instance usage")
}

#[tokio::test]
async fn private_uploads_count_toward_instance_quota_once_per_hash() {
    let app = TestApp::new().await; // for_tests：单文件 1 MiB，实例配额 4 MiB
    let device = app.enroll("desktop").await;

    let first = app.upload_private_blob(&device, &blob(1)).await;
    // 同内容重复上传：仍成功，但不重复记账。
    let again = app.upload_private_blob(&device, &blob(1)).await;
    assert_eq!(again.hash, first.hash);
    assert_eq!(instance_usage(&app).await, MIB as i64, "同内容只计一次");

    // 再传 3 个不同内容：1 + 3 = 4 MiB，正好用满。
    for marker in 2..=4u8 {
        app.upload_private_blob(&device, &blob(marker)).await;
    }
    assert_eq!(instance_usage(&app).await, (4 * MIB) as i64);

    // 第 5 个不同内容 → 超出实例配额。
    let request = app.signed_with(
        &device,
        "POST",
        "/api/files",
        &blob(5),
        &[("idempotency-key", "quota-over")],
    );
    let (status, body) = app.send(request).await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let error: serde_json::Value = TestApp::json(&body);
    assert_eq!(error["error"], "quota_exceeded");
    assert_eq!(error["scope"], "instance");
}
