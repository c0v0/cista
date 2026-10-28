//! SQLite 存储：私有库（`private.db`）与共享库（`shared.db`）。
//!
//! 两库物理分离（ADR 0001）：分享路由只能拿到 `SharedStore`。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

/// 私有库迁移（编译期嵌入，干净 clone 也能起库）。
pub static PRIVATE_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/private");
/// 共享库迁移。
pub static SHARED_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/shared");

fn sqlite_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5))
}

#[derive(Debug, Clone)]
pub struct PrivateStore {
    pool: SqlitePool,
}

impl PrivateStore {
    pub async fn connect(path: &Path) -> anyhow::Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(sqlite_options(path))
            .await?;
        PRIVATE_MIGRATIONS.run(&pool).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// 共享库 + 共享 blob 目录。这个类型里没有任何私有表的访问路径。
#[derive(Debug, Clone)]
pub struct SharedStore {
    pool: SqlitePool,
    blobs_dir: Arc<PathBuf>,
}

impl SharedStore {
    pub async fn connect(path: &Path, blobs_dir: PathBuf) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&blobs_dir).await?;
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(sqlite_options(path))
            .await?;
        SHARED_MIGRATIONS.run(&pool).await?;
        Ok(Self {
            pool,
            blobs_dir: Arc::new(blobs_dir),
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn blob_path(&self, hash: &str) -> PathBuf {
        self.blobs_dir.join(hash)
    }

    pub fn blobs_dir(&self) -> &Path {
        self.blobs_dir.as_path()
    }
}
