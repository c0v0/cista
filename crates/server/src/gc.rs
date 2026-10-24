//! 一次性回收：过期/用尽的分享链接、过期票据、无人引用的共享副本（ADR 0004 缺口）。
//!
//! 只操作共享库；私有库与私有原件不参与（ADR 0001）。
//! 由运维通过 SSH 执行：`cista-server gc`（systemd 定时器或手工）。

use crate::config::Config;
use crate::db::SharedStore;
use crate::quota;
use crate::state::now;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub expired_links: u64,
    pub released_blobs: u64,
    pub expired_tickets: u64,
}

impl std::fmt::Display for GcReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "expired_links={} released_blobs={} expired_tickets={}",
            self.expired_links, self.released_blobs, self.expired_tickets
        )
    }
}

/// 打开 `data_dir` 并回收；供 CLI 使用。
pub async fn run(cfg: &Config) -> anyhow::Result<GcReport> {
    let shared = SharedStore::connect(
        &cfg.data_dir.join("shared.db"),
        cfg.data_dir.join("blobs").join("shared"),
    )
    .await?;
    run_on(&shared).await
}

/// 在已打开的共享库上回收（测试直接调用）。
pub async fn run_on(shared: &SharedStore) -> anyhow::Result<GcReport> {
    let mut report = GcReport::default();
    let timestamp = now();

    // 过期（expires_at）或用尽（max_uses）但还没标记撤销的链接。
    let links: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, kind, object_id FROM share_links
         WHERE revoked_at IS NULL
           AND ((expires_at IS NOT NULL AND expires_at <= ?)
             OR (max_uses IS NOT NULL AND used_count >= max_uses))",
    )
    .bind(timestamp)
    .fetch_all(shared.pool())
    .await?;

    for (id, kind, object_id) in links {
        let affected = sqlx::query(
            "UPDATE share_links SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(timestamp)
        .bind(&id)
        .execute(shared.pool())
        .await?
        .rows_affected();
        if affected == 0 {
            continue;
        }
        report.expired_links += 1;
        if kind == "file" && release_shared_copy(shared, &object_id, &mut report).await? {
            tracing::info!(link = %id, blob = %object_id, "gc released shared copy");
        }
    }

    report.expired_tickets = sqlx::query("DELETE FROM blob_tickets WHERE expires_at <= ?")
        .bind(timestamp)
        .execute(shared.pool())
        .await?
        .rows_affected();

    Ok(report)
}

/// 与 `api::share::release_shared_copy` 同语义：引用归零时删元数据、本体并释放实例配额。
///
/// 返回是否真的删掉了本体。
async fn release_shared_copy(
    shared: &SharedStore,
    hash: &str,
    report: &mut GcReport,
) -> anyhow::Result<bool> {
    let mut tx = shared.pool().begin().await?;
    let row: Option<(i64, i64)> =
        sqlx::query_as("SELECT size, refcount FROM blobs WHERE hash = ? AND scope = 'shared'")
            .bind(hash)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((size, refcount)) = row else {
        return Ok(false);
    };
    let remaining = refcount - 1;
    if remaining <= 0 {
        sqlx::query("DELETE FROM blobs WHERE hash = ? AND scope = 'shared'")
            .bind(hash)
            .execute(&mut *tx)
            .await?;
        quota::release_instance(&mut tx, u64::try_from(size).unwrap_or(0)).await?;
    } else {
        sqlx::query("UPDATE blobs SET refcount = ? WHERE hash = ? AND scope = 'shared'")
            .bind(remaining)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    if remaining <= 0 {
        let _ = tokio::fs::remove_file(shared.blob_path(hash)).await;
        report.released_blobs += 1;
        return Ok(true);
    }
    Ok(false)
}
