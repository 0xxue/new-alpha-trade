//! v2 smart 策略决策。
//!
//! 输入：盘口快照 + 近期成交流 + 参数
//! 输出：Decision enum
//!
//! 决策矩阵见 docs/design/strategy-v2.md §3。

use std::collections::{BTreeMap, VecDeque};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use binance_alpha::{
    AggTradeEvent, AlphaRest, AlphaWsClient, DepthUpdateEvent, OrderBookSnapshot, StreamEvent,
};
use std::time::Instant;
use rust_decimal::Decimal;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Decision {
    /// 卖盘强 → 跳过本轮，sleep 1s 重评估
    SkipBearish,
    /// V2.tune5: 市场质量不达标（spread 太宽 / 价格波动大）→ 跳过等好时机
    WaitForBetter,
    /// spread 大 + 买盘强 + 涨势 → 双 maker 抢价差
    DoubleMaker,
    /// spread 大 + 中性 → working taker / pending maker（半价差）
    TakerMakerHybrid,
    /// spread 小 + 顺势 → working taker / pending maker
    SmallSpreadFollow,
    /// 其它（spread 中性、市场平淡）→ v1 fast 双 taker
    Fast,
}

impl Decision {
    pub fn label(&self) -> &'static str {
        match self {
            Self::SkipBearish => "skip_bearish",
            Self::WaitForBetter => "wait_for_better",
            Self::DoubleMaker => "double_maker",
            Self::TakerMakerHybrid => "taker_maker_hybrid",
            Self::SmallSpreadFollow => "small_spread_follow",
            Self::Fast => "fast",
        }
    }
    pub fn is_skip(&self) -> bool {
        matches!(self, Self::SkipBearish | Self::WaitForBetter)
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DecisionParams {
    pub spread_min_for_maker_tick: u32,
    pub imbalance_long_threshold: f64,
    pub imbalance_short_threshold: f64,
    pub trend_window: usize,
    pub trend_signal_threshold: i32,
    pub book_depth_levels: usize,
    /// V2.tune5：spread > 此值 → WaitForBetter（等好时机）
    /// NEX 上 1 tick ≈ 2 bps，max=2 表示愿意接受 ≤4 bps spread
    pub max_quality_spread_ticks: u32,
    /// V2.tune40：按币价 bps 的点差门槛（Some 时启用）。spread/best_bid*1e4 > 此值 → WaitForBetter。
    /// 解决固定 tick 门槛对不同币价失效的问题（NEX tick=1e-9 极小 → tick 门槛永不触发；
    /// 而按 bps 对所有币价一视同仁）。每-job 通过 params_json `_quality_spread_bps` 开启。
    pub max_quality_spread_bps: Option<u32>,
    /// V2.tune5：最近 N 笔成交价格 range > 此值 ticks → WaitForBetter
    /// 价格剧烈波动时 OTO 容易 timeout/部分成交，避开
    pub max_quality_volatility_ticks: u32,
    /// V2.tune5：价格波动率窗口
    pub volatility_window: usize,
}

impl Default for DecisionParams {
    fn default() -> Self {
        Self {
            // V2.tune1 实测把 3→2 让 maker timeout 翻倍，回退到 3。
            spread_min_for_maker_tick: 3,
            imbalance_long_threshold: 0.6,
            imbalance_short_threshold: 0.4,
            trend_window: 10,
            trend_signal_threshold: 3,
            book_depth_levels: 5,
            // V2.tune5 实测：max_spread=1 在当前 NEX 市场（spread 经常 5-12 ticks）会
            // 100% skip 卡死。先放宽到 99999 关闭 quality gate（行为退化为原 V2 fast）。
            // 等 LiveOrderBook 数据稳定后再启用更小阈值精筛。
            max_quality_spread_ticks: 99999,
            max_quality_spread_bps: None,
            max_quality_volatility_ticks: 99999,
            volatility_window: 8,
        }
    }
}

/// 一笔最近成交的精简记录
#[derive(Debug, Clone)]
pub struct RecentTrade {
    pub price: Decimal,
    pub qty: Decimal,
    /// m=true → 卖方主动（价格下方向）
    pub buyer_is_maker: bool,
    pub trade_ts_ms: i64,
    /// V2.tune15: 哪个 pair 的成交（"ALPHA_971USDT" / "ALPHA_162USDT" 等）。
    /// B2/NEX 同时跑时用来区分 anchor 价。
    pub symbol: String,
}

/// 共享的最近成交 buffer。
/// 启动后订阅 `alpha_<id>usdt@aggTrade`，每来一笔 push；只保留最近 N 笔。
pub struct RecentTrades {
    inner: RwLock<VecDeque<RecentTrade>>,
    cap: usize,
}

impl RecentTrades {
    pub fn new(cap: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(VecDeque::with_capacity(cap)),
            cap,
        })
    }

    pub fn push(&self, t: RecentTrade) {
        if let Ok(mut g) = self.inner.write() {
            if g.len() >= self.cap {
                g.pop_front();
            }
            g.push_back(t);
        }
    }

    pub fn snapshot(&self) -> Vec<RecentTrade> {
        self.inner.read().map(|g| g.iter().cloned().collect()).unwrap_or_default()
    }

    /// V2.tune15: 拿指定 symbol 最近一笔成交价。
    /// 没匹配返回 None；调用方应用 None 做退化处理。
    pub fn last_price_for(&self, symbol: &str) -> Option<Decimal> {
        let g = self.inner.read().ok()?;
        g.iter().rev().find(|t| t.symbol == symbol).map(|t| t.price)
    }

    pub fn len(&self) -> usize {
        self.inner.read().map(|g| g.len()).unwrap_or(0)
    }

    /// 启动后台任务订阅 ws 的 aggTrade 流并 push。返回 task handle 不主动 abort。
    pub fn spawn_consumer(self: &Arc<Self>, ws: Arc<AlphaWsClient>) {
        let me = self.clone();
        let mut rx = ws.subscribe_handle();
        tokio::spawn(async move {
            while let Ok(evt) = rx.recv().await {
                if let Some(t) = parse_agg_trade(&evt) {
                    me.push(t);
                }
            }
        });
    }
}

