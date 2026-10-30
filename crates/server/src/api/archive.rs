//! 过渡区、归档与级联（Phase 5，所有者侧）。
//!
//! 设计（`design.md` §3.5、§3.6、§6）：
//! - `active → expired → quarantined → archived | purged`；过渡区公网不可达，只有 B+ API 可见。
//! - 归档由客户端完成（服务端没有 MK）：客户端取回明文 → 用 MK 加密 → 校验 → 调
//!   `archive-complete`，服务端在这之后 **才** purge 共享数据。
//! - 源笔记删除：级联删除只读投影；有访客上传件时阻止（三种解决路径），编辑分叉不受影响。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cista_core::protocol::{WorkspaceInfo, WorkspaceOrigin, WorkspaceState};
use serde::Deserialize;

use crate::error::AppError;
use crate::quota;
use crate::state::{now, AppState};

type WorkspaceRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    i64,
);

fn row_to_info(row: WorkspaceRow) -> Result<WorkspaceInfo, AppError> {
    let (
        id,
        title,
        origin,
        source_note_id,
        state,
        expires_at,
        quarantined_at,
        archived_at,
        created_at,
    ) = row;
    Ok(WorkspaceInfo {
        id,
        title,
        origin: WorkspaceOrigin::parse(&origin)
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("corrupt workspace origin")))?,
        source_note_id,
        state: WorkspaceState::parse(&state)
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("corrupt workspace state")))?,
        expires_at,
        quarantined_at,
        archived_at,
        created_at,
    })
}

const WORKSPACE_COLUMNS: &str =
    "id, title, origin, source_note_id, state, expires_at, quarantined_at, archived_at, created_at";

/// 只有“过期/隔离中”或“已到期但仍 active”的工作区可恢复；
/// archived/purged 是终态，不能靠 restore 复活（否则 purge 后旧链接会重新可用）。
fn transitionable(workspace: &WorkspaceInfo) -> bool {
    match workspace.state {
        WorkspaceState::Expired | WorkspaceState::Quarantined => true,
        WorkspaceState::Active => workspace
            .expires_at
            .map(|expires_at| expires_at <= now())
            .unwrap_or(false),
        _ => false,
    }
}

async fn load_workspace(state: &AppState, id: &str) -> Result<WorkspaceInfo, AppError> {
    let sql = format!("SELECT {WORKSPACE_COLUMNS} FROM workspaces WHERE id = ?");
    let row: Option<WorkspaceRow> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(state.shared.pool())
        .await?;
    row_to_info(row.ok_or(AppError::NotFound)?)
}

/// `GET /api/quarantine`：过渡区列表（过期/隔离中，人工处理）。
pub async fn list_quarantine(
    State(state): State<AppState>,
) -> Result<Json<Vec<WorkspaceInfo>>, AppError> {
    let sql = format!(
        "SELECT {WORKSPACE_COLUMNS} FROM workspaces
         WHERE state IN ('expired', 'quarantined') ORDER BY created_at ASC"
    );
    let rows: Vec<WorkspaceRow> = sqlx::query_as(&sql).fetch_all(state.shared.pool()).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(row_to_info(row)?);
    }
    Ok(Json(items))
}

