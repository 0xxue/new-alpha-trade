//! HTTP + WebSocket API 层（axum）。

pub mod decision;
pub mod http;
pub mod orders;
pub mod qr_client;
pub mod reconciler;
pub mod strategy_runner;
pub mod tokens;
pub mod user_stream;
pub mod ws;

use std::sync::Arc;

use axum::Router;
use binance_alpha::{agg_trade_stream, depth_stream, AlphaRest, AlphaWsClient, SharedAlphaRest};
use persistence::DbPool;
use qr_client::SharedQrClient;
use strategy_runner::JobRunner;
use tokens::TokenRegistry;

#[derive(Clone)]
pub struct AppState {
    pub db: DbPool,
    pub alpha: SharedAlphaRest,
    pub qr: SharedQrClient,
    pub alpha_ws: Arc<AlphaWsClient>,
    pub runner: Arc<JobRunner>,
    pub tokens: Arc<TokenRegistry>,
    pub recent_trades: Arc<decision::RecentTrades>,
    pub live_book: Arc<decision::LiveOrderBook>,
}

pub fn router(
    db: DbPool,
    alpha: SharedAlphaRest,
    qr: SharedQrClient,
    alpha_ws: Arc<AlphaWsClient>,
    runner: Arc<JobRunner>,
    tokens: Arc<TokenRegistry>,
    recent_trades: Arc<decision::RecentTrades>,
    live_book: Arc<decision::LiveOrderBook>,
) -> Router {
    let state = AppState {
        db,
        alpha,
        qr,
        alpha_ws,
        runner,
        tokens,
        recent_trades,
        live_book,
    };
    Router::new()
        .merge(http::routes())
        .merge(orders::routes())
        .nest("/ws", ws::routes())
        .with_state(state)
}

pub fn build_clients(
    qr_base_url: &str,
) -> anyhow::Result<(
    SharedAlphaRest,
    SharedQrClient,
    Arc<AlphaWsClient>,
    Arc<JobRunner>,
    Arc<TokenRegistry>,
    Arc<decision::RecentTrades>,
    Arc<decision::LiveOrderBook>,
)> {
    let alpha = Arc::new(AlphaRest::new()?);
    let qr = Arc::new(qr_client::QrClient::new(qr_base_url));
    let alpha_ws = Arc::new(AlphaWsClient::new());
    let runner = JobRunner::new();
    let tokens = TokenRegistry::new(alpha.clone());
    tokens.spawn_refresh();
    let recent_trades = decision::RecentTrades::new(50); // 保留最近 50 笔
    recent_trades.spawn_consumer(alpha_ws.clone());
    // V2.tune15：WebSocket @depth@100ms 增量流维护本地真实 orderbook，fresh 窗口 3s
    // 注：需要 alpha REST 客户端用来拿初始 snapshot
    let live_book = decision::LiveOrderBook::new(3, alpha.clone());
    live_book.spawn_consumer(alpha_ws.clone());
    Ok((alpha, qr, alpha_ws, runner, tokens, recent_trades, live_book))
}

pub fn start_ws(alpha_ws: &Arc<AlphaWsClient>, live_book: Arc<decision::LiveOrderBook>) {
    let _rx = alpha_ws.spawn();
    let me = alpha_ws.clone();
    let book = live_book.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        // V2.tune15：默认订阅 NEX 的 aggTrade + depth@100ms 增量流
        me.add_subscriptions(vec![
            agg_trade_stream("ALPHA_971"),
            depth_stream("ALPHA_971"),
        ])
        .await;
        tracing::info!("default ws subscriptions registered (aggTrade + depth@100ms)");
        // 拿 REST snapshot 作为本地 orderbook 基线，之后 WS 增量应用
        // 等订阅 ACK + 几个 event buffer 后再 init，避免 WS first event 比 snapshot 老
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        if let Err(e) = book.init_snapshot("ALPHA_971USDT").await {
            tracing::warn!(err = %e, "live_book initial snapshot failed for ALPHA_971USDT");
        }
    });
}

pub fn start_user_streams(
    db: persistence::DbPool,
    alpha: SharedAlphaRest,
    qr: SharedQrClient,
    public_ws: Arc<AlphaWsClient>,
) {
    let mgr = user_stream::UserStreamManager::new(db, alpha, qr, public_ws);
    tokio::spawn(async move {
        if let Err(e) = mgr.start_for_active_accounts().await {
            tracing::warn!(err = %e, "failed to start user streams");
        }
    });
}

/// engine 启动时，扫 DB 把上次还在跑的 jobs 重新 spawn 给 runner（防止 engine 重启丢任务）。
pub fn resume_orphan_jobs(state: AppState) {
    tokio::spawn(async move {
        // 包含 running + paused（paused 进 runner 会自循环等待）
        let running = persistence::repo::jobs::list(&state.db, Some(persistence::repo::jobs::JobState::Running))
            .await
            .unwrap_or_default();
        let paused = persistence::repo::jobs::list(&state.db, Some(persistence::repo::jobs::JobState::Paused))
            .await
            .unwrap_or_default();
        let mut total = 0;
        for j in running.into_iter().chain(paused.into_iter()) {
            state.runner.start(state.clone(), j.id.clone()).await;
            tracing::info!(job_id = %j.id, state = %j.state, "resumed orphan job");
            total += 1;
        }
        if total > 0 {
            tracing::info!(%total, "orphan jobs resumed after engine restart");
        }
    });
}