/// V2.tune15: 完整实现 — 用 `@depth@100ms` 增量流维护本地真实 orderbook。
///
/// 旧 V2.tune6 用 `@fulldepth@500ms` 周期 snapshot，B2 测试发现币安推送的 snapshot
/// 包含大量 stale outliers（asks=0.50/0.60 vs 真实 0.66；bids=0.68 vs 真实 0.66）。
/// filter 救不了所有 stale → B2 下单全部 timeout。
///
/// 现改用 @depth 增量流（仿旧 trading_agent.py + websocket_orderbook.py）：
/// - 启动时 REST snapshot 加载基线 + lastUpdateId
/// - WS event 增量应用（qty=0 表示删除该价位）
/// - stale entries 被 remove event 自然清掉
///
/// 单 symbol 模式：start_with_symbol 时初始化一个 symbol 的 book，其它 symbol fallback REST。
pub struct LiveOrderBook {
    inner: RwLock<Option<LiveBookState>>,
    fresh_window: std::time::Duration,
    /// 拿 REST snapshot 用
    alpha: Arc<AlphaRest>,
}

/// 内部状态 — BTreeMap 自动按 key 排序
struct LiveBookState {
    symbol: String,
    last_update_id: u64,
    /// price → qty （bids 取最大 key 是 best_bid）
    bids: BTreeMap<Decimal, Decimal>,
    /// price → qty （asks 取最小 key 是 best_ask）
    asks: BTreeMap<Decimal, Decimal>,
    updated_at: Instant,
}

