# 设计文档：new-alpha-trade

> 路径：`E:\code-program\new-alpha-trade`
> 起源：从 `E:\code-program\binance-code\oneserver-binance`（约 7.7 万行）裁剪迁移而来。
> 起草日期：2026-05-21
> 状态：**草案**，等待用户审批

---

## 1. Overview（目标与问题陈述）

### 1.1 旧项目的问题
1. **臃肿**：14 个 GUI 面板 + 7+ 个扫码相关脚本 + 3 个 8000 行级 agent，共 ~7.7 万行，绝大部分对当前业务无价值。
2. **多版本并存**：qr_service / web_service / 三个 daemon 文件互为副本或迭代残骸，难以维护。
3. **单语言瓶颈**：核心交易循环 (`run_trading_cycle`) 是 Python 同步 REST 轮询，对低延迟刷量场景不友好。
4. **GUI 锁死**：CustomTkinter 桌面应用，无法远程操作、无法多账户并行可视化。
5. **策略陈旧**：14 种策略本质都是滑点参数微调，长时间没更新，效果递减。

### 1.2 新项目目标
- ✂️ **极简范围**：只做「扫码登录 + Alpha 交易」两件事。
- 🦀 **Rust 高性能引擎**：核心交易循环改用 Rust + tokio + WebSocket，目标延迟降低 5-10 倍。
- 🌐 **Web 控制面板**：远程可访问、多账户并行可视化、实时订单流。
- 🔄 **重写策略**：基于实时订单簿和波动率重新设计，不直接搬旧逻辑。
- 🧱 **清晰边界**：扫码 / 引擎 / 前端三个独立服务，HTTP+WebSocket 通信。

### 1.3 非目标（明确不做）
- 不做合约交易、现货交易（非 Alpha 板块）
- 不做磨损/团队/服务器/部署/订单监控等运维面板
- 不做 SSH 远程批量部署
- 不保留 GUI

---

## 2. Architecture（整体架构）

### 2.1 三服务架构图

```
┌─────────────────────────────────────────────────────────────┐
│                       Browser (用户)                          │
│              http://localhost:5173/  (Vite Dev)              │
└────────────────────────┬────────────────────────────────────┘
                         │ HTTP + WebSocket
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  Web UI (React + Vite + TS + Tailwind + shadcn/ui)          │
│  - 账户管理 / 扫码触发 / 策略配置 / 实时订单流 / 统计图表        │
└─────┬──────────────────────────────────┬────────────────────┘
      │ POST /trade/start                │ POST /qr/login
      │ GET  /accounts                   │ GET  /qr/status
      │ WS   /stream                     │
      ▼                                  ▼
┌─────────────────────────┐    ┌─────────────────────────────┐
│  trading-engine (Rust)  │    │  qr-service (Python)        │
│  axum HTTP + WS server  │    │  FastAPI + Playwright       │
│  tokio async runtime    │    │                             │
│  ┌───────────────────┐  │    │  ┌─────────────────────┐   │
│  │ binance-alpha     │  │    │  │ playwright_login.py │   │
│  │  REST + WS client │  │    │  │  控制 Chromium       │   │
│  └───────────────────┘  │    │  │  捕获 cookies/hdrs  │   │
│  ┌───────────────────┐  │    │  └─────────────────────┘   │
│  │ strategy          │  │    │  ┌─────────────────────┐   │
│  │  v1: 重写定价     │  │    │  │ token_refresh.py    │   │
│  └───────────────────┘  │    │  │  心跳 + 自动刷新     │   │
│  ┌───────────────────┐  │    │  └─────────────────────┘   │
│  │ risk              │  │    └────────────┬────────────────┘
│  │  止损/暂停/限频    │  │                 │ GET /auth/{user}
│  └───────────────────┘  │◀────────────────┘ 返回 cookies+hdrs
│  ┌───────────────────┐  │
│  │ persistence (sqlx)│  │
│  └─────────┬─────────┘  │
└────────────┼────────────┘
             │
             ▼
     ┌───────────────┐
     │ SQLite (.db)  │  trades / orders / accounts / wear / status
     └───────────────┘
```

### 2.2 关键抽象

| 概念 | 含义 |
|---|---|
| **Account** | 一个币安账户（有用户名 + cookies + 2FA secret），是所有操作的最小单位 |
| **Session** | 扫码完成后的一个可用凭证集合（cookies + headers），有过期时间 |
| **Strategy** | 一个定价/择时策略实例，决定买卖价格和触发条件 |
| **TradingJob** | 「某账户用某策略追到某目标交易量」的一次任务，可启动/暂停/停止 |
| **OrderBookFeed** | 某交易对的实时订单簿，WebSocket 订阅，所有策略共享 |
| **TradeRecord** | 一笔完整的买+卖记录，落盘到 SQLite |

### 2.3 数据流（典型一次 Alpha 交易循环）

1. 前端 → `POST /qr/login {username}` → qr-service 启动浏览器
2. 用户用手机扫码 → qr-service 捕获 cookies → 写 SQLite (`accounts.session_json`)
3. 前端 → `POST /trade/start {username, symbol, strategy, target_volume}` → trading-engine
4. trading-engine 调 qr-service `GET /auth/{username}` 拿最新 cookies
5. trading-engine 启 WebSocket 订阅订单簿 → 内存里维护买卖深度
6. trading-engine 按策略计算价格 → 调 REST 下买单 → 监听成交（WS）→ 下卖单 → 监听成交
7. 每笔成交写 SQLite，并通过 `/stream` WebSocket 推送给前端
8. 达到目标交易量 / 触发风控 → 任务结束，更新状态