/// `POST /api/workspaces/{id}/restore`：过渡区 → active（客户端随后生成新链接）。
pub async fn restore(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceInfo>, AppError> {
    let workspace = load_workspace(&state, &id).await?;
    if !transitionable(&workspace) {
        return Err(AppError::BadRequest("workspace_not_quarantined"));
    }

    let timestamp = now();
    let affected = sqlx::query(
        "UPDATE workspaces
         SET state = 'active', expires_at = NULL, quarantined_at = NULL, archived_at = NULL
         WHERE id = ?
           AND (state IN ('expired', 'quarantined')
                OR (state = 'active' AND expires_at IS NOT NULL AND expires_at <= ?))",
    )
    .bind(&id)
    .bind(timestamp)
    .execute(state.shared.pool())
    .await?
    .rows_affected();
    if affected != 1 {
        return Err(AppError::BadRequest("workspace_not_quarantined"));
    }

    tracing::info!(workspace = %id, "workspace restored from quarantine");
    load_workspace(&state, &id).await.map(Json)
}

/// `POST /api/workspaces/{id}/archive-complete`
///
/// 客户端已取回明文、用 MK 加密并校验完毕；这里 purge 共享数据（不可逆）。
pub async fn archive_complete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceInfo>, AppError> {
    let workspace = load_workspace(&state, &id).await?;
    if !transitionable(&workspace) {
        return Err(AppError::BadRequest("workspace_not_quarantined"));
    }

    let released = purge_shared_data(&state, &id, "archived").await?;
    // 内存中的 Yjs 文档也要丢弃；否则旧会话/空闲缓存可能把内容写回。
    state.documents.unload(&id);

    tracing::info!(workspace = %id, released_bytes = released, "workspace archived");
    load_workspace(&state, &id).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct DeleteNoteQuery {
    /// 有访客上传件时，强制删除需要显式 `?force=true`（记日志）。
    #[serde(default)]
    pub force: bool,
}

/// `DELETE /api/notes/{id}`
///
/// - 无投影：软删除笔记。
/// - 只读投影且无上传件：级联 purge 投影（链接 410）。
/// - 有访客上传件：409 + 三种处理路径；`?force=true` 才强制。
/// - 编辑分叉（fork）：不受影响。
pub async fn delete_note(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DeleteNoteQuery>,
) -> Result<Response, AppError> {
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM notes WHERE id = ? AND deleted_at IS NULL")
            .bind(&id)
            .fetch_optional(state.private.pool())
            .await?;
    exists.ok_or(AppError::NotFound)?;

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT w.id, (SELECT COUNT(*) FROM uploads u
                       WHERE u.workspace_id = w.id AND u.status = 'complete')
         FROM workspaces w
         WHERE w.origin = 'projection' AND w.source_note_id = ?
           AND w.state IN ('active', 'expired', 'quarantined')",
    )
    .bind(&id)
    .fetch_all(state.shared.pool())
    .await?;

    let blocked: Vec<serde_json::Value> = rows
        .iter()
        .filter(|(_, uploads)| *uploads > 0)
        .map(|(workspace_id, uploads)| {
            serde_json::json!({ "workspace_id": workspace_id, "uploads": uploads })
        })
        .collect();

    if !blocked.is_empty() && !query.force {
        return Ok((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "uploads_present",
                "workspaces": blocked,
                "options": ["cancel", "move_to_inbox", "force"],
            })),
        )
            .into_response());
    }

    for (workspace_id, uploads) in &rows {
        let released = purge_shared_data(&state, workspace_id, "purged").await?;
        // 投影不再可达：公网路由按状态拒绝（410），行保留用于溯源。
        state.documents.unload(workspace_id);
        tracing::info!(
            workspace = %workspace_id,
            source_note = %id,
            uploads = uploads,
            released_bytes = released,
            forced = query.force,
            "projection purged with source note"
        );
    }

    let timestamp = now();
    let mut tx = state.private.pool().begin().await?;
    sqlx::query(
        "UPDATE notes SET deleted_at = ?, rev = rev + 1 WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(timestamp)
    .bind(&id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO oplog (entity, entity_id, op, payload, created_at)
         VALUES ('note', ?, 'delete', '{}', ?)",
    )
    .bind(&id)
    .bind(timestamp)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    tracing::info!(note = %id, projections = rows.len(), "note deleted");
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// 清空一个工作区的共享数据并释放配额；返回释放的字节数。
///
/// 顺序：统计 → 删行 → 释放配额 → blob 引用递减（归零删本体）→ 提交 → 删文件。
async fn purge_shared_data(
    state: &AppState,
    workspace_id: &str,
    target_state: &str,
) -> Result<u64, AppError> {
    let files: Vec<(String, i64)> = sqlx::query_as(
        "SELECT blob_hash, size FROM workspace_files
         WHERE workspace_id = ? AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .fetch_all(state.shared.pool())
    .await?;

    let released: u64 = files
        .iter()
        .map(|(_, size)| u64::try_from(*size).unwrap_or(0))
        .sum();

    let mut tx = state.shared.pool().begin().await?;
    // 归档/删除即撤销该工作区的所有链接；restore 也不能把已撤销链接复活。
    sqlx::query(
        "UPDATE share_links SET revoked_at = ?
         WHERE kind = 'workspace' AND object_id = ? AND revoked_at IS NULL",
    )
    .bind(now())
    .bind(workspace_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM workspace_files WHERE workspace_id = ?")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM uploads WHERE workspace_id = ?")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM doc_updates WHERE workspace_id = ?")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM workspace_notes WHERE workspace_id = ?")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;

    // 工作区配额整体释放；实例配额在物理副本删除时逐块释放。
    quota::release_workspace(&mut tx, workspace_id, released).await?;

    let mut remove_files: Vec<String> = Vec::new();
    for (hash, size) in &files {
        sqlx::query("UPDATE blobs SET refcount = refcount - 1 WHERE hash = ? AND scope = 'shared'")
            .bind(hash)
            .execute(&mut *tx)
            .await?;
        let refcount: Option<i64> =
            sqlx::query_scalar("SELECT refcount FROM blobs WHERE hash = ? AND scope = 'shared'")
                .bind(hash)
                .fetch_optional(&mut *tx)
                .await?;
        if refcount.unwrap_or(1) <= 0 {
            sqlx::query("DELETE FROM blobs WHERE hash = ? AND scope = 'shared'")
                .bind(hash)
                .execute(&mut *tx)
                .await?;
            quota::release_instance(&mut tx, u64::try_from(*size).unwrap_or(0)).await?;
            remove_files.push(hash.clone());
        }
    }
    // 状态变更与清库同一事务：否则 purge 与 archive 更新之间，打开中的 WS
    // 仍看到 active 并把新的 doc_updates 写回来。
    sqlx::query("UPDATE workspaces SET state = ?, archived_at = ? WHERE id = ?")
        .bind(target_state)
        .bind(now())
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    for hash in remove_files {
        let _ = tokio::fs::remove_file(state.shared.blob_path(&hash)).await;
    }
    Ok(released)
}
