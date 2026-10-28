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
    pub quarantined_workspaces: u64,
    pub orphan_blobs: u64,
}

impl std::fmt::Display for GcReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "expired_links={} released_blobs={} expired_tickets={} quarantined_workspaces={} orphan_blobs={}",
            self.expired_links,
            self.released_blobs,
            self.expired_tickets,
            self.quarantined_workspaces,
            self.orphan_blobs
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

    // 工作区到期：active → quarantined，公网与所有者 WS 都会拒绝；由所有者导出后 archive-complete。
    report.quarantined_workspaces = sqlx::query(
        "UPDATE workspaces
         SET state = 'quarantined', quarantined_at = ?
         WHERE state = 'active' AND expires_at IS NOT NULL AND expires_at <= ?",
    )
    .bind(timestamp)
    .bind(timestamp)
    .execute(shared.pool())
    .await?
    .rows_affected();

    report.orphan_blobs = cleanup_orphan_blobs(shared, timestamp).await?;

    Ok(report)
}

/// 删除 `blobs/shared` 中没有数据库行、且落盘超过 1 小时的孤儿文件。
///
/// 上传先落盘后记账/崩溃会留下孤儿；没有这一步，配额保护可被绕过、磁盘只增不减。
async fn cleanup_orphan_blobs(shared: &SharedStore, timestamp: i64) -> anyhow::Result<u64> {
    let dir = shared.blobs_dir().to_path_buf();
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(_) => return Ok(0),
    };
    let mut removed = 0u64;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') || name.len() != 64 {
            continue;
        }
        let metadata = entry.metadata().await?;
        if !metadata.is_file() {
            continue;
        }
        let age = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        if age < 3600 {
            continue;
        }
        let exists: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM blobs WHERE hash = ? AND scope = 'shared'")
                .bind(name)
                .fetch_optional(shared.pool())
                .await?;
        if exists.is_none() {
            let _ = tokio::fs::remove_file(entry.path()).await;
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::info!(removed, at = timestamp, "gc removed orphan shared blobs");
    }
    Ok(removed)
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
