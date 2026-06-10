//! 服务器小元信息（到期日 / 购买日等）。简单 key-value 表。

use sqlx::{Row, SqlitePool};

pub async fn get(pool: &SqlitePool, key: &str) -> sqlx::Result<Option<String>> {
    let row = sqlx::query("SELECT value FROM server_meta WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<String, _>("value")))
}

pub async fn set(pool: &SqlitePool, key: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO server_meta(key, value) VALUES (?, ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}
