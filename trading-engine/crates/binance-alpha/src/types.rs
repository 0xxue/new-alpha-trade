//! Alpha 接口的共享类型。
//!
//! 设计原则：
//! - 价格 / 数量 / 金额 全部用 `rust_decimal::Decimal`（绝不 f64）
//! - 写入 payload 时 amount 字段走字符串序列化（抓包档证实币安网页这么做）
//! - 解析响应时所有数值字段也用字符串接（币安返回字符串）
//! - 字段命名按抓包档原样保留（camelCase）

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// =======================================================================
// 通用响应包装
// =======================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct ApiEnvelope<T> {
    pub code: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default, rename = "messageDetail")]
    pub message_detail: Option<String>,
    #[serde(default)]
    pub success: bool,
    pub data: Option<T>,
}

impl<T> ApiEnvelope<T> {
    pub fn into_data(self) -> Result<T, AlphaApiError> {
        if !self.success {
            return Err(AlphaApiError::Server {
                code: self.code,
                message: self.message.unwrap_or_default(),
                detail: self.message_detail,
            });
        }
        self.data.ok_or(AlphaApiError::EmptyData)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AlphaApiError {
    #[error("server rejected: code={code} message={message}")]
    Server {
        code: String,
        message: String,
        detail: Option<String>,
    },
    #[error("empty data field")]
    EmptyData,
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("decode error: {0}")]
    Decode(String),
}

// =======================================================================
// 订单方向 / 类型 / 钱包
// =======================================================================

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderType {
    Limit,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum WalletType {
    /// 买入时付款钱包
    Card,
    /// 卖出时持仓钱包
    Alpha,
}

// =======================================================================
// 下单请求
// =======================================================================

/// `POST /bapi/asset/v1/private/alpha-trade/order/place` payload。
///
/// 必填 `order_type`。`payment_details[0].amount` 写成字符串。
#[derive(Debug, Clone, Serialize)]
pub struct PlaceOrderRequest {
    #[serde(rename = "baseAsset")]
    pub base_asset: String,
    #[serde(rename = "quoteAsset")]
    pub quote_asset: String,
    pub side: Side,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub quantity: Decimal,
    #[serde(rename = "paymentDetails")]
    pub payment_details: Vec<PaymentDetail>,
    #[serde(rename = "orderType")]
    pub order_type: OrderType,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaymentDetail {
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
    #[serde(rename = "paymentWalletType")]
    pub payment_wallet_type: WalletType,
}

/// `POST /bapi/asset/v1/private/alpha-trade/oto-order/place` payload。
#[derive(Debug, Clone, Serialize)]
pub struct PlaceOtoOrderRequest {
    #[serde(rename = "baseAsset")]
    pub base_asset: String,
    #[serde(rename = "quoteAsset")]
    pub quote_asset: String,
    #[serde(rename = "workingSide")]
    pub working_side: Side,
    #[serde(rename = "workingPrice", with = "rust_decimal::serde::str")]
    pub working_price: Decimal,
    #[serde(rename = "workingQuantity", with = "rust_decimal::serde::str")]
    pub working_quantity: Decimal,
    #[serde(rename = "paymentDetails")]
    pub payment_details: Vec<PaymentDetail>,
    #[serde(rename = "pendingPrice", with = "rust_decimal::serde::str")]
    pub pending_price: Decimal,
    #[serde(rename = "pendingType")]
    pub pending_type: OrderType,
}

/// 普通单响应：`data` = 订单 ID 字符串
pub type PlaceOrderResponse = ApiEnvelope<String>;

/// OTO 单响应：`data` = 两个 ID
#[derive(Debug, Clone, Deserialize)]
pub struct PlaceOtoData {
    #[serde(rename = "workingOrderId")]
    pub working_order_id: u64,
    #[serde(rename = "pendingOrderId")]
    pub pending_order_id: u64,
}
pub type PlaceOtoResponse = ApiEnvelope<PlaceOtoData>;

// =======================================================================
// 撤单
// =======================================================================

/// `POST /bapi/defi/v1/private/alpha-trade/order/cancel`
/// 注意：抓包确认路径在 `defi/v1`，不是 `asset/v1`。
#[derive(Debug, Clone, Serialize)]
pub struct CancelOrderRequest {
    #[serde(rename = "orderId")]
    pub order_id: String,
    pub symbol: String,
}

/// 撤单响应：`data` 为 null（抓包确认），用 `()` 接
pub type CancelOrderResponse = ApiEnvelope<serde_json::Value>;

// =======================================================================
// 当前挂单
// =======================================================================

/// `GET /bapi/defi/v1/private/alpha-trade/order/get-open-order?side=BUY|SELL`
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenOrder {
    #[serde(rename = "orderId")]
    pub order_id: String,
    pub symbol: String,
    pub side: Side,
    #[serde(rename = "type")]
    pub order_type: String,
    pub status: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(rename = "origQty", with = "rust_decimal::serde::str")]
    pub orig_qty: Decimal,
    #[serde(rename = "executedQty", with = "rust_decimal::serde::str")]
    pub executed_qty: Decimal,
    #[serde(rename = "cumQuote", default, with = "rust_decimal::serde::str_option")]
    pub cum_quote: Option<Decimal>,
    #[serde(default)]
    pub time: Option<i64>,
    #[serde(default, rename = "updateTime")]
    pub update_time: Option<i64>,
    #[serde(default, rename = "baseAsset")]
    pub base_asset: Option<String>,
    #[serde(default, rename = "quoteAsset")]
    pub quote_asset: Option<String>,
    #[serde(default, rename = "alphaId")]
    pub alpha_id: Option<String>,
}

// =======================================================================
// 订单簿 fullDepth
// =======================================================================

/// `GET /bapi/defi/v1/public/alpha-trade/fullDepth?symbol=...&limit=1000`
///
/// 注意：bids/asks 是 [price, qty] 二元元组，全部字符串。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrderBookSnapshot {
    #[serde(rename = "lastUpdateId")]
    pub last_update_id: u64,
    pub symbol: String,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
    #[serde(rename = "E", default)]
    pub event_time: Option<i64>,
    #[serde(rename = "T", default)]
    pub transact_time: Option<i64>,
}

// =======================================================================
// 交易对元信息 + 过滤器（必须遵守！）
// =======================================================================

/// `GET /bapi/defi/v1/public/alpha-trade/get-exchange-info`
#[derive(Debug, Clone, Deserialize)]
pub struct ExchangeInfo {
    pub symbols: Vec<SymbolInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SymbolInfo {
    pub symbol: String,
    pub status: String,
    #[serde(rename = "baseAsset")]
    pub base_asset: String,
    #[serde(rename = "quoteAsset")]
    pub quote_asset: String,
    #[serde(rename = "pricePrecision")]
    pub price_precision: u32,
    #[serde(rename = "quantityPrecision")]
    pub quantity_precision: u32,
    #[serde(rename = "baseAssetPrecision", default)]
    pub base_asset_precision: u32,
    #[serde(rename = "quotePrecision", default)]
    pub quote_precision: u32,
    pub filters: Vec<SymbolFilter>,
    #[serde(rename = "orderTypes")]
    pub order_types: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "filterType")]
pub enum SymbolFilter {
    #[serde(rename = "PRICE_FILTER")]
    PriceFilter {
        #[serde(rename = "minPrice", with = "rust_decimal::serde::str")]
        min_price: Decimal,
        #[serde(rename = "maxPrice", with = "rust_decimal::serde::str")]
        max_price: Decimal,
        #[serde(rename = "tickSize", with = "rust_decimal::serde::str")]
        tick_size: Decimal,
    },
    #[serde(rename = "LOT_SIZE")]
    LotSize {
        #[serde(rename = "stepSize", with = "rust_decimal::serde::str")]
        step_size: Decimal,
        #[serde(rename = "minQty", with = "rust_decimal::serde::str")]
        min_qty: Decimal,
        #[serde(rename = "maxQty", with = "rust_decimal::serde::str")]
        max_qty: Decimal,
    },
    #[serde(rename = "MIN_NOTIONAL")]
    MinNotional {
        #[serde(rename = "minNotional", with = "rust_decimal::serde::str")]
        min_notional: Decimal,
    },
    #[serde(other)]
    Unknown,
}

// =======================================================================
// 手续费率
// =======================================================================

/// `GET /bapi/defi/v1/public/alpha-trade/get-fee-rate?symbol=...`
///
/// 抓包看 `buyerCommission: 100` —— 单位是基点的 1/100，所以 100 = 0.10%。
#[derive(Debug, Clone, Deserialize)]
pub struct FeeRate {
    #[serde(rename = "buyerCommission")]
    pub buyer_commission_bps: i64,
    #[serde(rename = "sellerCommission")]
    pub seller_commission_bps: i64,
}

// =======================================================================
// Alpha 钱包余额
// =======================================================================

/// `GET /bapi/defi/v1/private/wallet-direct/cloud-wallet/alpha`
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AlphaWallet {
    #[serde(rename = "totalValuation", default, with = "rust_decimal::serde::str_option")]
    pub total_valuation: Option<Decimal>,
    #[serde(default)]
    pub list: Vec<WalletEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WalletEntry {
    #[serde(rename = "chainId")]
    pub chain_id: String,
    #[serde(rename = "contractAddress")]
    pub contract_address: String,
    pub name: String,
    pub symbol: String,
    #[serde(rename = "tokenId")]
    pub token_id: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub free: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub freeze: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub valuation: Option<Decimal>,
}

// =======================================================================
// aggTicker24 — 所有 Alpha 代币列表（symbol → alphaId 映射来源）
// =======================================================================

/// `GET /bapi/defi/v1/public/alpha-trade/aggTicker24?dataType=aggregate` 返回 data 是数组。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AggTicker24Entry {
    #[serde(rename = "tokenId")]
    pub token_id: String,
    #[serde(rename = "chainId")]
    pub chain_id: String,
    #[serde(rename = "contractAddress")]
    pub contract_address: String,
    pub name: String,
    pub symbol: String,
    #[serde(rename = "alphaId")]
    pub alpha_id: String,
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub price: Option<Decimal>,
    #[serde(default)]
    pub decimals: u32,
    #[serde(rename = "tradeDecimal", default)]
    pub trade_decimal: u32,
    /// 不能下卖单
    #[serde(default)]
    pub offsell: bool,
    /// 整个代币下线
    #[serde(default)]
    pub offline: bool,
    /// 暂停交易
    #[serde(rename = "stockState", default)]
    pub stock_state: bool,
}

impl AggTicker24Entry {
    /// 是否当前可下单（不下线 + 不停盘）
    pub fn tradable(&self) -> bool {
        !self.offline && !self.stock_state
    }
    /// 拼接 symbol 对儿，例如 ALPHA_971USDT
    pub fn pair_symbol(&self) -> String {
        format!("{}USDT", self.alpha_id)
    }
}

// =======================================================================
// SPOT 钱包（wear 计算的关键，CARD 实际对应 funding bucket）
// =======================================================================

/// `POST /bapi/asset/v3/private/asset-service/asset/get-wallet-asset` 返回值是数组。
pub type SpotWallet = Vec<SpotAssetEntry>;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpotAssetEntry {
    pub asset: String,
    #[serde(default, rename = "coinBusinessType")]
    pub coin_business_type: Option<String>,
    #[serde(default)]
    pub spot: Option<WalletBucket>,
    #[serde(default)]
    pub funding: Option<WalletBucket>,
    #[serde(default)]
    pub earn: Option<WalletBucket>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WalletBucket {
    #[serde(with = "rust_decimal::serde::str")]
    pub free: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub locked: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub freeze: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub withdrawing: Decimal,
}

impl WalletBucket {
    pub fn total(&self) -> Decimal {
        self.free + self.locked + self.freeze + self.withdrawing
    }
}

/// 帮 wear 计算用：取出 USDT 在 funding wallet（CARD）的 free 余额。
pub fn usdt_funding_free(wallet: &SpotWallet) -> Option<Decimal> {
    wallet
        .iter()
        .find(|e| e.asset == "USDT")
        .and_then(|e| e.funding.as_ref().map(|b| b.free))
}

/// wear 计算用：USDT 跨 spot + funding + earn 三个 bucket 的 free 总和。
///
/// 币安会把资金账户(funding)里闲置的 USDT **自动申购**进理财(flexible earn)。
/// 旧的 `usdt_funding_free` 只看 funding，这种 funding→earn 的"搬家"会被 wear 误判成
/// 交易亏损（实测 QAIT job：funding 252→218，~33 USDT 进 earn，wear 假算 -429 bps →
/// 风控连续触发自动暂停）。baseline 与 current 都改用此口径后，earn 大额存量在两边抵消，
/// wear 只反映真实交易盈亏。
///
/// 返回 None 仅当钱包里完全没有 USDT 条目。
pub fn usdt_total_free(wallet: &SpotWallet) -> Option<Decimal> {
    wallet.iter().find(|e| e.asset == "USDT").map(|e| {
        let pick = |b: &Option<WalletBucket>| b.as_ref().map(|x| x.free).unwrap_or(Decimal::ZERO);
        pick(&e.spot) + pick(&e.funding) + pick(&e.earn)
    })
}

// =======================================================================
// 成交回执（user-trades，wear 的明细凭证）
// =======================================================================

/// `GET /bapi/defi/v1/private/alpha-trade/order/get-user-trades?orderId=...&symbol=...`
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TradeFill {
    pub symbol: String,
    pub id: String,
    #[serde(rename = "orderId")]
    pub order_id: String,
    #[serde(default, rename = "tradeId")]
    pub trade_id: Option<String>,
    pub side: Side,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub qty: Decimal,
    /// 币安给出的成交额（USDT），是权威数字 — 不要自己 price*qty
    #[serde(rename = "quoteQty", with = "rust_decimal::serde::str")]
    pub quote_qty: Decimal,
    /// 手续费。BUY 时是 base 代币（例如 NEX）；SELL 时是 quote 代币（USDT）；
    /// 如果用户开了 BNB 抵扣，会是 BNB
    #[serde(with = "rust_decimal::serde::str")]
    pub commission: Decimal,
    #[serde(rename = "commissionAsset")]
    pub commission_asset: String,
    pub time: i64,
    #[serde(default)]
    pub buyer: Option<bool>,
    #[serde(default, rename = "baseAsset")]
    pub base_asset: Option<String>,
    #[serde(default, rename = "quoteAsset")]
    pub quote_asset: Option<String>,
    #[serde(default, rename = "orderType")]
    pub order_type: Option<String>,
    #[serde(default, rename = "lastTrade")]
    pub last_trade: Option<bool>,
}

// =======================================================================
// 测试
// =======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn place_order_payload_uses_string_amount_and_order_type() {
        let req = PlaceOrderRequest {
            base_asset: "ALPHA_971".into(),
            quote_asset: "USDT".into(),
            side: Side::Buy,
            price: dec!(0.000005933),
            quantity: dec!(1685487.9),
            payment_details: vec![PaymentDetail {
                amount: dec!(9.99999971),
                payment_wallet_type: WalletType::Card,
            }],
            order_type: OrderType::Limit,
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["orderType"], "LIMIT");
        assert_eq!(v["paymentDetails"][0]["amount"], "9.99999971"); // 字符串
        assert_eq!(v["paymentDetails"][0]["paymentWalletType"], "CARD");
        assert_eq!(v["side"], "BUY");
        // 价格也是字符串
        assert_eq!(v["price"], "0.000005933");
    }

