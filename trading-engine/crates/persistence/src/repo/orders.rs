//! orders 表 CRUD。

use rust_decimal::Decimal;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct OrderRow {
    pub order_id: String,
    pub job_id: String,
    pub side: String,
    pub price: String,
    pub qty: String,
    pub status: String,
    pub raw_response: Option<String>,
    pub ts: String,
}

#[derive(Debug, Clone)]
pub struct NewOrder {
    pub order_id: String,
    pub job_id: String,
    pub side: String,
    pub price: Decimal,
    pub qty: Decimal,
    pub status: String,
    pub raw_response: Option<String>,
}

pub async fn insert(pool: &SqlitePool, o: &NewOrder) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO orders (order_id, job_id, side, price, qty, status, raw_response) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&o.order_id)
    .bind(&o.job_id)
    .bind(&o.side)
    .bind(o.price.to_string())
    .bind(o.qty.to_string())
    .bind(&o.status)
    .bind(o.raw_response.as_deref())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_status(pool: &SqlitePool, order_id: &str, status: &str) -> sqlx::Result<bool> {
    let r = sqlx::query("UPDATE orders SET status = ? WHERE order_id = ?")
        .bind(status)
        .bind(order_id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn lookup_job_id(pool: &SqlitePool, order_id: &str) -> sqlx::Result<Option<String>> {
    let row = sqlx::query("SELECT job_id FROM orders WHERE order_id = ?")
        .bind(order_id)
        .fetch_optional(pool)
        .await?;
    use sqlx::Row;
    Ok(row.and_then(|r| r.try_get::<Option<String>, _>("job_id").ok().flatten()))
}

pub async fn list_by_job(pool: &SqlitePool, job_id: &str) -> sqlx::Result<Vec<OrderRow>> {
    sqlx::query_as::<_, OrderRow>(
        "SELECT order_id, job_id, side, price, qty, status, raw_response, ts \
         FROM orders WHERE job_id = ? ORDER BY ts DESC",
    )
    .bind(job_id)
    .fetch_all(pool)
    .await
}
