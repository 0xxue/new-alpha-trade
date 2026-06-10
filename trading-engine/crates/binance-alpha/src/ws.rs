//! Binance Alpha WebSocket 客户端。
//!
//! 端点：
//! - `wss://nbstream.binance.com/w3w/wsa/stream`  公开行情流
//! - `wss://nbstream.binance.com/w3w/stream`      私有用户流（alpha@<listen_key>）
//!
//! 协议：SUBSCRIBE / UNSUBSCRIBE，行情消息 `{"stream":"<name>","data":{...}}`。
//!
//! 设计：
//! - `AlphaWsClient::spawn()` 起后台连接 + 自动重连
//! - `add_subscriptions()` 既入集合（用于重连恢复），又通过 mpsc 立刻通知发 SUBSCRIBE
//! - 所有行情消息走 `broadcast::Sender<StreamEvent>` 多消费者

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

const WS_PUBLIC_URL: &str = "wss://nbstream.binance.com/w3w/wsa/stream";

#[derive(Debug, Error)]
pub enum WsError {
    #[error("websocket transport: {0}")]
    Transport(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("channel closed")]
    Closed,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamEvent {
    pub stream: String,
    pub data: Value,
}

#[derive(Serialize)]
struct SubscribeMsg<'a> {
    method: &'a str,
    params: Vec<String>,
    id: u64,
}

/// 内部命令：让活动的 connect_loop 立即发一个 SUBSCRIBE 帧。
enum Cmd {
    Subscribe(Vec<String>),
}

pub struct AlphaWsClient {
    url: String,
    subs: Arc<RwLock<Vec<String>>>,
    next_id: Arc<Mutex<u64>>,
    tx: broadcast::Sender<StreamEvent>,
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    cmd_rx: Mutex<Option<mpsc::UnboundedReceiver<Cmd>>>,
}

impl AlphaWsClient {
    pub fn new() -> Self {
        Self::with_url(WS_PUBLIC_URL.into())
    }

    pub fn with_url(url: String) -> Self {
        let (tx, _) = broadcast::channel(1024);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        Self {
            url,
            subs: Arc::new(RwLock::new(Vec::new())),
            next_id: Arc::new(Mutex::new(1)),
            tx,
            cmd_tx,
            cmd_rx: Mutex::new(Some(cmd_rx)),
        }
    }

    pub fn subscribe_handle(&self) -> broadcast::Receiver<StreamEvent> {
        self.tx.subscribe()
    }

    /// 外部往 broadcast channel 推一条事件（user-stream 转发到公共 ws 用）。
    pub fn broadcast(&self, evt: StreamEvent) {
        let _ = self.tx.send(evt);
    }

    /// 启动后台连接 + 重连任务。返回 broadcast Receiver。
    /// 只能调用一次；二次调用 cmd_rx 已被 take，subscribe 命令收不到。
    pub fn spawn(self: &Arc<Self>) -> broadcast::Receiver<StreamEvent> {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            me.run().await;
        });
        self.tx.subscribe()
    }

    /// 添加订阅。既入"想订阅"集合（重连后会恢复），又给活动连接发 SUBSCRIBE 帧立即生效。
    pub async fn add_subscriptions(&self, streams: Vec<String>) {
        let mut to_send: Vec<String> = Vec::new();
        {
            let mut guard = self.subs.write().await;
            for s in streams {
                if !guard.contains(&s) {
                    guard.push(s.clone());
                    to_send.push(s);
                }
            }
        }
        if !to_send.is_empty() {
            let _ = self.cmd_tx.send(Cmd::Subscribe(to_send));
        }
    }

    async fn run(self: Arc<Self>) {
        let mut cmd_rx = match self.cmd_rx.lock().await.take() {
            Some(rx) => rx,
            None => {
                warn!("AlphaWsClient::spawn called twice; cmd channel already taken");
                return;
            }
        };
        let mut backoff_ms: u64 = 1000;
        loop {
            match self.connect_loop(&mut cmd_rx).await {
                Ok(()) => {
                    info!(url = %self.url, "ws session ended cleanly; reconnecting");
                    backoff_ms = 1000;
                }
                Err(e) => {
                    warn!(url = %self.url, err = %e, backoff_ms, "ws session errored; will reconnect");
                }
            }
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms = (backoff_ms * 2).min(30_000);
        }
    }

    async fn connect_loop(&self, cmd_rx: &mut mpsc::UnboundedReceiver<Cmd>) -> Result<(), WsError> {
        info!(url = %self.url, "ws connecting");
        let (ws, _resp) = connect_async(&self.url).await?;
        info!(url = %self.url, "ws connected");
        let (mut writer, mut reader) = ws.split();

        // 重连恢复：把已有订阅一次性发出去
        let initial = self.subs.read().await.clone();
        if !initial.is_empty() {
            let id = self.next_id().await;
            let frame = SubscribeMsg { method: "SUBSCRIBE", params: initial.clone(), id };
            let text = serde_json::to_string(&frame)?;
            debug!(streams = ?initial, %text, "initial SUBSCRIBE");
            writer.send(Message::text(text)).await?;
        }

        let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
        ping_interval.tick().await;

        loop {
            tokio::select! {
                msg = reader.next() => {
                    match msg {
                        Some(Ok(Message::Text(t))) => self.handle_text(&t).await,
                        Some(Ok(Message::Binary(b))) => {
                            if let Ok(t) = std::str::from_utf8(&b) {
                                self.handle_text(t).await;
                            }
                        }
                        Some(Ok(Message::Ping(p))) => { writer.send(Message::Pong(p)).await?; }
                        Some(Ok(Message::Close(_))) | None => {
                            info!("ws closed by peer");
                            return Ok(());
                        }
                        Some(Err(e)) => return Err(WsError::Transport(e)),
                        _ => {}
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(Cmd::Subscribe(streams)) => {
                            let id = self.next_id().await;
                            let frame = SubscribeMsg { method: "SUBSCRIBE", params: streams.clone(), id };
                            let text = serde_json::to_string(&frame)?;
                            info!(?streams, "dynamic SUBSCRIBE");
                            writer.send(Message::text(text)).await?;
                        }
                        None => {
                            warn!("cmd channel closed");
                            return Ok(());
                        }
                    }
                }
                _ = ping_interval.tick() => {
                    writer.send(Message::Ping(vec![].into())).await?;
                }
            }
        }
    }

    async fn handle_text(&self, t: &str) {
        let v: Value = match serde_json::from_str(t) {
            Ok(v) => v,
            Err(e) => {
                warn!(err = %e, text = %t, "non-json ws message");
                return;
            }
        };
        if v.get("result").is_some() && v.get("id").is_some() {
            debug!(?v, "subscribe ack");
            return;
        }
        if let (Some(s), Some(d)) = (v.get("stream").and_then(Value::as_str), v.get("data")) {
            let evt = StreamEvent { stream: s.to_string(), data: d.clone() };
            let _ = self.tx.send(evt);
            return;
        }
        debug!(?v, "unhandled ws message");
    }

    async fn next_id(&self) -> u64 {
        let mut g = self.next_id.lock().await;
        let n = *g;
        *g += 1;
        n
    }
}

