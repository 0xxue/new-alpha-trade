//! `/ws/stream` WebSocket：把 binance-alpha 来的实时行情广播给前端浏览器。
//!
//! V0：所有前端连接共享同一份订阅集，事件全广播；前端自己按 stream 名过滤。
//! 后续多 symbol / 用户私有事件再加 hub 路由层。

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use binance_alpha::StreamEvent;
use serde_json::json;
use tracing::{debug, info, warn};

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/stream", get(stream))
}

async fn stream(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let rx = state.alpha_ws.subscribe_handle();
    ws.on_upgrade(move |socket| handle_socket(socket, rx))
}

async fn handle_socket(mut socket: WebSocket, mut rx: tokio::sync::broadcast::Receiver<StreamEvent>) {
    info!("/ws/stream client connected");

    // 发个 hello，前端能立刻看到连接成功
    let hello = json!({
        "type": "hello",
        "service": "trading-engine",
        "version": env!("CARGO_PKG_VERSION"),
    });
    if socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
    ping_interval.tick().await;

    loop {
        tokio::select! {
            evt = rx.recv() => {
                match evt {
                    Ok(e) => {
                        let frame = json!({
                            "type": "market",
                            "stream": e.stream,
                            "data": e.data,
                        });
                        if socket.send(Message::Text(frame.to_string().into())).await.is_err() {
                            debug!("client send failed, drop");
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(lagged = n, "client lagged, dropped events");
                        // 不断连，继续往下推
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        warn!("broadcast closed, terminating socket");
                        return;
                    }
                }
            }
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => {
                        debug!("client disconnected");
                        return;
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = socket.send(Message::Pong(p)).await;
                    }
                    Some(Ok(Message::Text(t))) => {
                        // 暂时不处理前端发来的指令；后续多 symbol 订阅时在此扩展
                        debug!(?t, "client text frame ignored");
                    }
                    _ => {}
                }
            }
            _ = ping_interval.tick() => {
                if socket.send(Message::Ping(vec![].into())).await.is_err() {
                    return;
                }
            }
        }
    }
}
