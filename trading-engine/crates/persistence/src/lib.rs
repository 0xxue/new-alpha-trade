//! SQLite 持久化层。
//!
//! Schema 见 `migrations/0001_init.sql`。
//! 表：accounts / strategies / jobs / trades / orders。

use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

pub type DbPool = SqlitePool;

/// 打开 / 创建 SQLite 数据库，自动跑 migrations。
pub async fn open(path: impl AsRef<Path>) -> anyhow::Result<DbPool> {
    let path = path.as_ref();
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;
    sqlx::migrate!("./src/migrations").run(&pool).await?;
    tracing::info!("sqlite ready at {}", path.display());
    Ok(pool)
}

// TODO P2+: repo 模块（accounts / jobs / trades / orders CRUD）
pub mod repo;