---

## 3. 项目目录结构

```
new-alpha-trade/
├── README.md
├── docker-compose.yml            # 一键拉起三个服务（可选）
├── .gitignore
│
├── docs/
│   ├── architecture.md           # 本文件的精简版
│   ├── api/                      # OpenAPI / WebSocket 协议
│   └── strategies.md             # 策略说明
│
├── qr-service/                   # Python 扫码服务
│   ├── pyproject.toml            # uv/pdm 管理
│   ├── README.md
│   ├── src/
│   │   └── qr_service/
│   │       ├── __init__.py
│   │       ├── main.py           # FastAPI 入口
│   │       ├── playwright_login.py
│   │       ├── token_refresh.py
│   │       ├── storage.py        # 写 SQLite accounts 表
│   │       └── api/
│   │           ├── qr.py
│   │           └── auth.py
│   └── tests/
│
├── trading-engine/               # Rust 交易引擎（workspace）
│   ├── Cargo.toml                # workspace 清单
│   ├── README.md
│   ├── crates/
│   │   ├── binance-alpha/        # Binance Alpha API 客户端
│   │   │   ├── Cargo.toml
│   │   │   └── src/
│   │   │       ├── lib.rs
│   │   │       ├── rest.rs       # REST 客户端（带签名/headers）
│   │   │       ├── ws.rs         # WebSocket（订单簿 + 用户订单）
│   │   │       └── types.rs      # 共享类型
│   │   ├── strategy/             # 策略层
│   │   │   ├── Cargo.toml
│   │   │   └── src/
│   │   │       ├── lib.rs
│   │   │       ├── traits.rs     # Strategy trait
│   │   │       └── v1/           # 新策略 v1
│   │   ├── risk/                 # 风控
│   │   │   ├── Cargo.toml
│   │   │   └── src/lib.rs
│   │   ├── persistence/          # SQLite 持久化
│   │   │   ├── Cargo.toml
│   │   │   └── src/
│   │   │       ├── lib.rs
│   │   │       ├── migrations/   # sqlx migrations
│   │   │       └── repo.rs
│   │   └── api/                  # HTTP + WS 服务
│   │       ├── Cargo.toml
│   │       └── src/
│   │           ├── lib.rs
│   │           ├── http.rs       # axum routes
│   │           └── ws.rs         # WebSocket 推送
│   └── src/
│       └── main.rs               # 主进程入口
│
├── web-ui/                       # React 前端
│   ├── package.json
│   ├── vite.config.ts
│   ├── tsconfig.json
│   ├── tailwind.config.ts
│   ├── index.html
│   └── src/
│       ├── main.tsx
│       ├── App.tsx
│       ├── api/                  # API 客户端封装
│       │   ├── trading.ts
│       │   ├── qr.ts
│       │   └── ws.ts
│       ├── pages/
│       │   ├── Dashboard.tsx
│       │   ├── Accounts.tsx
│       │   ├── Trade.tsx
│       │   └── Strategy.tsx
│       ├── components/
│       │   ├── OrderStream.tsx
│       │   ├── ProgressCard.tsx
│       │   └── ConfigForm.tsx
│       └── store/                # zustand 状态管理
│
├── data/
│   ├── new-alpha-trade.db        # SQLite（运行时生成）
│   └── logs/                     # 日志
│
└── scripts/
    ├── dev.sh                    # 同时拉起三个服务
    └── init-db.sh                # 初始化 SQLite schema
```

---

## 4. API / 接口设计

### 4.1 qr-service（Python，端口 7001）

| 方法 | 路径 | 用途 |
|---|---|---|
| POST | `/qr/login` | `{username}` 启动浏览器扫码，返回 `{qr_image_url, session_id}` |
| GET  | `/qr/status/{session_id}` | 轮询扫码状态：`pending / scanned / success / expired` |
| GET  | `/auth/{username}` | 返回该用户最新 cookies+headers（给 trading-engine 用） |
| POST | `/auth/{username}/refresh` | 强制刷新 token / cookies |
| GET  | `/accounts` | 列出所有已登录账户和 token 状态 |
| DELETE | `/accounts/{username}` | 删除账户 |

### 4.2 trading-engine（Rust，端口 7002）

| 方法 | 路径 | 用途 |
|---|---|---|
| POST | `/trade/start` | `{username, symbol, strategy, target_volume, params}` 启动一个 TradingJob |
| POST | `/trade/pause/{job_id}` | 暂停 |
| POST | `/trade/resume/{job_id}` | 恢复 |
| POST | `/trade/stop/{job_id}` | 停止 |
| GET  | `/trade/status/{job_id}` | 当前进度、累计成交量、盈亏 |
| GET  | `/trade/jobs` | 列出所有 job |
| GET  | `/orderbook/{symbol}` | 当前订单簿快照（调试用） |
| GET  | `/strategies` | 列出可用策略元数据（参数 schema） |
| WS   | `/stream` | 服务端推送：`order_event / trade_event / status / log` |

### 4.3 WebSocket `/stream` 消息格式

```json
{ "type": "order_event", "job_id": "...", "data": { "side":"buy", "price":..., "qty":..., "status":"filled" } }
{ "type": "trade_event", "job_id": "...", "data": { "buy":..., "sell":..., "pnl":..., "wear_ratio":... } }
{ "type": "status",      "job_id": "...", "data": { "volume_done":..., "target":..., "pct":... } }
{ "type": "log",         "level": "info|warn|error", "msg": "..." }
```

