//! 配额：上传前在事务里预留，删除时释放（`design.md` §9）。
//!
//! 并发正确性：进入 `reserve*` 先对实例计数行做一次空更新，等于取 SQLite 写锁，
//! 之后的读/写都是串行的；调用方必须在同一事务里写文件行并提交。
//!
//! 超限直接映射成 `AppError::Quota`（413），调用方用 `?` 即可。

use sqlx::{Sqlite, Transaction};

use crate::config::Quotas;
use crate::error::AppError;

/// 预留 `bytes`：实例 + 工作区 + 链接三层上限。
///
/// `count_instance=false` 用于内容去重：物理 blob 已存在时实例总量不重复计。
pub async fn reserve(
    tx: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    link_id: &str,
    bytes: u64,
    quotas: Quotas,
    count_instance: bool,
) -> Result<(), AppError> {
    let bytes = i64::try_from(bytes).map_err(|_| AppError::Quota("instance"))?;
    lock_instance(tx).await?;

    if count_instance {
        let instance_used: i64 = sqlx::query_scalar(
            "SELECT bytes FROM usage_counters WHERE scope = 'instance' AND key = 'total'",
        )
        .fetch_one(&mut **tx)
        .await?;
        if instance_used.saturating_add(bytes) > to_i64(quotas.instance_bytes) {
            return Err(AppError::Quota("instance"));
        }
    }

    sqlx::query(
        "INSERT INTO usage_counters (scope, key, bytes) VALUES ('workspace', ?, 0)
         ON CONFLICT(scope, key) DO NOTHING",
    )
    .bind(workspace_id)
    .execute(&mut **tx)
    .await?;

    let workspace_used: i64 = sqlx::query_scalar(
        "SELECT bytes FROM usage_counters WHERE scope = 'workspace' AND key = ?",
    )
    .bind(workspace_id)
    .fetch_one(&mut **tx)
    .await?;
    if workspace_used.saturating_add(bytes) > to_i64(quotas.workspace_bytes) {
        return Err(AppError::Quota("workspace"));
    }

    let link_used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(size), 0) FROM uploads
         WHERE link_id = ? AND status IN ('pending', 'complete')",
    )
    .bind(link_id)
    .fetch_one(&mut **tx)
    .await?;
    if link_used.saturating_add(bytes) > to_i64(quotas.link_bytes) {
        return Err(AppError::Quota("link"));
    }

    if count_instance {
        sqlx::query(
            "UPDATE usage_counters SET bytes = bytes + ? WHERE scope = 'instance' AND key = 'total'",
        )
        .bind(bytes)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query(
        "UPDATE usage_counters SET bytes = bytes + ? WHERE scope = 'workspace' AND key = ?",
    )
    .bind(bytes)
    .bind(workspace_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 单文件链接复制到共享域：只计实例总量（不属于任何工作区/链接配额）。
pub async fn reserve_instance(
    tx: &mut Transaction<'_, Sqlite>,
    bytes: u64,
    quotas: Quotas,
) -> Result<(), AppError> {
    let bytes = i64::try_from(bytes).map_err(|_| AppError::Quota("instance"))?;
    lock_instance(tx).await?;
    let used: i64 = sqlx::query_scalar(
        "SELECT bytes FROM usage_counters WHERE scope = 'instance' AND key = 'total'",
    )
    .fetch_one(&mut **tx)
    .await?;
    if used.saturating_add(bytes) > to_i64(quotas.instance_bytes) {
        return Err(AppError::Quota("instance"));
    }
    sqlx::query(
        "UPDATE usage_counters SET bytes = bytes + ? WHERE scope = 'instance' AND key = 'total'",
    )
    .bind(bytes)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 释放 `bytes`（文件删除/链接清理）。计数不会低于 0。
pub async fn release(
    tx: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    bytes: u64,
) -> sqlx::Result<()> {
    let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE usage_counters SET bytes = MAX(0, bytes - ?)
         WHERE scope = 'instance' AND key = 'total'",
    )
    .bind(bytes)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE usage_counters SET bytes = MAX(0, bytes - ?)
         WHERE scope = 'workspace' AND key = ?",
    )
    .bind(bytes)
    .bind(workspace_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 只释放工作区配额：文件被软删、但共享副本仍被单文件链接等引用时使用。
pub async fn release_workspace(
    tx: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    bytes: u64,
) -> sqlx::Result<()> {
    let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE usage_counters SET bytes = MAX(0, bytes - ?)
         WHERE scope = 'workspace' AND key = ?",
    )
    .bind(bytes)
    .bind(workspace_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 只释放实例总量（单文件链接的共享副本）。
pub async fn release_instance(tx: &mut Transaction<'_, Sqlite>, bytes: u64) -> sqlx::Result<()> {
    let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE usage_counters SET bytes = MAX(0, bytes - ?)
         WHERE scope = 'instance' AND key = 'total'",
    )
    .bind(bytes)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn lock_instance(tx: &mut Transaction<'_, Sqlite>) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE usage_counters SET bytes = bytes WHERE scope = 'instance' AND key = 'total'",
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
