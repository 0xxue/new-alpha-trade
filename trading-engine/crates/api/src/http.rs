//! HTTP 路由。
//!
//! P2 范围实现：
//! - GET  /health
//! - GET  /strategies                  返回策略元数据列表（目前只有 placeholder）
//! - GET  /orderbook/:symbol           调币安 fullDepth
//! - POST /trade/start                 入库 job（不立即跑策略）
//! - GET  /trade/jobs                  列 jobs
//! - GET  /trade/status/:id            单 job 状态
//! - POST /trade/pause/:id             改 state -> paused
//! - POST /trade/resume/:id            改 state -> running
//! - POST /trade/stop/:id              改 state -> stopped
//! - GET  /accounts/:user/balance      通过 qr-service 拿 cookies → 调币安 wallet

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use binance_alpha::usdt_funding_free;
use persistence::repo::{jobs, rounds, server_meta, stats};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::str::FromStr;
use uuid::Uuid;

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/strategies", get(strategies))
        .route("/orderbook/:symbol", get(orderbook))
        .route("/debug/live-book", get(debug_live_book))
        .route("/debug/live-book/switch/:symbol", post(debug_live_book_switch))
        .route("/debug/filter-test/:symbol", get(debug_filter_test))
        .route("/trade/start", post(trade_start))
        .route("/trade/jobs", get(trade_jobs))
        .route("/trade/status/:id", get(trade_status))
        .route("/trade/pause/:id", post(trade_pause))
        .route("/trade/resume/:id", post(trade_resume))
        .route("/trade/stop/:id", post(trade_stop))
        .route("/trade/stats/:id", get(trade_stats))
        .route("/trade/timeseries/:id", get(trade_timeseries))
        .route("/accounts/:user/balance", get(account_balance))
        .route("/accounts/:user/spot-balance", get(account_spot_balance))
        .route("/tokens", get(list_tokens))
        .route("/tokens/:symbol", get(get_token))
        .route("/server-meta", get(server_meta_get))
        .route("/server-meta/renew", post(server_meta_renew))
}

// ============================================================ /health
async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "trading-engine",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

// ============================================================ /strategies
#[derive(Serialize)]
struct StrategyMeta {
    name: &'static str,
    version: &'static str,
    description: &'static str,
}

async fn strategies() -> Json<Vec<StrategyMeta>> {
    Json(vec![
        StrategyMeta {
            name: "oto",
            version: "v1-fast",
            description: "OTO 一发买卖两单（fast 模式：保证两端立即成交）。默认选这个，速度最快。",
        },
        StrategyMeta {
            name: "simple_round",
            version: "v1",
            description: "BUY → 等成交 → SELL → 等成交。简单稳，但慢（每轮 10-15s）。OTO 失败时作为兜底。",
        },
    ])
}

// ============================================================ /orderbook
async fn orderbook(
    Path(symbol): Path<String>,
    State(s): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let snap = s
        .alpha
        .get_full_depth(&symbol, 100)
        .await
        .map_err(ApiErr::upstream)?;
    Ok(Json(serde_json::to_value(&snap).map_err(ApiErr::internal)?))
}

// ============================================================ /debug/live-book/switch
/// V2.tune15: 手动触发 live_book 切换到指定 symbol（订阅 ws 流 + REST snapshot init）。
/// 用来在不实际下单的情况下验证 B2 等其他 token 的盘口加载。
async fn debug_live_book_switch(
    Path(symbol): Path<String>,
    State(s): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    // symbol 来自 URL（用户传 "ALPHA_162USDT" 或 "ALPHA_162"）
    let pair_symbol = if symbol.ends_with("USDT") {
        symbol.clone()
    } else {
        format!("{}USDT", symbol)
    };
    let alpha_id = pair_symbol.trim_end_matches("USDT").to_string();
    s.alpha_ws
        .add_subscriptions(vec![
            binance_alpha::agg_trade_stream(&alpha_id),
            binance_alpha::depth_stream(&alpha_id),
        ])
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    s.live_book
        .init_snapshot(&pair_symbol)
        .await
        .map_err(|e| ApiErr::internal(e))?;
    Ok(Json(json!({"ok": true, "symbol": pair_symbol})))
}