    #[test]
    fn place_order_response_decodes() {
        let raw = r#"{"code":"000000","message":null,"messageDetail":null,"data":"2459886","success":true}"#;
        let resp: PlaceOrderResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.into_data().unwrap(), "2459886");
    }

    #[test]
    fn oto_response_decodes() {
        let raw = r#"{"code":"000000","message":null,"messageDetail":null,"data":{"workingOrderId":2464786,"pendingOrderId":2464787},"success":true}"#;
        let resp: PlaceOtoResponse = serde_json::from_str(raw).unwrap();
        let data = resp.into_data().unwrap();
        assert_eq!(data.working_order_id, 2464786);
        assert_eq!(data.pending_order_id, 2464787);
    }

    #[test]
    fn user_trade_decodes_with_base_commission() {
        // 来自 2026-05-22 实测：BUY 单 commissionAsset 是 ALPHA_971（base）
        let raw = r#"{"symbol":"ALPHA_971USDT","id":"216930","orderId":"3570338","tradeId":"216930","side":"BUY","price":"0.000005665","qty":"52938","quoteQty":"0.29989377","commission":"5.29","commissionAsset":"ALPHA_971","time":1779385878732,"pageId":"59232870423","buyer":true,"baseAsset":"ALPHA_971","quoteAsset":"USDT","orderType":"LIMIT","lastTrade":false}"#;
        let t: TradeFill = serde_json::from_str(raw).unwrap();
        assert_eq!(t.side, Side::Buy);
        assert_eq!(t.quote_qty, dec!(0.29989377));
        assert_eq!(t.commission_asset, "ALPHA_971");
        assert_eq!(t.commission, dec!(5.29));
    }

    #[test]
    fn spot_wallet_usdt_funding() {
        let raw = r#"[
            {"asset":"USDT","coinBusinessType":"CRYPTO","spot":null,
             "funding":{"free":"103.20318663","locked":"0","freeze":"0","withdrawing":"0"},
             "earn":{"free":"9207.42344884","locked":"0","freeze":"0","withdrawing":"0"}},
            {"asset":"DOGE","coinBusinessType":"CRYPTO","spot":null,"funding":null,
             "earn":{"free":"0.90589984","locked":"0","freeze":"0","withdrawing":"0"}}
        ]"#;
        let w: SpotWallet = serde_json::from_str(raw).unwrap();
        assert_eq!(w.len(), 2);
        let usdt = usdt_funding_free(&w).unwrap();
        assert_eq!(usdt, dec!(103.20318663));
        // V2.tune38: total = spot(0) + funding(103.20318663) + earn(9207.42344884)
        let total = usdt_total_free(&w).unwrap();
        assert_eq!(total, dec!(9310.62663547));
    }

    #[test]
    fn exchange_info_filters_decode() {
        let raw = r#"{"symbols":[{"symbol":"ALPHA_971USDT","status":"TRADING","baseAsset":"ALPHA_971","quoteAsset":"USDT","pricePrecision":9,"quantityPrecision":2,"baseAssetPrecision":2,"quotePrecision":8,"orderTypes":["LIMIT"],"filters":[
            {"filterType":"PRICE_FILTER","minPrice":"0.000000001","maxPrice":"1000","tickSize":"0.000000001"},
            {"filterType":"LOT_SIZE","stepSize":"0.10","maxQty":"9999999999","minQty":"0.10"},
            {"filterType":"MIN_NOTIONAL","minNotional":"0.1"}
        ]}]}"#;
        let info: ExchangeInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(info.symbols.len(), 1);
        let s = &info.symbols[0];
        assert_eq!(s.symbol, "ALPHA_971USDT");
        assert_eq!(s.filters.len(), 3);
        match &s.filters[0] {
            SymbolFilter::PriceFilter { tick_size, .. } => {
                assert_eq!(*tick_size, dec!(0.000000001));
            }
            other => panic!("expected PriceFilter, got {other:?}"),
        }
        match &s.filters[1] {
            SymbolFilter::LotSize { step_size, min_qty, .. } => {
                assert_eq!(*step_size, dec!(0.10));
                assert_eq!(*min_qty, dec!(0.10));
            }
            other => panic!("expected LotSize, got {other:?}"),
        }
    }
}
