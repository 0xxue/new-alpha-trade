//! Binance Alpha REST 客户端。
//!
//! 关键约束：
//! - 撤单走 `/bapi/defi/v1/private/...`（不是旧代码的 `asset/v1`）
//! - 下单 payload `amount` 用字符串、必填 `orderType`
//! - cookies + headers 必须由调用方注入（来源是 qr-service `/auth/{user}`）
//! - 触发 2FA 时（响应 header 有 `risk_challenge_biz_no`）自动跑 verify 流程并重试一次

use std::collections::HashMap;
use std::sync::{Arc, RwLock as StdRwLock};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, COOKIE};
use reqwest::{Client, Method, RequestBuilder, Response};
use rust_decimal::Decimal;
use tracing::{info, warn};

use crate::twofa;
use crate::types::*;

const BASE: &str = "https://www.binance.com";
const REFERER_DEFAULT: &str = "https://www.binance.com/zh-CN/alpha";
const HDR_BIZ_NO: &str = "risk_challenge_biz_no";
const HDR_ENABLE_FLOW: &str = "risk_challenge_enable_flow";

/// 一个账户的全套认证信息（来自 qr-service `/auth/{user}` 响应）。
#[derive(Debug, Clone)]
pub struct AuthBundle {
    pub username: String,
    pub cookies: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    /// 可选：2FA TOTP secret（base32），用于自动 verify
    pub twofa_secret: Option<String>,
}

