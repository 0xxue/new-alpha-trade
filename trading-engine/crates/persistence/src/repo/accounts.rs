//! accounts 表读 API（qr-service 是主写者；trading-engine 只读）。

use sqlx::{FromRow, Row, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct AccountRow {
    pub username: String,
    pub cookies_json: String,
    pub headers_json: String,
    pub twofa_secret: Option<String>,
    pub last_refresh: Option<String>,
    pub status: String,
}

pub async fn list_active(pool: &SqlitePool) -> sqlx::Result<Vec<AccountRow>> {
    sqlx::query_as::<_, AccountRow>(
        "SELECT username, cookies_json, headers_json, twofa_secret, last_refresh, status \
         FROM accounts WHERE status = 'active' ORDER BY username",
    )
    .fetch_all(pool)
    .await
}

pub async fn get(pool: &SqlitePool, username: &str) -> sqlx::Result<Option<AccountRow>> {
    sqlx::query_as::<_, AccountRow>(
        "SELECT username, cookies_json, headers_json, twofa_secret, last_refresh, status \
         FROM accounts WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
}

pub async fn count(pool: &SqlitePool) -> sqlx::Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM accounts")
        .fetch_one(pool)
        .await?;
    Ok(row.try_get::<i64, _>("n")?)
}
