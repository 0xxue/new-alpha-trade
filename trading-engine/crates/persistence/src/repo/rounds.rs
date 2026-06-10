//! v2 rounds 表 — 显式记录每一轮的决策、成交、pnl，用于胜率统计。

use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::{FromRow, Row, SqlitePool};

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct RoundRow {
    pub id: i64,
    pub job_id: String,
    pub round_no: i64,
    pub decision_type: String,
    pub status: String,
    pub working_order_id: Option<String>,
    pub pending_order_id: Option<String>,
    pub buy_quote_qty: Option<String>,
    pub sell_quote_qty: Option<String>,
    pub pnl_usdt: Option<String>,
    pub commission_usdt: Option<String>,
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewRound {
    pub job_id: String,
    pub round_no: i64,
    pub decision_type: String,
    pub status: String,
    pub working_order_id: Option<String>,
    pub pending_order_id: Option<String>,
    pub buy_quote_qty: Option<Decimal>,
    pub sell_quote_qty: Option<Decimal>,
    pub pnl_usdt: Option<Decimal>,
    pub commission_usdt: Option<Decimal>,
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
}

pub async fn insert(pool: &SqlitePool, r: &NewRound) -> sqlx::Result<bool> {
    let q = sqlx::query(
        "INSERT OR IGNORE INTO rounds \
         (job_id, round_no, decision_type, status, working_order_id, pending_order_id, \
          buy_quote_qty, sell_quote_qty, pnl_usdt, commission_usdt, started_ms, ended_ms) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&r.job_id)
    .bind(r.round_no)
    .bind(&r.decision_type)
    .bind(&r.status)
    .bind(r.working_order_id.as_deref())
    .bind(r.pending_order_id.as_deref())
    .bind(r.buy_quote_qty.map(|d| d.to_string()))
    .bind(r.sell_quote_qty.map(|d| d.to_string()))
    .bind(r.pnl_usdt.map(|d| d.to_string()))
    .bind(r.commission_usdt.map(|d| d.to_string()))
    .bind(r.started_ms)
    .bind(r.ended_ms)
    .execute(pool)
    .await?;
    Ok(q.rows_affected() > 0)
}

pub async fn list_by_job(pool: &SqlitePool, job_id: &str) -> sqlx::Result<Vec<RoundRow>> {
    sqlx::query_as::<_, RoundRow>(
        "SELECT id, job_id, round_no, decision_type, status, working_order_id, pending_order_id, \
                buy_quote_qty, sell_quote_qty, pnl_usdt, commission_usdt, started_ms, ended_ms \
         FROM rounds WHERE job_id = ? ORDER BY round_no",
    )
    .bind(job_id)
    .fetch_all(pool)
    .await
}

pub async fn next_round_no(pool: &SqlitePool, job_id: &str) -> sqlx::Result<i64> {
    let row = sqlx::query("SELECT COALESCE(MAX(round_no), 0) + 1 AS n FROM rounds WHERE job_id = ?")
        .bind(job_id)
        .fetch_one(pool)
        .await?;
    Ok(row.try_get::<i64, _>("n")?)
}

/// 拉最近 n 个 round（按 round_no 升序返回）。
/// 给风控用：看连续 N 轮 working timeout 之类的判断。
pub async fn list_last_n(pool: &SqlitePool, job_id: &str, n: i64) -> sqlx::Result<Vec<RoundRow>> {
    let mut rows = sqlx::query_as::<_, RoundRow>(
        "SELECT id, job_id, round_no, decision_type, status, working_order_id, pending_order_id, \
                buy_quote_qty, sell_quote_qty, pnl_usdt, commission_usdt, started_ms, ended_ms \
         FROM rounds WHERE job_id = ? ORDER BY round_no DESC LIMIT ?",
    )
    .bind(job_id)
    .bind(n)
    .fetch_all(pool)
    .await?;
    rows.reverse();
    Ok(rows)
}

/// 胜率统计 / 决策分布
#[derive(Debug, Clone, Default, Serialize)]
pub struct RoundStats {
    pub total: i64,
    pub filled: i64,
    pub skipped: i64,
    pub failed: i64,
    pub win: i64,   // pnl > 0
    pub loss: i64,  // pnl < 0
    pub flat: i64,  // pnl == 0
    pub sum_pnl_usdt: Decimal,
    /// 决策类型 → 出现次数
    pub decision_counts: std::collections::BTreeMap<String, i64>,
}

pub async fn aggregate(pool: &SqlitePool, job_id: &str) -> sqlx::Result<RoundStats> {
    let mut st = RoundStats::default();
    let rows = list_by_job(pool, job_id).await?;
    for r in &rows {
        st.total += 1;
        match r.status.as_str() {
            "filled" => st.filled += 1,
            "skipped" => st.skipped += 1,
            "failed" => st.failed += 1,
            _ => {}
        }
        if let Some(p) = &r.pnl_usdt {
            if let Ok(d) = p.parse::<Decimal>() {
                st.sum_pnl_usdt += d;
                if r.status == "filled" {
                    if d > Decimal::ZERO {
                        st.win += 1;
                    } else if d < Decimal::ZERO {
                        st.loss += 1;
                    } else {
                        st.flat += 1;
                    }
                }
            }
        }
        *st.decision_counts.entry(r.decision_type.clone()).or_default() += 1;
    }
    Ok(st)
}

impl RoundStats {
    /// 胜率：win / (win + loss + flat)，flat 不计入分母也行，这里包含使总和=100%
    pub fn win_rate_pct(&self) -> Option<f64> {
        let denom = self.win + self.loss + self.flat;
        if denom == 0 {
            None
        } else {
            Some(self.win as f64 / denom as f64 * 100.0)
        }
    }
}