impl LiveOrderBook {
    pub fn new(fresh_window_s: u64, alpha: Arc<AlphaRest>) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(None),
            fresh_window: std::time::Duration::from_secs(fresh_window_s),
            alpha,
        })
    }

    /// 拿最新 orderbook，仅 fresh + sanity 通过返回 Some
    pub fn snapshot_fresh(&self) -> Option<OrderBookSnapshot> {
        let g = self.inner.read().ok()?;
        let state = g.as_ref()?;
        if state.updated_at.elapsed() > self.fresh_window {
            return None;
        }
        let best_bid = state.bids.keys().next_back()?;
        let best_ask = state.asks.keys().next()?;
        // crossed book (ask < bid) → stale，rejected。
        // locked book (ask == bid) 在 1-tick 精度的 Alpha 市场是正常 in-flight 状态，
        // 仅几 ms 后会被新的 add/remove event 修正 → 允许通过。
        if best_ask < best_bid {
            return None;
        }
        use rust_decimal::prelude::ToPrimitive;
        let bid_f = best_bid.to_f64()?;
        let ask_f = best_ask.to_f64()?;
        if ask_f > bid_f * 1.02 {
            return None;
        }
        // 输出 top 20 档
        Some(build_snapshot(state, 20))
    }

    /// 永远返回最新（含 stale）
    pub fn snapshot_any(&self) -> Option<OrderBookSnapshot> {
        let g = self.inner.read().ok()?;
        let state = g.as_ref()?;
        Some(build_snapshot(state, 20))
    }

    pub fn current_symbol(&self) -> Option<String> {
        self.inner.read().ok()?.as_ref().map(|s| s.symbol.clone())
    }

    pub fn age(&self) -> Option<std::time::Duration> {
        let g = self.inner.read().ok()?;
        g.as_ref().map(|s| s.updated_at.elapsed())
    }

    /// 初始化（或重新初始化）一个 symbol 的 book：REST snapshot 作基线。
    /// 调用方应在切换 symbol 时手动调一次。
    pub async fn init_snapshot(&self, symbol: &str) -> anyhow::Result<()> {
        let snap = self.alpha.get_full_depth(symbol, 100).await?;
        let mut bids = BTreeMap::new();
        let mut asks = BTreeMap::new();
        for b in &snap.bids {
            if let (Ok(p), Ok(q)) = (Decimal::from_str(&b[0]), Decimal::from_str(&b[1])) {
                if q > Decimal::ZERO {
                    bids.insert(p, q);
                }
            }
        }
        for a in &snap.asks {
            if let (Ok(p), Ok(q)) = (Decimal::from_str(&a[0]), Decimal::from_str(&a[1])) {
                if q > Decimal::ZERO {
                    asks.insert(p, q);
                }
            }
        }
        let state = LiveBookState {
            symbol: symbol.to_string(),
            last_update_id: snap.last_update_id,
            bids,
            asks,
            updated_at: Instant::now(),
        };
        if let Ok(mut g) = self.inner.write() {
            *g = Some(state);
        }
        tracing::info!(symbol, last_update_id = snap.last_update_id, "live_book: snapshot loaded");
        Ok(())
    }

    /// 应用一个增量 event。如果 symbol 不匹配或 lastUpdateId 落后则忽略。
    fn apply(&self, evt: &DepthUpdateEvent) {
        if let Ok(mut g) = self.inner.write() {
            let state = match g.as_mut() {
                Some(s) => s,
                None => return, // 未初始化
            };
            if evt.symbol != state.symbol {
                // 别的 symbol，忽略
                return;
            }
            // last_update_id 校验：alpha @depth 流的 event.u 应严格递增
            if evt.final_update_id <= state.last_update_id {
                return;
            }
            // 应用 bids
            for b in &evt.bids {
                if let (Ok(p), Ok(q)) = (Decimal::from_str(&b[0]), Decimal::from_str(&b[1])) {
                    if q.is_zero() {
                        state.bids.remove(&p);
                    } else {
                        state.bids.insert(p, q);
                    }
                }
            }
            // 应用 asks
            for a in &evt.asks {
                if let (Ok(p), Ok(q)) = (Decimal::from_str(&a[0]), Decimal::from_str(&a[1])) {
                    if q.is_zero() {
                        state.asks.remove(&p);
                    } else {
                        state.asks.insert(p, q);
                    }
                }
            }
            state.last_update_id = evt.final_update_id;
            state.updated_at = Instant::now();
        }
    }

    /// 启动后台任务订阅 ws depth 流并 apply
    pub fn spawn_consumer(self: &Arc<Self>, ws: Arc<AlphaWsClient>) {
        let me = self.clone();
        let mut rx = ws.subscribe_handle();
        tokio::spawn(async move {
            while let Ok(evt) = rx.recv().await {
                if let Some(depth) = parse_depth(&evt) {
                    me.apply(&depth);
                }
            }
        });
        // V2.tune15: 周期 + 异常 resync — 修复增量丢包导致的 drift。
        // - 每 5s 抽检：若 book 是 crossed（bid > ask），说明漏了 remove event → 立刻 REST 重建
        // - 兜底 60s 定期 resync 即使没有 crossed 也刷一次
        // Binance 推荐的 depth maintenance 协议要求严格的 U/u 重叠校验；我们用简化版（直接 init_snapshot）。
        let me2 = self.clone();
        tokio::spawn(async move {
            let mut last_force_resync = Instant::now();
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            interval.tick().await; // 第一次立刻 tick，跳过
            loop {
                interval.tick().await;
                let sym_opt = me2.current_symbol();
                let crossed = me2.is_crossed();
                let force = last_force_resync.elapsed() >= std::time::Duration::from_secs(60);
                if let Some(s) = sym_opt {
                    if crossed || force {
                        if let Err(e) = me2.init_snapshot(&s).await {
                            tracing::warn!(symbol=%s, err=%e, crossed, force, "live_book resync failed");
                        } else {
                            tracing::debug!(symbol=%s, crossed, force, "live_book resync ok");
                            if force {
                                last_force_resync = Instant::now();
                            }
                        }
                    }
                }
            }
        });
    }

    /// 检查当前 book 是否 crossed（bid > ask）。crossed 说明漏了 remove event。
    pub fn is_crossed(&self) -> bool {
        let g = match self.inner.read() {
            Ok(g) => g,
            Err(_) => return false,
        };
        let state = match g.as_ref() {
            Some(s) => s,
            None => return false,
        };
        let best_bid = match state.bids.keys().next_back() {
            Some(b) => b,
            None => return false,
        };
        let best_ask = match state.asks.keys().next() {
            Some(a) => a,
            None => return false,
        };
        best_bid > best_ask
    }
}

