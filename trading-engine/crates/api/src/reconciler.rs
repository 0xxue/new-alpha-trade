//! 兜底对账：60 秒扫一遍 orders 表，找出"status=filled 但 trades 表无对应 fill"的订单，
//! 补拉 user-trades 写入。
//!
//! 这是防御层；主路径是：
//! 1. orders.rs::spawn_fetch_fills 异步拉一次
//! 2. user_stream.rs 主推 executionReport TRADE 事件
//!
//! 但凡有一笔漏，60 秒内 reconciler 会补上。

use std::sync::Arc;
use std::time::Duration;

use binance_alpha::AlphaRest;
use persistence::repo::{accounts, trades};
use persistence::DbPool;
use rust_decimal::Decimal;
use sqlx::Row;
use tracing::{debug, info, warn};

use crate::qr_client::QrClient;

const SCAN_INTERVAL_SECS: u64 = 60;
// V2.tune24: 缩小回灌窗口（从 30→5min）防过度查询 Binance（活动 job 每 5s 一单
// → 5min ≈ 60 orders/cycle 可控；老 30min 窗口 = 360 orders × 100ms = 36s）
const LOOKBACK_MINUTES: i64 = 5;
// V2.tune32: pending cleanup 窗口必须比 LOOKBACK_MINUTES 大，否则下面 SQL 条件矛盾（
// "比 5min 老" AND "比 5min 新" = 不存在）。引擎重启时挂的 OTO 永远不被清。
// 60 min 给足够 grace period 但不会漏 1 小时前的 stuck。
const PENDING_CLEANUP_AGE_MIN: i64 = 5;
const PENDING_CLEANUP_LOOKBACK_HOURS: i64 = 48;

pub fn start(db: DbPool, alpha: Arc<AlphaRest>, qr: Arc<QrClient>) {
    tokio::spawn(async move {
        // 启动后等 10s 让 engine 自身先稳定
        tokio::time::sleep(Duration::from_secs(10)).await;
        let mut ticker = tokio::time::interval(Duration::from_secs(SCAN_INTERVAL_SECS));
        loop {
            ticker.tick().await;
            match run_once(&db, &alpha, &qr).await {
                Ok(n) if n > 0 => info!(backfilled = n, "reconciler backfilled"),
                Ok(_) => debug!("reconciler: nothing to fix"),
                Err(e) => warn!(err = %e, "reconciler scan failed"),
            }
        }
    });
}

