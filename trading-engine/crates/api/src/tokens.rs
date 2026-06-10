//! Token registry：内存里维护一份 symbol↔alphaId 映射，5 分钟刷新一次。
//!
//! 用户在前端输入 "NEX" → 后端查 registry → 拿到 alpha_id="ALPHA_971"
//! 和 symbol_pair="ALPHA_971USDT"。

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use binance_alpha::{AggTicker24Entry, AlphaRest, SymbolFilter};
use rust_decimal::Decimal;
use serde::Serialize;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize)]
pub struct TokenInfo {
    pub symbol: String, // "NEX"
    pub alpha_id: String, // "ALPHA_971"
    pub pair_symbol: String, // "ALPHA_971USDT"
    pub name: String, // "Nexus"
    pub chain_id: String,
    pub contract_address: String,
    pub trade_decimal: u32,
    pub tradable: bool,
    /// 从 get-exchange-info PRICE_FILTER.tickSize 拿。None = exchange info 没合并到这个 pair
    pub tick_size: Option<Decimal>,
    /// 从 LOT_SIZE.stepSize 拿。
    pub step_size: Option<Decimal>,
    /// 从 MIN_NOTIONAL 拿。
    pub min_notional: Option<Decimal>,
}

impl From<&AggTicker24Entry> for TokenInfo {
    fn from(e: &AggTicker24Entry) -> Self {
        Self {
            symbol: e.symbol.clone(),
            alpha_id: e.alpha_id.clone(),
            pair_symbol: e.pair_symbol(),
            name: e.name.clone(),
            chain_id: e.chain_id.clone(),
            contract_address: e.contract_address.clone(),
            trade_decimal: e.trade_decimal,
            tradable: e.tradable(),
            tick_size: None,
            step_size: None,
            min_notional: None,
        }
    }
}

#[derive(Default)]
struct Inner {
    /// V2.tune27: symbol uppercase → Vec<TokenInfo>（币安会有同 symbol 不同币，如 SLX
    /// 同时是 SLIMEX (ALPHA_417) 和 Solstice (ALPHA_978)。旧 HashMap 单值会被后者覆盖）
    by_symbol: HashMap<String, Vec<TokenInfo>>,
    /// alphaId → TokenInfo (alpha_id 是唯一的)
    by_alpha_id: HashMap<String, TokenInfo>,
    /// pair_symbol（如 ALPHA_971USDT）→ TokenInfo (pair 也唯一)
    by_pair: HashMap<String, TokenInfo>,
    last_refresh_ms: i64,
}

pub struct TokenRegistry {
    inner: RwLock<Inner>,
    alpha: Arc<AlphaRest>,
}