/// 从 BTreeMap 状态构建一个 OrderBookSnapshot（top N 档）
fn build_snapshot(state: &LiveBookState, depth: usize) -> OrderBookSnapshot {
    // bids: 按 price 降序取 top N
    let bids: Vec<[String; 2]> = state
        .bids
        .iter()
        .rev()
        .take(depth)
        .map(|(p, q)| [p.to_string(), q.to_string()])
        .collect();
    // asks: 按 price 升序取 top N
    let asks: Vec<[String; 2]> = state
        .asks
        .iter()
        .take(depth)
        .map(|(p, q)| [p.to_string(), q.to_string()])
        .collect();
    OrderBookSnapshot {
        last_update_id: state.last_update_id,
        symbol: state.symbol.clone(),
        bids,
        asks,
        event_time: None,
        transact_time: None,
    }
}

fn parse_depth(evt: &StreamEvent) -> Option<DepthUpdateEvent> {
    if !evt.stream.contains("depth") {
        return None;
    }
    serde_json::from_value(evt.data.clone()).ok()
}

fn parse_agg_trade(evt: &StreamEvent) -> Option<RecentTrade> {
    if !evt.stream.contains("aggTrade") {
        return None;
    }
    let e: AggTradeEvent = serde_json::from_value(evt.data.clone()).ok()?;
    Some(RecentTrade {
        price: Decimal::from_str(&e.price).ok()?,
        qty: Decimal::from_str(&e.qty).ok()?,
        buyer_is_maker: e.buyer_is_maker,
        trade_ts_ms: e.trade_time,
        symbol: e.symbol,
    })
}

