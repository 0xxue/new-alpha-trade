//! Binance Alpha 2FA 自动验证。
//!
//! 触发条件：下单响应 header 含 `risk_challenge_biz_no`（且 enable_flow=true）
//!
//! 完整流程（5 步）：
//! 1. 算 TOTP 6 位码（Google Authenticator 兼容，SHA1 / 30s 周期 / 6 位）
//! 2. `POST verifySingleFactor`（带 mfa-flag:1 header）
//! 3. 轮询 `getSteps` 直到 status=DONE（最多 10 次，每次 1s）
//! 4. `GET getChallengeToken` 拿一次性令牌
//! 5. 调用方把令牌塞到后续请求的 `x-passthrough-token` header
//!
//! 旧 trading_agent.py L484-820 实现的 Rust 化版本。

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use totp_rs::{Algorithm, Secret, TOTP};
use tracing::{debug, info, warn};

use crate::rest::AuthBundle;
use crate::types::ApiEnvelope;

#[derive(Debug, Error)]
pub enum TwofaError {
    #[error("invalid 2fa secret: {0}")]
    InvalidSecret(String),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("decode: {0}")]
    Decode(String),
    #[error("verify rejected: code={code} message={message}")]
    VerifyRejected { code: String, message: String },
    /// 币安风控要求 TOTP 之外的额外步骤（手机短信 / 人脸识别）。
    /// 旧项目识别 code=100001003 或 message 含 'step flows have not finish'。
    /// TOTP 程序处理不了，必须人工去币安 App 完成 → 调用方应 pause 整个 job。
    #[error("extra verification required (face/phone): code={code} message={message}")]
    ExtraVerificationRequired { code: String, message: String },
    #[error("steps did not reach DONE within {0} polls")]
    StepsTimeout(u32),
    #[error("getChallengeToken returned empty token")]
    EmptyToken,
}

/// 检测响应是否表示"需要额外验证步骤（人脸/手机）"。
/// 出自旧 trading_agent.py L755 实测匹配条件。
fn is_extra_verify_signal(code: &str, message: &str) -> bool {
    code == "100001003" || message.contains("step flows have not finish")
}

/// 用 base32 secret 算当前 30s 周期的 TOTP 6 位码。
///
/// 用 `TOTP::new_unchecked` 跳过 totp-rs 默认的 ≥128bit 长度检查 ——
/// Google Authenticator 标准 secret 是 16 字符 base32 = 80bit，
/// 严格 128bit 限制会把绝大多数用户的 secret 全部拒掉。
pub fn generate_totp_code(secret: &str) -> Result<String, TwofaError> {
    let bytes = Secret::Encoded(secret.to_string())
        .to_bytes()
        .map_err(|e| TwofaError::InvalidSecret(format!("{e:?}")))?;
    let totp = TOTP::new_unchecked(
        Algorithm::SHA1,
        6,
        1,
        30,
        bytes,
        Some("Binance".into()),
        "alpha".into(),
    );
    totp.generate_current()
        .map_err(|e| TwofaError::InvalidSecret(e.to_string()))
}

#[derive(Debug, Serialize)]
struct VerifyReq<'a> {
    #[serde(rename = "bizNo")]
    biz_no: &'a str,
    #[serde(rename = "bizType")]
    biz_type: &'a str,
    #[serde(rename = "verifyType")]
    verify_type: &'a str,
    #[serde(rename = "verifyCode")]
    verify_code: &'a str,
}

#[derive(Debug, Deserialize)]
struct StepsData {
    status: String,
}

#[derive(Debug, Deserialize)]
struct ChallengeTokenData {
    #[serde(rename = "challengeToken")]
    challenge_token: Option<String>,
}

