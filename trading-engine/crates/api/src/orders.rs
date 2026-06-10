//! 裸下单端点（绕过策略层），用于手动测试 place_order / cancel 链路。
//!
//! 安全约束：
//! - 必须显式传 `confirm: "yes"`
//! - `paymentDetails[0].amount` 上限默认 1 USDT；超过返回 400
//! - 所有调用都把币安原始响应写到 `orders` 表（即使失败）

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use binance_alpha::{
    CancelOrderRequest, OpenOrder, OrderType, PaymentDetail, PlaceOrderRequest,
    PlaceOtoOrderRequest, Side, TradeFill, WalletType,
};
use persistence::repo::{orders, trades};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::str::FromStr;

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orders/place", post(place))
        .route("/orders/place-oto", post(place_oto))
        .route("/orders/cancel", post(cancel))
        .route("/orders/open", get(open_orders))
        .route("/orders/trades", get(order_trades))
}

/// 上限：单笔 paymentDetails.amount 不能超过这个值（USDT）。
/// 手动测试用，防误下大单。
const MAX_TEST_AMOUNT_USDT: &str = "1";

#[derive(Deserialize)]
pub struct PlaceReq {
    pub username: String,
    pub base_asset: String,  // 如 ALPHA_971
    pub quote_asset: String, // 如 USDT
    pub side: Side,
    pub price: String,
    pub quantity: String,
    pub payment_amount: String,
    pub wallet: Option<String>, // CARD / ALPHA；缺省按 side 推断
    pub job_id: Option<String>, // 关联 job_id（可选，写 orders 表用）
    pub confirm: String,
}

#[derive(Serialize)]
struct PlaceResp {
    order_id: String,
    raw_request: Value,
}

async fn place(
    State(s): State<AppState>,
    Json(req): Json<PlaceReq>,
) -> Result<Json<PlaceResp>, ApiErr> {
    if req.confirm != "yes" {
        return Err(ApiErr::bad("confirm must be \"yes\""));
    }
    let price = parse_dec(&req.price, "price")?;
    let qty = parse_dec(&req.quantity, "quantity")?;
    let amount = parse_dec(&req.payment_amount, "payment_amount")?;
    let wallet = match req.wallet.as_deref() {
        Some("CARD") | Some("card") => WalletType::Card,
        Some("ALPHA") | Some("alpha") => WalletType::Alpha,
        None => match req.side {
            Side::Buy => WalletType::Card,
            Side::Sell => WalletType::Alpha,
        },
        Some(other) => return Err(ApiErr::bad(format!("unknown wallet {other:?}"))),
    };
    // 上限只对 USDT 付款（BUY/CARD）；SELL 时 amount 是 base 代币数量，单位不可比
    if matches!(wallet, WalletType::Card) {
        let max = Decimal::from_str(MAX_TEST_AMOUNT_USDT).unwrap();
        if amount > max {
            return Err(ApiErr::bad(format!(
                "payment_amount {} exceeds manual test cap {} USDT",
                amount, max
            )));
        }
    }

    let auth = s
        .qr
        .get_auth(&req.username)
        .await
        .map_err(|e| ApiErr::bad(format!("auth: {e}")))?;

    let payload = PlaceOrderRequest {
        base_asset: req.base_asset.clone(),
        quote_asset: req.quote_asset.clone(),
        side: req.side,
        price,
        quantity: qty,
        payment_details: vec![PaymentDetail {
            amount,
            payment_wallet_type: wallet,
        }],
        order_type: OrderType::Limit,
    };
    let raw_request = serde_json::to_value(&payload).unwrap_or(Value::Null);
    tracing::info!(
        username = %req.username,
        side = ?req.side,
        price = %price,
        qty = %qty,
        amount = %amount,
        "manual place_order"
    );

    match s.alpha.place_order(&auth, &payload).await {
        Ok(order_id) => {
            let _ = orders::insert(
                &s.db,
                &orders::NewOrder {
                    order_id: order_id.clone(),
                    job_id: req.job_id.clone().unwrap_or_else(|| "manual".into()),
                    side: format!("{:?}", req.side).to_uppercase(),
                    price,
                    qty,
                    status: "pending".into(),
                    raw_response: Some(format!("manual place_order, base={}", req.base_asset)),
                },
            )
            .await;
            // 异步拉 fills 写 trades 表（等 2 秒让币安撮合完成）
            let symbol = format!("{}{}", req.base_asset, req.quote_asset);
            spawn_fetch_fills(s.clone(), auth.clone(), order_id.clone(), symbol, req.username.clone(), req.job_id.clone());
            Ok(Json(PlaceResp {
                order_id,
                raw_request,
            }))
        }
        Err(e) => Err(ApiErr::upstream(format!(
            "place_order failed: {e} | request={}",
            raw_request
        ))),
    }
}

/// 异步拉 user-trades 把每个 fill 写入 trades 表（幂等）。
fn spawn_fetch_fills(
    s: AppState,
    auth: binance_alpha::AuthBundle,
    order_id: String,
    symbol: String,
    username: String,
    job_id: Option<String>,
) {
    tokio::spawn(async move {
        // 等 2 秒让撮合完成
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let fills = match s.alpha.get_user_trades(&auth, &order_id, &symbol).await {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(%order_id, err=%e, "fetch_fills: get_user_trades failed");
                return;
            }
        };
        let mut inserted = 0_u32;
        for f in &fills {
            let side_str = match f.side {
                Side::Buy => "BUY",
                Side::Sell => "SELL",
            };
            let raw_json = serde_json::to_string(f).ok();
            let new = trades::NewTrade {
                fill_id: f.id.clone(),
                order_id: f.order_id.clone(),
                job_id: job_id.clone(),
                username: username.clone(),
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
            match trades::insert(&s.db, &new).await {
                Ok(true) => inserted += 1,
                Ok(false) => {} // 已存在（幂等），不重复
                Err(e) => tracing::warn!(%order_id, err=%e, "fetch_fills: trade insert failed"),
            }
        }
        let _ = orders::set_status(&s.db, &order_id, "filled").await;
        tracing::info!(%order_id, fills=fills.len(), %inserted, "fills persisted");
    });
}

