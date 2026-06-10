# new-alpha-trade

A self-hosted **Binance Alpha** trading/volume tool. Python QR-login service + Rust trading engine + React control panel.

> 中文说明见下方 [中文](#中文说明)。

---

## ⚠️ Disclaimer

**This software is provided for educational and research purposes only.**

- Automated trading carries financial risk. You can lose money. Use at your own risk.
- Interacting with Binance via reverse-engineered private endpoints **may violate Binance's Terms of Service** and could result in your account being restricted or banned.
- This project is **not affiliated with, endorsed by, or connected to Binance** in any way.
- The authors accept **no liability** for any losses, account actions, or damages arising from use of this software.
- You are solely responsible for complying with the laws and regulations of your jurisdiction.

By using this software you acknowledge and accept all of the above.

---

## Architecture

| Service | Stack | Port | Role |
|---|---|---|---|
| `qr-service` | Python 3.11 + FastAPI + Playwright | 7001 | QR login / cookie maintenance / token refresh |
| `trading-engine` | Rust + tokio + axum + sqlx | 7002 | Trading engine / WebSocket market data / risk control / persistence |
| `web-ui` | React + Vite + TypeScript + Tailwind | 5173 (dev) | Control panel / live order stream |

Storage: a single SQLite file `data/new-alpha-trade.db`.

The trading engine runs an **OTO (One-Triggers-Other) smart strategy** with a decision matrix
(fast / maker-spread-capture / double-maker / wait-for-better) plus a dynamic price-laddered
liquidation fallback. Order books are maintained locally from the `@depth@100ms` incremental
WebSocket stream, with anchor-based stale-level filtering.

## Local development

```bash
# 1. qr-service
cd qr-service
python -m venv .venv && source .venv/bin/activate   # Windows: .venv\Scripts\activate
pip install -e .
uvicorn qr_service.main:app --port 7001 --reload

# 2. trading-engine (new terminal)
cd trading-engine
cargo run

# 3. web-ui (new terminal)
cd web-ui
npm install
npm run dev
```

Open http://localhost:5173

### First-time setup

1. Open the **Accounts** page, click "scan", and scan the QR with your Binance app.
   When the "stay signed in 5 days" dialog appears in the headless browser, the service auto-clicks it.
2. (Optional) Enter your 2FA TOTP secret on the account so the engine can auto-pass 2FA on order placement.
3. Go to the **Trade** page, pick a token, set volume target + per-order min/max, and start.

## Deploy to a server

See [docs/design/new-alpha-trade.md](docs/design/new-alpha-trade.md) appendix B.

```bash
# first-time provisioning (installs deps, systemd units, nginx)
scripts/install-server.sh <your-server-ip>

# subsequent deploys (rsync source, build, restart)
scripts/deploy.sh <your-server-ip>

# status / logs
scripts/server-status.sh <your-server-ip>
scripts/server-logs.sh <your-server-ip>
```

Set your own domain in `scripts/nginx/new-alpha-trade.conf` before running certbot.

## Configuration

Copy `.env.example` to `.env` and adjust as needed. The engine and qr-service read
`DB_PATH`, `QR_SERVICE_URL`, and `PORT` from the environment.

## License

[MIT](LICENSE)

---

## 中文说明

自托管的 **币安 Alpha** 刷量/交易工具。Python 扫码服务 + Rust 交易引擎 + React 控制台。

### ⚠️ 免责声明

**本软件仅供学习和研究用途。**

- 自动化交易有财务风险，可能亏损，使用风险自负。
- 通过逆向私有接口与币安交互**可能违反币安服务条款**，可能导致账户被限制或封禁。
- 本项目**与币安无任何关联、背书或连接**。
- 作者对使用本软件造成的任何损失、账户处置或损害**不承担任何责任**。
- 你需自行遵守所在司法辖区的法律法规。

使用即代表你已知晓并接受以上全部内容。

### 架构

三个服务：`qr-service`（Python 扫码，7001）、`trading-engine`（Rust 引擎，7002）、`web-ui`（React 控制台，5173）。
数据存 SQLite 单文件。引擎跑 OTO 智能策略（决策矩阵 + 阶梯降价清仓），盘口由 `@depth@100ms` 增量流本地维护。

本地开发和部署见上方英文段落。
