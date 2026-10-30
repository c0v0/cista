//! 所有者侧分享链接 API（B+ 签名）。
//!
//! - token 由客户端生成，服务端只存 `SHA-256(token)`，响应里永不回显 token。
//! - 单文件链接创建时把私有 blob 复制到共享域（服务端可见，UI 必须提示）；
//!   同一内容只计一次实例配额（refcount 去重）。

use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::Argon2;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use cista_core::protocol::{CreateLinkRequest, LinkInfo};
use cista_core::share::{
    can_read_files, file_scopes, hash_token, is_valid_token_shape, note_scopes, validate_scopes,
    FilesMode, ShareKind, MAX_PASSPHRASE_BYTES,
};

use crate::error::AppError;
use crate::quota;
use crate::state::{now, AppState};

/// 链接列表行（sqlx FromRow 元组；抽成别名避免 clippy::type_complexity）。
type LinkListRow = (
    String,
    String,
    String,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    i64,
    Option<i64>,
    i64,
);

/// `GET /api/share`
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<LinkInfo>>, AppError> {
    let rows: Vec<LinkListRow> = sqlx::query_as(
        "SELECT id, kind, object_id, note_scopes, file_scopes, expires_at, max_uses,
                used_count, revoked_at, created_at
         FROM share_links ORDER BY created_at DESC",
    )
    .fetch_all(state.shared.pool())
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(
                |(
                    id,
                    kind,
                    object_id,
                    note_scopes,
                    file_scopes,
                    expires_at,
                    max_uses,
                    used_count,
                    revoked_at,
                    created_at,
                )| LinkInfo {
                    id,
                    kind: if kind == "file" {
                        ShareKind::File
                    } else {
                        ShareKind::Workspace
                    },
                    object_id,
                    note_scopes: u32::try_from(note_scopes).unwrap_or(0),
                    file_scopes: u32::try_from(file_scopes).unwrap_or(0),
                    expires_at,
                    max_uses: max_uses.and_then(|v| u32::try_from(v).ok()),
                    used_count: u32::try_from(used_count).unwrap_or(0),
                    revoked_at,
                    created_at,
                },
            )
            .collect(),
    ))
}

/// `POST /api/share`
pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateLinkRequest>,
) -> Result<(StatusCode, Json<LinkInfo>), AppError> {
    if !is_valid_token_shape(&request.token) {
        return Err(AppError::BadRequest("invalid_token"));
    }
    let note_bits = note_scopes(request.note_mode);
    let file_bits = file_scopes(request.files_mode);
    validate_scopes(note_bits, file_bits).map_err(|_| AppError::BadRequest("invalid_scopes"))?;
    // 单文件链接本质是“读一个文件”：files=off/drop 不应签发可下载票据。
    if request.kind == ShareKind::File && !can_read_files(file_bits) {
        return Err(AppError::BadRequest("file_link_requires_read"));
    }

    // `drop` 默认要求口令（`design.md` §3.7）；空白口令按未填处理。
    let passphrase = request
        .passphrase
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if request.files_mode == FilesMode::Drop && passphrase.is_none() {
        return Err(AppError::BadRequest("passphrase_required"));
    }
    let passphrase_hash = match passphrase {
        Some(passphrase) => Some(hash_passphrase(&state, passphrase).await?),
        None => None,
    };
    // 工作区链接的“使用次数”没有可靠的计数语义（正文走 WS），明确拒绝而不是静默忽略。
    if request.kind == ShareKind::Workspace && request.max_uses.is_some() {
        return Err(AppError::BadRequest("max_uses_not_supported"));
    }

    let timestamp = now();
    let (object_id, title) = match request.kind {
        ShareKind::File => {
            if !is_blob_hash(&request.object_id) {
                return Err(AppError::BadRequest("invalid_blob_hash"));
            }
            let private_path = state
                .data_dir()
                .join("blobs")
                .join("private")
                .join(&request.object_id);
            let row: Option<(i64,)> =
                sqlx::query_as("SELECT size FROM blobs WHERE hash = ? AND scope = 'private'")
                    .bind(&request.object_id)
                    .fetch_optional(state.private.pool())
                    .await?;
            let (size,) = row.ok_or(AppError::NotFound)?;
            // 整包读取/加密复制最多 max_file_bytes；串行化，避免并发 120 份 OOM。
            let _link_permit = state
                .link_create_budget
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| AppError::Internal(anyhow::anyhow!("link budget closed")))?;
            let size_u64 = u64::try_from(size).unwrap_or(0);
            if let Some(free) = crate::state::free_bytes(state.data_dir()) {
                if free < state.cfg.upload_free_floor() {
                    return Err(AppError::InsufficientStorage);
                }
            }
            let bytes = tokio::fs::read(&private_path)
                .await
                .map_err(|_| AppError::NotFound)?;

            // 文件先落盘，再开事务记账：不把 100 MiB 的加密/写入放在 SQLite 事务里。
            crate::public::store_shared_blob(&state, &request.object_id, &bytes).await?;
            let mut tx = state.shared.pool().begin().await?;
            let existing: Option<(i64, i64)> = sqlx::query_as(
                "SELECT size, refcount FROM blobs WHERE hash = ? AND scope = 'shared'",
            )
            .bind(&request.object_id)
            .fetch_optional(&mut *tx)
            .await?;
            if existing.is_none() {
                quota::reserve_instance(&mut tx, size_u64, state.quotas()).await?;
            }
            sqlx::query(
                "INSERT INTO blobs (hash, scope, size, refcount, created_at)
                 VALUES (?, 'shared', ?, 1, ?)
                 ON CONFLICT(hash, scope) DO UPDATE SET refcount = refcount + 1",
            )
            .bind(&request.object_id)
            .bind(size)
            .bind(timestamp)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;

            let title = request
                .title
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "共享文件".to_string());
            (request.object_id.clone(), title)
        }
        ShareKind::Workspace => {
            let row: Option<(String, Option<i64>, String)> =
                sqlx::query_as("SELECT state, expires_at, title FROM workspaces WHERE id = ?")
                    .bind(&request.object_id)
                    .fetch_optional(state.shared.pool())
                    .await?;
            let (workspace_state, workspace_expires, workspace_title) =
                row.ok_or(AppError::NotFound)?;
            if workspace_state != "active" {
                return Err(AppError::Gone);
            }
            if let (Some(link_expires), Some(workspace_expires)) =
                (request.expires_at, workspace_expires)
            {
                if link_expires > workspace_expires {
                    return Err(AppError::BadRequest("link_outlives_workspace"));
                }
            }
            (request.object_id.clone(), workspace_title)
        }
    };

    let id = crate::ids::new_id("ln");
    let token_hash = hash_token(&request.token);
    let insert = sqlx::query(
        "INSERT INTO share_links
         (id, token_hash, kind, object_id, title, note_scopes, file_scopes,
          expires_at, max_uses, used_count, passphrase_hash, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
    )
    .bind(&id)
    .bind(&token_hash)
    .bind(match request.kind {
        ShareKind::File => "file",
        ShareKind::Workspace => "workspace",
    })
    .bind(&object_id)
    .bind(&title)
    .bind(i64::from(note_bits))
    .bind(i64::from(file_bits))
    .bind(request.expires_at)
    .bind(request.max_uses.map(i64::from))
    .bind(&passphrase_hash)
    .bind(timestamp)
    .execute(state.shared.pool())
    .await;

    if let Err(err) = insert {
        // 文件副本/引用已经提交；链接行失败时必须补偿，否则无主副本会泄漏配额。
        if request.kind == ShareKind::File {
            if let Err(cleanup) = release_shared_copy(&state, &object_id).await {
                tracing::warn!(blob = %object_id, error = ?cleanup, "release shared copy after link insert failure");
            }
        }
        if is_unique_violation(&err) {
            return Err(AppError::AlreadyExists);
        }
        return Err(err.into());
    }

    tracing::info!(link = %id, kind = ?request.kind, "share link created");

    Ok((
        StatusCode::CREATED,
        Json(LinkInfo {
            id,
            kind: request.kind,
            object_id,
            note_scopes: note_bits,
            file_scopes: file_bits,
            expires_at: request.expires_at,
            max_uses: request.max_uses,
            used_count: 0,
            revoked_at: None,
            created_at: timestamp,
        }),
    ))
}

