//! trades 表：每个币安 fill 一行（不是按 cycle 聚合）。
//!
//! volume / wear 计算的权威数据源。

use rust_decimal::Decimal;
use sqlx::{FromRow, Row, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct TradeRow {
    pub id: i64,
    pub fill_id: String,
    pub order_id: String,
    pub job_id: Option<String>,
    pub username: String,
    pub symbol: String,
    pub side: String,
    pub price: String,
    pub qty: String,
    pub quote_qty: String,
    pub commission: String,
    pub commission_asset: String,
    pub trade_ts_ms: i64,
    pub raw_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct NewTrade {
    pub fill_id: String,
    pub order_id: String,
    pub job_id: Option<String>,
    pub username: String,
    pub symbol: String,
    pub side: String,
    pub price: Decimal,
    pub qty: Decimal,
    pub quote_qty: Decimal,
    pub commission: Decimal,
    pub commission_asset: String,
    pub trade_ts_ms: i64,
    pub raw_json: Option<String>,
}

/// 插入一行；fill_id+symbol 重复时 ON CONFLICT IGNORE 实现幂等。
pub async fn insert(pool: &SqlitePool, t: &NewTrade) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "INSERT OR IGNORE INTO trades \
         (fill_id, order_id, job_id, username, symbol, side, price, qty, quote_qty, \
          commission, commission_asset, trade_ts_ms, raw_json) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&t.fill_id)
    .bind(&t.order_id)
    .bind(t.job_id.as_deref())
    .bind(&t.username)
    .bind(&t.symbol)
    .bind(&t.side)
    .bind(t.price.to_string())
    .bind(t.qty.to_string())
    .bind(t.quote_qty.to_string())
    .bind(t.commission.to_string())
    .bind(&t.commission_asset)
    .bind(t.trade_ts_ms)
    .bind(t.raw_json.as_deref())
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn list_by_job(pool: &SqlitePool, job_id: &str) -> sqlx::Result<Vec<TradeRow>> {
    sqlx::query_as::<_, TradeRow>(
        "SELECT id, fill_id, order_id, job_id, username, symbol, side, price, qty, \
                quote_qty, commission, commission_asset, trade_ts_ms, raw_json, created_at \
         FROM trades WHERE job_id = ? ORDER BY trade_ts_ms",
    )
    .bind(job_id)
    .fetch_all(pool)
    .await
}

pub async fn list_by_order(pool: &SqlitePool, order_id: &str) -> sqlx::Result<Vec<TradeRow>> {
    sqlx::query_as::<_, TradeRow>(
        "SELECT id, fill_id, order_id, job_id, username, symbol, side, price, qty, \
                quote_qty, commission, commission_asset, trade_ts_ms, raw_json, created_at \
         FROM trades WHERE order_id = ? ORDER BY trade_ts_ms",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await
}

// ---- 聚合查询（stats 计算用） ----

/// 累加某 job 的 BUY quote_qty（= volume，单边-买口径）。
pub async fn sum_buy_quote_qty(pool: &SqlitePool, job_id: &str) -> sqlx::Result<Decimal> {
    // 没行时 SUM 返回 NULL/0，COALESCE 把 0 (INTEGER) 跟 REAL 拌一起 → sqlx 类型解码出错
    // 用 ifnull + 显式 0.0 强制 REAL 类型，且用 String 取出再 parse 完全规避 f64 精度
    let row = sqlx::query(
        "SELECT IFNULL(SUM(CAST(quote_qty AS REAL)), 0.0) AS s \
         FROM trades WHERE job_id = ? AND side = 'BUY'",
    )
    .bind(job_id)
    .fetch_one(pool)
    .await?;
    let v: f64 = row.try_get("s").unwrap_or(0.0);
    Ok(Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))
}

pub async fn sum_buy_quote_qty_by_user(pool: &SqlitePool, username: &str) -> sqlx::Result<Decimal> {
    let row = sqlx::query(
        "SELECT IFNULL(SUM(CAST(quote_qty AS REAL)), 0.0) AS s \
         FROM trades WHERE username = ? AND side = 'BUY'",
    )
    .bind(username)
    .fetch_one(pool)
    .await?;
    let v: f64 = row.try_get("s").unwrap_or(0.0);
    Ok(Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))
}

/// V2.tune31: 这个 job 累计的 base asset 净 qty = BUY qty - SELL qty
/// 用于真实 in-flight 计算（包括 binance 自动转到 earn 的部分，钱包 free+freeze 看不到）。
pub async fn net_base_qty(pool: &SqlitePool, job_id: &str) -> sqlx::Result<Decimal> {
    let row = sqlx::query(
        "SELECT IFNULL(SUM(CASE WHEN side = 'BUY' THEN CAST(qty AS REAL) \
                               WHEN side = 'SELL' THEN -CAST(qty AS REAL) \
                               ELSE 0 END), 0.0) AS s \
         FROM trades WHERE job_id = ?",
    )
    .bind(job_id)
    .fetch_one(pool)
    .await?;
    let v: f64 = row.try_get("s").unwrap_or(0.0);
    Ok(Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))
}

pub async fn count_fills(pool: &SqlitePool, job_id: &str) -> sqlx::Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM trades WHERE job_id = ?")
        .bind(job_id)
        .fetch_one(pool)
        .await?;
    Ok(row.try_get::<i64, _>("n")?)
}