// ============================================================ /debug/filter-test
/// V2.tune15: 看 filter_stale_levels 对当前 symbol 的过滤效果。
/// 返回 raw REST + filtered + anchor，方便对比 B2 zombie 残留是否被清理。
async fn debug_filter_test(
    Path(symbol): Path<String>,
    State(s): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    use rust_decimal::Decimal;
    use std::str::FromStr;
    let pair_symbol = if symbol.ends_with("USDT") {
        symbol.clone()
    } else {
        format!("{}USDT", symbol)
    };
    let raw = s
        .alpha
        .get_full_depth(&pair_symbol, 20)
        .await
        .map_err(ApiErr::upstream)?;
    let anchor = s.recent_trades.last_price_for(&pair_symbol);
    let raw_top = json!({
        "best_bid": raw.bids.first(),
        "best_ask": raw.asks.first(),
        "bids_top5": raw.bids.iter().take(5).collect::<Vec<_>>(),
        "asks_top5": raw.asks.iter().take(5).collect::<Vec<_>>(),
    });
    // 镜像 strategy_runner::filter_stale_levels 的逻辑（避免 pub 出去）
    let lo_factor = Decimal::from_str("0.95").unwrap();
    let hi_factor = Decimal::from_str("1.05").unwrap();
    let mut filtered = raw.clone();
    if let Some(a) = anchor {
        let lo = a * lo_factor;
        let hi = a * hi_factor;
        filtered.bids.retain(|b| Decimal::from_str(&b[0]).map(|p| p <= hi).unwrap_or(false));
        filtered.asks.retain(|a2| Decimal::from_str(&a2[0]).map(|p| p >= lo).unwrap_or(false));
        // 二次清洗：仍 crossed → drop 离 anchor 远的一侧
        loop {
            let bb = filtered.bids.first().and_then(|b| Decimal::from_str(&b[0]).ok());
            let aa = filtered.asks.first().and_then(|a2| Decimal::from_str(&a2[0]).ok());
            match (bb, aa) {
                (Some(b_p), Some(a_p)) if b_p > a_p => {
                    let bid_dist = if b_p > a { b_p - a } else { a - b_p };
                    let ask_dist = if a_p > a { a_p - a } else { a - a_p };
                    if bid_dist > ask_dist { filtered.bids.remove(0); } else { filtered.asks.remove(0); }
                }
                _ => break,
            }
            if filtered.bids.is_empty() || filtered.asks.is_empty() { break; }
        }
    }
    let filt_top = json!({
        "best_bid": filtered.bids.first(),
        "best_ask": filtered.asks.first(),
        "bids_top5": filtered.bids.iter().take(5).collect::<Vec<_>>(),
        "asks_top5": filtered.asks.iter().take(5).collect::<Vec<_>>(),
    });
    Ok(Json(json!({
        "symbol": pair_symbol,
        "anchor": anchor.map(|d| d.to_string()),
        "raw": raw_top,
        "filtered": filt_top,
    })))
}

// ============================================================ /debug/live-book
/// V2.tune15: 调试 LiveOrderBook 当前状态。返回当前 symbol + age + top 5 档。
/// 用来验证 @depth@100ms 增量流是否在维护本地 book。
async fn debug_live_book(State(s): State<AppState>) -> Json<Value> {
    let current_symbol = s.live_book.current_symbol();
    let age_ms = s.live_book.age().map(|d| d.as_millis() as u64);
    let fresh = s.live_book.snapshot_fresh().map(|b| {
        json!({
            "kind": "fresh",
            "last_update_id": b.last_update_id,
            "symbol": b.symbol,
            "bids_top5": b.bids.iter().take(5).collect::<Vec<_>>(),
            "asks_top5": b.asks.iter().take(5).collect::<Vec<_>>(),
        })
    });
    let any = s.live_book.snapshot_any().map(|b| {
        json!({
            "kind": "any",
            "last_update_id": b.last_update_id,
            "symbol": b.symbol,
            "bids_top5": b.bids.iter().take(5).collect::<Vec<_>>(),
            "asks_top5": b.asks.iter().take(5).collect::<Vec<_>>(),
        })
    });
    Json(json!({
        "current_symbol": current_symbol,
        "age_ms": age_ms,
        "fresh": fresh,
        "any": any,
    }))
}