async fn run_once(
    db: &DbPool,
    alpha: &Arc<AlphaRest>,
    qr: &Arc<QrClient>,
) -> anyhow::Result<u32> {
    // V2.tune24: 扫所有最近 filled orders（不再限于"DB 0 笔"）—— wait_fills 早退会漏
    // sub-fill, 这里全 re-fetch + INSERT OR IGNORE 把漏的补上。
    // 已入库的 (fill_id, symbol, qty, quote_qty) 会被 UNIQUE 约束 IGNORE 不重复。
    let rows = sqlx::query(
        "SELECT o.order_id, o.job_id, COALESCE(a.username, '') AS username \
         FROM orders o \
         LEFT JOIN jobs j ON j.id = o.job_id \
         LEFT JOIN accounts a ON a.username = j.username \
         WHERE o.status = 'filled' \
           AND o.ts > datetime('now', ?) \
         GROUP BY o.order_id",
    )
    .bind(format!("-{LOOKBACK_MINUTES} minutes"))
    .fetch_all(db)
    .await?;

    // V2.tune29/32: 扫 "pending 老于 5min 但近 48h" — engine 重启或 round bug 留下的
    // stuck pending 都能逮。之前 bug: 用同一 LOOKBACK_MINUTES 写矛盾条件 → 永远空。
    let stuck_pending = sqlx::query(
        "SELECT o.order_id, o.job_id, COALESCE(a.username, '') AS username \
         FROM orders o \
         LEFT JOIN jobs j ON j.id = o.job_id \
         LEFT JOIN accounts a ON a.username = j.username \
         WHERE o.status = 'pending' \
           AND o.ts < datetime('now', ?) \
           AND o.ts > datetime('now', ?)",
    )
    .bind(format!("-{PENDING_CLEANUP_AGE_MIN} minutes"))
    .bind(format!("-{PENDING_CLEANUP_LOOKBACK_HOURS} hours"))
    .fetch_all(db)
    .await?;

    let mut backfilled = 0_u32;
    let mut pending_cleaned = 0_u32;
    // 处理卡死 pending
    for row in &stuck_pending {
        let order_id: String = row.try_get("order_id")?;
        let job_id: Option<String> = row.try_get("job_id").ok();
        let username: String = row.try_get("username")?;
        if username.is_empty() { continue; }
        let symbol = guess_symbol_for_job(db, job_id.as_deref()).await.unwrap_or_else(|| "ALPHA_971USDT".into());
        let auth = match qr.get_auth(&username).await {
            Ok(a) => a,
            Err(_) => continue,
        };
        let fills = match alpha.get_user_trades(&auth, &order_id, &symbol).await {
            Ok(f) => f,
            Err(_) => continue,
        };
        let new_status = if fills.is_empty() { "canceled" } else { "filled" };
        let updated = sqlx::query("UPDATE orders SET status = ? WHERE order_id = ?")
            .bind(new_status)
            .bind(&order_id)
            .execute(db)
            .await
            .map(|r| r.rows_affected() > 0)
            .unwrap_or(false);
        if updated {
            pending_cleaned += 1;
            info!(%order_id, %new_status, fills = fills.len(), "reconciler: cleared stuck pending");
        }
    }

    for row in rows {
        let order_id: String = row.try_get("order_id")?;
        let job_id: Option<String> = row.try_get("job_id").ok();
        let username: String = row.try_get("username")?;
        if username.is_empty() {
            // manual 单 + 无账户：跳过（拿不到 cookies）
            continue;
        }
        let symbol = guess_symbol_for_job(db, job_id.as_deref()).await.unwrap_or_else(|| "ALPHA_971USDT".into());
        let auth = match qr.get_auth(&username).await {
            Ok(a) => a,
            Err(e) => {
                warn!(%username, err = %e, "reconciler: no auth");
                continue;
            }
        };
        let fills = match alpha.get_user_trades(&auth, &order_id, &symbol).await {
            Ok(f) => f,
            Err(e) => {
                warn!(%order_id, err = %e, "reconciler: get_user_trades failed");
                continue;
            }
        };
        for f in fills {
            let side_str = match f.side {
                binance_alpha::Side::Buy => "BUY",
                binance_alpha::Side::Sell => "SELL",
            };
            let raw_json = serde_json::to_string(&f).ok();
            let inserted = trades::insert(
                db,
                &trades::NewTrade {
                    fill_id: f.id,
                    order_id: f.order_id,
                    job_id: job_id.clone(),
                    username: username.clone(),
                    symbol: f.symbol,
                    side: side_str.into(),
                    price: f.price,
                    qty: f.qty,
                    quote_qty: f.quote_qty,
                    commission: f.commission,
                    commission_asset: f.commission_asset,
                    trade_ts_ms: f.time,
                    raw_json,
                },
            )
            .await
            .unwrap_or(false);
            if inserted {
                backfilled += 1;
                info!(%order_id, %username, "reconciler backfilled a fill");
            }
        }
    }
    let _ = accounts::count(db).await; // 维持连接活
    let _ = Decimal::ZERO; // 防 unused
    if pending_cleaned > 0 {
        info!(cleaned = pending_cleaned, "reconciler: cleared stuck pending orders");
    }
    Ok(backfilled + pending_cleaned)
}

async fn guess_symbol_for_job(db: &DbPool, job_id: Option<&str>) -> Option<String> {
    let jid = job_id?;
    sqlx::query("SELECT symbol FROM jobs WHERE id = ?")
        .bind(jid)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.try_get::<String, _>("symbol").ok())
}