### 4.4 SQLite Schema

```sql
CREATE TABLE accounts (
    username       TEXT PRIMARY KEY,
    cookies_json   TEXT NOT NULL,
    headers_json   TEXT NOT NULL,
    twofa_secret   TEXT,
    last_refresh   TIMESTAMP,
    status         TEXT          -- active / expired
);

CREATE TABLE strategies (
    name           TEXT PRIMARY KEY,
    version        TEXT,
    params_schema  TEXT         -- JSON Schema
);

CREATE TABLE jobs (
    id             TEXT PRIMARY KEY,
    username       TEXT REFERENCES accounts(username),
    symbol         TEXT,
    strategy       TEXT,
    params_json    TEXT,
    target_volume  REAL,
    state          TEXT,        -- pending/running/paused/done/failed/stopped
    created_at     TIMESTAMP,
    updated_at     TIMESTAMP
);

CREATE TABLE trades (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id         TEXT REFERENCES jobs(id),
    cycle_no       INTEGER,
    buy_order_id   TEXT,
    sell_order_id  TEXT,
    buy_price      REAL,
    sell_price     REAL,
    quantity       REAL,
    pnl            REAL,
    wear_ratio     REAL,
    ts_buy         TIMESTAMP,
    ts_sell        TIMESTAMP
);

CREATE TABLE orders (
    order_id       TEXT PRIMARY KEY,
    job_id         TEXT REFERENCES jobs(id),
    side           TEXT,
    price          REAL,
    qty            REAL,
    status         TEXT,         -- pending/filled/canceled/expired
    raw_response   TEXT,
    ts             TIMESTAMP
);
```

---

## 5. Rust 实现选型

| 用途 | 选用 crate |
|---|---|
| 异步运行时 | `tokio` |
| HTTP server | `axum` |
| WebSocket server | `axum::extract::ws` |
| HTTP client | `reqwest` (with `rustls`) |
| WebSocket client | `tokio-tungstenite` |
| JSON | `serde` + `serde_json` |
| SQLite | `sqlx` (with `sqlite` 和 `runtime-tokio-rustls`) |
| 日志 | `tracing` + `tracing-subscriber` |
| 错误处理 | `thiserror` + `anyhow` |
| 时间 | `chrono` 或 `time` |
| 配置 | `figment` 或 `config` |

> **不要**用 `binance-rs` 之类的第三方 SDK，因为币安官方 SDK 不覆盖 Alpha 板块的私有端点 `/bapi/asset/v1/private/alpha-trade/...`，自己写更可控。

---

## 6. 策略重写方向（不直接搬旧 14 种）

### 6.1 策略 trait（Rust）

```rust
pub trait Strategy: Send + Sync {
    fn name(&self) -> &str;
    fn params_schema(&self) -> serde_json::Value;
    fn on_book_update(&mut self, book: &OrderBook, ctx: &Ctx) -> Option<Decision>;
    fn on_fill(&mut self, fill: &Fill, ctx: &Ctx) -> Option<Decision>;
    fn on_tick(&mut self, ctx: &Ctx) -> Option<Decision>;
}

pub enum Decision { Buy { price, qty }, Sell { price, qty }, Cancel { order_id }, Wait }
```

### 6.2 v1 默认策略：`adaptive_maker`

基本思想（待用户细化）：
- 用实时订单簿计算「微观价格」 = 中间价 + 加权失衡偏移
- 买在微观价 - dynamic_offset，卖在微观价 + dynamic_offset
- `dynamic_offset` 根据近 N 秒成交速度自适应（成交快 → 减小，成交慢 → 加大）
- 持仓超过 `max_hold_ms` 没成交 → 退化为市价清仓
- 全程实时由 WebSocket 驱动，REST 只用来下单/查询

> ⚠️ **此小节内容仅为占位框架，正式策略需要单独写一份 `docs/strategies-v1.md` 给用户审批**

---

## 7. 迁移路线图（Phased）

| 阶段 | 内容 | 交付物 |
|---|---|---|
| **P0** | 初始化项目骨架 | new-alpha-trade 目录、三个子项目脚手架、空 schema、README |
| **P1** | 扫码服务迁移 | qr-service 跑通：扫码 → 存 SQLite → 暴露 `/auth/{user}` |
| **P2** | Rust 引擎骨架 | trading-engine 起 axum server、对接 SQLite、调通 binance-alpha REST 客户端 |
| **P3** | WebSocket 订单簿 | binance-alpha::ws 实现订阅订单簿，OrderBookFeed 内存模型 |
| **P4** | 第一个策略 + 端到端跑通 | adaptive_maker v1 + 全链路：前端按钮 → 启动 job → 出成交记录 |
| **P5** | Web UI 完整页面 | Dashboard / Accounts / Trade / Strategy 四页 + 实时 stream |
| **P6** | 风控 + 暂停/紧急停止 | risk crate + 资金亏损阈值 + 文件信号 |
| **P7** | 多账户并行 | 同时跑 N 个 job，资源隔离与限流 |
| **P8** | 优化与压测 | 延迟监控、错误重试、批量持仓清理 |

每阶段结束都跑一次 **Ralph Loop** 验收。

---

## 8. 旧代码迁移映射表

