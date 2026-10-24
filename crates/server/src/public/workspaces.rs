//! 公网工作区路由：元信息、文件区（列/传/改/删/下）、SSE 事件。
//!
//! 所有权限位在服务端判定（`design.md` §3.3）；`drop` 链接列目录一律 403。
//! token 走 `Authorization: Share <token>` 头，不在 URL 里。

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Response;
use axum::Json;
use cista_core::protocol::{PublicFileEntry, PublicFileList, PublicMeta, PAGE_DEFAULT, PAGE_MAX};
use cista_core::share::{
    can_manage_files, can_read_files, can_upload, is_allowed_mime, sanitize_file_name, ShareKind,
};
use futures_util::stream::Stream;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::public::{
    ensure_public, load_link, load_workspace, require_passphrase, serve_blob, ShareToken,
};
use crate::quota;
use crate::state::{now, AppState};

/// `GET /public/workspaces/meta`
pub async fn meta(
    State(state): State<AppState>,
    ShareToken(token): ShareToken,
) -> Result<Json<PublicMeta>, AppError> {
    let link = load_link(&state, &token, ShareKind::Workspace).await?;
    let workspace = load_workspace(&state, &link.object_id).await?;
    ensure_public(&workspace)?;
    Ok(Json(PublicMeta {
        kind: ShareKind::Workspace,
        title: workspace.title,
        note_scopes: link.note_scopes,
        file_scopes: link.file_scopes,
        expires_at: link.expires_at,
        requires_passphrase: link.passphrase_hash.is_some(),
        max_uses: link.max_uses,
        used_count: link.used_count,
    }))
}

#[derive(Debug, Deserialize)]
pub struct FilesQuery {
    /// 一页多少条：缺省 `PAGE_DEFAULT`，超过 `PAGE_MAX` 时钳制。
    pub limit: Option<u64>,
    /// 从第几条开始（按 `created_at, id` 排序）。
    pub offset: Option<u64>,
}

/// `GET /public/workspaces/files?limit=&offset=`
pub async fn files(
    State(state): State<AppState>,
    ShareToken(token): ShareToken,
    Query(query): Query<FilesQuery>,
    headers: HeaderMap,
) -> Result<Json<PublicFileList>, AppError> {
    let link = load_link(&state, &token, ShareKind::Workspace).await?;
    let workspace = load_workspace(&state, &link.object_id).await?;
    ensure_public(&workspace)?;
    require_passphrase(&link, &headers)?;
    if !can_read_files(link.file_scopes) {
        // `drop` 链接只上传、不列目录。
        return Err(AppError::Forbidden);
    }

    // 分页硬上限（`phase6-hardening.md` §1）：默认 50、最大 200，越界钳制而不是 400。
    let limit = query.limit.unwrap_or(PAGE_DEFAULT).clamp(1, PAGE_MAX);
    let offset = query.offset.unwrap_or(0);
    let fetch = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
    let rows: Vec<(String, String, i64, String, i64, i64)> = sqlx::query_as(
        "SELECT id, name, size, mime, rev, created_at FROM workspace_files
         WHERE workspace_id = ? AND deleted_at IS NULL
         ORDER BY created_at ASC, id ASC LIMIT ? OFFSET ?",
    )
    .bind(&link.object_id)
    .bind(fetch)
    .bind(i64::try_from(offset).unwrap_or(i64::MAX))
    .fetch_all(state.shared.pool())
    .await?;

    let page_size = usize::try_from(limit).unwrap_or(usize::MAX);
    let has_more = rows.len() > page_size;
    let files = rows
        .into_iter()
        .take(page_size)
        .map(|(id, name, size, mime, rev, created_at)| PublicFileEntry {
            id,
            name,
            size: u64::try_from(size).unwrap_or(0),
            mime,
            rev: u64::try_from(rev).unwrap_or(0),
            created_at,
        })
        .collect();

    Ok(Json(PublicFileList {
        files,
        next_offset: if has_more {
            Some(offset.saturating_add(limit))
        } else {
            None
        },
    }))
}

