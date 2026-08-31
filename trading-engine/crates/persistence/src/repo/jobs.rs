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
    /// 自动暂停原因（风控/人脸验证等）。NULL=手动暂停或未暂停。
    pub pause_reason: Option<String>,
    /// 1 = 已被「一键清空」隐藏，不在任务列表里显示（数据仍保留）。
    pub hidden: i64,
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
        "SELECT id, username, symbol, strategy, params_json, target_volume, state, pause_reason, hidden, created_at, updated_at \
         FROM jobs WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn list(pool: &SqlitePool, state_filter: Option<JobState>) -> sqlx::Result<Vec<JobRow>> {
    if let Some(s) = state_filter {
        sqlx::query_as::<_, JobRow>(
            "SELECT id, username, symbol, strategy, params_json, target_volume, state, pause_reason, hidden, created_at, updated_at \
             FROM jobs WHERE state = ? AND hidden = 0 ORDER BY created_at DESC",
        )
        .bind(s.as_str())
        .fetch_all(pool)
        .await
    } else {
        sqlx::query_as::<_, JobRow>(
            "SELECT id, username, symbol, strategy, params_json, target_volume, state, pause_reason, hidden, created_at, updated_at \
             FROM jobs WHERE hidden = 0 ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await
    }
}

pub async fn set_state(pool: &SqlitePool, id: &str, state: JobState) -> sqlx::Result<bool> {
    // 任何显式状态转移都清空 pause_reason：手动暂停无原因、resume/停止/完成自然清空。
    // 引擎自动暂停(风控/人脸)走 set_paused_with_reason 单独写入原因。
    let r = sqlx::query(
        "UPDATE jobs SET state = ?, pause_reason = NULL, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(state.as_str())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// 自动暂停并记录原因（撞人脸/手机验证、磨损超阈值等）。
/// 前端据此在任务上显示原因 + 引导用户完成验证，而不是只看到裸 "paused"。
pub async fn set_paused_with_reason(
    pool: &SqlitePool,
    id: &str,
    reason: &str,
) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "UPDATE jobs SET state = 'paused', pause_reason = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(reason)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// 写状态并附一条原因（引擎重启后把孤儿任务置停等场景用）。
/// 与 set_state 的区别：set_state 会清空 pause_reason，这个会写入。
pub async fn set_state_with_reason(
    pool: &SqlitePool,
    id: &str,
    state: JobState,
    reason: &str,
) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "UPDATE jobs SET state = ?, pause_reason = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(state.as_str())
    .bind(reason)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// 一键清空任务列表：把非 running 的任务全部置停 + 隐藏。
///
/// 用户诉求是「眼不见心不烦」—— 清完列表里除了正在跑的，其余全部消失，
/// 也不会再被误点启动。隐藏而非删除：trades/orders/rounds 都挂在 job_id 上，
/// 真删会连历史盈亏一起抹掉。
/// 返回 (置停数, 隐藏数)。正在 running 的任务不动。
pub async fn cleanup_and_hide(pool: &SqlitePool) -> sqlx::Result<(u64, u64)> {
    let stopped = sqlx::query(
        "UPDATE jobs SET state = 'stopped', pause_reason = ?, updated_at = datetime('now')          WHERE state IN ('paused', 'pending')",
    )
    .bind("一键清空（不会再自动启动）")
    .execute(pool)
    .await?
    .rows_affected();
    let hidden = sqlx::query(
        "UPDATE jobs SET hidden = 1, updated_at = datetime('now')          WHERE state <> 'running' AND hidden = 0",
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok((stopped, hidden))
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
