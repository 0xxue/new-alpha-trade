//! 私有用户流（per-account）：listen-key + WS `alpha@<listen_key>` + 25min 续期。
//!
//! 设计：
//! - `UserStreamManager` 持有 N 个 per-username task
//! - 每个 task 自己 spawn 一个 `AlphaWsClient` 实例（独立 wss 连接到 user-stream URL）
//! - 同时一个续期循环每 25 分钟 PUT 一次 listen-key
//! - 收到的私有事件全部广播到 `AlphaWsClient` 的 broadcast channel
//!   → /ws/stream 端点同源转发给前端
//!
//! 触发：trading-engine 启动后 spawn `UserStreamManager::start_for_active_accounts`。
//! 它扫 SQLite accounts.status=active，每个起一个 task。
//! TODO P5: 账户增删时动态启停（监听 SQLite WAL 或暴露 API）

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use binance_alpha::{AlphaRest, AlphaWsClient};
use persistence::repo::{accounts, trades};
use persistence::DbPool;
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::qr_client::QrClient;

const USER_STREAM_WS: &str = "wss://nbstream.binance.com/w3w/stream";
const KEEPALIVE_INTERVAL_SECS: u64 = 25 * 60; // 25 min

/// 解析 executionReport 事件。
/// X='TRADE' 时是一笔成交，直接写 trades 表（fill_id 用 t 字段，幂等）。
/// 其它 X 值（NEW / CANCELED / EXPIRED 等）只是订单状态变更，不写 fill。
async fn try_persist_execution_report(
    db: &DbPool,
    username: &str,
    data: &Value,
) -> anyhow::Result<()> {
    let e = data.get("e").and_then(|v| v.as_str());
    if e != Some("executionReport") {
        return Ok(());
    }
    let x = data.get("X").and_then(|v| v.as_str()).unwrap_or("");
    if x != "TRADE" {
        // NEW / CANCELED / PARTIALLY_FILLED (没成交那部分) 不写库
        return Ok(());
    }
    // 抓字段（参考币安 spot WS executionReport 协议）
    // s=symbol, i=orderId, t=tradeId, S=side, L=last filled price,
    // l=last filled qty, Y=last filled quote qty, n=commission, N=commission asset, T=trade time
    let symbol = data.get("s").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let order_id = data.get("i").and_then(|v| v.as_i64()).map(|n| n.to_string());
    let trade_id = data.get("t").and_then(|v| v.as_i64()).map(|n| n.to_string());
    let side = data.get("S").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let price = parse_str_field(data, "L").unwrap_or(Decimal::ZERO);
    let qty = parse_str_field(data, "l").unwrap_or(Decimal::ZERO);
    let quote_qty = parse_str_field(data, "Y").unwrap_or(price * qty);
    let commission = parse_str_field(data, "n").unwrap_or(Decimal::ZERO);
    let commission_asset = data.get("N").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let trade_ts = data.get("T").and_then(|v| v.as_i64()).unwrap_or(0);

    let (Some(order_id), Some(fill_id)) = (order_id, trade_id) else {
        return Ok(()); // 缺关键字段，跳过
    };
    if symbol.is_empty() || side.is_empty() {
        return Ok(());
    }
    // 关联 job_id：查 orders 表反推
    let job_id = persistence::repo::orders::lookup_job_id(db, &order_id)
        .await
        .ok()
        .flatten();

    let new = trades::NewTrade {
        fill_id,
        order_id,
        job_id,
        username: username.to_string(),
        symbol,
        side,
        price,
        qty,
        quote_qty,
        commission,
        commission_asset,
        trade_ts_ms: trade_ts,
        raw_json: Some(data.to_string()),
    };
    match trades::insert(db, &new).await {
        Ok(true) => debug!(user = %username, order = %new.order_id, "fill persisted via ws"),
        Ok(false) => debug!(user = %username, "fill already in db (dup)"),
        Err(e) => warn!(user = %username, err = %e, "ws fill insert failed"),
    }
    Ok(())
}

fn parse_str_field(data: &Value, key: &str) -> Option<Decimal> {
    data.get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| Decimal::from_str(s).ok())
}