/// 计算盘口失衡：bid_qty_sum / (bid_qty_sum + ask_qty_sum)，前 N 档累加。
pub fn book_imbalance(book: &OrderBookSnapshot, depth_levels: usize) -> Option<f64> {
    let bid_sum: Decimal = book
        .bids
        .iter()
        .take(depth_levels)
        .filter_map(|b| Decimal::from_str(&b[1]).ok())
        .sum();
    let ask_sum: Decimal = book
        .asks
        .iter()
        .take(depth_levels)
        .filter_map(|a| Decimal::from_str(&a[1]).ok())
        .sum();
    let total = bid_sum + ask_sum;
    if total.is_zero() {
        return None;
    }
    let ratio = bid_sum / total;
    use rust_decimal::prelude::ToPrimitive;
    ratio.to_f64()
}

/// 趋势信号：m=false（买方主动）+1，m=true（卖方主动）-1，求和。
/// 范围 [-window, +window]
pub fn trend_signal(recent: &[RecentTrade], window: usize) -> i32 {
    let take = recent.iter().rev().take(window);
    let mut sum = 0_i32;
    for t in take {
        if t.buyer_is_maker {
            sum -= 1;
        } else {
            sum += 1;
        }
    }
    sum
}

/// V2.tune5：最近 window 笔成交价格的 max-min 范围（按 tick 数），用作波动率指标。
/// 0 = 价格完全稳定；越大表示市场越剧烈。
pub fn price_volatility_ticks(
    recent: &[RecentTrade],
    window: usize,
    tick: Decimal,
) -> u32 {
    use rust_decimal::prelude::ToPrimitive;
    if recent.is_empty() || tick.is_zero() {
        return 0;
    }
    let prices: Vec<Decimal> = recent.iter().rev().take(window).map(|t| t.price).collect();
    if prices.len() < 2 {
        return 0;
    }
    let max = prices.iter().copied().max().unwrap_or(Decimal::ZERO);
    let min = prices.iter().copied().min().unwrap_or(Decimal::ZERO);
    let range = max - min;
    (range / tick).to_u32().unwrap_or(0)
}

