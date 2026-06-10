//! Binance Alpha REST + WebSocket 客户端。
//!
//! 端点对照详见 `docs/design/alpha-network-capture-2026-05-21.md`。

pub mod rest;
pub mod twofa;
pub mod types;
pub mod ws;

pub use rest::{round_to_step, AlphaRest, AuthBundle, SharedAlphaRest};
pub use types::*;
pub use ws::{
    agg_trade_stream, depth_stream, AggTradeEvent, AlphaWs, AlphaWsClient, DepthUpdateEvent,
    StreamEvent, WsError,
};
