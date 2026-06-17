//! P4 通路验证策略：simple_round
//!
//! 循环 BUY → 等成交 → SELL → 等成交，直到 buy_volume_usdt 累积到 target_volume。
//!
//! 这不是"adaptive maker"真策略，只是为了端到端验证下单链路在自动循环下能跑通。
//! 真策略放在 crates/strategy/ 实现，等 P5+。

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use binance_alpha::{
    usdt_funding_free, AuthBundle, CancelOrderRequest, OrderType, PaymentDetail, PlaceOrderRequest,
    PlaceOtoOrderRequest, Side, WalletType,
};
use persistence::repo::rounds;

use crate::decision::{self, Decision, DecisionParams};
use persistence::repo::{jobs, orders, stats as stats_repo, trades};
use rust_decimal::{Decimal, RoundingStrategy};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::AppState;

/// taker 单价格越过 best 的 tick 数。
/// V2.tune1 实测：PRICE_BUMP=1 让部分成交率飙升，timeout 翻倍 (8→21)，
/// emergency_sell 拖累 wear 从 -6 → -13 bps。回退到 2 保证 taker 全成交。
const PRICE_BUMP_TICKS: u64 = 2;
const TICK_SIZE_STR: &str = "0.000000001";
const STEP_SIZE_STR: &str = "0.10";
const SLEEP_BETWEEN_FILLS_MS: u64 = 1500;
const SLEEP_BETWEEN_ROUNDS_MS: u64 = 800;
const FILL_WAIT_TIMEOUT_S: u64 = 8;
const DEFAULT_SINGLE_USDT: &str = "0.3";

// =============================================================================
// V2.7 风控阈值（命中后 set_state=paused，用户次日 resume 即可续刷）
// =============================================================================

/// wear 自动止损阈值（bps，负数）。命中 N 次连续 → pause。
/// V2.tune9: -15 实测在 mid-run 频繁误触发（vol 小时 bps 被单笔放大）。-50 后改善。
/// V2.tune22: 实测 -50 仍被 OTO 时序间隙假报触发 — pending SELL fill 瞬间 NEX freeze→0
/// 但 USDT 还没到 funding，wear 假算 -191 bps。改成 -300 + 要 2 次连续触发，
/// 避免单次时序噪声。-300 bps = -3% 真亏损，绝不会自然发生。
const WEAR_PAUSE_BPS_THRESHOLD: i64 = -300;
/// V2.tune22: 多少次连续触发才 pause（避免 OTO 时序间隙单次假报）
const WEAR_PAUSE_CONSECUTIVE_HITS: u32 = 2;

/// V2.tune9: wear 风控的 vol 门槛。
/// vol < 100 USDT 时 wear bps 计算被单笔噪声严重污染，跳过风控检查。
const WEAR_CHECK_MIN_VOL_USDT: &str = "100";

/// wear 检查频率（每 N 轮拉一次 SPOT wallet）。每轮拉太重（外网+2FA），N=5 是折衷。
const WEAR_CHECK_EVERY_N_ROUNDS: u32 = 5;

/// 连续 working timeout 多少轮触发 pause。
/// 5 轮 ≈ 25-40s 都挂不上 maker → 市场跑了，再下也是浪费。
const WORKING_TIMEOUT_CONSEC_THRESHOLD: usize = 5;

pub struct JobRunner {
    handles: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

impl JobRunner {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            handles: Mutex::new(HashMap::new()),
        })
    }

    /// 启动一个 job（如果已在跑，先 abort 旧的）。
    pub async fn start(self: &Arc<Self>, state: AppState, job_id: String) {
        let mut hs = self.handles.lock().await;
        if let Some(old) = hs.remove(&job_id) {
            old.abort();
        }
        let me = self.clone();
        let jid = job_id.clone();
        let handle = tokio::spawn(async move {
            run_job(state, jid.clone()).await;
            let mut hs2 = me.handles.lock().await;
            hs2.remove(&jid);
        });
        hs.insert(job_id, handle);
    }

    /// 显式停（如果在跑）。
    pub async fn abort(&self, job_id: &str) {
        let mut hs = self.handles.lock().await;
        if let Some(h) = hs.remove(job_id) {
            h.abort();
        }
    }
}