/// 决策矩阵实现。详见 docs/design/strategy-v2.md §3。
pub fn evaluate(
    book: &OrderBookSnapshot,
    recent: &[RecentTrade],
    tick_size: Decimal,
    p: &DecisionParams,
) -> (Decision, DecisionContext) {
    use rust_decimal::prelude::ToPrimitive;

    // 计算特征
    let best_bid = book
        .bids
        .first()
        .and_then(|b| Decimal::from_str(&b[0]).ok())
        .unwrap_or(Decimal::ZERO);
    let best_ask = book
        .asks
        .first()
        .and_then(|a| Decimal::from_str(&a[0]).ok())
        .unwrap_or(Decimal::ZERO);

    let spread_dec = best_ask - best_bid;
    let spread_ticks = if tick_size.is_zero() {
        0
    } else {
        (spread_dec / tick_size).to_u32().unwrap_or(0)
    };
    // V2.tune40：按币价 bps 的点差（跨不同币价一视同仁的质量门槛用）
    let spread_bps: u32 = if best_bid > Decimal::ZERO {
        ((spread_dec / best_bid) * Decimal::from(10000))
            .to_u32()
            .unwrap_or(0)
    } else {
        0
    };
    let imbalance = book_imbalance(book, p.book_depth_levels).unwrap_or(0.5);
    let trend = trend_signal(recent, p.trend_window);
    // V2.tune5：算市场波动率（最近 N 笔成交价 max-min）
    let volatility = price_volatility_ticks(recent, p.volatility_window, tick_size);

    // 决策矩阵
    let bear = imbalance < p.imbalance_short_threshold;
    let bull = imbalance > p.imbalance_long_threshold;
    let trending_up = trend > p.trend_signal_threshold;
    let trending_down = trend < -p.trend_signal_threshold;
    let large_spread = spread_ticks >= p.spread_min_for_maker_tick;
    let small_spread = spread_ticks > 0 && spread_ticks < p.spread_min_for_maker_tick;
    // V2.tune5 quality gate：spread 太宽 或 价格剧烈波动 → 等好时机
    // 注意：放在 SkipBearish 之后但在 maker/fast 决策之前
    // SkipBearish 是结构性看空（继续刷会拉低市场），WaitForBetter 是临时性流动性差
    let quality_bad = spread_ticks > p.max_quality_spread_ticks
        || volatility > p.max_quality_volatility_ticks
        || p.max_quality_spread_bps.map_or(false, |m| spread_bps > m);

    // 实验性优化 A：放宽 SmallSpreadFollow 触发（把更多 fast 流量导到 maker pending）。
    // 默认关闭（保守的原始决策矩阵已充分验证 ~-4 bps）。
    // 实测放宽对 NEX 收益不明显（NEX 大部分时间 spread=0，触发不到 small_spread 分支）。
    // 想试验设 `OPTIMIZATION_A=on`：把 small_spread 触发从 "bull && trending_up" 放宽到 "!bear"。
    let opt_a_on = std::env::var("OPTIMIZATION_A").ok().as_deref() == Some("on");
    let small_spread_trigger = if opt_a_on {
        small_spread && !bear  // 放宽：只要不 bear 就走 maker
    } else {
        small_spread && bull && trending_up  // 默认：严格触发
    };

    let d = if bear && trending_down {
        Decision::SkipBearish
    } else if quality_bad {
        // 跳过这一 round 等市场变好。下游 run_oto_smart_round 会 skip + sleep 重评估
        Decision::WaitForBetter
    } else if large_spread && bull && trending_up {
        Decision::DoubleMaker
    } else if large_spread && !bear {
        Decision::TakerMakerHybrid
    } else if small_spread_trigger {
        Decision::SmallSpreadFollow
    } else {
        Decision::Fast
    };

    // V2.tune7: 抓 best_bid_qty / best_ask_qty 给 single_qty cap 用
    let best_bid_qty = book
        .bids
        .first()
        .and_then(|b| Decimal::from_str(&b[1]).ok())
        .unwrap_or(Decimal::ZERO);
    let best_ask_qty = book
        .asks
        .first()
        .and_then(|a| Decimal::from_str(&a[1]).ok())
        .unwrap_or(Decimal::ZERO);
    let ctx = DecisionContext {
        spread_ticks,
        spread_bps,
        imbalance,
        trend,
        best_bid,
        best_ask,
        best_bid_qty,
        best_ask_qty,
    };
    (d, ctx)
}

#[derive(Debug, Clone, Copy)]
pub struct DecisionContext {
    pub spread_ticks: u32,
    pub spread_bps: u32,
    pub imbalance: f64,
    pub trend: i32,
    pub best_bid: Decimal,
    pub best_ask: Decimal,
    /// V2.tune7：best_bid / best_ask 那一档的数量（用于 cap single_qty 避免吃多档）
    pub best_bid_qty: Decimal,
    pub best_ask_qty: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn book(bids: &[(&str, &str)], asks: &[(&str, &str)]) -> OrderBookSnapshot {
        OrderBookSnapshot {
            last_update_id: 1,
            symbol: "ALPHA_971USDT".into(),
            bids: bids.iter().map(|(p, q)| [(*p).to_string(), (*q).to_string()]).collect(),
            asks: asks.iter().map(|(p, q)| [(*p).to_string(), (*q).to_string()]).collect(),
            event_time: None,
            transact_time: None,
        }
    }

    fn trades(seq: &[bool]) -> Vec<RecentTrade> {
        // bool = buyer_is_maker (true = 卖方主动 = -1)
        seq.iter()
            .enumerate()
            .map(|(i, &m)| RecentTrade {
                price: dec!(1),
                qty: dec!(1),
                buyer_is_maker: m,
                trade_ts_ms: i as i64,
                symbol: "ALPHA_971USDT".into(),
            })
            .collect()
    }

