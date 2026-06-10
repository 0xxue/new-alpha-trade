//! Volume / wear 计算（合约见 `docs/design/wear-volume-spec.md`）。

use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::SqlitePool;

use sqlx::Row;

use super::{jobs, trades};

/// 综合 stats（前端展示用）。
#[derive(Debug, Clone, Serialize)]
pub struct JobStats {
    pub job_id: String,
    pub username: String,
    pub symbol: String,
    pub strategy: String,
    pub state: String,

    /// 单边-买累计 USDT（= Σ buy.quoteQty）
    pub buy_volume_usdt: Decimal,
    /// 目标 USDT
    pub target_volume_usdt: Decimal,
    /// volume / target × 10000 → 万分比（避免浮点）
    pub progress_bps: i64,

    /// fill 总数
    pub fill_count: i64,

    /// wear baseline（job 启动时 SPOT funding.free USDT）
    pub baseline_spot_usdt: Option<Decimal>,
    /// 调用方传入的当前 SPOT funding.free USDT（让上层决定何时拉）
    pub current_spot_usdt: Option<Decimal>,
    /// = current - baseline，负数 = 亏损
    pub wear_amount_usdt: Option<Decimal>,
    /// wear / volume × 10000，单位 bps，负数 = 亏损
    pub wear_ratio_bps: Option<i64>,

    /// 当前持有 base 代币的 free 数量（业务上：刷量结束不应有残留）
    pub base_holding_qty: Option<Decimal>,
    /// 估算 base 持仓的 USDT 价值（当前价 × 持仓）
    pub base_holding_valuation_usdt: Option<Decimal>,
}

/// 时序数据点。
#[derive(Debug, Clone, Serialize)]
pub struct TimePoint {
    pub ts_ms: i64,
    pub side: String,
    /// 该笔成交额（USDT）
    pub quote_qty: Decimal,
    /// 截至该笔的累积 buy 总额
    pub cum_buy_volume: Decimal,
    /// 截至该笔的累积 sell 总额
    pub cum_sell_value: Decimal,
    /// 已实现 P&L = cum_sell - cum_buy（不含未平仓持仓 + 不含手续费里 base 部分）
    pub cum_pnl_realized: Decimal,
    /// 截至该笔的 fill 累计
    pub fill_count: i64,
}

/// 拉某 job 的所有 fills，按时间排序，逐笔累加算时序。
pub async fn timeseries(pool: &sqlx::SqlitePool, job_id: &str) -> sqlx::Result<Vec<TimePoint>> {
    let rows = sqlx::query(
        "SELECT trade_ts_ms, side, CAST(quote_qty AS REAL) AS qq \
         FROM trades WHERE job_id = ? ORDER BY trade_ts_ms",
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?;
    let mut cum_buy = Decimal::ZERO;
    let mut cum_sell = Decimal::ZERO;
    let mut out = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        let ts_ms: i64 = r.try_get("trade_ts_ms").unwrap_or(0);
        let side: String = r.try_get("side").unwrap_or_default();
        let qq_f64: f64 = r.try_get("qq").unwrap_or(0.0);
        let qq = Decimal::from_f64_retain(qq_f64).unwrap_or(Decimal::ZERO);
        if side == "BUY" {
            cum_buy += qq;
        } else if side == "SELL" {
            cum_sell += qq;
        }
        out.push(TimePoint {
            ts_ms,
            side,
            quote_qty: qq,
            cum_buy_volume: cum_buy,
            cum_sell_value: cum_sell,
            cum_pnl_realized: cum_sell - cum_buy,
            fill_count: (i + 1) as i64,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct WearStat {
    pub wear_amount_usdt: Decimal,
    pub wear_ratio_bps: i64,
}

/// 从 jobs.params_json 里取 baseline_spot_usdt（约定 key `_baseline_spot_usdt`）。
pub fn parse_baseline(params_json: &str) -> Option<Decimal> {
    let v: serde_json::Value = serde_json::from_str(params_json).ok()?;
    v.get("_baseline_spot_usdt")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse().ok())
}

/// 把 baseline 写回 jobs.params_json（替换/插入 `_baseline_spot_usdt`）。
pub fn inject_baseline(params_json: &str, baseline: Decimal) -> String {
    // 如果用户没传 params 或传了 null/数组/标量，都覆盖成空对象
    let mut v: serde_json::Value =
        serde_json::from_str(params_json).unwrap_or(serde_json::json!({}));
    if !v.is_object() {
        v = serde_json::json!({});
    }
    v.as_object_mut().unwrap().insert(
        "_baseline_spot_usdt".into(),
        serde_json::Value::String(baseline.to_string()),
    );
    v.to_string()
}

#[cfg(test)]
mod baseline_tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn inject_into_null() {
        let out = inject_baseline("null", dec!(103.20));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["_baseline_spot_usdt"], "103.20");
    }
    #[test]
    fn inject_into_object_preserves_existing() {
        let out = inject_baseline(r#"{"note":"hi"}"#, dec!(5.5));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["note"], "hi");
        assert_eq!(v["_baseline_spot_usdt"], "5.5");
    }
    #[test]
    fn parse_roundtrip() {
        let s = inject_baseline(r#"{}"#, dec!(42.0));
        assert_eq!(parse_baseline(&s), Some(dec!(42.0)));
    }
    #[test]
    fn parse_missing() {
        assert_eq!(parse_baseline(r#"{"foo":"bar"}"#), None);
    }
}

/// 计算 job stats。
/// current_spot_usdt / base_holding_qty / base_valuation 由上层从 binance-alpha 拉。
pub async fn compute(
    pool: &SqlitePool,
    job_id: &str,
    current_spot_usdt: Option<Decimal>,
    base_holding_qty: Option<Decimal>,
    base_holding_valuation_usdt: Option<Decimal>,
) -> sqlx::Result<Option<JobStats>> {
    let job = match jobs::get(pool, job_id).await? {
        Some(j) => j,
        None => return Ok(None),
    };
    let buy_volume = trades::sum_buy_quote_qty(pool, job_id).await?;
    let fill_count = trades::count_fills(pool, job_id).await?;
    let target: Decimal = job.target_volume.parse().unwrap_or(Decimal::ZERO);
    let progress_bps = if target.is_zero() {
        0
    } else {
        ((buy_volume / target) * Decimal::from(10000))
            .floor()
            .try_into()
            .unwrap_or(0i64)
    };

    let baseline = parse_baseline(&job.params_json);
    let (wear_amount, wear_bps) = match (baseline, current_spot_usdt) {
        (Some(b), Some(c)) => {
            let wear = c - b;
            let bps = if buy_volume.is_zero() {
                0
            } else {
                ((wear / buy_volume) * Decimal::from(10000))
                    .floor()
                    .try_into()
                    .unwrap_or(0i64)
            };
            (Some(wear), Some(bps))
        }
        _ => (None, None),
    };

    Ok(Some(JobStats {
        job_id: job.id,
        username: job.username,
        symbol: job.symbol,
        strategy: job.strategy,
        state: job.state,
        buy_volume_usdt: buy_volume,
        target_volume_usdt: target,
        progress_bps,
        fill_count,
        baseline_spot_usdt: baseline,
        current_spot_usdt,
        wear_amount_usdt: wear_amount,
        wear_ratio_bps: wear_bps,
        base_holding_qty,
        base_holding_valuation_usdt,
    }))
}
