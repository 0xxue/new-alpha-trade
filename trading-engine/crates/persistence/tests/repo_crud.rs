//! 用临时 SQLite 跑 jobs/orders/trades 的 CRUD。

use persistence::repo::{jobs, orders, trades};
use rust_decimal_macros::dec;

async fn setup() -> sqlx::SqlitePool {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    drop(tmp);
    let pool = persistence::open(&path).await.expect("open db");
    sqlx::query("INSERT INTO accounts (username, cookies_json, headers_json) VALUES ('alice', '{}', '{}')")
        .execute(&pool).await.unwrap();
    // migration 0002 已 seed adaptive_maker，OR IGNORE 防止冲突
    sqlx::query("INSERT OR IGNORE INTO strategies (name, version, params_schema) VALUES ('adaptive_maker', '0.1', '{}')")
        .execute(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn jobs_insert_get_list_set_state() {
    let pool = setup().await;
    let id = "job-test-1";
    jobs::insert(&pool, &jobs::NewJob {
        id: id.into(),
        username: "alice".into(),
        symbol: "ALPHA_971USDT".into(),
        strategy: "adaptive_maker".into(),
        params_json: "{\"foo\":1}".into(),
        target_volume: dec!(16400),
    }).await.unwrap();

    let got = jobs::get(&pool, id).await.unwrap().unwrap();
    assert_eq!(got.username, "alice");
    assert_eq!(got.target_volume, "16400");
    assert_eq!(got.state, "pending");

    let changed = jobs::set_state(&pool, id, jobs::JobState::Running).await.unwrap();
    assert!(changed);
    assert_eq!(jobs::get(&pool, id).await.unwrap().unwrap().state, "running");

    assert_eq!(jobs::list(&pool, Some(jobs::JobState::Running)).await.unwrap().len(), 1);
    assert_eq!(jobs::list(&pool, Some(jobs::JobState::Stopped)).await.unwrap().len(), 0);
}

#[tokio::test]
async fn orders_crud() {
    let pool = setup().await;
    jobs::insert(&pool, &jobs::NewJob {
        id: "j2".into(),
        username: "alice".into(),
        symbol: "ALPHA_971USDT".into(),
        strategy: "adaptive_maker".into(),
        params_json: "{}".into(),
        target_volume: dec!(100),
    }).await.unwrap();

    orders::insert(&pool, &orders::NewOrder {
        order_id: "ord-1".into(),
        job_id: "j2".into(),
        side: "BUY".into(),
        price: dec!(0.0000059),
        qty: dec!(1685487.9),
        status: "pending".into(),
        raw_response: None,
    }).await.unwrap();

    assert!(orders::set_status(&pool, "ord-1", "filled").await.unwrap());
    let by_job = orders::list_by_job(&pool, "j2").await.unwrap();
    assert_eq!(by_job.len(), 1);
    assert_eq!(by_job[0].status, "filled");
}

#[tokio::test]
async fn trades_fill_aggregation() {
    let pool = setup().await;
    jobs::insert(&pool, &jobs::NewJob {
        id: "j3".into(),
        username: "alice".into(),
        symbol: "ALPHA_971USDT".into(),
        strategy: "adaptive_maker".into(),
        params_json: "{}".into(),
        target_volume: dec!(16400),
    }).await.unwrap();

    // 模拟 BUY 单 2 个 fills + SELL 单 1 个 fill
    let buy1 = trades::NewTrade {
        fill_id: "f1".into(), order_id: "buy1".into(),
        job_id: Some("j3".into()), username: "alice".into(),
        symbol: "ALPHA_971USDT".into(), side: "BUY".into(),
        price: dec!(0.000005665), qty: dec!(30000), quote_qty: dec!(0.16995),
        commission: dec!(3.0), commission_asset: "ALPHA_971".into(),
        trade_ts_ms: 1779385878000, raw_json: None,
    };
    let buy2 = trades::NewTrade {
        fill_id: "f2".into(), order_id: "buy1".into(),
        job_id: Some("j3".into()), username: "alice".into(),
        symbol: "ALPHA_971USDT".into(), side: "BUY".into(),
        price: dec!(0.000005666), qty: dec!(22938), quote_qty: dec!(0.12994),
        commission: dec!(2.29), commission_asset: "ALPHA_971".into(),
        trade_ts_ms: 1779385878500, raw_json: None,
    };
    let sell1 = trades::NewTrade {
        fill_id: "f3".into(), order_id: "sell1".into(),
        job_id: Some("j3".into()), username: "alice".into(),
        symbol: "ALPHA_971USDT".into(), side: "SELL".into(),
        price: dec!(0.000005663), qty: dec!(52932.70), quote_qty: dec!(0.29977),
        commission: dec!(0.00003), commission_asset: "USDT".into(),
        trade_ts_ms: 1779385882000, raw_json: None,
    };
    assert!(trades::insert(&pool, &buy1).await.unwrap());
    assert!(trades::insert(&pool, &buy2).await.unwrap());
    assert!(trades::insert(&pool, &sell1).await.unwrap());

    // 幂等性测试：重复插入 fill_id+symbol → 返回 false
    assert!(!trades::insert(&pool, &buy1).await.unwrap());

    // sum_buy_quote_qty 应该只算 BUY = 0.16995 + 0.12994 = 0.29989
    let vol = trades::sum_buy_quote_qty(&pool, "j3").await.unwrap();
    let expected = dec!(0.29989);
    // 因为内部走 REAL 累加有浮点误差，比较容忍 8 位
    let diff = (vol - expected).abs();
    assert!(diff < dec!(0.00000001), "vol={vol} expected={expected} diff={diff}");

    let fills = trades::count_fills(&pool, "j3").await.unwrap();
    assert_eq!(fills, 3);

    let by_order = trades::list_by_order(&pool, "buy1").await.unwrap();
    assert_eq!(by_order.len(), 2);
}