impl TokenRegistry {
    pub fn new(alpha: Arc<AlphaRest>) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(Inner::default()),
            alpha,
        })
    }

    /// 启动后台刷新任务（5 分钟一次）。
    pub fn spawn_refresh(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            // 启动后立刻先拉一次
            if let Err(e) = me.refresh().await {
                warn!(err = %e, "initial token refresh failed");
            }
            let mut ticker = tokio::time::interval(Duration::from_secs(5 * 60));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if let Err(e) = me.refresh().await {
                    warn!(err = %e, "token refresh failed");
                }
            }
        });
    }

    pub async fn refresh(&self) -> anyhow::Result<usize> {
        // 并行拉两个接口（不互相依赖）
        let (entries_res, exchange_res) = tokio::join!(
            self.alpha.get_agg_ticker24(),
            self.alpha.get_exchange_info(),
        );
        let entries = entries_res?;

        // 把 exchange info 的 filters 拉成 by-pair 映射
        // 注意：get_exchange_info 失败不致命 — tick/step 仅作为额外信息，
        // 老 NEX 流程靠 fallback 仍可跑（V2.tune3 之前是硬编码 NEX 值）。
        let mut filters: HashMap<String, (Option<Decimal>, Option<Decimal>, Option<Decimal>)> =
            HashMap::new();
        match exchange_res {
            Ok(ex) => {
                for sym in &ex.symbols {
                    let mut tick = None;
                    let mut step = None;
                    let mut min_notional = None;
                    for f in &sym.filters {
                        match f {
                            SymbolFilter::PriceFilter { tick_size, .. } => tick = Some(*tick_size),
                            SymbolFilter::LotSize { step_size, .. } => step = Some(*step_size),
                            SymbolFilter::MinNotional { min_notional: m } => {
                                min_notional = Some(*m)
                            }
                            _ => {}
                        }
                    }
                    filters.insert(sym.symbol.clone(), (tick, step, min_notional));
                }
                info!(symbols = ex.symbols.len(), "exchange info merged");
            }
            Err(e) => {
                warn!(err = %e, "get_exchange_info failed; tick/step will be None (fallback)");
            }
        }

        let mut by_symbol: HashMap<String, Vec<TokenInfo>> = HashMap::new();
        let mut by_alpha_id = HashMap::new();
        let mut by_pair = HashMap::new();
        for e in &entries {
            let mut info = TokenInfo::from(e);
            if let Some((tick, step, mn)) = filters.get(&info.pair_symbol) {
                info.tick_size = *tick;
                info.step_size = *step;
                info.min_notional = *mn;
            }
            by_symbol
                .entry(e.symbol.to_ascii_uppercase())
                .or_default()
                .push(info.clone());
            by_alpha_id.insert(e.alpha_id.clone(), info.clone());
            by_pair.insert(info.pair_symbol.clone(), info);
        }
        // 日志：哪些 symbol 有歧义
        for (sym, list) in &by_symbol {
            if list.len() > 1 {
                let aids: Vec<String> = list.iter().map(|t| format!("{}({})", t.alpha_id, t.name)).collect();
                warn!(symbol = %sym, count = list.len(), variants = ?aids,
                    "ambiguous symbol — same symbol used by multiple tokens. find_by_symbol picks newest alpha_id; user should use alpha_id for precision");
            }
        }
        let count: usize = by_symbol.values().map(|v| v.len()).sum();
        let with_tick: usize = by_symbol.values().flat_map(|v| v.iter()).filter(|t| t.tick_size.is_some()).count();
        let now_ms = chrono::Utc::now().timestamp_millis();
        if let Ok(mut g) = self.inner.write() {
            g.by_symbol = by_symbol;
            g.by_alpha_id = by_alpha_id;
            g.by_pair = by_pair;
            g.last_refresh_ms = now_ms;
        }
        info!(count, with_tick, "token registry refreshed");
        Ok(count)
    }

    /// 通过 friendly symbol 查（大小写不敏感）。
    /// 多个 token 共享同 symbol（如 SLX = SLIMEX/Solstice）时，按 alpha_id 数字最大优先（最新上线）。
    pub fn find_by_symbol(&self, symbol: &str) -> Option<TokenInfo> {
        let key = symbol.to_ascii_uppercase();
        let g = self.inner.read().ok()?;
        let list = g.by_symbol.get(&key)?;
        if list.is_empty() {
            return None;
        }
        if list.len() == 1 {
            return Some(list[0].clone());
        }
        // 多个，挑 alpha_id 数字最大的（"ALPHA_978" > "ALPHA_417"）
        let pick = list.iter().max_by_key(|t| {
            t.alpha_id.trim_start_matches("ALPHA_").parse::<u64>().unwrap_or(0)
        })?;
        Some(pick.clone())
    }

    /// V2.tune27: 返回某 symbol 全部匹配的 token（前端歧义提示用）
    pub fn find_all_by_symbol(&self, symbol: &str) -> Vec<TokenInfo> {
        let key = symbol.to_ascii_uppercase();
        self.inner.read().ok()
            .and_then(|g| g.by_symbol.get(&key).cloned())
            .unwrap_or_default()
    }

    pub fn find_by_alpha_id(&self, alpha_id: &str) -> Option<TokenInfo> {
        self.inner.read().ok()?.by_alpha_id.get(alpha_id).cloned()
    }

    pub fn find_by_pair(&self, pair: &str) -> Option<TokenInfo> {
        self.inner.read().ok()?.by_pair.get(pair).cloned()
    }

    /// 拿全部（前端搜索建议用）— flatten Vec<Vec> → Vec
    pub fn list_all(&self) -> Vec<TokenInfo> {
        let g = match self.inner.read() {
            Ok(g) => g,
            Err(_) => return vec![],
        };
        let mut xs: Vec<TokenInfo> = g.by_symbol.values().flat_map(|v| v.iter().cloned()).collect();
        xs.sort_by(|a, b| a.symbol.cmp(&b.symbol).then(a.alpha_id.cmp(&b.alpha_id)));
        xs
    }

    pub fn last_refresh_ms(&self) -> i64 {
        self.inner.read().map(|g| g.last_refresh_ms).unwrap_or(0)
    }

    pub fn count(&self) -> usize {
        // 返回总 token 数（含同 symbol 多变体），不是 symbol 个数
        self.inner.read().map(|g| g.by_symbol.values().map(|v| v.len()).sum()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_entry_builds_pair() {
        let e = AggTicker24Entry {
            token_id: "x".into(),
            chain_id: "56".into(),
            contract_address: "0xabc".into(),
            name: "Nexus".into(),
            symbol: "NEX".into(),
            alpha_id: "ALPHA_971".into(),
            price: None,
            decimals: 18,
            trade_decimal: 8,
            offsell: false,
            offline: false,
            stock_state: false,
        };
        let t: TokenInfo = (&e).into();
        assert_eq!(t.pair_symbol, "ALPHA_971USDT");
        assert!(t.tradable);
        // 默认 filter 字段是 None（要 refresh 后 exchange info 合并才有值）
        assert!(t.tick_size.is_none());
        assert!(t.step_size.is_none());
    }

    #[test]
    fn untradable_when_offline() {
        let mut e = AggTicker24Entry {
            token_id: "x".into(), chain_id: "56".into(), contract_address: "0x".into(),
            name: "".into(), symbol: "".into(), alpha_id: "".into(),
            price: None, decimals: 0, trade_decimal: 0,
            offsell: false, offline: false, stock_state: false,
        };
        e.offline = true;
        let t: TokenInfo = (&e).into();
        assert!(!t.tradable);
    }
}
