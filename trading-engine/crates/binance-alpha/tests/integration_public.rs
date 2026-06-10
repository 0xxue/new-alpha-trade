//! 公开端点真实调用（无需 cookies）。需要互联网。
//!
//! 跑：cargo test -p binance-alpha --test integration_public -- --ignored --nocapture

use binance_alpha::AlphaRest;

#[tokio::test]
#[ignore = "需要网络访问 binance.com"]
async fn full_depth_alpha_971_usdt() {
    let rest = AlphaRest::new().unwrap();
    let book = rest.get_full_depth("ALPHA_971USDT", 50).await.expect("fullDepth ok");
    println!("symbol={} bids={} asks={} lastUpdateId={}",
             book.symbol, book.bids.len(), book.asks.len(), book.last_update_id);
    assert_eq!(book.symbol, "ALPHA_971USDT");
    assert!(!book.bids.is_empty());
    assert!(!book.asks.is_empty());
    // 每行都是 [price, qty] 两个字符串
    assert_eq!(book.bids[0].len(), 2);
}

#[tokio::test]
#[ignore = "需要网络访问 binance.com"]
async fn exchange_info_has_alpha_971() {
    let rest = AlphaRest::new().unwrap();
    let info = rest.get_exchange_info().await.expect("exchangeInfo ok");
    let s = info.symbols.iter().find(|s| s.symbol == "ALPHA_971USDT");
    assert!(s.is_some(), "ALPHA_971USDT not found in exchange info");
    let s = s.unwrap();
    println!(
        "ALPHA_971USDT: status={}, pricePrecision={}, filters={}",
        s.status, s.price_precision, s.filters.len()
    );
}

#[tokio::test]
#[ignore = "需要网络访问 binance.com"]
async fn fee_rate_alpha_971() {
    let rest = AlphaRest::new().unwrap();
    let fee = rest.get_fee_rate("ALPHA_971USDT").await.expect("feeRate ok");
    println!("buyer={}bps seller={}bps", fee.buyer_commission_bps, fee.seller_commission_bps);
    // 抓包档显示 100 = 0.10%
    assert!(fee.buyer_commission_bps >= 0);
    assert!(fee.seller_commission_bps >= 0);
}