// ============================================================ /trade/start
#[derive(Deserialize)]
struct StartReq {
    username: String,
    /// 可以是友好名（"NEX" / "ZEST"）、alpha_id（"ALPHA_971"）或 pair（"ALPHA_971USDT"）
    symbol: String,
    #[serde(default = "default_strategy")]
    strategy: String,
    target_volume: String,
    /// 单笔最低金额 USDT（默认 0.3）
    #[serde(default)]
    single_min_usdt: Option<String>,
    /// 单笔最高金额 USDT（默认 = single_min；若同 min 则固定金额）
    #[serde(default)]
    single_max_usdt: Option<String>,
    #[serde(default)]
    params: Value,
}

fn default_strategy() -> String {
    "oto_smart".into()
}

#[derive(Serialize)]
struct StartResp {
    job_id: String,
    state: String,
}

async fn trade_start(
    State(s): State<AppState>,
    Json(body): Json<StartReq>,
) -> Result<Json<StartResp>, ApiErr> {
    if body.username.trim().is_empty() {
        return Err(ApiErr::bad("username required"));
    }
    if body.symbol.trim().is_empty() {
        return Err(ApiErr::bad("symbol required"));
    }
    let tv = Decimal::from_str(&body.target_volume)
        .map_err(|e| ApiErr::bad(format!("target_volume: {e}")))?;
    if tv <= Decimal::ZERO {
        return Err(ApiErr::bad("target_volume must be > 0"));
    }

    // 解析金额范围
    let min_usdt = body
        .single_min_usdt
        .as_deref()
        .map(|s| Decimal::from_str(s).map_err(|e| ApiErr::bad(format!("single_min_usdt: {e}"))))
        .transpose()?
        .unwrap_or_else(|| Decimal::from_str("0.3").unwrap());
    let max_usdt = body
        .single_max_usdt
        .as_deref()
        .map(|s| Decimal::from_str(s).map_err(|e| ApiErr::bad(format!("single_max_usdt: {e}"))))
        .transpose()?
        .unwrap_or(min_usdt);
    if min_usdt <= Decimal::ZERO || max_usdt < min_usdt {
        return Err(ApiErr::bad("single_min_usdt > 0 且 single_max_usdt >= single_min_usdt"));
    }
    if max_usdt > tv {
        return Err(ApiErr::bad(format!(
            "single_max_usdt {max_usdt} 不能大于 target_volume {tv}"
        )));
    }

    // 通过 token registry 解析 friendly name → pair_symbol
    let token = s
        .tokens
        .find_by_symbol(&body.symbol)
        .or_else(|| s.tokens.find_by_alpha_id(&body.symbol))
        .or_else(|| s.tokens.find_by_pair(&body.symbol))
        .ok_or_else(|| {
            ApiErr::bad(format!(
                "symbol {:?} not in token registry (refresh in <= 5min)",
                body.symbol
            ))
        })?;
    if !token.tradable {
        return Err(ApiErr::bad(format!(
            "{} ({}) 当前不可交易（offline 或 stockState）",
            token.symbol, token.alpha_id
        )));
    }
    let symbol_pair = token.pair_symbol.clone();

    let auth = s.qr.get_auth(&body.username).await.map_err(|e| match e {
        crate::qr_client::QrClientError::NotFound(_) => ApiErr::bad(format!("{e}")),
        other => ApiErr::upstream(other),
    })?;

    // 拉 spot 余额做 wear baseline
    let wallet = s.alpha.get_spot_wallet(&auth).await.map_err(ApiErr::upstream)?;
    let baseline = usdt_funding_free(&wallet).ok_or_else(|| {
        ApiErr::upstream("could not read USDT.funding.free for wear baseline")
    })?;

    // 把 baseline + min/max + base_asset 嵌到 params_json 里
    let mut params: Value = if body.params.is_object() {
        body.params.clone()
    } else {
        serde_json::json!({})
    };
    if let Some(obj) = params.as_object_mut() {
        obj.insert("_baseline_spot_usdt".into(), Value::String(baseline.to_string()));
        obj.insert("single_min_usdt".into(), Value::String(min_usdt.to_string()));
        obj.insert("single_max_usdt".into(), Value::String(max_usdt.to_string()));
        obj.insert("_base_asset".into(), Value::String(token.alpha_id.clone()));
        obj.insert("_token_symbol".into(), Value::String(token.symbol.clone()));
    }

    let id = Uuid::new_v4().to_string();
    let new_job = jobs::NewJob {
        id: id.clone(),
        username: body.username,
        symbol: symbol_pair.clone(),
        strategy: body.strategy,
        params_json: params.to_string(),
        target_volume: tv,
    };
    jobs::insert(&s.db, &new_job).await.map_err(ApiErr::internal)?;
    tracing::info!(
        job_id = %id, %baseline, symbol = %symbol_pair,
        token_symbol = %token.symbol, alpha_id = %token.alpha_id,
        %min_usdt, %max_usdt,
        "job inserted"
    );

    s.runner.start(s.clone(), id.clone()).await;

    Ok(Json(StartResp {
        job_id: id,
        state: "running".into(),
    }))
}