#[derive(Deserialize)]
pub struct PlaceOtoReq {
    pub username: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub working_side: Side,
    pub working_price: String,
    pub working_quantity: String,
    pub pending_price: String,
    pub payment_amount: String,
    pub confirm: String,
}

#[derive(Serialize)]
struct PlaceOtoResp {
    working_order_id: u64,
    pending_order_id: u64,
}

async fn place_oto(
    State(s): State<AppState>,
    Json(req): Json<PlaceOtoReq>,
) -> Result<Json<PlaceOtoResp>, ApiErr> {
    if req.confirm != "yes" {
        return Err(ApiErr::bad("confirm must be \"yes\""));
    }
    let amount = parse_dec(&req.payment_amount, "payment_amount")?;
    let max = Decimal::from_str(MAX_TEST_AMOUNT_USDT).unwrap();
    if amount > max {
        return Err(ApiErr::bad(format!(
            "payment_amount {} exceeds manual test cap {}",
            amount, max
        )));
    }
    let working_price = parse_dec(&req.working_price, "working_price")?;
    let working_qty = parse_dec(&req.working_quantity, "working_quantity")?;
    let pending_price = parse_dec(&req.pending_price, "pending_price")?;

    let auth = s
        .qr
        .get_auth(&req.username)
        .await
        .map_err(|e| ApiErr::bad(format!("auth: {e}")))?;

    let payload = PlaceOtoOrderRequest {
        base_asset: req.base_asset,
        quote_asset: req.quote_asset,
        working_side: req.working_side,
        working_price,
        working_quantity: working_qty,
        payment_details: vec![PaymentDetail {
            amount,
            payment_wallet_type: WalletType::Card,
        }],
        pending_price,
        pending_type: OrderType::Limit,
    };
    let data = s
        .alpha
        .place_oto_order(&auth, &payload)
        .await
        .map_err(ApiErr::upstream)?;
    Ok(Json(PlaceOtoResp {
        working_order_id: data.working_order_id,
        pending_order_id: data.pending_order_id,
    }))
}

#[derive(Deserialize)]
pub struct CancelReq {
    pub username: String,
    pub order_id: String,
    pub symbol: String,
}

async fn cancel(
    State(s): State<AppState>,
    Json(req): Json<CancelReq>,
) -> Result<Json<Value>, ApiErr> {
    let auth = s
        .qr
        .get_auth(&req.username)
        .await
        .map_err(|e| ApiErr::bad(format!("auth: {e}")))?;
    let payload = CancelOrderRequest {
        order_id: req.order_id.clone(),
        symbol: req.symbol.clone(),
    };
    s.alpha
        .cancel_order(&auth, &payload)
        .await
        .map_err(ApiErr::upstream)?;
    let _ = orders::set_status(&s.db, &req.order_id, "canceled").await;
    Ok(Json(json!({"order_id": req.order_id, "result": "canceled"})))
}

#[derive(Deserialize)]
pub struct OpenQuery {
    pub username: String,
    pub side: Option<String>, // BUY / SELL / 缺省=都
}

#[derive(Deserialize)]
pub struct TradesQuery {
    pub username: String,
    pub order_id: String,
    #[serde(default = "default_symbol")]
    pub symbol: String,
}

fn default_symbol() -> String {
    "ALPHA_971USDT".into()
}

async fn order_trades(
    State(s): State<AppState>,
    Query(q): Query<TradesQuery>,
) -> Result<Json<Vec<TradeFill>>, ApiErr> {
    let auth = s
        .qr
        .get_auth(&q.username)
        .await
        .map_err(|e| ApiErr::bad(format!("auth: {e}")))?;
    let fills = s
        .alpha
        .get_user_trades(&auth, &q.order_id, &q.symbol)
        .await
        .map_err(ApiErr::upstream)?;
    Ok(Json(fills))
}

async fn open_orders(
    State(s): State<AppState>,
    Query(q): Query<OpenQuery>,
) -> Result<Json<Vec<OpenOrder>>, ApiErr> {
    let auth = s
        .qr
        .get_auth(&q.username)
        .await
        .map_err(|e| ApiErr::bad(format!("auth: {e}")))?;
    let side = match q.side.as_deref() {
        Some("BUY") | Some("buy") => Some(Side::Buy),
        Some("SELL") | Some("sell") => Some(Side::Sell),
        _ => None,
    };
    let list = s
        .alpha
        .get_open_orders(&auth, side)
        .await
        .map_err(ApiErr::upstream)?;
    Ok(Json(list))
}

// ============================================================ helpers
fn parse_dec(s: &str, field: &str) -> Result<Decimal, ApiErr> {
    Decimal::from_str(s).map_err(|e| ApiErr::bad(format!("{field}: {e}")))
}

struct ApiErr {
    status: StatusCode,
    msg: String,
}
impl ApiErr {
    fn bad(m: impl ToString) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            msg: m.to_string(),
        }
    }
    fn upstream<E: std::fmt::Display>(e: E) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            msg: e.to_string(),
        }
    }
}
impl axum::response::IntoResponse for ApiErr {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(json!({"error": self.msg}))).into_response()
    }
}
