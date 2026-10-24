//! 所有者侧工作区 API（B+ 签名）。

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use cista_core::protocol::{
    CreateWorkspaceRequest, WorkspaceInfo, WorkspaceOrigin, WorkspaceState,
};

use crate::error::AppError;
use crate::state::{now, AppState};

/// 工作区行（sqlx FromRow 元组；抽成别名避免 clippy::type_complexity）。
type WorkspaceInfoRow = (
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

/// `POST /api/workspaces`
pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<WorkspaceInfo>), AppError> {
    let title = request.title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(AppError::BadRequest("invalid_title"));
    }

    // fork/projection 需要来源私有笔记（弱引用，仅溯源）。
    if request.origin != WorkspaceOrigin::Blank {
        let source_note_id = request
            .source_note_id
            .as_deref()
            .ok_or(AppError::BadRequest("missing_source_note"))?;
        let exists: Option<(String,)> =
            sqlx::query_as("SELECT id FROM notes WHERE id = ? AND deleted_at IS NULL")
                .bind(source_note_id)
                .fetch_optional(state.private.pool())
                .await?;
        if exists.is_none() {
            return Err(AppError::NotFound);
        }
    }

    let id = crate::ids::new_id("ws");
    let created_at = now();
    sqlx::query(
        "INSERT INTO workspaces (id, title, origin, source_note_id, state, expires_at, created_at)
         VALUES (?, ?, ?, ?, 'active', ?, ?)",
    )
    .bind(&id)
    .bind(title)
    .bind(request.origin.as_str())
    .bind(request.source_note_id.as_deref())
    .bind(request.expires_at)
    .bind(created_at)
    .execute(state.shared.pool())
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(WorkspaceInfo {
            id,
            title: title.to_string(),
            origin: request.origin,
            source_note_id: request.source_note_id,
            state: WorkspaceState::Active,
            expires_at: request.expires_at,
            quarantined_at: None,
            archived_at: None,
            created_at,
        }),
    ))
}

/// `GET /api/workspaces/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceInfo>, AppError> {
    let row: Option<WorkspaceInfoRow> = sqlx::query_as(
        "SELECT id, title, origin, source_note_id, state, expires_at, quarantined_at, archived_at, created_at
         FROM workspaces WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(state.shared.pool())
    .await?;

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
    ) = row.ok_or(AppError::NotFound)?;

    Ok(Json(WorkspaceInfo {
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
    }))
}
