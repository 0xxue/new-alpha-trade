//! 策略层。
//!
//! P4 阶段会有一个**通路验证策略**（中间价 ± 固定 bp 挂单），目的是把全链路跑通。
//! P5+ 阶段才写真正的 `adaptive_maker` v1 策略。
//!
//! 真正的策略设计要单独写一份 `docs/strategies-v1.md`，本 crate 只提供 trait 和注册表。

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Decision {
    Buy { price: Decimal, qty: Decimal },
    Sell { price: Decimal, qty: Decimal },
    Cancel { order_id: String },
    Wait,
}

/// 所有策略实现这个 trait。
///
/// 注意：在 P4 之前这个 trait 都不会被真正调用，只是占位让架构闭环。
pub trait Strategy: Send + Sync {
    fn name(&self) -> &str;
    fn params_schema(&self) -> serde_json::Value;
    // TODO P4: on_book_update / on_fill / on_tick 三个钩子
}