    /// V2.tune5 之后：现有测试要禁用 quality gate（max_spread/volatility=∞）
    /// 才能命中原 decision 矩阵（否则大 spread 测试用例会被 WaitForBetter 拦截）。
    fn permissive_params() -> DecisionParams {
        DecisionParams {
            max_quality_spread_ticks: 9999,
            max_quality_volatility_ticks: 9999,
            ..DecisionParams::default()
        }
    }

    #[test]
    fn imbalance_balanced() {
        let b = book(&[("1", "100")], &[("2", "100")]);
        assert_eq!(book_imbalance(&b, 5), Some(0.5));
    }

    #[test]
    fn imbalance_bull() {
        let b = book(&[("1", "300")], &[("2", "100")]);
        assert_eq!(book_imbalance(&b, 5), Some(0.75));
    }

    #[test]
    fn trend_calc() {
        // 8 个 m=false (+1) + 2 个 m=true (-1) = +6
        let xs = trades(&[false, false, false, false, false, false, false, false, true, true]);
        assert_eq!(trend_signal(&xs, 10), 6);
    }

    #[test]
    fn decision_skip_when_bearish() {
        let p = permissive_params();
        // bid sum < ask sum (imbalance < 0.4) + 卖方主动趋势
        let b = book(&[("1", "10")], &[("2", "100"), ("3", "100")]);
        let recent = trades(&[true; 10]); // 全 m=true → trend = -10
        let (d, _) = evaluate(&b, &recent, dec!(1), &p);
        assert_eq!(d, Decision::SkipBearish);
    }

    #[test]
    fn decision_double_maker_when_bull_large_spread() {
        let p = permissive_params();
        // spread = 5 (≥3 tick=1), bid 重 ask 轻 (imbalance > 0.6), 趋势涨
        let b = book(&[("100", "300")], &[("105", "100")]); // spread = 5, imbalance = 0.75
        let recent = trades(&[false; 10]); // trend = +10
        let (d, ctx) = evaluate(&b, &recent, dec!(1), &p);
        assert_eq!(d, Decision::DoubleMaker);
        assert_eq!(ctx.spread_ticks, 5);
    }

    #[test]
    fn decision_fast_when_small_spread_neutral() {
        let p = permissive_params();
        let b = book(&[("100", "100")], &[("101", "100")]); // spread 1 tick, balanced
        let recent = trades(&[false, false, true, true, false, true]); // mixed
        let (d, _) = evaluate(&b, &recent, dec!(1), &p);
        assert_eq!(d, Decision::Fast);
    }

    #[test]
    fn decision_fast_when_spread_zero() {
        let p = permissive_params();
        let b = book(&[("100", "100")], &[("100", "100")]);
        let (d, _) = evaluate(&b, &[], dec!(1), &p);
        assert_eq!(d, Decision::Fast);
    }

    #[test]
    fn decision_taker_maker_hybrid_when_large_spread_neutral() {
        let p = permissive_params();
        let b = book(&[("100", "100")], &[("105", "100")]); // spread 5
        let (d, _) = evaluate(&b, &trades(&[false, true]), dec!(1), &p);
        assert_eq!(d, Decision::TakerMakerHybrid);
    }

    // V2.tune5 新增 quality gate 测试

    /// V2.tune5 之后 defaults 把 quality gate 调成 99999（关闭）。
    /// 这两个测试针对 quality gate 行为，需手动启用小阈值。
    fn quality_gate_params() -> DecisionParams {
        DecisionParams {
            max_quality_spread_ticks: 1,
            max_quality_volatility_ticks: 2,
            ..DecisionParams::default()
        }
    }

    #[test]
    fn decision_wait_for_better_on_wide_spread() {
        let p = quality_gate_params(); // max_quality_spread=1
        let b = book(&[("100", "100")], &[("105", "100")]); // spread=5 ticks
        let (d, ctx) = evaluate(&b, &trades(&[false, true, false]), dec!(1), &p);
        assert_eq!(d, Decision::WaitForBetter);
        assert_eq!(ctx.spread_ticks, 5);
    }