/// `POST /public/workspaces/files/upload`
///
/// v1 在 handler 内缓冲请求体（路由层按 `max_file_bytes` 硬限）；
/// 真正的流式截断留给 Phase 6 加固。
pub async fn upload(
    State(state): State<AppState>,
    ShareToken(token): ShareToken,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<PublicFileEntry>), AppError> {
    let link = load_link(&state, &token, ShareKind::Workspace).await?;
    let workspace = load_workspace(&state, &link.object_id).await?;
    ensure_public(&workspace)?;
    if !can_upload(link.file_scopes) {
        return Err(AppError::Forbidden);
    }
    require_passphrase(&link, &headers)?;

    let raw_name = headers
        .get("x-file-name")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let name =
        sanitize_file_name(raw_name).map_err(|_| AppError::BadRequest("invalid_file_name"))?;
    let mime = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !is_allowed_mime(&mime) {
        return Err(AppError::BadRequest("unsupported_media_type"));
    }

    let size = u64::try_from(body.len()).unwrap_or(u64::MAX);
    if size == 0 {
        return Err(AppError::BadRequest("empty_file"));
    }
    if size > state.cfg.max_file_bytes {
        return Err(AppError::PayloadTooLarge);
    }

    let blob_hash = hex::encode(Sha256::digest(&body));
    let final_name = unique_name(&state, &link.object_id, &name).await?;
    let file_id = crate::ids::new_id("f");
    let created_at = now();

    let mut tx = state.shared.pool().begin().await?;
    quota::reserve(&mut tx, &link.object_id, &link.id, size, state.quotas()).await?;
    sqlx::query(
        "INSERT INTO workspace_files
         (id, workspace_id, parent_id, name, blob_hash, size, mime, rev, created_at)
         VALUES (?, ?, NULL, ?, ?, ?, ?, 1, ?)",
    )
    .bind(&file_id)
    .bind(&link.object_id)
    .bind(&final_name)
    .bind(&blob_hash)
    .bind(i64::try_from(size).unwrap_or(i64::MAX))
    .bind(&mime)
    .bind(created_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO blobs (hash, scope, size, refcount, created_at)
         VALUES (?, 'shared', ?, 1, ?)
         ON CONFLICT(hash, scope) DO UPDATE SET refcount = refcount + 1",
    )
    .bind(&blob_hash)
    .bind(i64::try_from(size).unwrap_or(i64::MAX))
    .bind(created_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO uploads (id, link_id, workspace_id, blob_hash, name, mime, size, status, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'complete', ?)",
    )
    .bind(crate::ids::new_id("up"))
    .bind(&link.id)
    .bind(&link.object_id)
    .bind(&blob_hash)
    .bind(&final_name)
    .bind(&mime)
    .bind(i64::try_from(size).unwrap_or(i64::MAX))
    .bind(created_at)
    .execute(&mut *tx)
    .await?;
    crate::public::store_shared_blob(&state, &blob_hash, &body).await?;
    tx.commit().await?;

    state.events.publish(&link.object_id, 1);
    tracing::info!(
        workspace = %link.object_id,
        link = %link.id,
        bytes = size,
        "shared file uploaded"
    );

    Ok((
        StatusCode::CREATED,
        Json(PublicFileEntry {
            id: file_id,
            name: final_name,
            size,
            mime,
            rev: 1,
            created_at,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub name: String,
}

/// `PATCH /public/workspaces/files/{id}`
pub async fn rename(
    State(state): State<AppState>,
    ShareToken(token): ShareToken,
    Path(file_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<RenameRequest>,
) -> Result<Json<PublicFileEntry>, AppError> {
    let link = load_link(&state, &token, ShareKind::Workspace).await?;
    let workspace = load_workspace(&state, &link.object_id).await?;
    ensure_public(&workspace)?;
    if !can_manage_files(link.file_scopes) {
        return Err(AppError::Forbidden);
    }
    require_passphrase(&link, &headers)?;

    let name =
        sanitize_file_name(&request.name).map_err(|_| AppError::BadRequest("invalid_file_name"))?;
    let row: Option<(i64, String, i64)> = sqlx::query_as(
        "SELECT size, mime, created_at FROM workspace_files
         WHERE id = ? AND workspace_id = ? AND deleted_at IS NULL",
    )
    .bind(&file_id)
    .bind(&link.object_id)
    .fetch_optional(state.shared.pool())
    .await?;
    let (size, mime, created_at) = row.ok_or(AppError::NotFound)?;

    let update = sqlx::query(
        "UPDATE workspace_files SET name = ?, rev = rev + 1
         WHERE id = ? AND workspace_id = ? AND deleted_at IS NULL",
    )
    .bind(&name)
    .bind(&file_id)
    .bind(&link.object_id)
    .execute(state.shared.pool())
    .await;
    match update {
        Ok(result) if result.rows_affected() == 1 => {}
        Ok(_) => return Err(AppError::NotFound),
        Err(err) if is_unique_violation(&err) => return Err(AppError::AlreadyExists),
        Err(err) => return Err(err.into()),
    }

    let rev: i64 = sqlx::query_scalar("SELECT rev FROM workspace_files WHERE id = ?")
        .bind(&file_id)
        .fetch_one(state.shared.pool())
        .await?;
    state.events.publish(&link.object_id, rev);
    tracing::info!(workspace = %link.object_id, link = %link.id, "shared file renamed");

    Ok(Json(PublicFileEntry {
        id: file_id,
        name,
        size: u64::try_from(size).unwrap_or(0),
        mime,
        rev: u64::try_from(rev).unwrap_or(0),
        created_at,
    }))
}

/// `DELETE /public/workspaces/files/{id}`
pub async fn delete(
    State(state): State<AppState>,
    ShareToken(token): ShareToken,
    Path(file_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, AppError> {
    let link = load_link(&state, &token, ShareKind::Workspace).await?;
    let workspace = load_workspace(&state, &link.object_id).await?;
    ensure_public(&workspace)?;
    if !can_manage_files(link.file_scopes) {
        return Err(AppError::Forbidden);
    }
    require_passphrase(&link, &headers)?;

    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT blob_hash, size FROM workspace_files
         WHERE id = ? AND workspace_id = ? AND deleted_at IS NULL",
    )
    .bind(&file_id)
    .bind(&link.object_id)
    .fetch_optional(state.shared.pool())
    .await?;
    let (blob_hash, size) = row.ok_or(AppError::NotFound)?;

    let mut tx = state.shared.pool().begin().await?;
    sqlx::query(
        "UPDATE workspace_files SET deleted_at = ?, rev = rev + 1
         WHERE id = ? AND workspace_id = ? AND deleted_at IS NULL",
    )
    .bind(now())
    .bind(&file_id)
    .bind(&link.object_id)
    .execute(&mut *tx)
    .await?;
    quota::release(&mut tx, &link.object_id, u64::try_from(size).unwrap_or(0)).await?;
    sqlx::query("UPDATE blobs SET refcount = refcount - 1 WHERE hash = ? AND scope = 'shared'")
        .bind(&blob_hash)
        .execute(&mut *tx)
        .await?;
    let refcount: Option<i64> =
        sqlx::query_scalar("SELECT refcount FROM blobs WHERE hash = ? AND scope = 'shared'")
            .bind(&blob_hash)
            .fetch_optional(&mut *tx)
            .await?;
    let remove_blob = refcount.unwrap_or(1) <= 0;
    if remove_blob {
        sqlx::query("DELETE FROM blobs WHERE hash = ? AND scope = 'shared'")
            .bind(&blob_hash)
            .execute(&mut *tx)
            .await?;
    }
    let rev: i64 = sqlx::query_scalar("SELECT rev FROM workspace_files WHERE id = ?")
        .bind(&file_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;

    if remove_blob {
        let _ = tokio::fs::remove_file(state.shared.blob_path(&blob_hash)).await;
    }
    state.events.publish(&link.object_id, rev);
    tracing::info!(workspace = %link.object_id, link = %link.id, "shared file deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /public/workspaces/files/{id}/blob`
pub async fn download(
    State(state): State<AppState>,
    ShareToken(token): ShareToken,
    Path(file_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let link = load_link(&state, &token, ShareKind::Workspace).await?;
    let workspace = load_workspace(&state, &link.object_id).await?;
    ensure_public(&workspace)?;
    require_passphrase(&link, &headers)?;
    if !can_read_files(link.file_scopes) {
        return Err(AppError::Forbidden);
    }

    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT name, blob_hash FROM workspace_files
         WHERE id = ? AND workspace_id = ? AND deleted_at IS NULL",
    )
    .bind(&file_id)
    .bind(&link.object_id)
    .fetch_optional(state.shared.pool())
    .await?;
    let (name, blob_hash) = row.ok_or(AppError::NotFound)?;
    serve_blob(&state, &blob_hash, &name).await
}

/// `GET /public/workspaces/events`：SSE，只发 `rev`。
pub async fn events(
    State(state): State<AppState>,
    ShareToken(token): ShareToken,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    let link = load_link(&state, &token, ShareKind::Workspace).await?;
    let workspace = load_workspace(&state, &link.object_id).await?;
    ensure_public(&workspace)?;

    let receiver = state.events.subscribe(&link.object_id);
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(rev) => {
                    let event = Event::default().event("rev").data(rev.to_string());
                    return Some((Ok(event), receiver));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// 同工作区重名自动后缀 `-2`、`-3` …（v1 文件区扁平）。
async fn unique_name(state: &AppState, workspace_id: &str, name: &str) -> Result<String, AppError> {
    let mut candidate = name.to_string();
    for attempt in 0..100_u32 {
        let exists: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM workspace_files
             WHERE workspace_id = ? AND name = ? AND deleted_at IS NULL",
        )
        .bind(workspace_id)
        .bind(&candidate)
        .fetch_optional(state.shared.pool())
        .await?;
        if exists.is_none() {
            return Ok(candidate);
        }
        candidate = suffixed(name, attempt + 2);
    }
    Err(AppError::AlreadyExists)
}

fn suffixed(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("{stem}-{n}.{extension}"),
        _ => format!("{name}-{n}"),
    }
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .map(|db_error| db_error.is_unique_violation())
        .unwrap_or(false)
}