| 旧文件 | 新位置 / 命运 |
|---|---|
| `trading_agent.py` 中 Alpha 核心（`run_trading_cycle`、`run_oto_trading_cycle`、`place_order`、`get_alpha_token_*`、`calculate_trading_prices`、订单簿调用） | 重写为 Rust，分布到 `binance-alpha` / `strategy` |
| `trading_agent.py` 中现货 / 合约 / 数据导出 | ❌ 丢弃 |
| `websocket_orderbook.py` | 重写为 Rust `binance-alpha::ws::OrderBook` |
| `order_manager.py` | 重写为 Rust `binance-alpha::ws::UserOrder` |
| `wear_manager.py` | 简化为 Rust `risk::wear` 模块（盈亏比 = wear，写入 SQLite trades 表） |
| `web_service/web_qr_server.py` | 大幅瘦身后迁到 `qr-service/`，去掉团队/交易记录上传/管理面板等无关功能 |
| `server_qr_refresh_daemon.py` | 重写为 `qr-service/token_refresh.py` 后台任务 |
| `qr_service/qr_server.py` `qr_server_v2.py` `qr_refresh_daemon*.py` | ❌ 丢弃 |
| GUI（所有 `gui/`） | ❌ 全部丢弃 |
| `core/*.py` | 大部分丢弃；账户配置概念合并到 SQLite `accounts` 表 |
| `alpha_order_monitor.py` `strategy_simulator.py` `strategy_test_record.py` | 业务逻辑参考，代码不复用；新项目里订单监控直接走 WebSocket + SQLite |
| `query_api_server.py` | 功能合并到 trading-engine 的 HTTP API |
| `auth_record_manager.py` | 简化为 qr-service 内部模块 |

---

## 9. Trade-offs（替代方案与权衡）

| 决策 | 选定 | 拒绝的替代 | 理由 |
|---|---|---|---|
| 后端语言 | Rust | Python（保持现状）/ Go | 用户明确要求；Rust 在 WebSocket 性能 + 类型安全上最优 |
| 前端 | React + Vite + TS | Vue / 原生 HTML / Yew | 生态最完备、有 shadcn/ui 现成、用户已选 |
| 扫码 | Python + Playwright | Rust + chromiumoxide | Playwright 生态成熟，浏览器自动化坑少 |
| 进程间通信 | HTTP + WebSocket | gRPC / Unix Socket / 共享数据库 | 跨语言友好、调试简单、前端直连不用网关 |
| 存储 | SQLite | PostgreSQL / 文件 | 零运维、单机够用、Rust+Python 都有成熟客户端 |
| 单进程 vs 多进程 | 多进程（三服务） | 单 Rust 进程内嵌 Python | 扫码用 Playwright 必须独立进程；解耦后部署/调试更灵活 |
| 配置 | SQLite + 环境变量 | YAML/TOML 文件 | 账户/策略可热加载、UI 可修改 |
| 策略 | 重写 v1 `adaptive_maker` | 移植旧 14 种 | 用户明确要求重写；旧策略本质是滑点微调 |

---

## 10. Risks & Mitigations

| 风险 | 影响 | 缓解 |
|---|---|---|
| Binance Alpha API 私有签名格式不公开，Rust 重写时签名出错 | 阻断 P2 | P2 阶段先用 Python 已验证的请求作为黄金参考，逐字段比对 |
| Playwright 浏览器进程死掉、cookies 过期 | 扫码服务挂掉 | `token_refresh.py` 心跳 + 自动重启浏览器 + 失败时主动通知前端重新扫码 |
| WebSocket 断线导致订单簿状态过期 | 策略下单价格错误 | 心跳 + 重连 + 重连后强制 REST 拉一次快照对齐；状态过期时拒绝下单 |
| 策略 v1 设计不当导致刷量效率不如旧版 | 项目失去意义 | 先做 A/B：旧 Python 跑一组 + 新 Rust adaptive_maker 跑一组，目标交易量相同对比 wear ratio |
| SQLite 并发写瓶颈 | 多账户并行时拖慢 | 一个 Job 一个连接 + WAL 模式；后期可平移到 Postgres |
| 旧策略中隐含的"业务参数知识"丢失 | 容易踩同样的坑 | 把旧 `trading_agent.py` 的关键常量（滑点档位、超时时长、暂停阈值）原样抄到 `docs/legacy-parameters.md` 作为参考 |
| 用户在迁移期间还要继续跑旧系统 | 不能直接删旧代码 | 旧项目保留原样；新项目独立目录；扫码 cookies 文件格式保持兼容 |

---

## 11. 待用户审批的开放问题

1. **策略 v1 方向**：本文档第 6.2 节是占位，正式策略要单独写。当前的 `adaptive_maker` 思路（订单簿失衡 + 自适应 offset）你认可吗？还是另有方向（比如基于成交量加权、跟单某地址、特定时段策略等）？
2. **是否需要兼容旧 cookies 文件**：是否要让 qr-service 在迁移期同时支持读旧的 `/root/new_account_headers_{username}.txt`？
3. **多账户上限**：预计同时跑几个账户？这影响 Rust 并发设计和 SQLite 模式（默认 vs WAL）。
4. **部署目标**：~~待定~~ → 已定：**云服务器 `<your-server-ip>`（Ubuntu，root SSH 已开）**。三服务同机部署。Docker 化暂不强制，待 P0 阶段再决定（基础架构能 systemd + bash 起就先用，等多机/多账户再上 docker-compose）。
5. **是否需要鉴权**：Web UI 需要登录保护吗？还是只在本地用、信任内网？
6. **策略测试**：要不要做策略回测/模拟器？旧项目有 `strategy_simulator.py`，但不一定能直接用。

---

## 12. 下一步

