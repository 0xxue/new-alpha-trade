//! HTTP 客户端：trading-engine 调用 qr-service `/auth/{username}` 拿凭据。
//!
//! 端点契约见 qr-service `src/qr_service/api/auth.py::AuthBundle`。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use binance_alpha::AuthBundle;
use reqwest::Client;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum QrClientError {
    #[error("account {0} not found in qr-service")]
    NotFound(String),
    #[error("qr-service http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("qr-service decode error: {0}")]
    Decode(String),
    #[error("qr-service returned status {0}")]
    Status(u16),
}

#[derive(Debug, Deserialize)]
struct AuthBundleResp {
    username: String,
    cookies: HashMap<String, String>,
    headers: HashMap<String, String>,
    #[allow(dead_code)]
    last_refresh: Option<String>,
    #[allow(dead_code)]
    expires_at_ms: Option<i64>,
    status: String,
    #[serde(default)]
    twofa_secret: Option<String>,
}

#[derive(Clone)]
pub struct QrClient {
    http: Client,
    base: String,
}

impl QrClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base: base_url.into(),
        }
    }

    /// 拿一个账户的最新凭据。账户必须处于 `active` 状态，否则视作 NotFound。
    pub async fn get_auth(&self, username: &str) -> Result<AuthBundle, QrClientError> {
        let url = format!("{}/auth/{}", self.base, urlencode(username));
        let resp = self.http.get(&url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(QrClientError::NotFound(username.to_string()));
        }
        if !resp.status().is_success() {
            return Err(QrClientError::Status(resp.status().as_u16()));
        }
        let body: AuthBundleResp = resp
            .json()
            .await
            .map_err(|e| QrClientError::Decode(e.to_string()))?;
        if body.status != "active" {
            return Err(QrClientError::NotFound(format!(
                "{} (status={})",
                body.username, body.status
            )));
        }
        Ok(AuthBundle::from_maps(body.username, body.cookies, body.headers)
            .with_twofa(body.twofa_secret))
    }
}

pub type SharedQrClient = Arc<QrClient>;

fn urlencode(s: &str) -> String {
    // 简版 percent-encode：只保留 ASCII 字母数字和常见标点，其它按字节 %XX
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_safe_chars_passthrough() {
        assert_eq!(urlencode("alice"), "alice");
        assert_eq!(urlencode("acct-1"), "acct-1");
    }

    #[test]
    fn url_encode_chinese() {
        // "测试" 在 UTF-8 是 6 字节
        assert_eq!(urlencode("测试"), "%E6%B5%8B%E8%AF%95");
    }

    #[test]
    fn url_encode_special() {
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }
}