impl AuthBundle {
    pub fn cookie_header(&self) -> String {
        if let Some(c) = self.headers.get("cookie") {
            return c.clone();
        }
        self.cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn from_maps(
        username: impl Into<String>,
        cookies: HashMap<String, String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self {
            username: username.into(),
            cookies,
            headers,
            twofa_secret: None,
        }
    }

    pub fn with_twofa(mut self, secret: Option<String>) -> Self {
        self.twofa_secret = secret;
        self
    }
}

#[derive(Clone)]
pub struct AlphaRest {
    http: Client,
    base: String,
    /// `username -> 最新 challenge_token`（2FA 后下次下单自动带）
    challenge_tokens: Arc<StdRwLock<HashMap<String, String>>>,
}

impl AlphaRest {
    pub fn new() -> anyhow::Result<Self> {
        let http = Client::builder()
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
            )
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            base: BASE.into(),
            challenge_tokens: Arc::new(StdRwLock::new(HashMap::new())),
        })
    }

    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    /// 构造私有请求 RequestBuilder。自动注入 cookies/headers/cached challenge_token。
    fn private(&self, method: Method, path: &str, auth: &AuthBundle) -> RequestBuilder {
        let url = format!("{}{}", self.base, path);
        let mut headers = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&auth.cookie_header()) {
            headers.insert(COOKIE, v);
        }
        for (k, v) in &auth.headers {
            if k.starts_with(':') {
                continue;
            }
            let lk = k.to_ascii_lowercase();
            if matches!(
                lk.as_str(),
                "host"
                    | "content-length"
                    | "content-type"
                    | "cookie"
                    | "connection"
                    | "accept-encoding"
                    | "x-passthrough-token" // 我们的缓存优先
            ) {
                continue;
            }
            if let (Ok(name), Ok(val)) =
                (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
            {
                headers.insert(name, val);
            }
        }
        if !auth.headers.contains_key("referer") {
            headers.insert(
                reqwest::header::REFERER,
                HeaderValue::from_static(REFERER_DEFAULT),
            );
        }
        // 注入缓存的 challenge_token
        if let Some(tok) = self.cached_token(&auth.username) {
            if let Ok(v) = HeaderValue::from_str(&tok) {
                headers.insert(HeaderName::from_static("x-passthrough-token"), v);
            }
        }
        self.http.request(method, url).headers(headers)
    }

    fn public(&self, method: Method, path: &str) -> RequestBuilder {
        let url = format!("{}{}", self.base, path);
        self.http.request(method, url)
    }

    fn cached_token(&self, username: &str) -> Option<String> {
        self.challenge_tokens.read().ok()?.get(username).cloned()
    }

    fn set_token(&self, username: &str, token: String) {
        if let Ok(mut g) = self.challenge_tokens.write() {
            g.insert(username.to_string(), token);
        }
    }

    #[allow(dead_code)] // 暂未使用，后续 token 失效检测会用
    fn clear_token(&self, username: &str) {
        if let Ok(mut g) = self.challenge_tokens.write() {
            g.remove(username);
        }
    }

    // ============================================================== 私有：下单（含 2FA 重试）
    pub async fn place_order(
        &self,
        auth: &AuthBundle,
        req: &PlaceOrderRequest,
    ) -> Result<String, AlphaApiError> {
        let path = "/bapi/asset/v1/private/alpha-trade/order/place";
        match self.place_order_once(auth, req, path).await {
            // 2FA 需求：自动尝试一次
            Err(TwofaNeeded { biz_no }) => {
                let token = self.run_twofa(auth, &biz_no).await?;
                self.set_token(&auth.username, token);
                info!(user = %auth.username, "2FA done, retrying place_order");
                self.place_order_once(auth, req, path).await.map_err(unwrap_inner)
            }
            Ok(id) => Ok(id),
            Err(InnerErr::Api(e)) => Err(e),
        }
    }

    async fn place_order_once(
        &self,
        auth: &AuthBundle,
        req: &PlaceOrderRequest,
        path: &str,
    ) -> Result<String, InnerErr> {
        let resp = self.private(Method::POST, path, auth).json(req).send().await
            .map_err(|e| InnerErr::Api(AlphaApiError::Http(e)))?;
        if let Some(biz_no) = detect_twofa(&resp) {
            return Err(InnerErr::TwofaNeeded { biz_no });
        }
        let env: PlaceOrderResponse = resp
            .json()
            .await
            .map_err(|e| InnerErr::Api(AlphaApiError::Decode(e.to_string())))?;
        env.into_data().map_err(InnerErr::Api)
    }

    pub async fn place_oto_order(
        &self,
        auth: &AuthBundle,
        req: &PlaceOtoOrderRequest,
    ) -> Result<PlaceOtoData, AlphaApiError> {
        let path = "/bapi/asset/v1/private/alpha-trade/oto-order/place";
        match self.place_oto_once(auth, req, path).await {
            Ok(d) => Ok(d),
            Err(InnerErr::TwofaNeeded { biz_no }) => {
                let token = self.run_twofa(auth, &biz_no).await?;
                self.set_token(&auth.username, token);
                info!(user = %auth.username, "2FA done, retrying place_oto_order");
                self.place_oto_once(auth, req, path).await.map_err(unwrap_inner)
            }
            Err(InnerErr::Api(e)) => Err(e),
        }
    }

    async fn place_oto_once(
        &self,
        auth: &AuthBundle,
        req: &PlaceOtoOrderRequest,
        path: &str,
    ) -> Result<PlaceOtoData, InnerErr> {
        // V2.tune14 debug：失败时 dump request body + response body 到 log，定位 B2 decode error
        let req_body = serde_json::to_string(req).unwrap_or_default();
        let resp = self.private(Method::POST, path, auth).json(req).send().await
            .map_err(|e| InnerErr::Api(AlphaApiError::Http(e)))?;
        if let Some(biz_no) = detect_twofa(&resp) {
            return Err(InnerErr::TwofaNeeded { biz_no });
        }
        let status = resp.status();
        let text = resp.text().await
            .map_err(|e| InnerErr::Api(AlphaApiError::Decode(format!("read body: {e}"))))?;
        match serde_json::from_str::<PlaceOtoResponse>(&text) {
            Ok(env) => env.into_data().map_err(InnerErr::Api),
            Err(e) => {
                // 错误时 dump 详情，文本截 500 字符防爆
                let preview: String = text.chars().take(500).collect();
                tracing::warn!(
                    %status, parse_err = %e, req = %req_body, resp_body = %preview,
                    "place_oto_once decode failed"
                );
                Err(InnerErr::Api(AlphaApiError::Decode(format!(
                    "{e} | body[0..200]: {}",
                    text.chars().take(200).collect::<String>()
                ))))
            }
        }
    }

    async fn run_twofa(&self, auth: &AuthBundle, biz_no: &str) -> Result<String, AlphaApiError> {
        let secret = auth
            .twofa_secret
            .as_deref()
            .ok_or_else(|| AlphaApiError::Server {
                code: "no-2fa".into(),
                message: format!(
                    "2FA required (biz_no={biz_no}) but account {} has no 2fa_secret on file",
                    auth.username
                ),
                detail: None,
            })?;
        twofa::run_2fa_flow(&self.http, &self.base, auth, biz_no, secret)
            .await
            .map_err(|e| {
                // 区分两类失败：
                //   - extra-verify-required: 币安要求人脸/手机，TOTP 救不了 → 用专用 code
                //     让上层策略识别并 pause job（不要 retry，retry 会触发账户风控）
                //   - 其它（错码 / 网络 / 解码） → 通用 2fa-failed code
                let code = match &e {
                    twofa::TwofaError::ExtraVerificationRequired { .. } => "2fa-extra-verify-required",
                    _ => "2fa-failed",
                };
                AlphaApiError::Server {
                    code: code.into(),
                    message: e.to_string(),
                    detail: None,
                }
            })
    }

    // ============================================================== 私有：撤单
    pub async fn cancel_order(
        &self,
        auth: &AuthBundle,
        req: &CancelOrderRequest,
    ) -> Result<(), AlphaApiError> {
        let resp = self
            .private(
                Method::POST,
                "/bapi/defi/v1/private/alpha-trade/order/cancel",
                auth,
            )
            .json(req)
            .send()
            .await?;
        let env: CancelOrderResponse =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        if !env.success {
            return Err(AlphaApiError::Server {
                code: env.code,
                message: env.message.unwrap_or_default(),
                detail: env.message_detail,
            });
        }
        Ok(())
    }

    // ============================================================== 私有：查询挂单
    pub async fn get_open_orders(
        &self,
        auth: &AuthBundle,
        side: Option<Side>,
    ) -> Result<Vec<OpenOrder>, AlphaApiError> {
        let mut url = "/bapi/defi/v1/private/alpha-trade/order/get-open-order".to_string();
        if let Some(s) = side {
            let s_str = match s {
                Side::Buy => "BUY",
                Side::Sell => "SELL",
            };
            url.push_str("?side=");
            url.push_str(s_str);
        } else {
            url.push_str("?side=");
        }
        let resp = self.private(Method::GET, &url, auth).send().await?;
        let env: ApiEnvelope<Vec<OpenOrder>> =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        Ok(env.into_data().unwrap_or_default())
    }

    pub async fn get_user_trades(
        &self,
        auth: &AuthBundle,
        order_id: &str,
        symbol: &str,
    ) -> Result<Vec<TradeFill>, AlphaApiError> {
        let path = format!(
            "/bapi/defi/v1/private/alpha-trade/order/get-user-trades?orderId={order_id}&symbol={symbol}"
        );
        let resp = self.private(Method::GET, &path, auth).send().await?;
        let env: ApiEnvelope<Vec<TradeFill>> =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        Ok(env.into_data().unwrap_or_default())
    }

    pub async fn get_spot_wallet(&self, auth: &AuthBundle) -> Result<SpotWallet, AlphaApiError> {
        let payload = serde_json::json!({
            "includeWallets": ["CARD", "MAIN", "SAVING"],
            "includeEq": true,
        });
        let resp = self
            .private(
                Method::POST,
                "/bapi/asset/v3/private/asset-service/asset/get-wallet-asset",
                auth,
            )
            .json(&payload)
            .send()
            .await?;
        let env: ApiEnvelope<SpotWallet> =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        env.into_data()
    }

    pub async fn get_alpha_wallet(
        &self,
        auth: &AuthBundle,
    ) -> Result<AlphaWallet, AlphaApiError> {
        let resp = self
            .private(
                Method::GET,
                "/bapi/defi/v1/private/wallet-direct/cloud-wallet/alpha",
                auth,
            )
            .send()
            .await?;
        let env: ApiEnvelope<AlphaWallet> =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        env.into_data()
    }

    // ============================================================== 公开
    pub async fn get_full_depth(
        &self,
        symbol: &str,
        limit: u32,
    ) -> Result<OrderBookSnapshot, AlphaApiError> {
        let path = format!(
            "/bapi/defi/v1/public/alpha-trade/fullDepth?symbol={symbol}&limit={limit}"
        );
        let resp = self.public(Method::GET, &path).send().await?;
        let env: ApiEnvelope<OrderBookSnapshot> =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        env.into_data()
    }

    pub async fn get_exchange_info(&self) -> Result<ExchangeInfo, AlphaApiError> {
        let resp = self
            .public(Method::GET, "/bapi/defi/v1/public/alpha-trade/get-exchange-info")
            .send()
            .await?;
        let env: ApiEnvelope<ExchangeInfo> =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        env.into_data()
    }

    // ============================================================== 私有：listen-key（user stream）
    /// 申请新的 listen key（POST，旧的会被新 key 替换/失效，币安行为）。
    pub async fn create_listen_key(&self, auth: &AuthBundle) -> Result<String, AlphaApiError> {
        let path = "/bapi/defi/v1/private/alpha-trade/get-listen-key";
        let resp = self.private(Method::POST, path, auth).send().await?;
        let env: ApiEnvelope<String> =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        env.into_data()
    }

    /// 续期 listen key（PUT）。币安通常 25 分钟内续期一次保活。
    pub async fn keepalive_listen_key(&self, auth: &AuthBundle) -> Result<(), AlphaApiError> {
        let path = "/bapi/defi/v1/private/alpha-trade/get-listen-key";
        let resp = self.private(Method::PUT, path, auth).send().await?;
        // 抓包档没给响应格式，假设是标准 ApiEnvelope
        let env: ApiEnvelope<serde_json::Value> =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        if !env.success {
            return Err(AlphaApiError::Server {
                code: env.code,
                message: env.message.unwrap_or_default(),
                detail: env.message_detail,
            });
        }
        Ok(())
    }

    /// 拉所有 Alpha 代币的 24h ticker — 是 symbol↔alphaId 映射的主数据源。
    pub async fn get_agg_ticker24(&self) -> Result<Vec<AggTicker24Entry>, AlphaApiError> {
        let path = "/bapi/defi/v1/public/alpha-trade/aggTicker24?dataType=aggregate";
        let resp = self.public(Method::GET, path).send().await?;
        let env: ApiEnvelope<Vec<AggTicker24Entry>> =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        Ok(env.into_data().unwrap_or_default())
    }

    pub async fn get_fee_rate(&self, symbol: &str) -> Result<FeeRate, AlphaApiError> {
        let path = format!("/bapi/defi/v1/public/alpha-trade/get-fee-rate?symbol={symbol}");
        let resp = self.public(Method::GET, &path).send().await?;
        let env: ApiEnvelope<FeeRate> =
            resp.json().await.map_err(|e| AlphaApiError::Decode(e.to_string()))?;
        env.into_data()
    }
}

