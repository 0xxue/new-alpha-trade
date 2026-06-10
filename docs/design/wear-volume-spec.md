# 交易量 (volume) & 磨损 (wear) 计算规范

> 实地实验日期：2026-05-22（probe_wear_volume.py BUY→SELL 一笔 0.3 USDT 完整循环）
> 公式不是猜的，是用真实币安返回的字段反推出来的
> 旧项目 `wear_manager.py` 里的口径未参考（用户原话："旧的有可能是错的"）

---

## 1. 名词约定

| 名词 | 定义 |
|---|---|
| **trade** | 一次币安撮合（一个 order 可能拆成多个 fill，每个 fill 是一个 trade） |
| **cycle / round** | 一组 buy + sell 配对，完整一轮"借 USDT 买入 → 卖出还 USDT" |
| **base** | 计价的底层代币，例如 `ALPHA_971` (NEX) |
| **quote** | 报价代币，目前只支持 `USDT` |
| **quoteQty** | 币安给的"成交额"字段，单位 USDT，**这是权威数字**，不要自己用 `price × qty` 算 |

---

## 2. volume（交易量）：单边-买口径

**公式**：

```
volume = Σ quoteQty over all BUY trades for a given (job | account | date | scope)
```

**关键约定**：

- 只算**已成交**（FILLED / PARTIALLY_FILLED 的 executedQty 部分），CANCELED / REJECTED 不计
- 用 **quoteQty 字段直接累加**，不要 `price × qty`（币安那边可能因撮合优惠或精度调整有差异）
- SELL 单**不计入** volume
- 部分成交（partial fill）的 qty 也算 — 用 fill 表的累加，不要用 origQty

**Rust 类型**（决定 SQLite 字段命名）：

```rust
pub struct VolumeStat {
    pub job_id: String,
    pub buy_volume_usdt: Decimal,  // 单边-买累计
    pub buy_fill_count: u64,       // 多少笔成交（不是订单数，是 fill 数）
    pub last_update: DateTime<Utc>,
}
```

---

## 3. wear（磨损）：SPOT USDT 净变化口径

**公式**：

```
wear = current_spot_usdt - baseline_spot_usdt
```

- **正数 = 赚（多 USDT）**，负数 = 亏（少 USDT），与旧项目 wear_amount 符号一致
- baseline = 业务开始前的 spot USDT 余额（job 启动时快照一次）
- current = 任意时点的 spot USDT 余额（实时拉或定期 poll）

**为什么不用 Alpha wallet `totalValuation`**：

实验数据：
```
before: 183.21928877  ← 业务开始
after:  183.21929058  ← 一轮买卖后
δ:      +0.00000181   ← 几乎为 0，看不出真实成本！
```

原因：Alpha `totalValuation` 只反映"Alpha 钱包持有代币的估值"，但是：
- 买入花的 USDT 来自 **SPOT** 钱包，不在 Alpha
- 卖出收到的 USDT 也回到 **SPOT** 钱包
- 持仓清零时（一轮做完），Alpha valuation 几乎不变，**磨损被隐藏了**

所以必须查 SPOT 钱包。端点：
```
POST /bapi/asset/v3/private/asset-service/asset/get-wallet-asset
body: {"includeWallets":["CARD","MAIN","SAVING"],"includeEq":true}
```

**Rust 类型**：

```rust
pub struct WearStat {
    pub job_id: String,
    pub baseline_spot_usdt: Decimal,  // job 启动时快照
    pub current_spot_usdt: Decimal,   // 当前
    pub wear_amount: Decimal,         // = current - baseline，负 = 亏
    pub wear_ratio_bps: i64,          // wear / buy_volume × 10000，单位 bps
    pub last_update: DateTime<Utc>,
}
```

`wear_ratio_bps` 用基点（万分之一）避免百分号歧义。实验数据：
- buy_volume = 0.29989377 USDT
- wear = -0.00006002 USDT
- wear_ratio = -0.00006002 / 0.29989377 = **-0.02% = -2 bps**

---

## 4. 手续费机制（实测，与抓包档 fee-rate API 字面值不符）

**实测**：

| 场景 | 扣谁 | 比率 |
|---|---|---|
| BUY taker | **base 代币（NEX）** — 直接从你买到的数量里扣 | 0.01% |
| SELL taker | **quote 代币（USDT）** — 从卖出收到的 USDT 里扣 | 0.01% |

**fee-rate API 返回 `buyerCommission=100`**：

抓包档原以为单位是 bps（100 bps = 1%），实测下来 100 表示 **10000 分之一 = 0.01%**。