// ============================================================ /trade/jobs
async fn trade_jobs(State(s): State<AppState>) -> Result<Json<Vec<Value>>, ApiErr> {
    let rows = jobs::list(&s.db, None).await.map_err(ApiErr::internal)?;
    Ok(Json(rows.into_iter().map(job_to_json).collect()))
}

// ============================================================ /trade/status/:id
async fn trade_status(
    Path(id): Path<String>,
    State(s): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let row = jobs::get(&s.db, &id).await.map_err(ApiErr::internal)?;
    match row {
        Some(j) => Ok(Json(job_to_json(j))),
        None => Err(ApiErr::not_found("job not found")),
    }
}

async fn trade_set_state(
    s: AppState,
    id: String,
    new_state: jobs::JobState,
) -> Result<Json<Value>, ApiErr> {
    let changed = jobs::set_state(&s.db, &id, new_state)
        .await
        .map_err(ApiErr::internal)?;
    if !changed {
        return Err(ApiErr::not_found("job not found"));
    }
    Ok(Json(json!({"job_id": id, "state": new_state.as_str()})))
}

async fn trade_stats(
    Path(id): Path<String>,
    State(s): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let job = jobs::get(&s.db, &id).await.map_err(ApiErr::internal)?
        .ok_or_else(|| ApiErr::not_found("job not found"))?;
    // 拉当前 spot USDT + alpha wallet base 持仓
    let (current_spot, base_qty, base_val) = match s.qr.get_auth(&job.username).await {
        Ok(auth) => {
            let spot = s
                .alpha
                .get_spot_wallet(&auth)
                .await
                .ok()
                .and_then(|w| usdt_funding_free(&w));
            let (qty, val) = match s.alpha.get_alpha_wallet(&auth).await {
                Ok(w) => {
                    let base_asset: String = serde_json::from_str::<serde_json::Value>(&job.params_json)
                        .ok()
                        .and_then(|v| v.get("_base_asset").and_then(|x| x.as_str()).map(String::from))
                        .unwrap_or_default();
                    let entry = w.list.iter().find(|e| e.token_id == base_asset);
                    // V2.tune31: 真在途库存 = max(钱包看到的 free+freeze, DB 累计 buy-sell)。
                    // 币安会把 alpha 闲置 token 自动转到 earn（free 变 0 但 amount 还在）→
                    // 旧用 free+freeze 漏掉 earn 部分 → wear 假报 -89 bps。
                    // DB 的 buy_qty - sell_qty 是 ground truth: 这个 job 累计买了多少未卖。
                    let net_qty_db = persistence::repo::trades::net_base_qty(&s.db, &id)
                        .await
                        .unwrap_or(Decimal::ZERO);
                    let per_token_price = entry
                        .and_then(|e| {
                            if e.amount > Decimal::ZERO {
                                e.valuation.map(|v| v / e.amount)
                            } else { None }
                        })
                        .unwrap_or(Decimal::ZERO);
                    let wallet_visible = entry.map(|e| e.free + e.freeze).unwrap_or(Decimal::ZERO);
                    let effective_qty = if net_qty_db > wallet_visible { net_qty_db } else { wallet_visible };
                    let qty = Some(effective_qty);
                    let val = if per_token_price > Decimal::ZERO {
                        Some(effective_qty * per_token_price)
                    } else { None };
                    (qty, val)
                }
                Err(_) => (None, None),
            };
            (spot, qty, val)
        }
        Err(e) => {
            tracing::warn!(job_id=%id, err=%e, "stats: no auth, skipping wallet");
            (None, None, None)
        }
    };
    let st = stats::compute(&s.db, &id, current_spot, base_qty, base_val)
        .await
        .map_err(ApiErr::internal)?
        .ok_or_else(|| ApiErr::not_found("job not found"))?;

    // V2.6: rounds 聚合（决策分布 / 胜率 / round 总数）
    // 只有 oto_smart job 会产 rounds，其他 strategy round_stats 是空的（zero defaults）也无害。
    let round_stats = rounds::aggregate(&s.db, &id)
        .await
        .map_err(ApiErr::internal)?;
    let win_rate = round_stats.win_rate_pct();

    // 合并两份 JSON：base stats + rounds 子对象
    let mut v = serde_json::to_value(&st).map_err(ApiErr::internal)?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "rounds".into(),
            json!({
                "total": round_stats.total,
                "filled": round_stats.filled,
                "skipped": round_stats.skipped,
                "failed": round_stats.failed,
                "win": round_stats.win,
                "loss": round_stats.loss,
                "flat": round_stats.flat,
                "win_rate_pct": win_rate,
                "sum_pnl_usdt": round_stats.sum_pnl_usdt.to_string(),
                "decision_counts": round_stats.decision_counts,
            }),
        );
    }
    Ok(Json(v))
}