impl Default for AlphaRest {
    fn default() -> Self {
        Self::new().expect("default reqwest client")
    }
}

// ---------- 内部错误 + 2FA 检测 ----------

enum InnerErr {
    TwofaNeeded { biz_no: String },
    Api(AlphaApiError),
}

use InnerErr::TwofaNeeded;

fn unwrap_inner(e: InnerErr) -> AlphaApiError {
    match e {
        InnerErr::Api(a) => a,
        InnerErr::TwofaNeeded { biz_no } => AlphaApiError::Server {
            code: "2fa-loop".into(),
            message: format!("2FA required again after retry (biz_no={biz_no})"),
            detail: None,
        },
    }
}

/// 看响应 header，判断是不是触发了 2FA。
fn detect_twofa(resp: &Response) -> Option<String> {
    let h = resp.headers();
    let enable = h
        .get(HDR_ENABLE_FLOW)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let biz_no = h.get(HDR_BIZ_NO).and_then(|v| v.to_str().ok())?.to_string();
    if enable && !biz_no.is_empty() {
        warn!(%biz_no, "2FA flow required");
        Some(biz_no)
    } else {
        None
    }
}

pub type SharedAlphaRest = Arc<AlphaRest>;

pub fn round_to_step(qty: Decimal, step: Decimal) -> Decimal {
    if step.is_zero() {
        return qty;
    }
    let n = (qty / step).floor();
    n * step
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn round_to_step_basic() {
        assert_eq!(round_to_step(dec!(1685487.95), dec!(0.10)), dec!(1685487.9));
        assert_eq!(round_to_step(dec!(0.099), dec!(0.10)), dec!(0));
        assert_eq!(round_to_step(dec!(10), dec!(0.5)), dec!(10));
    }

    #[test]
    fn auth_bundle_cookie_header_from_map() {
        let mut cookies = HashMap::new();
        cookies.insert("a".to_string(), "1".to_string());
        cookies.insert("b".to_string(), "2".to_string());
        let auth = AuthBundle::from_maps("u", cookies, HashMap::new());
        let h = auth.cookie_header();
        assert!(h.contains("a=1"));
        assert!(h.contains("b=2"));
    }

    #[test]
    fn auth_bundle_cookie_header_prefers_raw() {
        let mut headers = HashMap::new();
        headers.insert("cookie".into(), "session=raw; foo=bar".into());
        let auth = AuthBundle::from_maps("u", HashMap::new(), headers);
        assert_eq!(auth.cookie_header(), "session=raw; foo=bar");
    }

    #[test]
    fn token_cache_per_user() {
        let r = AlphaRest::new().unwrap();
        assert!(r.cached_token("alice").is_none());
        r.set_token("alice", "tok1".into());
        assert_eq!(r.cached_token("alice").unwrap(), "tok1");
        r.set_token("bob", "tok2".into());
        assert_eq!(r.cached_token("bob").unwrap(), "tok2");
        r.clear_token("alice");
        assert!(r.cached_token("alice").is_none());
        assert_eq!(r.cached_token("bob").unwrap(), "tok2");
    }
}