pub struct UserStreamManager {
    db: DbPool,
    alpha_rest: Arc<AlphaRest>,
    qr: Arc<QrClient>,
    /// per-username 出口 ws client（共享给 /ws/stream broadcast）
    /// 实际上我们把事件直接广播到 *公开* alpha_ws 的 channel，
    /// 这样前端单一 ws 同时收到行情 + 订单事件。
    public_ws: Arc<AlphaWsClient>,
    /// per-user 后台 task 句柄（重启时 abort 旧的）
    tasks: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

impl UserStreamManager {
    pub fn new(
        db: DbPool,
        alpha_rest: Arc<AlphaRest>,
        qr: Arc<QrClient>,
        public_ws: Arc<AlphaWsClient>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            alpha_rest,
            qr,
            public_ws,
            tasks: Mutex::new(HashMap::new()),
        })
    }

    /// 启动所有 active 账户的 user stream。
    pub async fn start_for_active_accounts(self: &Arc<Self>) -> anyhow::Result<()> {
        let rows = accounts::list_active(&self.db).await?;
        info!(count = rows.len(), "starting user streams for active accounts");
        for row in rows {
            self.clone().spawn_one(row.username).await;
        }
        Ok(())
    }

    /// 启动单个账户的 user stream（重复启动会先 abort 旧的）。
    pub async fn spawn_one(self: Arc<Self>, username: String) {
        let mut tasks = self.tasks.lock().await;
        if let Some(old) = tasks.remove(&username) {
            old.abort();
        }
        let me = self.clone();
        let user = username.clone();
        let handle = tokio::spawn(async move {
            me.run_loop(user).await;
        });
        tasks.insert(username, handle);
    }

    async fn run_loop(self: Arc<Self>, username: String) {
        let mut backoff = Duration::from_secs(2);
        loop {
            match self.run_once(&username).await {
                Ok(()) => {
                    info!(%username, "user stream session ended cleanly");
                    backoff = Duration::from_secs(2);
                }
                Err(e) => {
                    warn!(%username, err=%e, backoff_secs=backoff.as_secs(), "user stream errored");
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(120));
        }
    }

    async fn run_once(&self, username: &str) -> anyhow::Result<()> {
        // 拿凭据
        let auth = self.qr.get_auth(username).await?;
        // 申请 listen-key
        let listen_key = self.alpha_rest.create_listen_key(&auth).await?;
        info!(%username, key_prefix = &listen_key[..listen_key.len().min(12)], "got listen-key");

        // 起 ws client 订阅 alpha@<listen_key>
        let ws = Arc::new(AlphaWsClient::with_url(USER_STREAM_WS.into()));
        let _rx = ws.spawn();
        // 转发 ws -> public_ws broadcast（让 /ws/stream 同源收到）
        let public_tx_ws = self.public_ws.clone();
        let mut forward_rx = ws.subscribe_handle();
        let username_for_fwd = username.to_string();
        let db_for_fwd = self.db.clone();
        tokio::spawn(async move {
            while let Ok(evt) = forward_rx.recv().await {
                debug!(user = %username_for_fwd, stream = %evt.stream, "user event");
                // 先尝试解析 executionReport TRADE 写库（主推路径）
                if let Err(e) = try_persist_execution_report(&db_for_fwd, &username_for_fwd, &evt.data).await {
                    warn!(user = %username_for_fwd, err = %e, "persist exec report failed");
                }
                // 转发给前端
                public_tx_ws.broadcast(binance_alpha::StreamEvent {
                    stream: format!("user:{username_for_fwd}|{}", evt.stream),
                    data: evt.data,
                });
            }
        });

        ws.add_subscriptions(vec![format!("alpha@{listen_key}")]).await;
        info!(%username, "user stream subscribed");

        // 续期循环
        let mut ticker = tokio::time::interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
        ticker.tick().await; // 第一次立刻 tick，跳过
        loop {
            ticker.tick().await;
            match self.alpha_rest.keepalive_listen_key(&auth).await {
                Ok(()) => info!(%username, "listen-key keepalive ok"),
                Err(e) => {
                    warn!(%username, err=%e, "keepalive failed, will reconnect");
                    return Err(anyhow::anyhow!("keepalive failed: {e}"));
                }
            }
        }
    }
}