→ Rust `FeeRate` 类型里把字段改名 `buyer_commission_ten_thousandths`（或注释清楚单位）

**对 wear 的意义**：

- 买入扣 NEX 等价于"花同样 USDT 买到更少 NEX"——表现为有效买价上升 0.01%
- 卖出扣 USDT 等价于"卖到 USDT 后再被收 0.01%"——表现为有效卖价下降 0.01%
- 单边 0.01% × 2 = 0.02% = 一轮买卖最低固定成本
- 加上价差损失（一个 tick 价差，对 ALPHA_971 是 ~0.05%）—— 一轮总成本 ~0.07%
- 刷 16400 USDT 单边买入累计 → 预期总磨损 ≈ 16400 × 0.07% ≈ **11.5 USDT**

---

## 5. 实验原始数据（2026-05-22 一轮）

```
Step 0 baseline:
  Alpha wallet totalValuation = 183.21928877
  NEX = 0

BUY  order_id=3570338 fill=1
  price=0.000005665  qty=52938  quoteQty=0.29989377
  commission=5.29 NEX (扣 base)

State after BUY:
  totalValuation = 183.51874058 (+0.29945, 增加是因为持有 NEX 被算入)
  NEX free = 52932.71 (扣手续费后)

SELL order_id=3570419 fill=1
  price=0.000005665  qty=52932.70  quoteQty=0.29986374
  commission=0.00002999 USDT (扣 quote)

State after SELL:
  totalValuation = 183.21929058 (回到基线 +0.00000181，几乎为 0)
  NEX = 0.01 (步长截断残留)

Volume (单边-买):     0.29989377 USDT
价差盈亏:             0.29986374 - 0.29989377 = -0.00003003 USDT
USDT 手续费:          0.00002999 USDT
NEX 手续费折 USDT:    5.29 × 0.000005665 ≈ 0.00002997 USDT
SPOT USDT 净变化:    ≈ -0.00006002 USDT (实测口径 = wear)
wear_ratio_bps:       -2 bps (相对单边 buy_volume)
```

---

## 6. 后续要在 Rust 项目里实现的清单

### Phase A：能拿 spot 余额（baseline 需要）
- [ ] `binance-alpha::rest::get_spot_wallet(auth)` 调 `POST /bapi/asset/v3/private/asset-service/asset/get-wallet-asset`
- [ ] `api::http::accounts/{user}/spot-balance` 端点
- [ ] 单测：拉一次余额 + 断言含 USDT free 字段

### Phase B：能拿 user-trades（fill 明细）
- [ ] `binance-alpha::rest::get_user_trades(auth, order_id, symbol) -> Vec<TradeFill>`
- [ ] `api::http::orders/trades?orderId=...` 端点

### Phase C：trades 表 + stats 计算
- [ ] orders.rs 下单后**异步**拉 user-trades 写 trades 表（每个 fill 一行）
- [ ] 新 `crates/stats/` 模块实现 `compute_volume`, `compute_wear`
- [ ] `api::http::trade/stats/{job_id}` 返回 `{volume, wear, wear_bps, fill_count}`

### Phase D：前端展示
- [ ] Trade 页面（P5）显示：buy_volume / wear / wear_bps / progress = volume/target × 100%
- [ ] 全局 dashboard 显示当日所有 jobs 的累计 volume + wear

### Phase E：策略层用
- [ ] strategy crate 的 `Ctx` 里暴露 volume / wear 给策略决策（比如 wear < -X 时 pause）
- [ ] risk crate 监控 `wear_ratio_bps < -100` 自动暂停

---

## 7. 边界与陷阱

1. **多账户聚合时**：每个 username 各自 baseline，**不要把全部账户 USDT 加一起取 baseline**（共享钱包会污染）
2. **跨日聚合**：建议按 UTC 日切，旧项目按本地日，要决定一个统一时区
3. **NEX 残留**：每次卖单 step 截断会留 < 0.10 NEX，**这点价值不要算入 wear**（数额太小，纳入会让 wear 看起来不稳定）。或者：每天结束时把残留也算进 wear 的负向调整
4. **快速买卖间的钱包 valuation 不一致**：alpha wallet 和 spot wallet 之间有内部转账延迟（毫秒级），但 `get-wallet-asset` 是聚合所有 wallets 的总余额，所以不受影响 — **必须用 get-wallet-asset，不要单独看 spot/main/saving**
5. **手续费可能用 BNB 抵扣**：如果用户开了"用 BNB 付手续费 25% 折扣"，commissionAsset 会变成 BNB —— 计算 wear 时需要把 BNB commission 折算成 USDT。先假设用户**没开 BNB 折扣**，后续遇到再处理
