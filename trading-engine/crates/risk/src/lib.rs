//! 风控模块（P6 实现）。
//!
//! 旧项目 fast_low_wear 策略带 `fund_diff_limit=-5U` 资金亏损阈值，新版至少要保持等价。
//! 待实现：
//! - 资金损失监控（每 N 分钟检查一次，亏损超过阈值 -> 暂停 job）
//! - 持仓时长上限（旧版 7s 自动清仓）
//! - 单笔/单日最大订单数限制
//! - 异常错误码序列检测（连续 2FA 失败 / 余额不足 / 风控拦截）-> 暂停

/// 全局风控状态机（待实现）。
pub struct RiskMonitor;

impl RiskMonitor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RiskMonitor {
    fn default() -> Self {
        Self::new()
    }
}
