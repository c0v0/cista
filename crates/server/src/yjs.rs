//! 每工作区一个 yrs 文档：快照 + 尾部 update，先落库后应用（ADR 0006）。
//!
//! 不含 WebSocket 协议细节（见 `ws.rs`）；这里只负责：
//! - 从 `workspace_notes`（快照）与 `doc_updates`（尾部）恢复文档；
//! - 校验单 update 上限，落库后 apply；
//! - 阈值触发快照并在同一事务里清理已合并的尾部。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Transact, Update};

use crate::error::AppError;
use crate::state::{now, AppState};

/// 共享笔记的根 `Y.Text` 名称（客户端必须一致）。
pub const ROOT_TEXT: &str = "note";
/// 单 update 上限：1 MiB（`design.md` §3.8）。
pub const MAX_UPDATE_BYTES: usize = 1024 * 1024;
/// 单文档上限（快照时检查）。
pub const MAX_DOC_BYTES: usize = 5 * 1024 * 1024;
/// 每多少条 update 做一次快照。
pub const SNAPSHOT_EVERY_UPDATES: u64 = 100;

pub struct WorkspaceDoc {
    pub workspace_id: String,
    doc: Doc,
    text: yrs::TextRef,
    updates_since_snapshot: Mutex<u64>,
    oversize: Mutex<bool>,
    approx_bytes: Mutex<usize>,
}

impl WorkspaceDoc {
    fn new(workspace_id: &str) -> Self {
        let doc = Doc::new();
        let text = doc.get_or_insert_text(ROOT_TEXT);
        Self {
            workspace_id: workspace_id.to_string(),
            doc,
            text,
            updates_since_snapshot: Mutex::new(0),
            oversize: Mutex::new(false),
            approx_bytes: Mutex::new(0),
        }
    }

    /// 当前正文（测试与归档导出用）。
    pub fn text(&self) -> String {
        let txn = self.doc.transact();
        self.text.get_string(&txn)
    }

    /// 当前状态向量（v1 编码）。
    pub fn state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    /// 相对给定状态向量计算差量 update。
    pub fn diff_since(&self, state_vector: &[u8]) -> Result<Vec<u8>, AppError> {
        let sv = StateVector::decode_v1(state_vector)
            .map_err(|_| AppError::BadRequest("invalid_state_vector"))?;
        Ok(self.doc.transact().encode_diff_v1(&sv))
    }

