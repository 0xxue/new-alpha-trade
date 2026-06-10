//! 仓储层。
//!
//! 设计：
//! - 每个表一个子模块
//! - 所有金额/价格字段在 SQLite 里存 TEXT（rust_decimal Decimal 字符串），不存 REAL
//! - 时间戳存 ISO8601 字符串（UTC）

pub mod accounts;
pub mod jobs;
pub mod orders;
pub mod rounds;
pub mod server_meta;
pub mod stats;
pub mod trades;

pub use accounts::AccountRow;
pub use jobs::{JobRow, JobState, NewJob};
pub use orders::{NewOrder, OrderRow};
pub use rounds::{NewRound, RoundRow, RoundStats};
pub use stats::{JobStats, WearStat};
pub use trades::{NewTrade, TradeRow};