async fn trade_timeseries(
    Path(id): Path<String>,
    State(s): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let pts = stats::timeseries(&s.db, &id).await.map_err(ApiErr::internal)?;
    let job = jobs::get(&s.db, &id).await.map_err(ApiErr::internal)?;
    Ok(Json(json!({
        "job_id": id,
        "target_volume": job.as_ref().map(|j| j.target_volume.as_str()).unwrap_or("0"),
        "points": pts,
    })))
}

async fn trade_pause(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiErr> {
    trade_set_state(s, id, jobs::JobState::Paused).await
}
async fn trade_resume(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiErr> {
    // 设 state 到 running，并确保 runner 在跑（如已 done/stopped 也会重启）
    let resp = trade_set_state(s.clone(), id.clone(), jobs::JobState::Running).await?;
    s.runner.start(s.clone(), id).await;
    Ok(resp)
}
async fn trade_stop(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiErr> {
    // soft stop：只改 state；runner 在循环里发现 → 跑 final_cleanup → 自然退出
    // 不 abort（abort 会 kill runner 来不及 cleanup → 持仓残留）
    // 卡在长任务时最多 8s（fill timeout）才响应
    trade_set_state(s, id, jobs::JobState::Stopped).await
}

fn job_to_json(j: jobs::JobRow) -> Value {
    json!({
        "id": j.id,
        "username": j.username,
        "symbol": j.symbol,
        "strategy": j.strategy,
        "params": serde_json::from_str::<Value>(&j.params_json).unwrap_or(Value::Null),
        "target_volume": j.target_volume,
        "state": j.state,
        "created_at": j.created_at,
        "updated_at": j.updated_at,
    })
}

// ============================================================ /accounts/:user/balance
async fn account_balance(
    Path(user): Path<String>,
    State(s): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let auth = s.qr.get_auth(&user).await.map_err(|e| match e {
        crate::qr_client::QrClientError::NotFound(_) => ApiErr::not_found(format!("{e}")),
        other => ApiErr::upstream(other),
    })?;
    let wallet = s
        .alpha
        .get_alpha_wallet(&auth)
        .await
        .map_err(ApiErr::upstream)?;
    Ok(Json(serde_json::to_value(&wallet).map_err(ApiErr::internal)?))
}

// ============================================================ /tokens
async fn list_tokens(State(s): State<AppState>) -> Json<Value> {
    let all = s.tokens.list_all();
    Json(json!({
        "count": all.len(),
        "last_refresh_ms": s.tokens.last_refresh_ms(),
        "tokens": all,
    }))
}

async fn get_token(Path(symbol): Path<String>, State(s): State<AppState>) -> Result<Json<Value>, ApiErr> {
    // 支持按 symbol（"NEX"）或 alpha_id（"ALPHA_971"）或 pair（"ALPHA_971USDT"）查
    let t = s
        .tokens
        .find_by_symbol(&symbol)
        .or_else(|| s.tokens.find_by_alpha_id(&symbol))
        .or_else(|| s.tokens.find_by_pair(&symbol))
        .ok_or_else(|| ApiErr::not_found(format!("token {symbol:?} not in registry")))?;
    Ok(Json(serde_json::to_value(&t).map_err(ApiErr::internal)?))
}

// ============================================================ /accounts/:user/spot-balance
/// 返回所有 SPOT 资产 + 关键摘要：USDT 在 funding/CARD 的 free，wear baseline 用这个。
async fn account_spot_balance(
    Path(user): Path<String>,
    State(s): State<AppState>,
) -> Result<Json<Value>, ApiErr> {
    let auth = s.qr.get_auth(&user).await.map_err(|e| match e {
        crate::qr_client::QrClientError::NotFound(_) => ApiErr::not_found(format!("{e}")),
        other => ApiErr::upstream(other),
    })?;
    let wallet = s.alpha.get_spot_wallet(&auth).await.map_err(ApiErr::upstream)?;
    let usdt_funding = binance_alpha::usdt_funding_free(&wallet);
    let usdt_total: rust_decimal::Decimal = wallet
        .iter()
        .find(|e| e.asset == "USDT")
        .map(|e| {
            let s = e.spot.as_ref().map(|b| b.free).unwrap_or_default();
            let f = e.funding.as_ref().map(|b| b.free).unwrap_or_default();
            let r = e.earn.as_ref().map(|b| b.free).unwrap_or_default();
            s + f + r
        })
        .unwrap_or_default();
    Ok(Json(json!({
        "usdt_funding_free": usdt_funding,   // wear baseline 用这个
        "usdt_total_free": usdt_total,
        "assets": wallet,
    })))
}

// ============================================================ /server-meta
/// 服务器到期 / 购买日小工具
async fn server_meta_get(State(s): State<AppState>) -> Result<Json<Value>, ApiErr> {
    let body = compute_server_meta(&s.db).await?;
    Ok(Json(body))
}

/// 续费 — expires_at += 30 天，然后返回最新 meta
async fn server_meta_renew(State(s): State<AppState>) -> Result<Json<Value>, ApiErr> {
    let cur = server_meta::get(&s.db, "expires_at")
        .await
        .map_err(ApiErr::internal)?
        .unwrap_or_else(|| chrono::Utc::now().date_naive().to_string());
    let cur_date = chrono::NaiveDate::parse_from_str(&cur, "%Y-%m-%d")
        .map_err(|e| ApiErr::internal(format!("bad expires_at {cur:?}: {e}")))?;
    let new_date = cur_date + chrono::Duration::days(30);
    server_meta::set(&s.db, "expires_at", &new_date.to_string())
        .await
        .map_err(ApiErr::internal)?;
    let body = compute_server_meta(&s.db).await?;
    Ok(Json(body))
}

async fn compute_server_meta(db: &persistence::DbPool) -> Result<Value, ApiErr> {
    let purchased = server_meta::get(db, "purchased_at")
        .await
        .map_err(ApiErr::internal)?
        .unwrap_or_else(|| "2026-05-21".into());
    let expires = server_meta::get(db, "expires_at")
        .await
        .map_err(ApiErr::internal)?
        .unwrap_or_else(|| "2026-06-20".into());

    let today = chrono::Utc::now().date_naive();
    let pur_date = chrono::NaiveDate::parse_from_str(&purchased, "%Y-%m-%d").unwrap_or(today);
    let exp_date = chrono::NaiveDate::parse_from_str(&expires, "%Y-%m-%d").unwrap_or(today);

    let days_left = (exp_date - today).num_days();
    let days_total = (exp_date - pur_date).num_days();
    let days_used = (today - pur_date).num_days();

    Ok(json!({
        "purchased_at": purchased,
        "expires_at": expires,
        "days_left": days_left,
        "days_total": days_total,
        "days_used": days_used,
    }))
}

// ============================================================ 错误类型
struct ApiErr {
    status: StatusCode,
    msg: String,
}

impl ApiErr {
    fn bad(m: impl ToString) -> Self {
        Self { status: StatusCode::BAD_REQUEST, msg: m.to_string() }
    }
    fn not_found(m: impl ToString) -> Self {
        Self { status: StatusCode::NOT_FOUND, msg: m.to_string() }
    }
    fn upstream<E: std::fmt::Display>(e: E) -> Self {
        Self { status: StatusCode::BAD_GATEWAY, msg: e.to_string() }
    }
    fn internal<E: std::fmt::Display>(e: E) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, msg: e.to_string() }
    }
}

impl axum::response::IntoResponse for ApiErr {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(json!({"error": self.msg}))).into_response()
    }
}