用户确认本设计文档后，按 P0 开始：
1. 在 `E:\code-program\new-alpha-trade` 初始化目录结构
2. 写三个子项目的最小可运行骨架（FastAPI hello、axum hello、Vite dev server）
3. 提交第一个 commit（如果走 git）
4. 推进 P1 扫码服务迁移

如果用户对设计有调整意见，**回到本文档修订**，不直接动代码。

---

## 附录 A：Binance Alpha 接口实地调研（2026-05-21）

通过浏览器 DevTools 抓取实际页面 `https://www.binance.com/zh-CN/alpha/bsc/0x365de036a1f7dccb621530d517133521debb2013`（代币 ALPHA_971/NEX）真实发起的请求。

### A.1 端点全清单（与旧代码对照）

| 端点 | Method | 旧代码 (trading_agent.py) | 当前页面 | 状态 |
|---|---|---|---|---|
| `/bapi/asset/v1/private/alpha-trade/order/place` | POST | ✅ L2676 | ✅ | 沿用 |
| `/bapi/asset/v1/private/alpha-trade/oto-order/place` | POST | ✅ L2836 | ✅ | 沿用 |
| `/bapi/defi/v1/private/alpha-trade/order/cancel` | POST | ✅ L4435 | ✅ | 当前页面使用 defi/v1 路径 |
| `/bapi/asset/v1/private/alpha-trade/order/get-order-detail` | GET | ✅ L4190 | ⏳ 待抓 | 沿用 |
| `/bapi/defi/v1/private/alpha-trade/order/cancel-all` | POST | ✅ L3173 | ⏳ 待抓 | 沿用 |
| `/bapi/defi/v1/private/alpha-trade/order/get-open-order` | GET | ✅ L3834 | ✅ | 沿用 |
| `/bapi/defi/v1/private/alpha-trade/order/get-order-history-web` | GET | ✅ L1087 | ⏳ 待抓 | 沿用 |
| `/bapi/defi/v1/private/alpha-trade/order/get-order-history-merge` | GET | ✅ L2229 | ✅ | 沿用 |
| `/bapi/defi/v1/private/alpha-trade/order/get-user-trades` | GET | ✅ L1188 | ✅ | 沿用；展开历史委托行触发 |
| `/bapi/defi/v1/private/alpha-trade/get-listen-key` | POST | ✅ L218 | ✅ | 沿用 |
| `/bapi/defi/v1/public/alpha-trade/fullDepth` | GET | ✅ L4532 | ✅ | 沿用 |
| `/bapi/defi/v1/public/alpha-trade/agg-klines` | GET | ✅ L4468 | ✅ | 沿用 (新增 1s 粒度) |
| `/bapi/defi/v1/public/alpha-trade/aggTicker24` | GET | ❌ | ✅ | **🆕 新增** |
| `/bapi/defi/v1/public/alpha-trade/agg-trades` | GET | ❌ | ✅ | **🆕 新增 - 关键** |
| `/bapi/defi/v1/public/alpha-trade/get-exchange-info` | GET | ❌ | ✅ | **🆕 新增** |
| `/bapi/defi/v1/public/alpha-trade/get-fee-rate` | GET | ❌ | ✅ | **🆕 新增** |
| `/bapi/defi/v1/public/alpha-trade/get-from-asset` | GET | ❌ | ✅ | **🆕 新增** |
| `/bapi/defi/v1/private/wallet-direct/cloud-wallet/alpha` | GET | ❌ | ✅ | **🆕 钱包余额** |
| `/bapi/defi/v1/private/wallet-direct/swap/terms-agreement` | GET | ❌ | ✅ | **🆕 协议同意** |
| `/bapi/defi/v1/public/wallet-direct/buw/wallet/cex/alpha/chain/list` | GET | ❌ | ✅ | **🆕 链列表** |
| `/bapi/defi/v1/public/wallet-direct/buw/wallet/cex/alpha/token/full/info` | GET | ❌ | ✅ | **🆕 代币完整信息 - 关键** |

**结论**：旧代码端点路径**仍然有效**。新项目需新增 ≥ 9 个端点的客户端封装，主要是行情元信息和钱包查询。

### A.2 下单 Payload 真实差异（关键！）

#### `/bapi/asset/v1/private/alpha-trade/order/place`

旧代码（trading_agent.py L2719-2729）：
```python
{
  "baseAsset": alpha_id,        # 如 "ALPHA_971"
  "quoteAsset": "USDT",
  "side": "BUY",
  "price": 0.000005933,          # 数字
  "quantity": 1685487.9,         # 数字
  "paymentDetails": [{
    "amount": 9.99999971,        # ⚠️ 数字
    "paymentWalletType": "CARD"  # BUY=CARD, SELL=ALPHA
  }]
  # ⚠️ 没有 orderType 字段
}
```

当前网页实际发的：
```json
{
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "side": "BUY",
  "price": 0.000005933,
  "quantity": 1685487.9,
  "paymentDetails": [{
    "amount": "9.99999971",       // ✅ 字符串（高精度安全）
    "paymentWalletType": "CARD"
  }],
  "orderType": "LIMIT"            // ✅ 显式声明类型
}
```

**Rust 重写时必须**：
1. `amount` 用字符串序列化（`serde` 直接用 `String` 字段类型，或自定义 serializer 把 `Decimal` 序列化成字符串）
2. 必须发 `orderType: "LIMIT"`
3. `price` / `quantity` 用 `rust_decimal::Decimal` 或类似精确类型，不要 f64

#### `/bapi/asset/v1/private/alpha-trade/oto-order/place`

