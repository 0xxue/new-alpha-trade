# OTO 策略 v2：smart 模式（胜率版）

> 状态：**草案，等用户审批**
> 起源：用户原话 "0.0011 买 0.0012 卖，要有胜率机制，可以偶尔小亏，可以偶尔小赢，但是速度要快"

---

## 1. v1 vs v2 一图对比

| 维度 | **v1 fast**（已实现，简单粗暴） | **v2 smart**（这里要做） |
|---|---|---|
| 入场时机 | 无脑立刻下 | 看信号，市场不利时**跳过本轮** |
| working 价格 | 永远 best_ask + 2 tick (taker) | spread 大时 **best_bid + 1 (maker)** 抢价差；spread 小时 taker |
| pending 价格 | 永远 best_bid - 2 tick (taker) | 看深度 + 趋势，maker / taker 自适应 |
| pending 没成交 | 立即清仓 | 先等几秒（maker 有机会成交）；超时再清仓 |
| 决策依据 | 无 | spread + book imbalance + 近期成交趋势 |
| 期望 wear | -0.04 bps 固定 | 可能 0 ~ -0.04 bps，**偶尔 +正 wear**（赚 spread） |

**核心思想**：v2 不追求每轮都赚，追求**长期 wear 比 v1 好**。还能保证每轮都能成交。

---

## 2. 信号源（不引入新数据，全用现有）

### 2.1 Spread (best_ask - best_bid)
- 单位：tick 数
- 来源：每轮开头拉 `get_full_depth`
- 含义：spread 大 → 有 maker 套利空间；spread 小 → 直接 taker 没价差可吃

### 2.2 Book Imbalance（盘口失衡）
- 公式：`bid_qty_sum_5 / (bid_qty_sum_5 + ask_qty_sum_5)`
- 5 = 前 5 档深度累加
- 范围 0.0 ~ 1.0
- `> 0.6` → 买盘强，价格倾向涨
- `< 0.4` → 卖盘强，价格倾向跌
- `0.4 ~ 0.6` → 中性

### 2.3 近期成交趋势（最近 N=10 笔 aggTrade）
- 数据源：现有 `AlphaWsClient` 的 broadcast，订阅 `alpha_<id>usdt@aggTrade` 已存在
- v2 加一个 `RecentTrades` 环形 buffer，维护最近 10 笔（price, qty, side）
- 计算：
  - `up_count`: m=false 的笔数（买方主动 → 涨方向）
  - `down_count`: m=true 的笔数（卖方主动 → 跌方向）
  - `trend`: up_count - down_count，范围 [-10, +10]
  - `trend > +3` → up
  - `trend < -3` → down
  - 中间 → flat

---

## 3. 决策矩阵

| spread (tick) | imbalance | trend | 行动 | working price | pending price | maker timeout |
|---|---|---|---|---|---|---|
| ≥ 3 | > 0.6 | up | **抢价差** | best_bid + 1 tick (maker) | best_ask − 1 tick (maker) | 5s |
| ≥ 3 | < 0.4 | down | **跳过本轮**（市场往下走，强买容易亏） | — | — | — |
| ≥ 3 | 0.4-0.6 | flat | 半价差 | best_ask − 1 tick (taker) | best_bid + 1 tick (maker) | 3s |
| 1-2 | > 0.6 | up | 快刷（顺势） | best_ask + 1 tick (taker) | best_bid + 1 tick (maker) | 3s |
| 1-2 | < 0.4 | down | **跳过本轮** | — | — | — |
| 1-2 | 0.4-0.6 | 任何 | **fast 模式**（v1 行为） | best_ask + 1 (taker) | best_bid − 1 (taker) | 0 |
| 0 (spread=0) | 任何 | 任何 | fast 模式 | best_ask + 1 (taker) | best_bid − 1 (taker) | 0 |

**核心规则**：
- 卖盘强 + spread 大 → 跳过（不强买）
- 买盘强 + spread 大 → 抢价差（双 maker）
- 其它 → 偏 taker，保证成交

