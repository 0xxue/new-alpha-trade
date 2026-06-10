//! trading-engine 主进程入口。

use std::net::SocketAddr;

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7002);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    let db_path = std::env::var("DB_PATH").unwrap_or_else(|_| "../data/new-alpha-trade.db".into());
    let qr_base = std::env::var("QR_SERVICE_URL").unwrap_or_else(|_| "http://127.0.0.1:7001".into());

    tracing::info!(%db_path, %qr_base, "opening sqlite + qr client + alpha ws");
    let pool = persistence::open(&db_path).await?;
    let (alpha, qr, alpha_ws, runner, tokens, recent_trades, live_book) =
        api::build_clients(&qr_base)?;
    api::start_ws(&alpha_ws, live_book.clone());
    api::start_user_streams(pool.clone(), alpha.clone(), qr.clone(), alpha_ws.clone());

    // 把 AppState 实例化两份共享：一份给 resume_orphan_jobs，一份进 router
    let state = api::AppState {
        db: pool.clone(),
        alpha: alpha.clone(),
        qr: qr.clone(),
        alpha_ws: alpha_ws.clone(),
        runner: runner.clone(),
        tokens: tokens.clone(),
        recent_trades: recent_trades.clone(),
        live_book: live_book.clone(),
    };
    api::resume_orphan_jobs(state.clone());
    api::reconciler::start(pool.clone(), alpha.clone(), qr.clone());

    tracing::info!(%addr, "trading-engine listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let app = api::router(
        pool,
        alpha,
        qr,
        alpha_ws,
        runner,
        tokens,
        recent_trades,
        live_book,
    );
    axum::serve(listener, app).await?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().compact())
        .init();
}