    /// 应用一条已持久化的 update（不做权限/大小判定，调用方负责）。
    fn apply(&self, bytes: &[u8]) -> Result<(), AppError> {
        let update =
            Update::decode_v1(bytes).map_err(|_| AppError::BadRequest("invalid_update"))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|_| AppError::BadRequest("invalid_update"))?;
        drop(txn);
        *self
            .approx_bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) += bytes.len();
        Ok(())
    }

    fn approx_bytes(&self) -> usize {
        *self
            .approx_bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn add_approx_bytes(&self, bytes: usize) {
        *self
            .approx_bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) += bytes;
    }

    fn reset_approx_bytes(&self, bytes: usize) {
        *self
            .approx_bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = bytes;
    }

    pub fn has_pending_updates(&self) -> bool {
        *self
            .updates_since_snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            > 0
    }

    fn is_oversize(&self) -> bool {
        *self
            .oversize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 进程内文档注册表（单实例；空闲且无连接的文档会被回收）。
#[derive(Default)]
pub struct Documents {
    inner: Mutex<HashMap<String, Arc<WorkspaceDoc>>>,
    last_used: Mutex<HashMap<String, i64>>,
    active: Mutex<HashMap<String, usize>>,
}

impl Documents {
    pub fn new() -> Self {
        Self::default()
    }

    fn touch(&self, workspace_id: &str) {
        self.last_used
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(workspace_id.to_string(), crate::state::now());
    }

    /// WS 会话开始：标记为活跃，空闲回收会跳过它。
    pub fn acquire(&self, workspace_id: &str) {
        *self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(workspace_id.to_string())
            .or_insert(0) += 1;
        self.touch(workspace_id);
    }

    /// WS 会话结束：减计数并刷新空闲计时。
    pub fn release(&self, workspace_id: &str) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = active.get_mut(workspace_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                active.remove(workspace_id);
            }
        }
        drop(active);
        self.touch(workspace_id);
    }

    /// 回收超过 `idle_secs` 未被访问且没有活跃连接的文档；返回回收数量。
    pub fn unload_idle(&self, idle_secs: i64) -> usize {
        let now = crate::state::now();
        let active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let idle: Vec<String> = self
            .last_used
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|(id, seen)| {
                now.saturating_sub(**seen) >= idle_secs && !active.contains_key(*id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        // 全程持有 active 锁：新会话无法在「判定空闲」与「真正卸载」之间拿到旧文档。
        for id in &idle {
            self.unload(id);
        }
        let mut last_used = self
            .last_used
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for id in &idle {
            last_used.remove(id);
        }
        drop(active);
        idle.len()
    }

    /// 取或从库里恢复一个工作区文档。
    pub async fn get_or_load(
        &self,
        state: &AppState,
        workspace_id: &str,
    ) -> Result<Arc<WorkspaceDoc>, AppError> {
        if let Some(existing) = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(workspace_id)
        {
            self.touch(workspace_id);
            return Ok(Arc::clone(existing));
        }

        let snapshot: Option<(Vec<u8>,)> =
            sqlx::query_as("SELECT yjs_state FROM workspace_notes WHERE workspace_id = ?")
                .bind(workspace_id)
                .fetch_optional(state.shared.pool())
                .await?;
        let tail: Vec<(Vec<u8>,)> = sqlx::query_as(
            "SELECT \"update\" FROM doc_updates WHERE workspace_id = ? ORDER BY seq ASC",
        )
        .bind(workspace_id)
        .fetch_all(state.shared.pool())
        .await?;

        let doc = WorkspaceDoc::new(workspace_id);
        if let Some((bytes,)) = snapshot {
            doc.apply(&bytes)?;
        }
        for (bytes,) in tail {
            doc.apply(&bytes)?;
        }

        let mut map = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let doc = Arc::clone(
            map.entry(workspace_id.to_string())
                .or_insert_with(|| Arc::new(doc)),
        );
        drop(map);
        self.touch(workspace_id);
        Ok(doc)
    }

    /// 卸载（测试/维护用：下次访问会从快照+尾部重建）。
    pub fn unload(&self, workspace_id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(workspace_id);
    }
}

/// 落库并应用一条 update；只读连接返回 `Ok(None)`（服务端强制丢弃）。
pub async fn apply_client_update(
    state: &AppState,
    doc: &WorkspaceDoc,
    bytes: &[u8],
    can_write: bool,
) -> Result<Option<Vec<u8>>, AppError> {
    if bytes.is_empty() {
        return Err(AppError::BadRequest("empty_update"));
    }
    if bytes.len() > MAX_UPDATE_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    // 先解码校验，坏包不入库。
    let update = Update::decode_v1(bytes).map_err(|_| AppError::BadRequest("invalid_update"))?;

    if !can_write {
        // 只读：丢弃（不报错，避免把只读状态探测成信号）。
        return Ok(None);
    }
    if doc.is_oversize() || doc.approx_bytes().saturating_add(bytes.len()) > MAX_DOC_BYTES {
        return Err(AppError::PayloadTooLarge);
    }

    // 先应用（yrs 对重复 update 幂等），成功后再落库：
    // 若先落库而 apply 失败，坏行会永久毒化该工作区的后续加载。
    {
        let mut txn = doc.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|_| AppError::BadRequest("invalid_update"))?;
    }
    doc.add_approx_bytes(bytes.len());

    // 落库失败时内存已应用该 update；客户端重试会幂等地再次应用并落库。
    sqlx::query("INSERT INTO doc_updates (workspace_id, \"update\", created_at) VALUES (?, ?, ?)")
        .bind(&doc.workspace_id)
        .bind(bytes)
        .bind(now())
        .execute(state.shared.pool())
        .await?;

    let should_snapshot = {
        let mut counter = doc
            .updates_since_snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *counter += 1;
        *counter >= SNAPSHOT_EVERY_UPDATES
    };
    if should_snapshot {
        snapshot(state, doc).await?;
    }

    Ok(Some(bytes.to_vec()))
}

/// 生成快照并清理已合并的尾部（同一事务，避免「快照没写却删了尾部」）。
pub async fn snapshot(state: &AppState, doc: &WorkspaceDoc) -> Result<(), AppError> {
    // 先读 max(seq)，再编码内存状态：这样快照状态一定覆盖被删除的尾部。
    // 反过来（先编码后读 seq）会把「编码之后、读 seq 之前」的并发 update 删掉但未写入快照。
    let max_seq: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM doc_updates WHERE workspace_id = ?")
            .bind(&doc.workspace_id)
            .fetch_one(state.shared.pool())
            .await?;

    let (state_bytes, state_vector) = {
        let txn = doc.doc.transact();
        (
            txn.encode_state_as_update_v1(&StateVector::default()),
            txn.state_vector().encode_v1(),
        )
    };

    let oversize = state_bytes.len() > MAX_DOC_BYTES;
    if oversize {
        tracing::warn!(
            workspace = %doc.workspace_id,
            bytes = state_bytes.len(),
            "workspace document exceeds size limit; further updates rejected"
        );
        *doc.oversize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
    }

    let mut tx = state.shared.pool().begin().await?;
    sqlx::query(
        "INSERT INTO workspace_notes (workspace_id, rev, yjs_state, state_vector, updated_at)
         VALUES (?, 1, ?, ?, ?)
         ON CONFLICT(workspace_id) DO UPDATE SET
             rev = rev + 1,
             yjs_state = excluded.yjs_state,
             state_vector = excluded.state_vector,
             updated_at = excluded.updated_at",
    )
    .bind(&doc.workspace_id)
    .bind(&state_bytes)
    .bind(&state_vector)
    .bind(now())
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM doc_updates WHERE workspace_id = ? AND seq <= ?")
        .bind(&doc.workspace_id)
        .bind(max_seq)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    *doc.updates_since_snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = 0;
    doc.reset_approx_bytes(state_bytes.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_text_name_is_stable() {
        // 客户端（web/desktop）必须使用同一个根类型名。
        assert_eq!(ROOT_TEXT, "note");
    }

    #[test]
    fn size_limits_match_design() {
        assert_eq!(MAX_UPDATE_BYTES, 1024 * 1024);
        assert_eq!(MAX_DOC_BYTES, 5 * 1024 * 1024);
    }
}