async fn run_job(state: AppState, job_id: String) {
    let job = match jobs::get(&state.db, &job_id).await {
        Ok(Some(j)) => j,
        Ok(None) => {
            warn!(%job_id, "job not found");
            return;
        }
        Err(e) => {
            warn!(%job_id, err = %e, "load job failed");
            return;
        }
    };
    let target = match Decimal::from_str(&job.target_volume) {
        Ok(d) => d,
        Err(e) => {
            warn!(%job_id, err = %e, "bad target_volume");
            return;
        }
    };
    let symbol = job.symbol.clone();
    let username = job.username.clone();
    // base_asset 优先从 params._base_asset 拿（trade/start 注入）；缺则按 symbol 去 USDT
    let params: serde_json::Value =
        serde_json::from_str(&job.params_json).unwrap_or(serde_json::json!({}));
    let base_asset = params
        .get("_base_asset")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| symbol.trim_end_matches("USDT").to_string());
    let single_min = params
        .get("single_min_usdt")
        .and_then(|v| v.as_str())
        .and_then(|s| Decimal::from_str(s).ok())
        .unwrap_or_else(|| Decimal::from_str(DEFAULT_SINGLE_USDT).unwrap());
    let single_max = params
        .get("single_max_usdt")
        .and_then(|v| v.as_str())
        .and_then(|s| Decimal::from_str(s).ok())
        .unwrap_or(single_min);

    info!(
        %job_id, %username, %symbol, %base_asset, %target, %single_min, %single_max,
        "job started"
    );
    let _ = jobs::set_state(&state.db, &job_id, jobs::JobState::Running).await;

    // V2.tune15: 切换 LiveOrderBook 到当前 job 的 symbol（默认 NEX，B2/其他 token 要切）。
    // - 订阅对应 symbol 的 @aggTrade + @depth@100ms 增量流
    // - REST snapshot 拉初始基线，之后 WS 增量维护
    if state.live_book.current_symbol().as_deref() != Some(symbol.as_str()) {
        info!(
            %job_id, %symbol, %base_asset,
            current_book_symbol = ?state.live_book.current_symbol(),
            "live_book: switching symbol for job"
        );
        // 订阅新 symbol 的流（add_subscriptions 内部去重，重复订阅 NEX 是 noop）
        state.alpha_ws
            .add_subscriptions(vec![
                binance_alpha::agg_trade_stream(&base_asset),
                binance_alpha::depth_stream(&base_asset),
            ])
            .await;
        // 等 WS 订阅 ACK + 几个 event buffer 后再 init snapshot，避免漏增量
        tokio::time::sleep(Duration::from_millis(800)).await;
        if let Err(e) = state.live_book.init_snapshot(&symbol).await {
            warn!(%job_id, %symbol, err = %e, "live_book init_snapshot failed; will fallback REST per round");
        }
    }

    // V2.tune3：从 TokenRegistry 拿真实 tick/step（来自 get-exchange-info），
    // 不再硬编码 NEX 的值。fallback 到 TICK_SIZE_STR/STEP_SIZE_STR 兼容 registry 没拉到的情况。
    let token = state.tokens.find_by_pair(&symbol);
    let tick = token
        .as_ref()
        .and_then(|t| t.tick_size)
        .unwrap_or_else(|| Decimal::from_str(TICK_SIZE_STR).unwrap());
    let step = token
        .as_ref()
        .and_then(|t| t.step_size)
        .unwrap_or_else(|| Decimal::from_str(STEP_SIZE_STR).unwrap());
    info!(
        %job_id, %tick, %step,
        token_tick = ?token.as_ref().and_then(|t| t.tick_size),
        token_step = ?token.as_ref().and_then(|t| t.step_size),
        "tick/step resolved"
    );

    let mut round = 0_u32;
    let mut wear_consec_hits: u32 = 0;  // V2.tune22 连续 wear 触发计数
    loop {
        round += 1;
        // 检查 jobs.state；外部 pause/stop 通过 SQL 改 state
        let st = match jobs::get(&state.db, &job_id).await {
            Ok(Some(j)) => j.state,
            _ => {
                warn!(%job_id, "job vanished mid-run");
                return;
            }
        };
        match st.as_str() {
            "paused" => {
                info!(%job_id, "paused, waiting...");
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            "stopped" | "done" | "failed" => {
                info!(%job_id, state = %st, "stop signal seen → final cleanup → exiting");
                if let Ok(a) = state.qr.get_auth(&username).await {
                    final_cleanup(&state, &a, &job_id, &symbol, &base_asset).await;
                }
                return;
            }
            _ => {}
        }

        // 拿凭据
        let auth = match state.qr.get_auth(&username).await {
            Ok(a) => a,
            Err(e) => {
                warn!(%job_id, %username, err = %e, "get_auth failed, sleep + retry");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        // 检查是否够 target
        let vol = match trades::sum_buy_quote_qty(&state.db, &job_id).await {
            Ok(v) => v,
            Err(e) => {
                warn!(%job_id, err = %e, "sum_buy_quote_qty failed");
                Decimal::ZERO
            }
        };
        if vol >= target {
            info!(%job_id, %vol, %target, "target reached → final cleanup → done");
            final_cleanup(&state, &auth, &job_id, &symbol, &base_asset).await;
            let _ = jobs::set_state(&state.db, &job_id, jobs::JobState::Done).await;
            return;
        }

        // 每轮在 [min, max] 随机选 single（min==max 时退化成固定金额）
        let this_single = if single_min == single_max {
            single_min
        } else {
            let span = single_max - single_min;
            // 用 i64 范围避免 Decimal/u64 转换问题
            let pct: u64 = rand::Rng::gen_range(&mut rand::thread_rng(), 0u64..=10_000u64);
            let frac = Decimal::from(pct) / Decimal::from(10000);
            single_min + span * frac
        };

        // 跑一轮（按 strategy 走不同实现）
        let round_result = match job.strategy.as_str() {
            "oto_smart" => {
                run_oto_smart_round(
                    &state, &auth, &job_id, &symbol, &base_asset, this_single, tick, step, round,
                )
                .await
            }
            "oto" => {
                run_oto_round(
                    &state, &auth, &job_id, &symbol, &base_asset, this_single, tick, step, round,
                )
                .await
            }
            _ => {
                run_round(
                    &state, &auth, &job_id, &symbol, &base_asset, this_single, tick, step, round,
                )
                .await
            }
        };
        if let Err(e) = round_result {
            // V2.fix3：币安要求人脸/手机额外验证 → 立刻 pause，绝不 retry。
            // retry 会反复触发风控，可能升级到账户冻结。用户去币安 App 完成
            // 验证后手动 resume 即可续刷。
            if is_extra_verify_required_error(&e) {
                warn!(
                    %job_id, err = %e,
                    "RISK: face/phone verification required → auto pause (complete in Binance app, then resume)"
                );
                let _ = jobs::set_state(&state.db, &job_id, jobs::JobState::Paused).await;
                continue; // loop 顶部的 state check 会跑 final_cleanup 然后 return
            }
            warn!(%job_id, round, err = %e, "round failed; sleep + continue");
            tokio::time::sleep(Duration::from_millis(800)).await;
        } else {
            tokio::time::sleep(Duration::from_millis(SLEEP_BETWEEN_ROUNDS_MS)).await;
        }

        // V2.7 风控：只对 oto_smart 启用（其他 strategy 不查 rounds 表）。
        // 命中阈值 → set_state=paused，循环下一轮顶上的 state 检查会让 final_cleanup
        // 跑完并退出，job 留在 paused，用户次日 resume 续刷。
        if job.strategy == "oto_smart" && check_risk_and_pause(&state, &job, &job_id, round, &auth, &mut wear_consec_hits).await {
            // 不直接 return — 让 loop 顶部的 state 检查走 final_cleanup 路径，
            // 保证清仓动作走到。
            continue;
        }
    }
}

/// #29 V2.tune6: 优先用 WebSocket LiveOrderBook 拿 fresh 快照，stale 时 fallback REST。
/// fresh 数据延迟 < 500ms（WS 推送频率），比每轮 REST 拉的快得多 + 没有外网开销。
///
/// V2.tune13 关键 bug 修复：LiveOrderBook 全局只缓存一个 symbol 的 book（默认 NEX），
/// 给 B2 job 返回了 NEX 的 book → 下单价格离谱 → decode error。
/// 修法：检查 live_book.symbol 是否匹配请求的 symbol，不匹配立即 fallback REST。
/// 后续 V2.tune14 可改成 multi-symbol live book。
async fn get_orderbook_smart(
    state: &AppState,
    symbol: &str,
) -> Result<binance_alpha::OrderBookSnapshot, binance_alpha::AlphaApiError> {
    // 1) 优先用 live_book（WS 增量）。过滤后非空就用。
    if let Some(b) = state.live_book.snapshot_fresh() {
        if b.symbol == symbol {
            let filtered = anchor_filter(state, symbol, &b);
            if !filtered.bids.is_empty() && !filtered.asks.is_empty() {
                return Ok(filtered);
            }
            // V2.tune36: live_book 被 zombie 污染（如 QAIT ask 端累积 20+ 个 stale 低价单，
            // snapshot 取最低 20 档全是 zombie，真实档被挤出）→ 过滤后整端空。
            // 此时**改用 REST**（更干净，top-20 通常含真实档），而不是返回 live_book raw zombie。
            tracing::warn!(
                symbol, filt_bids = filtered.bids.len(), filt_asks = filtered.asks.len(),
                "live_book filtered empty (zombie-polluted) → refetch REST"
            );
        }
    }
    // 2) REST fallback（live_book 不 fresh / 别的 symbol / 被 zombie 污染）
    let rest = state.alpha.get_full_depth(symbol, 20).await?;
    let filtered = anchor_filter(state, symbol, &rest);
    if filtered.bids.is_empty() || filtered.asks.is_empty() {
        // REST 都过滤空 → 盘口真的全是 zombie（极端情况），返回 raw 让 price-sanity 兜底拒单
        tracing::warn!(
            symbol, raw_bids = rest.bids.len(), raw_asks = rest.asks.len(),
            filt_bids = filtered.bids.len(), filt_asks = filtered.asks.len(),
            "REST also filtered empty → return raw (price-sanity will refuse if crossed)"
        );
        return Ok(rest);
    }
    Ok(filtered)
}

/// V2.tune15/34/36：用 anchor 价对称过滤盘口 stale 残留。
/// anchor 优先 last_trade（aggTrade，最精确），离 orderbook 中位价 >5% 时改用中位价
/// （波动币 last_trade 过时；中位价抗 zombie outlier）。
fn anchor_filter(
    state: &AppState,
    symbol: &str,
    book: &binance_alpha::OrderBookSnapshot,
) -> binance_alpha::OrderBookSnapshot {
    let last_trade = state.recent_trades.last_price_for(symbol);
    let ob_median = orderbook_median_price(book);
    let anchor = match (last_trade, ob_median) {
        (Some(lt), Some(med)) if med > Decimal::ZERO => {
            let dev = ((lt - med) / med).abs();
            if dev <= Decimal::from_str("0.05").unwrap() { Some(lt) } else { Some(med) }
        }
        (Some(lt), _) => Some(lt),
        (None, Some(med)) => Some(med),
        (None, None) => None,
    };
    filter_stale_levels(book.clone(), anchor)
}

/// V2.tune15: 对称过滤双边 stale 残留。
///
/// B2 实测：alpha REST 返回的盘口含大量长期不变的"僵尸"挂单（bid=0.68/ask=0.40 实际中价 0.66），
/// 这些价位永远不会触发 @depth 流的 remove event，所以 incremental orderbook 也清不掉。
/// 必须在客户端用 anchor 价对称过滤。
///
/// 规则：以 anchor (最近一笔成交价) 为中心，过滤掉偏离 > 5% 的 bid/ask。
/// - bid > anchor × 1.05 → 拒（不可能合法的 bid 比成交价高 5%）
/// - ask < anchor × 0.95 → 拒（不可能合法的 ask 比成交价低 5%）
///
/// V2.tune34: orderbook 自身所有档位价格的中位数。zombie 单是极少数 outlier，
/// 中位数天然抗它 → 当 last_trade（aggTrade）过时时用它重新锚定真实价。
fn orderbook_median_price(book: &binance_alpha::OrderBookSnapshot) -> Option<Decimal> {
    let mut prices: Vec<Decimal> = book
        .bids
        .iter()
        .chain(book.asks.iter())
        .filter_map(|lvl| Decimal::from_str(&lvl[0]).ok())
        .collect();
    if prices.is_empty() {
        return None;
    }
    prices.sort();
    Some(prices[prices.len() / 2])
}

/// 若 anchor 不存在（recent_trades 还空），退化为 V2.tune13 的 "ask < best_bid × 0.95 → 拒"。
fn filter_stale_levels(
    mut book: binance_alpha::OrderBookSnapshot,
    anchor: Option<Decimal>,
) -> binance_alpha::OrderBookSnapshot {
    let lo_factor = Decimal::from_str("0.95").unwrap_or(Decimal::ONE);
    let hi_factor = Decimal::from_str("1.05").unwrap_or(Decimal::ONE);

    if let Some(a) = anchor {
        let lo = a * lo_factor;
        let hi = a * hi_factor;
        let orig_b = book.bids.len();
        let orig_a = book.asks.len();
        book.bids.retain(|b| {
            Decimal::from_str(&b[0]).map(|p| p <= hi).unwrap_or(false)
        });
        book.asks.retain(|a| {
            Decimal::from_str(&a[0]).map(|p| p >= lo).unwrap_or(false)
        });
        let drop_b = orig_b - book.bids.len();
        let drop_a = orig_a - book.asks.len();
        if drop_b > 0 || drop_a > 0 {
            tracing::info!(
                anchor = %a, lo = %lo, hi = %hi, drop_b, drop_a,
                "filter_stale_levels: dropped {} bid(s) {} ask(s) (>5% from anchor)",
                drop_b, drop_a
            );
        }
        // 二次清洗：anchor 滤过后如果仍然 crossed（best_bid > best_ask），
        // 说明 bid/ask 两侧仍有 "近距离 zombie"（如 B2 测试：bid 0.68 / ask 0.646）。
        // 用 anchor 当真实中价的判断：离 anchor 更远的那一侧更可能是 zombie，drop。
        loop {
            let bb = book.bids.first().and_then(|b| Decimal::from_str(&b[0]).ok());
            let aa = book.asks.first().and_then(|a| Decimal::from_str(&a[0]).ok());
            match (bb, aa) {
                (Some(b_p), Some(a_p)) if b_p > a_p => {
                    // 离 anchor 远的一侧 = zombie
                    let bid_dist = if b_p > a_p { b_p - a } else { a - b_p };
                    let ask_dist = if a_p > a { a_p - a } else { a - a_p };
                    if bid_dist > ask_dist {
                        let drop_v = book.bids.remove(0);
                        tracing::info!(anchor = %a, dropped = ?drop_v, "post-filter cross: dropped crossing bid");
                    } else {
                        let drop_v = book.asks.remove(0);
                        tracing::info!(anchor = %a, dropped = ?drop_v, "post-filter cross: dropped crossing ask");
                    }
                }
                _ => break,
            }
            // 安全保护：万一逻辑写错防死循环
            if book.bids.is_empty() || book.asks.is_empty() {
                break;
            }
        }
    } else {
        // anchor 不存在 → 兼容老逻辑（仅 ask < best_bid × 0.95 → 拒）
        let best_bid = book.bids.first().and_then(|b| Decimal::from_str(&b[0]).ok());
        if let Some(bid_price) = best_bid {
            let threshold = bid_price * lo_factor;
            let orig_len = book.asks.len();
            book.asks.retain(|a| {
                Decimal::from_str(&a[0]).map(|p| p >= threshold).unwrap_or(false)
            });
            let dropped = orig_len - book.asks.len();
            if dropped > 0 {
                tracing::info!(
                    %bid_price, %threshold, dropped,
                    "filter_stale_levels (no anchor): dropped {} stale ask(s)",
                    dropped
                );
            }
        }
    }
    book
}

/// 旧别名（兼容 — 仅给单测）：等价于 filter_stale_levels(book, None)。
#[cfg(test)]
fn filter_stale_asks(book: binance_alpha::OrderBookSnapshot) -> binance_alpha::OrderBookSnapshot {
    filter_stale_levels(book, None)
}

/// V2.fix3：从 round_result 的 anyhow::Error 里 downcast 出 AlphaApiError，
/// 判断是不是 binance-alpha::twofa 标的"需要人脸/手机额外验证"（code=2fa-extra-verify-required）。
fn is_extra_verify_required_error(e: &anyhow::Error) -> bool {
    e.downcast_ref::<binance_alpha::AlphaApiError>()
        .map(|api| {
            matches!(
                api,
                binance_alpha::AlphaApiError::Server { code, .. }
                    if code == "2fa-extra-verify-required"
            )
        })
        .unwrap_or(false)
}

/// V2.7：检查 wear 阈值 / 连续 working timeout，命中则 set state=paused。
/// 返回 true = 已 pause，调用方应在下一轮 loop 顶上的 state 检查里被 cleanup + 退出。
async fn check_risk_and_pause(
    state: &AppState,
    job: &jobs::JobRow,
    job_id: &str,
    round: u32,
    auth: &AuthBundle,
    wear_consec_hits: &mut u32,
) -> bool {
    // ---- 风控 #1：wear 阈值（V2.tune22：要求连续 N 次触发才 pause，避免 OTO 时序假报）
    if round % WEAR_CHECK_EVERY_N_ROUNDS == 0 {
        let hit = check_wear_pause(state, job, job_id, round, auth).await;
        if hit {
            *wear_consec_hits += 1;
            if *wear_consec_hits >= WEAR_PAUSE_CONSECUTIVE_HITS {
                warn!(
                    %job_id, hits = *wear_consec_hits, required = WEAR_PAUSE_CONSECUTIVE_HITS,
                    "RISK: wear hit threshold {} consecutive times → auto pause",
                    *wear_consec_hits,
                );
                let _ = jobs::set_state(&state.db, job_id, jobs::JobState::Paused).await;
                return true;
            }
            warn!(
                %job_id, hits = *wear_consec_hits, required = WEAR_PAUSE_CONSECUTIVE_HITS,
                "wear hit (1/2) — likely OTO timing gap, waiting next check before pause",
            );
        } else {
            // 健康 → 计数器清零
            *wear_consec_hits = 0;
        }
    }

    // ---- 风控 #2：连续 working timeout
    // V2.tune10 修：风控只看"最近 60 秒内"的 round，避免 resume 后立刻被
    // pause 前的旧 timeout 数据触发（导致 resume 30s 就再次 pause 的死循环）。
    if let Ok(recent) = rounds::list_last_n(
        &state.db,
        job_id,
        WORKING_TIMEOUT_CONSEC_THRESHOLD as i64,
    )
    .await
    {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let stale_cutoff_ms = now_ms - 60_000; // 60 秒前的数据不算
        let fresh_recent: Vec<_> = recent
            .iter()
            .filter(|r| r.started_ms >= stale_cutoff_ms)
            .collect();
        if fresh_recent.len() >= WORKING_TIMEOUT_CONSEC_THRESHOLD
            && fresh_recent
                .iter()
                .all(|r| r.status == "skipped" && r.decision_type.ends_with("_working_timeout"))
        {
            // V2.tune11: working timeout 不再 pause（用户反馈 pause 打断刷量太烦）。
            // 改成 sleep 30s 等市场恢复，自动续刷。
            // 真巨亏（wear ≤ -50 bps）的风控 #1 保留 pause，那个才需要人介入。
            warn!(
                %job_id, n = WORKING_TIMEOUT_CONSEC_THRESHOLD,
                "RISK: 5 consecutive working timeouts → sleep 30s wait market recover (no pause)"
            );
            tokio::time::sleep(Duration::from_secs(30)).await;
            return false;
        }
    }

    false
}

/// V2.fix4：wear 阈值检查（替换旧的 inline 实现）。
///
/// 返回 true = wear 命中阈值，已 set state=paused，调用方应 return。
/// 返回 false = 不触发（包括所有上游数据获取失败的 silent skip 情况，
/// 但每次都打 info 日志，方便排查）。
///
/// 关键修复：total_value = SPOT.USDT + base_asset.valuation，避免 USDT 到账延迟误报。
async fn check_wear_pause(
    state: &AppState,
    job: &jobs::JobRow,
    job_id: &str,
    round: u32,
    auth: &AuthBundle,
) -> bool {
    let baseline = match stats_repo::parse_baseline(&job.params_json) {
        Some(b) => b,
        None => {
            warn!(%job_id, round, "wear check skipped: no baseline in params_json");
            return false;
        }
    };

    // 并行拉 SPOT + alpha wallet
    let (spot_res, alpha_res) = tokio::join!(
        state.alpha.get_spot_wallet(auth),
        state.alpha.get_alpha_wallet(auth),
    );

    let current_spot = match spot_res {
        Ok(w) => match usdt_funding_free(&w) {
            Some(v) => v,
            None => {
                warn!(%job_id, round, "wear check skipped: SPOT USDT.funding missing");
                return false;
            }
        },
        Err(e) => {
            warn!(%job_id, round, err = %e, "wear check skipped: get_spot_wallet failed");
            return false;
        }
    };

    // base_asset valuation: 从 alpha wallet 用 token_id (= alpha_id) 精确匹配。
    // V2.tune28: 之前用 _token_symbol 匹配 e.symbol → 同 symbol 多币（SLX 有两个）时找错。
    // params_json 里的 _base_asset 是 alpha_id（如 "ALPHA_978"），用它直接和 wallet.token_id 比。
    let base_asset_for_wear: String = serde_json::from_str::<serde_json::Value>(&job.params_json)
        .ok()
        .and_then(|v| v.get("_base_asset").and_then(|x| x.as_str()).map(String::from))
        .unwrap_or_default();
    // V2.tune17/21/31: 真在途库存 = max(钱包 free+freeze, DB 累计 buy-sell qty).
    // 币安自动把闲置 alpha token 转 earn (free=0 但 amount 在) → 旧 free+freeze 漏 earn 部分。
    // 用 DB net_qty 当 ground truth (本 job 累计买入未卖)，避免 wear 风控假触发。
    let per_token_price = match &alpha_res {
        Ok(w) => w
            .list
            .iter()
            .find(|e| e.token_id == base_asset_for_wear)
            .and_then(|e| {
                if e.amount > Decimal::ZERO {
                    e.valuation.map(|v| v / e.amount)
                } else { None }
            })
            .unwrap_or(Decimal::ZERO),
        Err(e) => {
            warn!(%job_id, round, err = %e, "wear check: get_alpha_wallet failed, treating base valuation as 0");
            Decimal::ZERO
        }
    };
    let wallet_visible = match &alpha_res {
        Ok(w) => w.list.iter()
            .find(|e| e.token_id == base_asset_for_wear)
            .map(|e| e.free + e.freeze)
            .unwrap_or(Decimal::ZERO),
        Err(_) => Decimal::ZERO,
    };
    let net_qty_db = trades::net_base_qty(&state.db, job_id).await.unwrap_or(Decimal::ZERO);
    let effective_qty = if net_qty_db > wallet_visible { net_qty_db } else { wallet_visible };
    let base_valuation = effective_qty * per_token_price;

    let vol = trades::sum_buy_quote_qty(&state.db, job_id)
        .await
        .unwrap_or_default();
    let min_vol = Decimal::from_str(WEAR_CHECK_MIN_VOL_USDT).unwrap();
    if vol < min_vol {
        info!(
            %job_id, round, %vol, %min_vol,
            "wear check skipped: vol below 100 USDT threshold (bps too noisy)"
        );
        return false;
    }

    let total_value = current_spot + base_valuation;
    let wear = total_value - baseline;
    let bps: i64 = ((wear / vol) * Decimal::from(10000))
        .floor()
        .try_into()
        .unwrap_or(0);

    info!(
        %job_id, round, %current_spot, %base_valuation, %total_value,
        %baseline, %vol, %wear, %bps,
        "wear check"
    );

    // V2.tune22: 不在这里直接 pause —— 由 caller (check_risk_and_pause) 累计连续触发次数后才 pause
    bps <= WEAR_PAUSE_BPS_THRESHOLD
}

#[allow(clippy::too_many_arguments)]
async fn run_round(
    state: &AppState,
    auth: &AuthBundle,
    job_id: &str,
    symbol: &str,
    base_asset: &str,
    single_target: Decimal,
    tick: Decimal,
    step: Decimal,
    round: u32,
) -> anyhow::Result<()> {
    // 1) 拿盘口
    let book = get_orderbook_smart(state, symbol).await?;
    if book.bids.is_empty() || book.asks.is_empty() {
        anyhow::bail!("empty book");
    }
    let best_bid = Decimal::from_str(&book.bids[0][0])?;
    let best_ask = Decimal::from_str(&book.asks[0][0])?;

    // 2) BUY: 略高 best_ask 立即成交
    let buy_price = best_ask + tick * Decimal::from(PRICE_BUMP_TICKS);
    let buy_qty = round_step(single_target / buy_price, step);
    if buy_qty.is_zero() {
        anyhow::bail!("buy_qty rounds to 0");
    }
    let buy_payment = (buy_qty * buy_price).round_dp_with_strategy(8, RoundingStrategy::ToZero);

    let buy_req = PlaceOrderRequest {
        base_asset: base_asset.into(),
        quote_asset: "USDT".into(),
        side: Side::Buy,
        price: buy_price,
        quantity: buy_qty,
        payment_details: vec![PaymentDetail {
            amount: buy_payment,
            payment_wallet_type: WalletType::Card,
        }],
        order_type: OrderType::Limit,
    };
    info!(%job_id, round, %buy_price, %buy_qty, %buy_payment, "round BUY");
    let buy_oid = state.alpha.place_order(auth, &buy_req).await?;

    // 记 orders 表
    let _ = orders::insert(
        &state.db,
        &orders::NewOrder {
            order_id: buy_oid.clone(),
            job_id: job_id.into(),
            side: "BUY".into(),
            price: buy_price,
            qty: buy_qty,
            status: "pending".into(),
            raw_response: Some(format!("round {round} BUY")),
        },
    )
    .await;

    // 等成交（拉 user-trades 看 lastTrade）
    let fills_buy = wait_fills(state, auth, &buy_oid, symbol).await;
    persist_fills(state, &fills_buy, Some(job_id), &auth.username).await;
    let _ = orders::set_status(&state.db, &buy_oid, "filled").await;

    if fills_buy.is_empty() {
        // 没成交 → 撤掉避免堆积
        let _ = state
            .alpha
            .cancel_order(
                auth,
                &CancelOrderRequest {
                    order_id: buy_oid.clone(),
                    symbol: symbol.into(),
                },
            )
            .await;
        anyhow::bail!("BUY no fill within {FILL_WAIT_TIMEOUT_S}s");
    }
    let filled_qty: Decimal = fills_buy.iter().map(|f| f.qty).sum();
    info!(%job_id, round, buy_filled_qty = %filled_qty, "BUY filled");

    tokio::time::sleep(Duration::from_millis(SLEEP_BETWEEN_FILLS_MS)).await;

    // 3) SELL: 卖光 NEX free（包括账户原有库存 + 本轮买的）
    //    业务规则：刷量结束不能留刷的币（波动大会浮亏）
    //    所以每轮都清仓
    let sell_qty_raw = current_base_free(state, auth, base_asset).await?;
    let sell_qty = round_step(sell_qty_raw, step);
    if sell_qty.is_zero() {
        anyhow::bail!("sell_qty rounds to 0; base_free={sell_qty_raw}");
    }
    let book2 = get_orderbook_smart(state, symbol).await?;
    let best_bid2 = Decimal::from_str(&book2.bids[0][0]).unwrap_or(best_bid);
    let sell_price = best_bid2 - tick * Decimal::from(PRICE_BUMP_TICKS);

    let sell_req = PlaceOrderRequest {
        base_asset: base_asset.into(),
        quote_asset: "USDT".into(),
        side: Side::Sell,
        price: sell_price,
        quantity: sell_qty,
        payment_details: vec![PaymentDetail {
            amount: sell_qty,
            payment_wallet_type: WalletType::Alpha,
        }],
        order_type: OrderType::Limit,
    };
    info!(%job_id, round, %sell_price, %sell_qty, "round SELL");
    let sell_oid = state.alpha.place_order(auth, &sell_req).await?;
    let _ = orders::insert(
        &state.db,
        &orders::NewOrder {
            order_id: sell_oid.clone(),
            job_id: job_id.into(),
            side: "SELL".into(),
            price: sell_price,
            qty: sell_qty,
            status: "pending".into(),
            raw_response: Some(format!("round {round} SELL")),
        },
    )
    .await;

    let fills_sell = wait_fills(state, auth, &sell_oid, symbol).await;
    persist_fills(state, &fills_sell, Some(job_id), &auth.username).await;
    let _ = orders::set_status(&state.db, &sell_oid, "filled").await;

    if fills_sell.is_empty() {
        let _ = state
            .alpha
            .cancel_order(
                auth,
                &CancelOrderRequest {
                    order_id: sell_oid.clone(),
                    symbol: symbol.into(),
                },
            )
            .await;
        anyhow::bail!("SELL no fill within {FILL_WAIT_TIMEOUT_S}s (NEX 留持仓，下轮自动卖)");
    }
    info!(%job_id, round, "round done");
    Ok(())
}

/// 等订单 user-trades 出来。
/// taker 单基本第一次 poll 就能拿到 fill。
/// 拿到**任何** fill 立即返回（不再等 last_trade=true，因为 OTO 模式经常缺这个字段且 taker 单本就是 atomic 一次成交）。
async fn wait_fills(
    state: &AppState,
    auth: &AuthBundle,
    order_id: &str,
    symbol: &str,
) -> Vec<binance_alpha::TradeFill> {
    // V2.tune20: 等到 lastTrade=true 或 1.2s sub-fill 兜底窗口，避免漏 sub-fill
    wait_fills_with_timeout(state, auth, order_id, symbol, FILL_WAIT_TIMEOUT_S).await
}

async fn persist_fills(
    state: &AppState,
    fills: &[binance_alpha::TradeFill],
    job_id: Option<&str>,
    username: &str,
) {
    for f in fills {
        let side_str = match f.side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        let raw_json = serde_json::to_string(f).ok();
        let new = trades::NewTrade {
            fill_id: f.id.clone(),
            order_id: f.order_id.clone(),
            job_id: job_id.map(|s| s.to_string()),
            username: username.into(),
            symbol: f.symbol.clone(),
            side: side_str.into(),
            price: f.price,
            qty: f.qty,
            quote_qty: f.quote_qty,
            commission: f.commission,
            commission_asset: f.commission_asset.clone(),
            trade_ts_ms: f.time,
            raw_json,
        };
        if let Err(e) = trades::insert(&state.db, &new).await {
            warn!(err = %e, "persist fill failed");
        }
    }
}

/// OTO 一轮：一次性挂 BUY (taker) + 关联 SELL (pending → 触发后 taker)
/// working=best_ask+1tick, pending=best_bid-1tick
/// 两单几乎一秒内全成交。
#[allow(clippy::too_many_arguments)]
async fn run_oto_round(
    state: &AppState,
    auth: &AuthBundle,
    job_id: &str,
    symbol: &str,
    base_asset: &str,
    single_target: Decimal,
    tick: Decimal,
    step: Decimal,
    round: u32,
) -> anyhow::Result<()> {
    // 1) 拿盘口
    let book = get_orderbook_smart(state, symbol).await?;
    if book.bids.is_empty() || book.asks.is_empty() {
        anyhow::bail!("empty book");
    }
    let best_bid = Decimal::from_str(&book.bids[0][0])?;
    let best_ask = Decimal::from_str(&book.asks[0][0])?;
    // bid == ask（spread=0）完全正常，只在 bid > ask（crossed book）才 bail
    if best_ask < best_bid {
        anyhow::bail!("crossed book: bid={best_bid} ask={best_ask}");
    }

    // 2) 算价格 + 数量
    let working_price = best_ask + tick * Decimal::from(PRICE_BUMP_TICKS);
    let pending_price = best_bid - tick * Decimal::from(PRICE_BUMP_TICKS);
    if pending_price <= Decimal::ZERO {
        anyhow::bail!("pending_price {pending_price} <= 0; bid too low");
    }
    let working_qty = round_step(single_target / working_price, step);
    if working_qty.is_zero() {
        anyhow::bail!("working_qty rounds to 0");
    }
    let working_payment =
        (working_qty * working_price).round_dp_with_strategy(8, rust_decimal::RoundingStrategy::ToZero);

    let req = PlaceOtoOrderRequest {
        base_asset: base_asset.into(),
        quote_asset: "USDT".into(),
        working_side: Side::Buy,
        working_price,
        working_quantity: working_qty,
        payment_details: vec![PaymentDetail {
            amount: working_payment,
            payment_wallet_type: WalletType::Card,
        }],
        pending_price,
        pending_type: OrderType::Limit,
    };
    info!(
        %job_id, round, %working_price, %working_qty, %working_payment, %pending_price,
        "round OTO"
    );

    // 3) 下 OTO 单
    let oto = state.alpha.place_oto_order(auth, &req).await?;
    let working_oid = oto.working_order_id.to_string();
    let pending_oid = oto.pending_order_id.to_string();
    info!(%job_id, round, %working_oid, %pending_oid, "OTO placed");

    // 入 orders 表
    let _ = persistence::repo::orders::insert(
        &state.db,
        &persistence::repo::orders::NewOrder {
            order_id: working_oid.clone(),
            job_id: job_id.into(),
            side: "BUY".into(),
            price: working_price,
            qty: working_qty,
            status: "pending".into(),
            raw_response: Some(format!("oto round {round} working")),
        },
    )
    .await;
    let _ = persistence::repo::orders::insert(
        &state.db,
        &persistence::repo::orders::NewOrder {
            order_id: pending_oid.clone(),
            job_id: job_id.into(),
            side: "SELL".into(),
            price: pending_price,
            qty: working_qty, // pending qty 实际 == working filled，下面成交后会修正
            status: "pending".into(),
            raw_response: Some(format!("oto round {round} pending")),
        },
    )
    .await;

    // 4) 等两单都成交（working 通常 1s 内，pending 需要触发后 1s）
    let fills_working = wait_fills(state, auth, &working_oid, symbol).await;
    persist_fills(state, &fills_working, Some(job_id), &auth.username).await;
    let _ = persistence::repo::orders::set_status(&state.db, &working_oid, "filled").await;

    if fills_working.is_empty() {
        // working 没成交 → 撤掉两单（pending 会跟着失效）
        let _ = state
            .alpha
            .cancel_order(
                auth,
                &CancelOrderRequest {
                    order_id: working_oid.clone(),
                    symbol: symbol.into(),
                },
            )
            .await;
        anyhow::bail!("OTO working no fill within {FILL_WAIT_TIMEOUT_S}s");
    }

    // pending 触发后稍微等一下（OTO 模式短一些）
    tokio::time::sleep(Duration::from_millis(500)).await;
    let fills_pending = wait_fills(state, auth, &pending_oid, symbol).await;
    persist_fills(state, &fills_pending, Some(job_id), &auth.username).await;
    let _ = persistence::repo::orders::set_status(&state.db, &pending_oid, "filled").await;

    if fills_pending.is_empty() {
        // pending 没成交 → 撤掉，并**立即清仓**避免持仓累积
        let _ = state
            .alpha
            .cancel_order(
                auth,
                &CancelOrderRequest {
                    order_id: pending_oid.clone(),
                    symbol: symbol.into(),
                },
            )
            .await;
        // V2.tune26: 同 oto_smart 修复，DB status 也要更新成 canceled
        let _ = persistence::repo::orders::set_status(&state.db, &pending_oid, "canceled").await;
        warn!(%job_id, round, %pending_oid, "OTO pending no fill → emergency_sell_all");
        emergency_sell_all(state, auth, job_id, symbol, base_asset, tick, step, round).await;
        return Ok(()); // 不算失败，继续下一轮
    }
    info!(%job_id, round, "OTO round done");

    // 即使 pending 成交了，也 sweep 一次防止 step 截断/部分成交导致的微残留累积
    // （只在残留 > 多个 step 时才下单，避免每轮都浪费 API）
    let residual = current_base_free(state, auth, base_asset).await.unwrap_or(Decimal::ZERO);
    if residual >= step * Decimal::from(3) {
        warn!(%job_id, round, %residual, "residual after OTO ≥ 3 steps → sweep");
        emergency_sell_all(state, auth, job_id, symbol, base_asset, tick, step, round).await;
    }
    Ok(())
}

/// V2.tune4: 动态降价清仓（仿旧 trading_agent.py L5295-5306）
///
/// 阶梯：万一 → 万二 → 万三 → 万四 → 万五 → 1‰ → 0.3% → 0.5% → 1%
/// 每级 N 次尝试，挂限价 sell 单等 2 秒，没全成交 → cancel + 升级到下一档。
///
/// 核心思路：先用最小亏损（万一 = 1 bps）试，95% 的卖单能在 1-3 bps 完成；
/// 只有极端低流动性才会跌到 0.3%-1% 大幅亏损。预期 timeout loss 从 3.5 bps → 0.5-1 bps。
const SELL_PRICE_LEVELS: &[(&str, u32, &str)] = &[
    // V2.tune8: 顶部插入 0 档 — 用 best_bid 价不降，先试一次能否秒成交（0 loss）。
    // 实测 NEX best_bid 档量经常 > 我们要卖的量，0 档大概率成功，省下万一的 1 bps loss。
    ("0", 1, "0档"),
    ("0.0001", 5, "万一"),
    ("0.0002", 3, "万二"),
    ("0.0003", 2, "万三"),
    ("0.0004", 2, "万四"),
    ("0.0005", 1, "万五"),
    ("0.0010", 1, "1‰"),
    ("0.003", 3, "0.3%"),
    ("0.005", 2, "0.5%"),
    ("0.010", 1, "1%"),
];

/// 把 price floor 到 tick 倍数。
/// 关键：动态降价计算 best_bid × (1-offset) 后小数位会超 tick 精度，
/// 必须 floor 回 tick 倍数才能让币安接受（否则 decode error）。
fn round_price_to_tick(price: Decimal, tick: Decimal) -> Decimal {
    if tick.is_zero() {
        return price;
    }
    (price / tick).floor() * tick
}

/// pending 没成交 或 中间发现残留时，阶梯降价清仓账户全部 base 持仓。
/// 失败不抛错（兜底机制，避免阻塞主循环）。
#[allow(clippy::too_many_arguments)]
async fn emergency_sell_all(
    state: &AppState,
    auth: &AuthBundle,
    job_id: &str,
    symbol: &str,
    base_asset: &str,
    tick: Decimal,
    step: Decimal,
    round: u32,
) {
    let total_qty_raw = match current_base_free(state, auth, base_asset).await {
        Ok(q) => q,
        Err(e) => {
            warn!(%job_id, err = %e, "emergency_sell: read base failed");
            return;
        }
    };
    let mut remaining = round_step(total_qty_raw, step);
    if remaining.is_zero() {
        return;
    }

    let min_notional = Decimal::from_str("0.1").unwrap();

    for (offset_str, attempts, level_name) in SELL_PRICE_LEVELS {
        let offset = match Decimal::from_str(offset_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for attempt in 1..=*attempts {
            // 每次重新拉 best_bid（市场动）+ 重读 wallet 余额
            let book = match get_orderbook_smart(state, symbol).await {
                Ok(b) => b,
                Err(e) => {
                    warn!(%job_id, err = %e, "dynamic_sell: get_book failed");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            let best_bid = match book
                .bids
                .first()
                .and_then(|b| Decimal::from_str(&b[0]).ok())
            {
                Some(v) => v,
                None => continue,
            };

            // current_price * (1 - offset) — 用 best_bid 当 reference
            // ⚠️ 关键：必须 floor 到 tick 倍数，否则 0.000004448114 这种 12 位精度
            // 会被币安拒绝（decode error）。V2.tune4 第一次实测就是因为没 round 全军覆没。
            let sell_price_raw = best_bid * (Decimal::ONE - offset);
            let sell_price = round_price_to_tick(sell_price_raw, tick);

            // 重新读 wallet 拿真实剩余 qty（防止上一轮部分成交了）
            let cur_qty = current_base_free(state, auth, base_asset)
                .await
                .unwrap_or(remaining);
            remaining = round_step(cur_qty, step);
            if remaining.is_zero() {
                info!(%job_id, %level_name, "dynamic_sell: balance cleared");
                return;
            }

            let notional = remaining * sell_price;
            if notional < min_notional {
                info!(
                    %job_id, qty = %remaining, %sell_price, %notional,
                    "dynamic_sell: remaining below minNotional 0.1 USDT, accept dust"
                );
                return;
            }

            let req = PlaceOrderRequest {
                base_asset: base_asset.into(),
                quote_asset: "USDT".into(),
                side: Side::Sell,
                price: sell_price,
                quantity: remaining,
                payment_details: vec![PaymentDetail {
                    amount: remaining,
                    payment_wallet_type: WalletType::Alpha,
                }],
                order_type: OrderType::Limit,
            };
            warn!(
                %job_id, round, %level_name, attempt, attempts = %attempts,
                qty = %remaining, %sell_price,
                "dynamic_sell: placing"
            );
            let oid = match state.alpha.place_order(auth, &req).await {
                Ok(o) => o,
                Err(e) => {
                    warn!(%job_id, %level_name, err = %e, "dynamic_sell: place_order failed");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            let _ = persistence::repo::orders::insert(
                &state.db,
                &persistence::repo::orders::NewOrder {
                    order_id: oid.clone(),
                    job_id: job_id.into(),
                    side: "SELL".into(),
                    price: sell_price,
                    qty: remaining,
                    status: "pending".into(),
                    raw_response: Some(format!(
                        "dynamic_sell round {round} {level_name} attempt {attempt}/{attempts}"
                    )),
                },
            )
            .await;

            // 等 2 秒看成交（旧代码风格）
            tokio::time::sleep(Duration::from_secs(2)).await;
            let fills = state
                .alpha
                .get_user_trades(auth, &oid, symbol)
                .await
                .unwrap_or_default();
            persist_fills(state, &fills, Some(job_id), &auth.username).await;

            // 检查 wallet 余额，看是否还有剩
            let after_qty = current_base_free(state, auth, base_asset)
                .await
                .unwrap_or(Decimal::ZERO);
            let after_step = round_step(after_qty, step);

            // 取消未成交的部分（如果有的话）
            let _ = state
                .alpha
                .cancel_order(
                    auth,
                    &CancelOrderRequest {
                        order_id: oid.clone(),
                        symbol: symbol.into(),
                    },
                )
                .await;
            let _ = persistence::repo::orders::set_status(&state.db, &oid, "filled").await;

            info!(
                %job_id, %level_name, attempt,
                filled_count = fills.len(), remaining_after = %after_step,
                "dynamic_sell: attempt result"
            );

            if after_step.is_zero() || (after_step * sell_price) < min_notional {
                info!(%job_id, %level_name, attempt, "dynamic_sell: cleared (or dust)");
                return;
            }
            remaining = after_step;
        }
    }

    warn!(
        %job_id, remaining = %remaining,
        "dynamic_sell: all 9 levels exhausted; residual remains"
    );
}

fn round_step(qty: Decimal, step: Decimal) -> Decimal {
    if step.is_zero() {
        return qty;
    }
    (qty / step).floor() * step
}

/// 拿当前 base 代币在 Alpha wallet 里的 free 余额（找不到返回 0）。
/// Alpha wallet 用 token symbol（"NEX" / "BILL"），而 base_asset 是 alpha_id（"ALPHA_971"/"ALPHA_953"）。
///
/// V2.tune3 修复：通过 TokenRegistry 把 alpha_id 翻译成 symbol，不再硬编码。
async fn current_base_free(
    state: &AppState,
    auth: &AuthBundle,
    base_asset: &str,
) -> anyhow::Result<Decimal> {
    let wallet = state.alpha.get_alpha_wallet(auth).await?;
    // V2.tune28: 用 token_id (= alpha_id) 精确匹配，不用 symbol。
    // 之前用 symbol → 钱包里同 symbol 两个币时（如 SLX = SLIMEX/Solstice），
    // iter().find() 拿第一个 wrong 的 → final_cleanup 看到 dust 就退，真正库存漏卖。
    let qty = wallet
        .list
        .iter()
        .find(|e| e.token_id == base_asset)
        .map(|e| e.free)
        .unwrap_or(Decimal::ZERO);
    Ok(qty)
}

/// 退出 loop 前的清仓。如果还持有 base 代币，挂市价卖单清光。
/// 不抛错 — 即使清不掉也要让 state 转 done/stopped，避免 job 卡在 running。
async fn final_cleanup(state: &AppState, auth: &AuthBundle, job_id: &str, symbol: &str, base_asset: &str) {
    // V2.tune3: tick/step 也要从 TokenRegistry 拿（之前 final_cleanup 用硬编码 NEX tick=1e-9
    // 对 BILL 这种 tick=1e-8 的币会让 sell_price 精度超过币安接受范围 → decode error）。
    let token = state.tokens.find_by_pair(symbol);
    let step = token
        .as_ref()
        .and_then(|t| t.step_size)
        .unwrap_or_else(|| Decimal::from_str(STEP_SIZE_STR).unwrap());
    let tick = token
        .as_ref()
        .and_then(|t| t.tick_size)
        .unwrap_or_else(|| Decimal::from_str(TICK_SIZE_STR).unwrap());
    let free = match current_base_free(state, auth, base_asset).await {
        Ok(q) => q,
        Err(e) => {
            warn!(%job_id, err = %e, "cleanup: read wallet failed");
            return;
        }
    };
    let sell_qty = round_step(free, step);
    if sell_qty.is_zero() {
        info!(%job_id, %free, "cleanup: nothing to sell");
        return;
    }
    let book = match get_orderbook_smart(state, symbol).await {
        Ok(b) => b,
        Err(e) => {
            warn!(%job_id, err = %e, "cleanup: get_full_depth failed; leaving residual");
            return;
        }
    };
    let best_bid = Decimal::from_str(&book.bids[0][0]).unwrap_or(Decimal::ZERO);
    let sell_price = best_bid - tick * Decimal::from(PRICE_BUMP_TICKS);
    // minNotional 保护：太小卖不出去，接受 dust
    let notional = sell_qty * sell_price;
    let min_notional = Decimal::from_str("0.1").unwrap();
    if notional < min_notional {
        info!(%job_id, %sell_qty, %sell_price, %notional, "cleanup: residual < minNotional 0.1, leave as dust");
        return;
    }
    let sell_req = PlaceOrderRequest {
        base_asset: base_asset.into(),
        quote_asset: "USDT".into(),
        side: Side::Sell,
        price: sell_price,
        quantity: sell_qty,
        payment_details: vec![PaymentDetail {
            amount: sell_qty,
            payment_wallet_type: WalletType::Alpha,
        }],
        order_type: OrderType::Limit,
    };
    warn!(%job_id, %sell_qty, %sell_price, "cleanup: clearing residual base position");
    match state.alpha.place_order(auth, &sell_req).await {
        Ok(oid) => {
            let _ = orders::insert(
                &state.db,
                &orders::NewOrder {
                    order_id: oid.clone(),
                    job_id: job_id.into(),
                    side: "SELL".into(),
                    price: sell_price,
                    qty: sell_qty,
                    status: "pending".into(),
                    raw_response: Some("final cleanup".into()),
                },
            )
            .await;
            // 不等 fills 入库（避免 cleanup 卡住），后续 monitor 可以反推
            tokio::time::sleep(Duration::from_secs(2)).await;
            let fills = state
                .alpha
                .get_user_trades(auth, &oid, symbol)
                .await
                .unwrap_or_default();
            persist_fills(state, &fills, Some(job_id), &auth.username).await;
            let _ = orders::set_status(&state.db, &oid, "filled").await;
            info!(%job_id, fills = fills.len(), "cleanup done");
        }
        Err(e) => warn!(%job_id, err = %e, "cleanup: place_order failed"),
    }
}

// =============================================================================
// V2 smart OTO round — 决策矩阵 + maker/taker 混合 + rounds 表入库
// =============================================================================

/// pending/working maker 等待时间。
/// V2.tune2 实测把 5→8s + fast 改 maker 后 timeout 飙到 67%，wear -31 bps 触发风控 pause。
/// V2.tune35：5→3s。低流动性币（BILL/SLX）maker 大概率挂不上，实测 BILL double_maker
/// 失败率 77%、taker_maker_hybrid 71%，每个 timeout 浪费 5-9s。缩到 3s 让失败 round 快速
/// fallback emergency_sell，整体提速 ~40%。能挂上的 maker 通常 1-2s 内就成交，3s 够。
const MAKER_TIMEOUT_S: u64 = 3;

/// Smart 模式：根据 decision 选择 maker / taker 价格
#[allow(clippy::too_many_arguments)]
async fn run_oto_smart_round(
    state: &AppState,
    auth: &AuthBundle,
    job_id: &str,
    symbol: &str,
    base_asset: &str,
    single_target: Decimal,
    tick: Decimal,
    step: Decimal,
    round: u32,
) -> anyhow::Result<()> {
    let started_ms = chrono::Utc::now().timestamp_millis();
    let round_no = rounds::next_round_no(&state.db, job_id).await.unwrap_or(round as i64);

    let book = get_orderbook_smart(state, symbol).await?;
    // V2.tune16: RecentTrades buffer 是全局的（NEX/B2 共用），切换 symbol 后会含跨币种价格，
    // 让 volatility 算出天文数字 → quality gate 永久触发 wait_for_better。
    // 修法：evaluate 前按当前 symbol 过滤。
    let recent: Vec<_> = state.recent_trades.snapshot()
        .into_iter()
        .filter(|t| t.symbol == symbol)
        .collect();
    let params = DecisionParams::default();
    let (decision_kind, ctx) = decision::evaluate(&book, &recent, tick, &params);
    info!(
        %job_id, round_no, decision = %decision_kind.label(),
        spread_ticks = ctx.spread_ticks, imbalance = ctx.imbalance, trend = ctx.trend,
        "v2 decision"
    );

    if decision_kind.is_skip() {
        let _ = rounds::insert(&state.db, &rounds::NewRound {
            job_id: job_id.into(),
            round_no,
            decision_type: decision_kind.label().into(),
            status: "skipped".into(),
            working_order_id: None,
            pending_order_id: None,
            buy_quote_qty: None,
            sell_quote_qty: None,
            pnl_usdt: None,
            commission_usdt: None,
            started_ms,
            ended_ms: Some(chrono::Utc::now().timestamp_millis()),
        }).await;
        // V2.tune5: WaitForBetter 等市场变好需要更久；SkipBearish 短一点重评估
        let sleep_s = if matches!(decision_kind, Decision::WaitForBetter) { 2 } else { 1 };
        tokio::time::sleep(Duration::from_secs(sleep_s)).await;
        return Ok(());
    }

    let (working_price, working_is_maker, pending_price, pending_is_maker) = match decision_kind {
        Decision::DoubleMaker => (
            ctx.best_bid + tick,
            true,
            ctx.best_ask - tick,
            true,
        ),
        Decision::TakerMakerHybrid => (
            ctx.best_ask - tick,
            false,
            ctx.best_ask - tick,
            true,
        ),
        Decision::SmallSpreadFollow => (
            ctx.best_ask + tick,
            false,
            ctx.best_bid + tick,
            true,
        ),
        // V2.tune2 实测：fast 改 maker pending(+2tick) 让 NEX timeout 率飙到 67%，
        // wear -31 bps 触发风控 pause。NEX 波动太大，maker pending 5-8s 内 fill 率太低，
        // emergency_sell 累计亏损 >> maker spread 收益。回退保守 taker-taker 策略。
        _ => (
            ctx.best_ask + tick * Decimal::from(PRICE_BUMP_TICKS),
            false,
            ctx.best_bid - tick * Decimal::from(PRICE_BUMP_TICKS),
            false,
        ),
    };

    if pending_price <= Decimal::ZERO || working_price <= Decimal::ZERO {
        anyhow::bail!("bad prices: working={working_price} pending={pending_price}");
    }
    // V2.tune33: sanity check — working 和 pending 价差应 < 20%（OTO 协议设计是中价附近）。
    // 之前 anchor 太老让 best_ask 被过滤成 0 → working_price = 1e-8、pending = 0.02
    // 价差 100 万倍 → qty 算成 18 亿超 binance 5 亿 max → 481011 错。
    let max_ratio = Decimal::from_str("1.2").unwrap();  // 20%
    let min_ratio = Decimal::from_str("0.8").unwrap();
    let ratio = if pending_price > Decimal::ZERO { working_price / pending_price } else { Decimal::ZERO };
    if ratio > max_ratio || ratio < min_ratio {
        anyhow::bail!("price sanity failed: working={working_price} pending={pending_price} ratio={ratio} (anchor/orderbook may be stale)");
    }
    let mut working_qty = round_step(single_target / working_price, step);
    if working_qty.is_zero() {
        anyhow::bail!("working_qty rounds to 0");
    }

    // V2.tune8: cap working_qty 到 min(best_ask_qty, best_bid_qty) × 0.9
    // working buy 看 best_ask 档量，pending sell 看 best_bid 档量，取小的避免任一侧 part-fill。
    // 0.9 留 10% safety margin（防 best 档量在我们下单前被其他 taker 吃了一部分）。
    if ctx.best_ask_qty > Decimal::ZERO && ctx.best_bid_qty > Decimal::ZERO {
        let limit_qty = ctx.best_ask_qty.min(ctx.best_bid_qty);
        let safe_cap = round_step(
            limit_qty * Decimal::from_str("0.9").unwrap(),
            step,
        );
        if !safe_cap.is_zero() && working_qty > safe_cap {
            info!(
                %job_id, round_no,
                original_qty = %working_qty,
                capped_qty = %safe_cap,
                best_ask_qty = %ctx.best_ask_qty,
                best_bid_qty = %ctx.best_bid_qty,
                "v2 cap working_qty to 0.9×min(ask_qty,bid_qty) (bilateral)"
            );
            working_qty = safe_cap;
        }
    }

    // cap 后 notional 可能低于 minNotional（NEX 是 0.1 USDT）
    let working_payment = (working_qty * working_price)
        .round_dp_with_strategy(8, rust_decimal::RoundingStrategy::ToZero);
    if working_payment < Decimal::from_str("0.1").unwrap() {
        // best 档量太少 → 本轮 skip + 等下次（market depth 会变）
        warn!(
            %job_id, round_no,
            %working_qty, %working_payment,
            best_ask_qty = %ctx.best_ask_qty,
            "v2 cap后 notional <0.1U → skip round (market too thin)"
        );
        let _ = rounds::insert(&state.db, &rounds::NewRound {
            job_id: job_id.into(),
            round_no,
            decision_type: format!("{}_thin_book", decision_kind.label()),
            status: "skipped".into(),
            working_order_id: None,
            pending_order_id: None,
            buy_quote_qty: None,
            sell_quote_qty: None,
            pnl_usdt: None,
            commission_usdt: None,
            started_ms,
            ended_ms: Some(chrono::Utc::now().timestamp_millis()),
        }).await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        return Ok(());
    }

    let req = PlaceOtoOrderRequest {
        base_asset: base_asset.into(),
        quote_asset: "USDT".into(),
        working_side: Side::Buy,
        working_price,
        working_quantity: working_qty,
        payment_details: vec![PaymentDetail {
            amount: working_payment,
            payment_wallet_type: WalletType::Card,
        }],
        pending_price,
        pending_type: OrderType::Limit,
    };
    let oto = state.alpha.place_oto_order(auth, &req).await?;
    let working_oid = oto.working_order_id.to_string();
    let pending_oid = oto.pending_order_id.to_string();

    let _ = persistence::repo::orders::insert(&state.db, &persistence::repo::orders::NewOrder {
        order_id: working_oid.clone(),
        job_id: job_id.into(),
        side: "BUY".into(),
        price: working_price,
        qty: working_qty,
        status: "pending".into(),
        raw_response: Some(format!(
            "v2 r{round_no} {} working{}",
            decision_kind.label(),
            if working_is_maker { "(maker)" } else { "(taker)" }
        )),
    }).await;
    let _ = persistence::repo::orders::insert(&state.db, &persistence::repo::orders::NewOrder {
        order_id: pending_oid.clone(),
        job_id: job_id.into(),
        side: "SELL".into(),
        price: pending_price,
        qty: working_qty,
        status: "pending".into(),
        raw_response: Some(format!(
            "v2 r{round_no} {} pending{}",
            decision_kind.label(),
            if pending_is_maker { "(maker)" } else { "(taker)" }
        )),
    }).await;

    let working_timeout = if working_is_maker { MAKER_TIMEOUT_S } else { FILL_WAIT_TIMEOUT_S };
    let fills_working = wait_fills_with_timeout(state, auth, &working_oid, symbol, working_timeout).await;
    persist_fills(state, &fills_working, Some(job_id), &auth.username).await;

    if fills_working.is_empty() {
        let _ = state.alpha.cancel_order(auth, &CancelOrderRequest {
            order_id: working_oid.clone(),
            symbol: symbol.into(),
        }).await;
        warn!(%job_id, round_no, %working_oid, "v2 working no fill → skipped");
        let _ = persistence::repo::orders::set_status(&state.db, &working_oid, "canceled").await;
        let _ = persistence::repo::orders::set_status(&state.db, &pending_oid, "canceled").await;
        let _ = rounds::insert(&state.db, &rounds::NewRound {
            job_id: job_id.into(),
            round_no,
            decision_type: format!("{}_working_timeout", decision_kind.label()),
            status: "skipped".into(),
            working_order_id: Some(working_oid),
            pending_order_id: Some(pending_oid),
            buy_quote_qty: None,
            sell_quote_qty: None,
            pnl_usdt: None,
            commission_usdt: None,
            started_ms,
            ended_ms: Some(chrono::Utc::now().timestamp_millis()),
        }).await;
        return Ok(());
    }
    let _ = persistence::repo::orders::set_status(&state.db, &working_oid, "filled").await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let pending_timeout = if pending_is_maker { MAKER_TIMEOUT_S } else { FILL_WAIT_TIMEOUT_S };
    let fills_pending = wait_fills_with_timeout(state, auth, &pending_oid, symbol, pending_timeout).await;
    persist_fills(state, &fills_pending, Some(job_id), &auth.username).await;

    // 注意：wait_fills 在 loop 内多次调 get_user_trades，币安对 OTO order_id
    // 返回的 fill 列表可能含同 fill_id 重复条目（已在 fills 实测验证：sell_quote
    // 50x 虚高即来自此）。trades 表用 INSERT OR IGNORE + fill_id 唯一规避了入库
    // 重复，但内存 sum 必须显式 dedup，否则 rounds.{buy,sell}_quote_qty 失真。
    // 同时严格按 side 过滤，防 OTO API 把 working/pending 双侧 fills 串到一起。
    let buy_quote = dedup_quote_by_side(&fills_working, Side::Buy);
    let sell_quote = dedup_quote_by_side(&fills_pending, Side::Sell);
    // V2.tune18: 算 pnl 用 qty 配对（避免 OTO partial fill 把历史库存差当利润）
    let buy_qty = dedup_qty_by_side(&fills_working, Side::Buy);
    let sell_qty = dedup_qty_by_side(&fills_pending, Side::Sell);

    // V2.tune30: pending 无条件 cancel — wait_fills 早退（V2.tune24）后 binance 那边
    // 可能还活着（partial fill）。screenshot 实测：7 min 前下的 ZEST sell 43% filled
    // 还挂着没撤。cancel 是幂等的：已 filled 返回错误（忽略），活着的撤掉剩余。
    // 然后看 buy/sell qty 是否对得上判断 partial vs full。
    let fully_filled = !buy_qty.is_zero()
        && sell_qty >= buy_qty * Decimal::from_str("0.99").unwrap();
    if !fully_filled {
        let _ = state.alpha.cancel_order(auth, &CancelOrderRequest {
            order_id: pending_oid.clone(),
            symbol: symbol.into(),
        }).await;
        let _ = persistence::repo::orders::set_status(&state.db, &pending_oid, "canceled").await;
        if fills_pending.is_empty() {
            warn!(%job_id, round_no, %pending_oid, "v2 pending no fill → emergency_sell");
        } else {
            warn!(
                %job_id, round_no, %pending_oid, %buy_qty, %sell_qty,
                "v2 pending partial fill ({:.1}%) → cancel + emergency_sell remainder",
                if !buy_qty.is_zero() {
                    use rust_decimal::prelude::ToPrimitive;
                    (sell_qty / buy_qty * Decimal::from(100)).to_f64().unwrap_or(0.0)
                } else { 0.0 }
            );
        }
        emergency_sell_all(state, auth, job_id, symbol, base_asset, tick, step, round).await;
        let _ = rounds::insert(&state.db, &rounds::NewRound {
            job_id: job_id.into(),
            round_no,
            decision_type: format!("{}_pending_timeout", decision_kind.label()),
            status: "failed".into(),
            working_order_id: Some(working_oid),
            pending_order_id: Some(pending_oid),
            buy_quote_qty: Some(buy_quote),
            sell_quote_qty: if sell_quote.is_zero() { None } else { Some(sell_quote) },
            pnl_usdt: None,
            commission_usdt: None,
            started_ms,
            ended_ms: Some(chrono::Utc::now().timestamp_millis()),
        }).await;
        return Ok(());
    }
    let _ = persistence::repo::orders::set_status(&state.db, &pending_oid, "filled").await;

    let pnl = matched_pnl(buy_qty, buy_quote, sell_qty, sell_quote);
    let _ = rounds::insert(&state.db, &rounds::NewRound {
        job_id: job_id.into(),
        round_no,
        decision_type: decision_kind.label().into(),
        status: "filled".into(),
        working_order_id: Some(working_oid),
        pending_order_id: Some(pending_oid),
        buy_quote_qty: Some(buy_quote),
        sell_quote_qty: Some(sell_quote),
        pnl_usdt: Some(pnl),
        commission_usdt: None,
        started_ms,
        ended_ms: Some(chrono::Utc::now().timestamp_millis()),
    }).await;
    info!(%job_id, round_no, decision = decision_kind.label(), %pnl, "v2 round done");

    let residual = current_base_free(state, auth, base_asset).await.unwrap_or(Decimal::ZERO);
    if residual >= step * Decimal::from(3) {
        warn!(%job_id, round_no, %residual, "v2 residual ≥ 3 step → sweep");
        emergency_sell_all(state, auth, job_id, symbol, base_asset, tick, step, round).await;
    }
    Ok(())
}

/// fills 列表内可能因 OTO 多次轮询返回同 fill_id 重复条目；按 side 过滤 + fill_id dedup 后累加 quote_qty。
/// V2.tune19 fix: 旧 dedup 只用 fill_id，但 Binance 同一 fill_id 会返回多笔 sub-fill
/// (不同 qty/quote_qty)，按 id 去重会漏 sub-fill。改用 (id, qty, quote_qty) 三元组去重，
/// 真正重复（同 id 同 qty 同 quote）的 IGNORE，sub-fill（同 id 不同 qty）保留。
fn dedup_quote_by_side(fills: &[binance_alpha::TradeFill], side: Side) -> Decimal {
    let mut seen = std::collections::HashSet::new();
    fills
        .iter()
        .filter(|f| f.side == side && seen.insert((f.id.clone(), f.qty, f.quote_qty)))
        .map(|f| f.quote_qty)
        .sum()
}

/// V2.tune18: pnl 计算用，dedup 后按 side 累加 qty（base asset 数量）。
fn dedup_qty_by_side(fills: &[binance_alpha::TradeFill], side: Side) -> Decimal {
    let mut seen = std::collections::HashSet::new();
    fills
        .iter()
        .filter(|f| f.side == side && seen.insert((f.id.clone(), f.qty, f.quote_qty)))
        .map(|f| f.qty)
        .sum()
}

/// V2.tune18: 按配对部分计算 round pnl，解决 OTO partial fill 引起的虚假 pnl。
///
/// 问题：OTO 的 working BUY 部分成交 0.5u（实际买到 239k NEX），
/// 但 pending SELL 卖出 38u（实际卖了 8.8M NEX，含历史库存）→ 旧算法 pnl=+$37.5（假）。
///
/// 修法：matched_qty = min(buy_qty, sell_qty)，按 matched 比例算两侧 quote。
/// 剩下的库存不在本 round pnl，会流到后续 round / final_cleanup。
fn matched_pnl(
    buy_qty: Decimal,
    buy_quote: Decimal,
    sell_qty: Decimal,
    sell_quote: Decimal,
) -> Decimal {
    if buy_qty.is_zero() || sell_qty.is_zero() {
        return Decimal::ZERO;
    }
    let matched = buy_qty.min(sell_qty);
    let buy_cost = buy_quote * matched / buy_qty;
    let sell_rev = sell_quote * matched / sell_qty;
    sell_rev - buy_cost
}

#[cfg(test)]
mod fills_dedup_tests {
    use super::*;
    use binance_alpha::TradeFill;
    use rust_decimal_macros::dec;

    fn mk_fill(id: &str, side: Side, quote: rust_decimal::Decimal) -> TradeFill {
        TradeFill {
            id: id.into(),
            order_id: "o".into(),
            trade_id: Some(id.into()),
            side,
            price: dec!(1),
            qty: dec!(1),
            quote_qty: quote,
            commission: dec!(0),
            commission_asset: "USDT".into(),
            symbol: "ALPHA_971USDT".into(),
            base_asset: Some("ALPHA_971".into()),
            quote_asset: Some("USDT".into()),
            order_type: Some("LIMIT".into()),
            buyer: Some(matches!(side, Side::Buy)),
            last_trade: Some(true),
            time: 0,
        }
    }

    #[test]
    fn dedups_same_id_keeps_first_quote() {
        // 同 fill_id 出现 3 次（模拟 OTO API 重复返回）
        let fills = vec![
            mk_fill("a", Side::Sell, dec!(0.197)),
            mk_fill("a", Side::Sell, dec!(0.197)),
            mk_fill("a", Side::Sell, dec!(0.197)),
        ];
        // 修复前会算成 0.591，修复后 0.197
        assert_eq!(dedup_quote_by_side(&fills, Side::Sell), dec!(0.197));
    }

    #[test]
    fn filters_by_side() {
        let fills = vec![
            mk_fill("buy1", Side::Buy, dec!(2.17)),
            mk_fill("sell1", Side::Sell, dec!(0.197)),
        ];
        assert_eq!(dedup_quote_by_side(&fills, Side::Buy), dec!(2.17));
        assert_eq!(dedup_quote_by_side(&fills, Side::Sell), dec!(0.197));
    }

    #[test]
    fn unique_ids_sum_normally() {
        let fills = vec![
            mk_fill("s1", Side::Sell, dec!(1.0)),
            mk_fill("s2", Side::Sell, dec!(2.0)),
            mk_fill("s3", Side::Sell, dec!(3.0)),
        ];
        assert_eq!(dedup_quote_by_side(&fills, Side::Sell), dec!(6.0));
    }

    // ==== V2.tune18: matched_pnl 单测 ====

    #[test]
    fn matched_pnl_balanced_round_normal_profit() {
        // 平衡 round：buy 10 NEX cost 1.0u, sell 10 NEX got 1.05u → pnl=0.05
        let pnl = matched_pnl(dec!(10), dec!(1.0), dec!(10), dec!(1.05));
        assert_eq!(pnl, dec!(0.05));
    }

    #[test]
    fn matched_pnl_partial_buy_pending_sells_inventory() {
        // round 141 真实场景：buy 0.24M NEX cost $1.06, sell 8.8M NEX (含历史库存) got $39.26
        // 旧算法: pnl = 39.26 - 1.06 = +$38.2（假！）
        // 新算法: matched = 0.24M, sell_rev = 39.26 * 0.24M/8.8M = $1.07, pnl ≈ 0.01（对）
        let pnl = matched_pnl(
            dec!(239658),
            dec!(1.06408240),
            dec!(8844680),
            dec!(39.26153629),
        );
        // 配对部分 pnl 应该接近 0（不是 +$38）
        assert!(pnl.abs() < dec!(0.05), "matched_pnl should be near 0, got {}", pnl);
    }

    #[test]
    fn matched_pnl_full_buy_pending_undersell() {
        // round 964 真实场景：buy 8M NEX cost $36.49, sell 152k NEX got $0.69
        // 旧算法: pnl = 0.69 - 36.49 = -$35.8（假！剩 7.8M NEX 留作库存）
        // 新算法: matched = 152k, buy_cost = 36.49 * 152k/8M = $0.69, pnl ≈ 0
        let pnl = matched_pnl(
            dec!(8045792),
            dec!(36.49571478),
            dec!(151747),
            dec!(0.68741753),
        );
        assert!(pnl.abs() < dec!(0.05), "matched_pnl should be near 0, got {}", pnl);
    }

    #[test]
    fn matched_pnl_zero_qty_returns_zero() {
        assert_eq!(matched_pnl(dec!(0), dec!(0), dec!(10), dec!(1)), dec!(0));
        assert_eq!(matched_pnl(dec!(10), dec!(1), dec!(0), dec!(0)), dec!(0));
    }
}

#[cfg(test)]
mod extra_verify_detection_tests {
    use super::*;

    #[test]
    fn detects_extra_verify_required_code() {
        let api_err = binance_alpha::AlphaApiError::Server {
            code: "2fa-extra-verify-required".into(),
            message: "extra verification required (face/phone): code=100001003 message=...".into(),
            detail: None,
        };
        let any: anyhow::Error = api_err.into();
        assert!(is_extra_verify_required_error(&any));
    }

    #[test]
    fn rejects_normal_2fa_failed() {
        let api_err = binance_alpha::AlphaApiError::Server {
            code: "2fa-failed".into(),
            message: "wrong 2fa code".into(),
            detail: None,
        };
        let any: anyhow::Error = api_err.into();
        assert!(!is_extra_verify_required_error(&any));
    }

    #[test]
    fn rejects_non_alpha_error() {
        let any = anyhow::anyhow!("some other error: 2fa-extra-verify-required");
        // 即使消息里包含触发字符串，downcast 失败 → 不当作 extra-verify
        assert!(!is_extra_verify_required_error(&any));
    }
}

/// 跟 wait_fills 一样，但 timeout 可参数化（给 maker 5s, taker 8s）
async fn wait_fills_with_timeout(
    state: &AppState,
    auth: &AuthBundle,
    order_id: &str,
    symbol: &str,
    timeout_s: u64,
) -> Vec<binance_alpha::TradeFill> {
    // V2.tune24: 还原"早退"模式 — round 速度回到 2-3s/round（V2.tune23 改成
    // 等满 timeout 让每 round 慢到 16s，太慢）。
    //
    // 漏 sub-fill 的问题改由 reconciler 后台兜底：每 5 min 扫最近 jobs 的 orders，
    // 从 Binance 重拉一次 fills，INSERT OR IGNORE 把漏的补上。
    // (见 reconciler.rs::reconcile_fills_from_binance)
    //
    // 早退条件：拿到任何 fill 就 return。空响应 deadline 兜底。
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_s);
    loop {
        match state.alpha.get_user_trades(auth, order_id, symbol).await {
            Ok(fs) if !fs.is_empty() => return fs,
            _ => {
                if std::time::Instant::now() >= deadline {
                    return Vec::new();
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
