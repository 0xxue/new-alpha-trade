//! jobs 表 CRUD。

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Pending,
    Running,
    Paused,
    Done,
    Failed,
    Stopped,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct JobRow {
    pub id: String,
    pub username: String,
    pub symbol: String,
    pub strategy: String,
    pub params_json: String,
    pub target_volume: String, // Decimal 字符串
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewJob {
    pub id: String,
    pub username: String,
    pub symbol: String,
    pub strategy: String,
    pub params_json: String,
    pub target_volume: Decimal,
}

pub async fn insert(pool: &SqlitePool, j: &NewJob) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO jobs (id, username, symbol, strategy, params_json, target_volume, state) \
         VALUES (?, ?, ?, ?, ?, ?, 'pending')",
    )
    .bind(&j.id)
    .bind(&j.username)
    .bind(&j.symbol)
    .bind(&j.strategy)
    .bind(&j.params_json)
    .bind(j.target_volume.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get(pool: &SqlitePool, id: &str) -> sqlx::Result<Option<JobRow>> {
    sqlx::query_as::<_, JobRow>(
        "SELECT id, username, symbol, strategy, params_json, target_volume, state, created_at, updated_at \
         FROM jobs WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn list(pool: &SqlitePool, state_filter: Option<JobState>) -> sqlx::Result<Vec<JobRow>> {
    if let Some(s) = state_filter {
        sqlx::query_as::<_, JobRow>(
            "SELECT id, username, symbol, strategy, params_json, target_volume, state, created_at, updated_at \
             FROM jobs WHERE state = ? ORDER BY created_at DESC",
        )
        .bind(s.as_str())
        .fetch_all(pool)
        .await
    } else {
        sqlx::query_as::<_, JobRow>(
            "SELECT id, username, symbol, strategy, params_json, target_volume, state, created_at, updated_at \
             FROM jobs ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await
    }
}

pub async fn set_state(pool: &SqlitePool, id: &str, state: JobState) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "UPDATE jobs SET state = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(state.as_str())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// 覆盖整条 params_json（wear 自动重置基线用）。
pub async fn set_params_json(pool: &SqlitePool, id: &str, params_json: &str) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "UPDATE jobs SET params_json = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(params_json)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}