网页实际：
```json
{
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "workingSide": "BUY",
  "workingPrice": 0.000005929,
  "workingQuantity": 1686625,
  "paymentDetails": [{
    "amount": "9.99999962",
    "paymentWalletType": "CARD"
  }],
  "pendingPrice": 0.000005924,    // ⚠️ 注意：pendingPrice 是关联单的目标价
  "pendingType": "LIMIT"          // ⚠️ 没有 pendingQuantity（系统自动用成交量）
}
```

`workingSide=BUY` + `pendingPrice<workingPrice` → 当前抓到的这单是一个**反向 OTO**（先买，成交后挂卖单价格比买入价**低**），是经典的"刷量"模式。旧代码 `place_oto_order` 逻辑相符。

### A.3 K 线接口签名（旧代码遗漏点）

```
GET /bapi/defi/v1/public/alpha-trade/agg-klines
?chainId=56                          ← BSC=56, Base=8453
&tokenAddress=0x365de036a1f7dccb...  ← 合约地址（不再是 alpha_id）
&interval=1s|1m|15m|1h|1d            ← 1s 是新粒度
&limit=500
&dataType=aggregate
```

**重要发现**：
- 行情接口现在用 `tokenAddress + chainId` 索引，**不再用 alpha_id**
- 新增 `1s` 粒度 K 线 — 对 Rust 高频策略有用
- 旧代码 `get_klines()` 没用过 1s，新策略可以基于此设计

### A.4 抓包补遗状态（完整成果见姊妹文档）

完整原始记录见 [`docs/design/alpha-network-capture-2026-05-21.md`](alpha-network-capture-2026-05-21.md)（22 KB），下面只摘录与设计强相关的事实变更：

#### A.4.1 撤单路径变了 ⚠️

```diff
- 旧代码 trading_agent.py L4435:
- POST /bapi/asset/v1/private/alpha-trade/order/cancel
+ 当前网页实际:
+ POST /bapi/defi/v1/private/alpha-trade/order/cancel
```

**body**：仅需 `{"orderId":"...", "symbol":"ALPHA_xxxUSDT"}`，**无需 `orderListId`**。
**风险**：旧代码 `asset` 路径若已被币安废弃则**撤单功能损坏**；Rust 重写时必须用 `defi` 路径。

#### A.4.2 OTO 响应结构（旧代码可能漏处理）

```json
{
  "code": "000000",
  "data": {
    "workingOrderId": 2464786,     // 数字，不是字符串
    "pendingOrderId": 2464787       // 数字，不是字符串
  },
  "success": true
}
```

普通单的 `data` 是订单 ID 字符串（如 `"2459886"`），OTO 是对象包两个数字 ID — Rust 端必须用 `serde::Untagged` 或两个不同的响应类型分别反序列化。

#### A.4.3 历史订单查询（多了一条路径）

```http
GET /bapi/defi/v1/private/alpha-trade/order/get-order-history-merge
?page=1&rows=50
&orderStatus=FILLED,PARTIALLY_FILLED,EXPIRED,CANCELED,REJECTED
&startTime=...&endTime=...&kind=LIMIT
```

OTO 单在历史里**会被拆成 2 行**，靠 `orderListId` 关联，靠 `contingencyOrderPosition=OTO_WORKING/OTO_PENDING` 区分。

#### A.4.4 get-exchange-info 的 filters 是新项目必须遵守的（防被币安拒单）

```json
{
  "filters": [
    { "filterType": "PRICE_FILTER", "tickSize": "0.000000001" },
    { "filterType": "LOT_SIZE",     "stepSize": "0.10", "minQty": "0.10" },
    { "filterType": "MIN_NOTIONAL", "minNotional": "0.1" }
  ],
  "orderTypes": ["LIMIT"]
}
```

**Rust `strategy` crate 在生成订单前必须**：
1. 价格按 `tickSize` 向下取整
2. 数量按 `stepSize` 向下取整
3. 总额 ≥ `minNotional`
4. 仅生成 `orderTypes` 中列出的类型

旧代码 `place_order` 里 `quantity = math.floor(quantity * 100) / 100` 是硬编码两位小数，**没去拉 exchangeInfo 的 stepSize** — 这是埋雷。

### A.5 WebSocket 架构（已确认）

#### A.5.1 端点 & 用途分工

| URL | 用途 |
|---|---|
| `wss://nbstream.binance.com/w3w/wsa/stream` | 公开 Alpha 市场流（depth / aggTrade） |
| `wss://nbstream.binance.com/w3w/stream` | **私有用户流**（`alpha@<listen_key>`） |
| `wss://nbstream.binance.com/market?token=<market_ws_token>` | K 线 + 全币种 ticker（`came@...`） |
| `wss://stream.binance.com/stream` | 通用 binance 现货流（兼容） |

#### A.5.2 订阅频道速查

| 频道名 | 类型 | 推荐用途 |
|---|---|---|
| `alpha_<id>usdt@fulldepth@500ms` | 深度增量 | 维护本地 OrderBook，策略主输入 |
| `alpha_<id>usdt@aggTrade` | 聚合成交 | 最新成交价、短期成交强度信号 |
| `came@alpha_<id>@short@kline_1s` | 1s K 线 | 趋势/波动率特征 |
| `came@allTokens@ticker24` | 24h 全币种 ticker | 扫币、排行（**不适合**高频核心） |
| `came@stockToken@metaInfo@change` | 代币元信息变更 | 上下架/状态切换告警 |
| `alpha@<alpha_listen_key>` | **私有订单事件** | 实时知道下单/成交/撤单结果 |

#### A.5.3 关键消息结构（已抓样本）