**跳过本轮**意味着：sleep 1 秒后重新评估，不下单也不计失败。

---

## 4. maker / taker 切换的两个细节

### 4.1 maker working 没立即成交
- working = best_bid + 1 tick → 挂在 bid 二档位置
- 期望：bid 一档很快被吃，我的单上升为 bid 一档，然后被卖方吃
- maker timeout（默认 5 秒）：超时 → 撤掉 working → 整轮跳过（不下 OTO）
- 这样能避免持仓敞口

### 4.2 pending maker 没立即成交
- pending 触发后挂在 best_ask − 1 tick (ask 二档)
- 等被买方吃
- maker timeout 内没成交 → emergency_sell_all 立即 taker 清仓

---

## 5. 风控

| 触发条件 | 行动 |
|---|---|
| 连续 3 轮 pending 没成交 | 自动降级到 fast 模式（剩余 job 都 taker），跑完结束 |
| 累积 wear_bps < -100 (= -1%) | pause job |
| 连续 5 轮 working 没成交（市场死了） | pause job |
| 跑了 N 轮但 vol 没动 | pause job |

---

## 6. 参数表（jobs.params_json）

```json
{
  "aggression": "smart",         // "smart" 走决策矩阵；"fast" 走 v1
  "single_min_usdt": "10",
  "single_max_usdt": "20",
  "spread_min_for_maker_tick": 3,
  "imbalance_long_threshold": 0.6,
  "imbalance_short_threshold": 0.4,
  "trend_window": 10,
  "trend_signal_threshold": 3,
  "maker_timeout_s": 5,
  "max_consecutive_pending_fail": 3,
  "stop_on_wear_bps": -100
}
```

前端表单只暴露 `aggression` 一个开关（smart / fast），其它都默认值，进阶用户可以从 API 直接传。

---

## 7. 期望效果（保守估算）

跑 1000 轮 OTO（target ≈ 15000 USDT，单笔 15）：
- v1 fast：100% 立即成交，wear ≈ -0.04 bps × 1000 = -6 USDT
- v2 smart：
  - ~700 轮 taker（fast 模式）：-0.04 bps × 700 ≈ -4 USDT
  - ~200 轮 双 maker 成交：+0.02 bps × 200 ≈ +0.6 USDT（赚 spread）
  - ~80 轮 半价差：-0.02 bps × 80 ≈ -0.3 USDT
  - ~20 轮 maker 超时降级：-0.05 bps × 20 ≈ -0.15 USDT
  - 总 wear ≈ **-4 USDT**（比 v1 省 30%）

**注意**：上面是非常乐观估算。实际 maker 成交率 / 价差捕获率高度依赖币种流动性和市场情绪。在高波动时段 v2 可能不如 v1（maker 被反方向吃）；在横盘时段 v2 能赚一点。

---

## 8. 实现工作量

| 模块 | 工作量 | 文件 |
|---|---|---|
| `RecentTrades` buffer + 订阅 aggTrade | ~80 行 | `crates/strategy/src/recent_trades.rs` 或 `crates/api/src/strategy_runner.rs` |
| `evaluate_decision()` 决策函数 | ~100 行 | strategy_runner.rs |
| 改 `run_oto_round` 接受 decision 参数 | ~50 行 | strategy_runner.rs |
| maker timeout 等待逻辑 | ~50 行 | strategy_runner.rs |
| 风控阈值检查 | ~50 行 | strategy_runner.rs |
| 前端 strategy 下拉加 "smart" 选项 | ~10 行 | Trade.tsx |
| 单元测试 | ~150 行 | tests |
| 端到端验证 | 半天 | — |

总计：~500 行 Rust 代码 + 测试 + 调参，预计 **1 天工程量**。

---

## 9. 风险与限制