    #[test]
    fn decision_normal_passes_quality_when_spread_small() {
        let p = quality_gate_params();
        // spread = 1 tick (= max_quality_spread_ticks), 应该过 quality gate
        // imbalance balanced + 价格稳定 → 命中 Fast
        let b = book(&[("100", "100")], &[("101", "100")]); // spread=1
        let (d, _) = evaluate(&b, &trades(&[false, true, false, true]), dec!(1), &p);
        assert_eq!(d, Decision::Fast);
    }

    // V2.tune40: 按 bps 的点差门槛
    #[test]
    fn decision_wait_for_better_on_wide_spread_bps() {
        // spread=5, best_bid=100 → spread_bps = 5/100*1e4 = 500 bps；门槛 100 → 触发跳过
        let p = DecisionParams {
            max_quality_spread_bps: Some(100),
            ..DecisionParams::default()
        };
        let b = book(&[("100", "100")], &[("105", "100")]);
        let (d, ctx) = evaluate(&b, &trades(&[false, true, false]), dec!(1), &p);
        assert_eq!(ctx.spread_bps, 500);
        assert_eq!(d, Decision::WaitForBetter);
    }

    #[test]
    fn decision_bps_gate_passes_when_tight() {
        // spread=1, best_bid=100 → 100 bps == 门槛，不超过（> 才拦）→ 不跳过
        let p = DecisionParams {
            max_quality_spread_bps: Some(100),
            ..DecisionParams::default()
        };
        let b = book(&[("100", "100")], &[("101", "100")]);
        let (d, _) = evaluate(&b, &trades(&[false, true, false, true]), dec!(1), &p);
        assert_ne!(d, Decision::WaitForBetter);
    }

    #[test]
    fn price_volatility_zero_when_all_same() {
        let mut xs = trades(&[false; 5]);
        for t in xs.iter_mut() {
            t.price = dec!(100);
        }
        assert_eq!(price_volatility_ticks(&xs, 5, dec!(1)), 0);
    }

    #[test]
    fn price_volatility_ticks_range() {
        let mut xs = vec![
            RecentTrade { price: dec!(100), qty: dec!(1), buyer_is_maker: false, trade_ts_ms: 0, symbol: "X".into() },
            RecentTrade { price: dec!(103), qty: dec!(1), buyer_is_maker: false, trade_ts_ms: 1, symbol: "X".into() },
            RecentTrade { price: dec!(101), qty: dec!(1), buyer_is_maker: false, trade_ts_ms: 2, symbol: "X".into() },
        ];
        // range = 103-100 = 3, tick=1 → 3 ticks
        assert_eq!(price_volatility_ticks(&xs, 10, dec!(1)), 3);
        // 不够 window 不影响（window=10 但只 3 条）
        xs.push(RecentTrade { price: dec!(105), qty: dec!(1), buyer_is_maker: false, trade_ts_ms: 3, symbol: "X".into() });
        assert_eq!(price_volatility_ticks(&xs, 10, dec!(1)), 5); // 105-100=5
    }

    #[test]
    fn decision_wait_for_better_on_volatile_price() {
        // spread=0 (能过 spread gate) 但价格波动 5 ticks > max_quality_volatility=2
        let p = quality_gate_params();
        let b = book(&[("100", "100")], &[("100", "100")]); // spread=0
        let mut recent = trades(&[false; 5]);
        let prices = [dec!(100), dec!(103), dec!(101), dec!(99), dec!(105)]; // range=6
        for (t, p) in recent.iter_mut().zip(prices.iter()) {
            t.price = *p;
        }
        let (d, _) = evaluate(&b, &recent, dec!(1), &p);
        assert_eq!(d, Decision::WaitForBetter);
    }

    #[test]
    fn recent_trades_buffer_caps() {
        let rt = RecentTrades::new(3);
        for i in 0..5 {
            rt.push(RecentTrade {
                price: Decimal::from(i),
                qty: dec!(1),
                buyer_is_maker: false,
                trade_ts_ms: i,
                symbol: "X".into(),
            });
        }
        let snap = rt.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].trade_ts_ms, 2);
        assert_eq!(snap[2].trade_ts_ms, 4);
    }
}
