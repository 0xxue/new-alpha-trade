//! 真实 WS 端点冒烟测试。
//!
//! cargo test -p binance-alpha --test integration_ws -- --ignored --nocapture

use std::sync::Arc;
use std::time::Duration;

use binance_alpha::{agg_trade_stream, AggTradeEvent, AlphaWsClient};

#[tokio::test]
#[ignore = "需要网络访问 binance"]
async fn agg_trade_stream_alpha_971() {
    let client = Arc::new(AlphaWsClient::new());
    let mut rx = client.spawn();
    client
        .add_subscriptions(vec![agg_trade_stream("ALPHA_971")])
        .await;

    // 等最多 60 秒收到至少 1 条 aggTrade（流动性差时可能需要更久）
    let timeout = tokio::time::sleep(Duration::from_secs(60));
    tokio::pin!(timeout);

    let mut count = 0_u32;
    loop {
        tokio::select! {
            evt = rx.recv() => {
                let evt = evt.expect("broadcast closed");
                if evt.stream.contains("aggTrade") {
                    let parsed: AggTradeEvent = serde_json::from_value(evt.data).unwrap();
                    println!(
                        "[{}] {} price={} qty={} m={}",
                        evt.stream, parsed.symbol, parsed.price, parsed.qty, parsed.buyer_is_maker
                    );
                    count += 1;
                    if count >= 3 {
                        return;
                    }
                }
            }
            _ = &mut timeout => {
                if count == 0 {
                    panic!("60 秒内没收到任何 aggTrade，可能币安或网络问题");
                }
                println!("only got {} events in 60s, but at least 1 — pass", count);
                return;
            }
        }
    }
}