**深度增量 `depthUpdate`**：
```json
{ "e":"depthUpdate", "U":..., "u":..., "pu":...,
  "b":[["price","qty"]], "a":[["price","qty"]] }
```
- 用 `U/u/pu` 做序列号对齐（与币安主网现货协议一致）
- `qty=0.00` 表示删除该档

**聚合成交 `aggTrade`**：
```json
{ "e":"aggTrade", "p":"价格", "q":"数量", "T":时间戳, "m":false }
```

**K 线 `kline`**：
```json
{ "e":"kline", "ca":"<contract>@<chainId>",
  "k":{ "i":"1s", "ot":..., "ct":..., "o":..., "h":..., "l":..., "c":..., "v":... } }
```

完整样本见姊妹抓包档第 7 节。

#### A.5.4 已挂载但**尚未抓到事件体**的私有用户流

- 订阅 `alpha@<alpha_listen_key>` 已确认 ACK 成功
- **真实订单事件 JSON 还没抓到**（需要在订阅状态下发起新下单/撤单）
- 集成时让扫码服务/前端在订阅期间触发一次试单，再补 docs

### A.6 影响新项目设计的关键调整（最终版）

1. **`binance-alpha` crate 端点数**：约 **23 个 REST + 6 个 WS 流**（原计划严重低估）
2. **数据类型**：交易相关字段用 `rust_decimal::Decimal`，订单响应 ID 用 `serde` 同时支持 string 和 number
3. **撤单路径**：必须用 `defi/v1` 而非旧代码的 `asset/v1`
4. **下单 payload**：`amount` 字符串、`orderType:"LIMIT"`、SELL 时 amount 是数量数字而非引号字符串
5. **必须遵守 exchangeInfo filters**：tickSize / stepSize / minNotional / orderTypes
6. **代币元数据**：通过 `token/full/info` + `aggTicker24` 动态拉取
7. **手续费精算**：用 `get-fee-rate`（注意是基点 `100` = 0.10%）
8. **价格源优先级**：WS aggTrade > WS kline_1s close > REST aggTicker > REST agg-trades > REST fullDepth 中间价
9. **OrderBook 同步协议**：REST fullDepth 拉快照 → WS depthUpdate 增量（U/u/pu 对齐）
10. **listen-key 续期**：参考币安通用规则，后台任务 25 分钟续期一次
11. **历史订单 OTO 拆分**：靠 `orderListId` + `contingencyOrderPosition` 关联两行

### A.7 仍需补抓的（不阻塞 P0–P2）

- ⏳ `alpha@<listen_key>` 私有流的真实订单事件 payload — P3 阶段集成时随手抓
- ⏳ `POST /order/cancel-all` 批量撤单 payload — P3 阶段需要时抓
- ⏳ `GET /order/get-order-detail` query 参数 — P3 阶段需要时抓
- ⏳ 失败下单的响应体（被风控/2FA 拦截时的 error code/message 完整列表）— P6 阶段需要时抓

---

## 附录 B：部署架构（目标：<your-server-ip>）

### B.1 旧项目部署模式回顾（哪些借鉴、哪些丢弃）

旧 `oneserver-binance` 的部署机制：

| 维度 | 旧做法 | 新做法 | 理由 |
|---|---|---|---|
| 传输 | paramiko SFTP 单文件推送 + 3 次重试 | **rsync over SSH** | 增量上传、目录同步、速度快；保留 paramiko 仅用于一次性命令（systemctl 操作） |
| 目录 | 全堆 `/root/` 平铺 | **`/opt/new-alpha-trade/`** 分层 | 符合 FHS、易备份、不污染 root 主目录 |
| 进程管理 | GNU Screen 会话 `screen -dmS trading_{user}` | **systemd unit**（3 个服务） | 自动重启、开机自启、journalctl 标准日志、Ubuntu 原生支持 |
| 多账户隔离 | 单进程多账户（命令行参数传 JSON config） 或 每账户一个 Screen 会话 | **单进程内多 Job**（tokio 任务级隔离） | Rust 引擎天然支持并发，无需多进程；账户 = SQLite 行，Job = 内存任务 |
| 配置 | `/root/new_account_headers_{user}.txt` 明文 + 命令行 JSON | **SQLite `accounts` 表 + `/etc/new-alpha-trade/secrets.env`** | 集中、可加密、systemd 标准 EnvironmentFile |
| 信号控制 | `pause_signal_{user}.flag` / `stop_signal_{user}.flag` 文件 | **HTTP API `/trade/pause/{job_id}`** | 通过 Web UI 直接控制，无需登录服务器 |
| 状态查询 | SSH 远程跑命令 + `tail` 日志文件 | **HTTP API `/trade/jobs` 远程拉** + journalctl 备用 | 前端实时刷新，无需 SSH |
| 更新流程 | 先 `rm -f /root/*.py` 清理再上传 | **rsync `--delete` 同步 → systemctl restart** | 原子性更好 |

**借鉴保留的**：
- ✅ 文件标志信号思路（演化成 SQLite `jobs.state` 列，让程序自检）
- ✅ paramiko 做一次性远程命令（重启、看日志）
- ✅ 部署前先停服务（systemd `stop` 比 Screen `quit` 更可控）

**完全丢弃的**：
- ❌ Screen 会话管理（systemd 替代）
- ❌ `/root/{filename}_{username}.txt` 命名约定（SQLite 一切）
- ❌ `qr_refresh_daemon.py` / `qr_refresh_daemon2.py` 双 daemon（已合并，且改成 systemd 服务）
- ❌ 团队配置 / 多团队密码差异管理（单机单用户场景不需要）
- ❌ Web 批量部署面板（部署是低频运维，不需要 UI）

