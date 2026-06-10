# trading-engine

Rust 交易引擎（axum + tokio + sqlx + tokio-tungstenite）。

## 本地开发

```bash
cargo run                              # 默认端口 7002
DB_PATH=../data/new-alpha-trade.db cargo run
RUST_LOG=debug cargo run
```

```bash
curl http://localhost:7002/health
```

## Workspace 结构

```
trading-engine/
├── src/main.rs              # 进程入口
└── crates/
    ├── binance-alpha/       # REST + WS 客户端（P2/P3）
    ├── strategy/            # 策略层 trait + 注册表（P4）
    ├── risk/                # 风控（P6）
    ├── persistence/         # SQLite + migrations（P0 schema, P2 repo）
    └── api/                 # axum HTTP + WS 路由（P0 hello, P2+ 业务）
```

## 阶段

| 阶段 | 内容 |
|---|---|
| P0 | ✅ 骨架可编译运行；`/health` 通；SQLite migration 自动跑 |
| P2 | `binance-alpha::rest` 调通 / 仓储 CRUD / `/trade/start` |
| P3 | `binance-alpha::ws` 订单簿 + 用户流 / `/stream` 推送前端 |
| P4 | 通路验证策略（中间价 ± 固定 bp）跑通端到端 |
| P5 | Web UI 完整 |
| P6 | 风控 / 暂停 / 紧急停止 |
| P7+ | 多账户并行、`adaptive_maker` v1 真策略 |