1. **v2 在小币种 / 流动性差时可能更差** —— spread 看起来大但其实没人挂单。可以通过看 5 档累计深度 < 阈值时禁用 maker 模式。
2. **maker 单容易被"插队"** —— 别人在 bid 二档挂的时间比你早，先成交。我们的 working 可能永远排在他后面。
3. **趋势信号有滞后** —— 看最近 10 笔成交判趋势，市场拐点时会误判。
4. **maker timeout 期间持仓敞口** —— 5 秒内市场波动也吃磨损。但 5 秒 spread 通常 < 1 tick。
5. **pending maker 超时后 taker 清仓** —— 这个 wear 比 v1 还差（双向都 taker）。出现频率取决于市场。

---

## 10. 已确认的决策（2026-05-22 用户审批）

1. ✅ **阈值用默认**（imbalance 0.6/0.4 + trend ±3）— 跑实战看数据再调
2. ✅ **卖盘强时跳过本轮**（不降级 fast）— sleep 1 秒重评估，市场反转后才下单
3. ✅ **fast 降级后不恢复**（连 3 次 pending 失败 → 本 job 剩余全 fast；下个 job 重新走 smart）
4. ✅ **单笔金额由用户在前端输入**，代码层 default 不动（旧 0.3 不变；用户实测自己输 10-20）
5. ✅ **maker timeout**：撤单后算"跳过本轮，不计入风控失败计数"
6. ✅ **胜率指标加到 stats**：新 `rounds` 表记每轮 pnl，前端 stats 显示 `win_rate = pnl>0 轮数 / 总轮数`
7. ✅ **RecentTrades 共享**（同 symbol 不重复维护）
8. ✅ **不做回测**，跑实战验证

---

## 10b. v1 基准实测（2026-05-22）

```
币种:        NEX (ALPHA_971)
target:      300 USDT
单笔:        10-20 USDT 随机
策略:        v1 fast OTO
持续时间:    67 秒（23 轮，2.9 秒/轮）
实际 vol:    304.39 USDT (over-shoot 4)
最终 wear:   -0.19984 USDT
wear bps:    -7（即 -0.066%）
手续费占比:  ~25% (≈ -0.05 USDT)
价差占比:    ~75% (≈ -0.15 USDT)
NEX 残留:    0.13（dust 接受）
```

⚠️ 当时 NEX 价格 30 分钟内从 5.51e-6 跌到 5.48e-6（-0.5%），处于下行行情。
v2 在该时段**应该跳过卖盘强的轮**（avoid down market），预期 wear 应小于 -7 bps。

**v2 改进目标**：wear bps 从 -7 改善到 ≤ -4（≥ 40% 改善）。

⚠️ 胜率算法在 v1 不准（无 round_id 配对），v2 必须用 rounds 表显式记录。

---

## 11. 实现顺序（已审批，开干）

1. Migration 0005：新建 `rounds` 表（id / job_id / round_no / decision_type / status / pnl_usdt / started_ms / ended_ms）
2. `RecentTrades` 共享 buffer（订阅公开 ws 的 aggTrade，环形队列保留最近 N 笔）
3. `evaluate_decision()` 决策函数（5 种 Decision: SkipBearish / DoubleMaker / TakerMakerHybrid / TakerMakerPending / Fast）+ 单元测试覆盖矩阵
4. 改 `run_oto_round`：接受 `Decision`；skip 时 sleep 1s 返回；maker 模式用 maker 价 + maker_timeout
5. round 入库 `rounds` 表：每轮算 pnl_usdt（= 该轮 SELL 总额 - BUY 总额，从 trades 表反推）
6. stats 加 `win_count / loss_count / round_count / win_rate_pct` 字段
7. 风控：连 3 次 pending 失败 → fast 模式；wear < -100 bps → pause；连 5 次 working 失败 → pause
8. 前端 Trade 表单 strategy 下拉加 "smart (v2 默认)" 选项；stats 卡片显示胜率
9. 部署 + target=50 USDT 真实战验证（约 3-5 分钟跑完，能看到决策分布）
10. 看数据调参

---

请就上面 11 节中**任意点**给反馈：
- 决策矩阵阈值要调
- 跳过逻辑有不同想法
- 信号源加新的（比如订单簿 1 档深度比？）
- 参数默认值调整
- 期望效果不一样
- 实现顺序换