/// 完整 2FA 流程，返回 `challenge_token`。
///
/// `biz_type` 默认 `DEX_ALPHA_LIMIT`（来自旧代码抓包）。
///
/// 调用方拿到 token 后应该塞进 AuthBundle.headers["x-passthrough-token"]，
/// 下次下单时 binance-alpha::rest::private 会自动透传过去。
pub async fn run_2fa_flow(
    http: &reqwest::Client,
    base: &str,
    auth: &AuthBundle,
    biz_no: &str,
    secret: &str,
) -> Result<String, TwofaError> {
    let biz_type = "DEX_ALPHA_LIMIT";
    let code = generate_totp_code(secret)?;
    info!(%biz_no, code_prefix = &code[..2], "starting 2FA flow");

    // ---- Step 1: POST verifySingleFactor
    let verify_url = format!("{base}/bapi/accounts/v1/private/risk/challenge/verifySingleFactor");
    let headers = build_headers(auth, true);
    let req_body = VerifyReq {
        biz_no,
        biz_type,
        verify_type: "GOOGLE",
        verify_code: &code,
    };
    let resp = http
        .request(Method::POST, &verify_url)
        .headers(headers.clone())
        .json(&req_body)
        .send()
        .await?;
    let env: ApiEnvelope<serde_json::Value> =
        resp.json().await.map_err(|e| TwofaError::Decode(e.to_string()))?;
    if !env.success {
        let message = env.message.unwrap_or_default();
        if is_extra_verify_signal(&env.code, &message) {
            warn!(%biz_no, code = %env.code, %message,
                  "verifySingleFactor: extra verification required (face/phone)");
            return Err(TwofaError::ExtraVerificationRequired {
                code: env.code,
                message,
            });
        }
        return Err(TwofaError::VerifyRejected {
            code: env.code,
            message,
        });
    }
    debug!(%biz_no, "verifySingleFactor ok");

    // ---- Step 2: 等一下让币安状态机收敛
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ---- Step 3: 轮询 getSteps
    let steps_url = format!(
        "{base}/bapi/accounts/v1/protect/risk/challenge/getSteps?bizNo={biz_no}"
    );
    const MAX_POLLS: u32 = 10;
    let mut done = false;
    for i in 1..=MAX_POLLS {
        let resp = http
            .request(Method::GET, &steps_url)
            .headers(headers.clone())
            .send()
            .await?;
        let env: ApiEnvelope<StepsData> =
            resp.json().await.map_err(|e| TwofaError::Decode(e.to_string()))?;
        if let Some(d) = env.data.as_ref() {
            debug!(%biz_no, status = %d.status, "getSteps poll #{i}");
            if d.status == "DONE" {
                done = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if !done {
        warn!(%biz_no, "getSteps not DONE within {MAX_POLLS} polls, try token anyway");
    }

    // ---- Step 4: 拿 challenge_token
    let token_url = format!(
        "{base}/bapi/accounts/v1/private/risk/challenge/getChallengeToken?bizNo={biz_no}"
    );
    let resp = http
        .request(Method::GET, &token_url)
        .headers(headers)
        .send()
        .await?;
    let env: ApiEnvelope<ChallengeTokenData> =
        resp.json().await.map_err(|e| TwofaError::Decode(e.to_string()))?;
    if !env.success {
        let message = env.message.unwrap_or_default();
        if is_extra_verify_signal(&env.code, &message) {
            warn!(%biz_no, code = %env.code, %message,
                  "getChallengeToken: extra verification required (face/phone)");
            return Err(TwofaError::ExtraVerificationRequired {
                code: env.code,
                message,
            });
        }
        return Err(TwofaError::VerifyRejected {
            code: env.code,
            message,
        });
    }
    let token = env.data.and_then(|d| d.challenge_token).filter(|s| !s.is_empty());
    let token = token.ok_or(TwofaError::EmptyToken)?;
    info!(%biz_no, token_prefix = &token[..token.len().min(16)], "2FA done");
    Ok(token)
}

/// 给所有 2FA 请求统一构造 headers：透传 cookies + 加 mfa-flag。
fn build_headers(auth: &AuthBundle, with_mfa_flag: bool) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&auth.cookie_header()) {
        h.insert(reqwest::header::COOKIE, v);
    }
    for (k, v) in &auth.headers {
        if k.starts_with(':') {
            continue;
        }
        let lk = k.to_ascii_lowercase();
        if matches!(
            lk.as_str(),
            "host" | "content-length" | "content-type" | "cookie" | "connection" | "accept-encoding"
        ) {
            continue;
        }
        if let (Ok(name), Ok(val)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
        {
            h.insert(name, val);
        }
    }
    if with_mfa_flag {
        h.insert(
            HeaderName::from_static("mfa-flag"),
            HeaderValue::from_static("1"),
        );
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_secret_decodes_and_generates_6_digit() {
        // 32 字符 base32 = 160 位 secret
        let secret = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP";
        let code = generate_totp_code(secret).expect("generate ok");
        assert_eq!(code.len(), 6, "code should be 6 digits, got {code}");
        assert!(code.chars().all(|c| c.is_ascii_digit()), "code={code}");
    }

    #[test]
    fn totp_accepts_16_char_google_authenticator_secret() {
        // 16 字符 base32 = 80 位 secret（Google Authenticator 默认长度）
        // 之前用 TOTP::new 会因 ≥128bit 检查而拒掉，现在 new_unchecked 通过
        let secret = "JBSWY3DPEHPK3PXP";
        let code = generate_totp_code(secret).expect("80-bit secret should be accepted");
        assert_eq!(code.len(), 6, "code should be 6 digits, got {code}");
        assert!(code.chars().all(|c| c.is_ascii_digit()), "code={code}");
    }

    #[test]
    fn totp_rejects_invalid_secret() {
        let r = generate_totp_code("not-base32!!");
        assert!(r.is_err());
    }

    // ---- is_extra_verify_signal: 旧代码命中条件 ----
    #[test]
    fn extra_verify_matches_code_100001003() {
        assert!(is_extra_verify_signal("100001003", ""));
        assert!(is_extra_verify_signal("100001003", "anything"));
    }

    #[test]
    fn extra_verify_matches_step_flows_msg() {
        assert!(is_extra_verify_signal(
            "999999",
            "step flows have not finish"
        ));
        assert!(is_extra_verify_signal(
            "ok",
            "...prefix... step flows have not finish ...suffix..."
        ));
    }

    #[test]
    fn extra_verify_rejects_normal_2fa_failure() {
        assert!(!is_extra_verify_signal("100001008", "wrong 2fa code"));
        assert!(!is_extra_verify_signal("", ""));
        assert!(!is_extra_verify_signal("000000", "success"));
    }
}