impl Default for AlphaWsClient {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 流名构造便捷函数
// ============================================================================

pub fn agg_trade_stream(alpha_id: &str) -> String {
    let id = alpha_id.trim_start_matches("ALPHA_");
    format!("alpha_{}usdt@aggTrade", id.to_ascii_lowercase())
}

/// V2.tune15: 改用增量 depth 流（仿旧 trading_agent.py + websocket_orderbook.py）。
/// @depth@100ms 推送增量更新（add/update/remove level），客户端维护本地真实 orderbook。
/// 比 @fulldepth@500ms 快 5x，且无 stale outliers 问题。
pub fn depth_stream(alpha_id: &str) -> String {
    let id = alpha_id.trim_start_matches("ALPHA_");
    format!("alpha_{}usdt@depth@100ms", id.to_ascii_lowercase())
}

// ============================================================================
// 行情消息结构（强类型解析）
// ============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AggTradeEvent {
    #[serde(rename = "e")] pub event_type: String,
    #[serde(rename = "E")] pub event_time: i64,
    #[serde(rename = "T")] pub trade_time: i64,
    #[serde(rename = "s")] pub symbol: String,
    #[serde(rename = "p")] pub price: String,
    #[serde(rename = "q")] pub qty: String,
    #[serde(rename = "m")] pub buyer_is_maker: bool,
    #[serde(rename = "a", default)] pub agg_trade_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DepthUpdateEvent {
    #[serde(rename = "e")] pub event_type: String,
    #[serde(rename = "E")] pub event_time: i64,
    #[serde(rename = "T", default)] pub transact_time: Option<i64>,
    #[serde(rename = "s")] pub symbol: String,
    #[serde(rename = "U")] pub first_update_id: u64,
    #[serde(rename = "u")] pub final_update_id: u64,
    #[serde(rename = "pu", default)] pub prev_final_update_id: Option<u64>,
    #[serde(rename = "b", default)] pub bids: Vec<[String; 2]>,
    #[serde(rename = "a", default)] pub asks: Vec<[String; 2]>,
}

pub type AlphaWs = AlphaWsClient;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_name_builders() {
        assert_eq!(agg_trade_stream("ALPHA_971"), "alpha_971usdt@aggTrade");
        assert_eq!(depth_stream("ALPHA_971"), "alpha_971usdt@depth@100ms");
        assert_eq!(agg_trade_stream("971"), "alpha_971usdt@aggTrade");
    }

    #[test]
    fn agg_trade_decodes() {
        let raw = r#"{"e":"aggTrade","E":1779369537030,"T":1779369536868,"s":"ALPHA_971USDT","a":164916,"p":"0.000005540","q":"98228237.90","f":164918,"l":164918,"m":false}"#;
        let e: AggTradeEvent = serde_json::from_str(raw).unwrap();
        assert_eq!(e.symbol, "ALPHA_971USDT");
        assert_eq!(e.price, "0.000005540");
    }

    #[test]
    fn depth_update_decodes() {
        let raw = r#"{"e":"depthUpdate","E":1779369537289,"T":1779369537256,"s":"ALPHA_971USDT","U":59178703253,"u":59178704340,"pu":59178703146,"b":[["0.000005530","34940853.20"]],"a":[["0.000005540","9802441.00"]]}"#;
        let e: DepthUpdateEvent = serde_json::from_str(raw).unwrap();
        assert_eq!(e.first_update_id, 59178703253);
        assert_eq!(e.prev_final_update_id, Some(59178703146));
    }
}
