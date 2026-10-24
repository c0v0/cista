//! 私有笔记：服务端只存 `id + rev + 密文信封`，不解析明文。
//!
//! 并发模型见 `docs/adr/0003-notes-concurrency.md`：`If-Match: rev` 乐观并发，
//! 冲突返回 409 + 当前 rev，由客户端生成冲突副本。

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use cista_core::crypto::NOTE_VERSION;
use cista_core::protocol::{NoteRecord, NotesPage, PutNoteRequest, SINCE_BATCH_MAX};
use serde::Deserialize;

use crate::error::AppError;
use crate::state::{now, AppState};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// 增量游标（oplog.seq）；缺省 0 表示从头拉。
    pub since: Option<u64>,
}

/// `GET /api/notes?since=<seq>`：按 oplog 增量返回每个笔记的最新状态。
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<NotesPage>, AppError> {
    let since = query.since.unwrap_or(0);
    let limit = i64::try_from(SINCE_BATCH_MAX).unwrap_or(i64::MAX);

    let rows: Vec<(String, i64, i64, String, i64)> = sqlx::query_as(
        "SELECT n.id, n.rev, n.updated_at, n.sealed, MAX(o.seq) AS seq
         FROM oplog o
         JOIN notes n ON n.id = o.entity_id
         WHERE o.entity = 'note' AND o.seq > ? AND n.deleted_at IS NULL
         GROUP BY n.id
         ORDER BY seq ASC
         LIMIT ?",
    )
    .bind(i64::try_from(since).unwrap_or(i64::MAX))
    .bind(limit)
    .fetch_all(state.private.pool())
    .await?;

    let mut notes = Vec::with_capacity(rows.len());
    let mut next_since = since;
    for (id, rev, updated_at, sealed_json, seq) in rows {
        notes.push(NoteRecord {
            id,
            rev: u64::try_from(rev).unwrap_or(0),
            updated_at,
            sealed: parse_sealed(&sealed_json)?,
        });
        next_since = u64::try_from(seq).unwrap_or(next_since);
    }

    Ok(Json(NotesPage { notes, next_since }))
}

/// `GET /api/notes/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<NoteRecord>, AppError> {
    if !valid_note_id(&id) {
        return Err(AppError::NotFound);
    }
    let row: Option<(i64, i64, String)> = sqlx::query_as(
        "SELECT rev, updated_at, sealed FROM notes WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(&id)
    .fetch_optional(state.private.pool())
    .await?;

    let (rev, updated_at, sealed_json) = row.ok_or(AppError::NotFound)?;
    Ok(Json(NoteRecord {
        id,
        rev: u64::try_from(rev).unwrap_or(0),
        updated_at,
        sealed: parse_sealed(&sealed_json)?,
    }))
}

/// `PUT /api/notes/{id}`：创建用无 `If-Match`（或 `If-Match: 0`），更新必须带当前 rev。
pub async fn put(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PutNoteRequest>,
) -> Result<(StatusCode, Json<NoteRecord>), AppError> {
    if !valid_note_id(&id) {
        return Err(AppError::BadRequest("invalid_note_id"));
    }
    if body.sealed.v != NOTE_VERSION {
        return Err(AppError::BadRequest("unsupported_version"));
    }

    let expected = match headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) {
        None => None,
        Some(raw) => Some(parse_etag(raw).ok_or(AppError::BadRequest("invalid_if_match"))?),
    };

    let updated_at = now();
    let sealed_json =
        serde_json::to_string(&body.sealed).map_err(|_| AppError::BadRequest("invalid_sealed"))?;

    let mut tx = state.private.pool().begin().await?;
    let current: Option<i64> =
        sqlx::query_scalar("SELECT rev FROM notes WHERE id = ? AND deleted_at IS NULL")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;

    let (rev, created) = match (expected, current) {
        (None, None) | (Some(0), None) => (1_i64, true),
        (None, Some(cur)) | (Some(0), Some(cur)) => {
            return Err(AppError::RevConflict {
                rev: u64::try_from(cur).unwrap_or(0),
            })
        }
        (Some(exp), Some(cur)) if exp == u64::try_from(cur).unwrap_or(u64::MAX) => (
            cur.checked_add(1)
                .ok_or_else(|| AppError::BadRequest("rev_overflow"))?,
            false,
        ),
        (Some(_), Some(cur)) => {
            return Err(AppError::RevConflict {
                rev: u64::try_from(cur).unwrap_or(0),
            })
        }
        (Some(_), None) => return Err(AppError::NotFound),
    };

    if body.sealed.rev != u64::try_from(rev).unwrap_or(0) {
        // AAD 绑定 rev：信封里的 rev 必须等于服务端即将写入的 rev。
        return Err(AppError::BadRequest("rev_mismatch"));
    }

    if created {
        sqlx::query("INSERT INTO notes (id, rev, sealed, updated_at) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(rev)
            .bind(&sealed_json)
            .bind(updated_at)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("UPDATE notes SET rev = ?, sealed = ?, updated_at = ? WHERE id = ?")
            .bind(rev)
            .bind(&sealed_json)
            .bind(updated_at)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "INSERT INTO oplog (entity, entity_id, op, payload, created_at)
         VALUES ('note', ?, 'upsert', ?, ?)",
    )
    .bind(&id)
    .bind(&sealed_json)
    .bind(updated_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(NoteRecord {
            id,
            rev: u64::try_from(rev).unwrap_or(0),
            updated_at,
            sealed: body.sealed,
        }),
    ))
}

fn parse_sealed(raw: &str) -> Result<cista_core::crypto::SealedNote, AppError> {
    serde_json::from_str(raw)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("corrupt sealed note")))
}

fn parse_etag(raw: &str) -> Option<u64> {
    let value = raw.trim().trim_start_matches("W/").trim_matches('"');
    value.parse().ok()
}

/// 笔记 ID 只允许 URL 安全字符，避免路径/日志注入。
pub fn valid_note_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}