/// `DELETE /api/share/{id}`：撤销；单文件链接同时释放共享副本引用。
pub async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT kind, object_id FROM share_links WHERE id = ?")
            .bind(&id)
            .fetch_optional(state.shared.pool())
            .await?;
    let (kind, object_id) = row.ok_or(AppError::NotFound)?;

    let affected =
        sqlx::query("UPDATE share_links SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(now())
            .bind(&id)
            .execute(state.shared.pool())
            .await?
            .rows_affected();
    if affected == 0 {
        return Err(AppError::NotFound);
    }

    if kind == "file" {
        release_shared_copy(&state, &object_id).await?;
    }

    tracing::info!(link = %id, "share link revoked");
    Ok(StatusCode::NO_CONTENT)
}

/// 减共享 blob 引用；归零时删除本体、元数据并释放实例配额（私有原件不动）。
pub(crate) async fn release_shared_copy(state: &AppState, hash: &str) -> Result<(), AppError> {
    let mut tx = state.shared.pool().begin().await?;
    let row: Option<(i64, i64)> =
        sqlx::query_as("SELECT size, refcount FROM blobs WHERE hash = ? AND scope = 'shared'")
            .bind(hash)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((size, refcount)) = row else {
        return Ok(());
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

    // 不在这里删物理文件：并发的同内容上传可能刚看到文件存在并决定复用。
    // 留下无 DB 行的孤儿，由每小时 GC 在 1 小时后回收。
    Ok(())
}

async fn hash_passphrase(state: &AppState, passphrase: &str) -> Result<String, AppError> {
    if passphrase.len() > MAX_PASSPHRASE_BYTES {
        return Err(AppError::BadRequest("passphrase_too_long"));
    }
    let permit = state
        .hash_budget
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::Internal(anyhow::anyhow!("hash budget closed")))?;
    let passphrase = passphrase.to_string();
    let hashed = tokio::task::spawn_blocking(move || {
        let mut salt_bytes = [0u8; 16];
        getrandom::fill(&mut salt_bytes)
            .map_err(|_| AppError::Internal(anyhow::anyhow!("rng unavailable")))?;
        let salt = SaltString::encode_b64(&salt_bytes)
            .map_err(|_| AppError::Internal(anyhow::anyhow!("salt encoding")))?;
        Argon2::default()
            .hash_password(passphrase.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| AppError::Internal(anyhow::anyhow!("passphrase hashing")))
    })
    .await
    .map_err(|_| AppError::Internal(anyhow::anyhow!("passphrase hashing task failed")))?;
    drop(permit);
    hashed
}

fn is_blob_hash(raw: &str) -> bool {
    raw.len() == 64 && raw.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .map(|db_error| db_error.is_unique_violation())
        .unwrap_or(false)
}