### B.2 服务器目录布局

```
/opt/new-alpha-trade/
├── qr-service/                    # Python 应用
│   ├── .venv/                     # virtualenv
│   ├── src/qr_service/            # 代码
│   └── pyproject.toml
│
├── trading-engine                  # Rust 单二进制（cargo build --release 产物）
│
├── web-ui/
│   └── dist/                      # Vite build 产物，nginx serve
│
├── data/                          # 运行时数据
│   ├── new-alpha-trade.db         # SQLite
│   ├── logs/                      # 应用日志（systemd journalctl 兼用）
│   └── playwright-state/          # Playwright 浏览器 user-data-dir
│
└── README.md                      # 部署说明

/etc/new-alpha-trade/
├── qr-service.env                 # PORT=7001, DB_PATH=..., 等
├── trading-engine.env             # PORT=7002, DB_PATH=..., LOG_LEVEL=info
└── secrets.env                    # gitignored 凭据（cookies 加密 key 等）

/etc/systemd/system/
├── nat-qr-service.service
├── nat-trading-engine.service
└── nat-nginx.service              # 或复用现成 nginx 配置 web-ui/

/etc/nginx/sites-available/new-alpha-trade
                                   # 反向代理：
                                   #   /         → web-ui dist
                                   #   /api/trade → :7002
                                   #   /api/qr    → :7001
                                   #   /ws/*      → :7002 (Upgrade)
```

### B.3 systemd unit 设计

**`nat-qr-service.service`**（示意）：
```ini
[Unit]
Description=new-alpha-trade QR Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=newalpha
Group=newalpha
WorkingDirectory=/opt/new-alpha-trade/qr-service
EnvironmentFile=/etc/new-alpha-trade/qr-service.env
EnvironmentFile=/etc/new-alpha-trade/secrets.env
ExecStart=/opt/new-alpha-trade/qr-service/.venv/bin/uvicorn qr_service.main:app --host 127.0.0.1 --port ${PORT}
Restart=on-failure
RestartSec=5s
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

**`nat-trading-engine.service`** 同构，`ExecStart` 改成 `/opt/new-alpha-trade/trading-engine`。

> 注：所有 systemd unit 命名前缀 `nat-`（new-alpha-trade 缩写），避免与其它 `binance-*` / `trading-*` unit 重名。

### B.4 部署脚本（5 个文件，对齐旧项目建议）

| 文件 | 用途 | 旧项目对应 |
|---|---|---|
| `scripts/deploy.sh` | 一键部署：rsync 代码 + systemctl restart + 健康检查 | 旧 `deploy_panel.py` 的核心逻辑 |
| `scripts/install-server.sh` | **首次**初始化服务器：apt 装依赖、创建 `newalpha` 用户、装 systemd unit、配 nginx | 旧无对应（旧靠手动） |
| `scripts/server-status.sh` | 远程拉三服务状态（systemctl status + curl 健康检查） | 旧 SSH + tail 日志 |
| `scripts/server-logs.sh <service>` | 远程 tail journalctl | 旧 SSH + tail 日志 |
| `scripts/config/servers.json` | 服务器清单（IP / SSH 用户 / SSH key 路径） | 旧 `config/servers.txt` |

**所有脚本默认从 `~/.ssh/new-alpha-trade.key` 读 SSH key**，不接受明文密码（密码模式只在 `tools/change_root_password.py` 这种一次性运维场景用）。

### B.5 第一次部署 checklist

1. **本机**：`cargo build --release` + `npm run build`（Rust 二进制 + 前端静态）
2. **本机**：`scripts/install-server.sh <your-server-ip>`
   - 在服务器创建 `newalpha` 用户、装 Python 3.11 / nginx / sqlite / playwright deps
   - 装 systemd unit、nginx 配置
3. **本机**：`scripts/deploy.sh <your-server-ip>`
   - rsync `qr-service/` `trading-engine` `web-ui/dist/` 到 `/opt/new-alpha-trade/`
   - `systemctl restart nat-qr-service nat-trading-engine`
   - curl 健康检查
4. **本机**：`scripts/server-status.sh` 验证三服务全绿
5. **浏览器**访问 `http://<your-server-ip>/`，跑首个扫码任务

### B.6 安全基线（首次部署必做）

- [ ] 立刻把 root SSH 密码改成 ≥16 位强密码或**直接禁用密码登录**（`PasswordAuthentication no`）
- [ ] 配置 `ufw`：仅开 22 / 80 / 443，其它端口（7001/7002）只绑 `127.0.0.1`
- [ ] nginx 配 Basic Auth 或 IP 白名单保护 Web UI（无内置鉴权）
- [ ] `secrets.env` 文件权限 `600`，属主 `newalpha`
- [ ] SQLite 文件权限 `600`
- [ ] 配 letsencrypt 上 HTTPS（如有域名）

这些不写进设计文档主要章节，作为部署 README 附录。

### B.7 后续阶段对接

- **P0** 创建 `scripts/deploy.sh` 和 `scripts/install-server.sh` 骨架（空逻辑，只打印 echo）
- **P1** 扫码服务跑通后，写 `nat-qr-service.service` unit 并接入 `install-server.sh`
- **P2** 引擎跑通后，同上加 `nat-trading-engine.service`
- **P5** Web UI 完整后，接入 nginx 反代
- **P8** 优化：用 Docker 化或者保持 systemd 都行（小规模 systemd 已经够）
