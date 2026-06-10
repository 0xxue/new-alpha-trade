# qr-service

Binance 扫码登录服务（Playwright + FastAPI）。

## 本地运行

```bash
python -m venv .venv && .venv/Scripts/activate   # Windows
# source .venv/bin/activate                      # Linux/Mac
pip install -e ".[dev]"
playwright install chromium                       # 首次

uvicorn qr_service.main:app --port 7001 --reload
```

打开 http://localhost:7001/docs 看 OpenAPI。

## 端点（计划）

| 方法 | 路径 | 状态 |
|---|---|---|
| GET  | `/health` | ✅ P0 |
| POST | `/qr/login` | ⏳ P1 |
| GET  | `/qr/status/{session_id}` | ⏳ P1 |
| GET  | `/auth/{username}` | ⏳ P1 |
| POST | `/auth/{username}/refresh` | ⏳ P1 |

## 环境变量

| 名字 | 默认 | 说明 |
|---|---|---|
| `PORT` | `7001` | 监听端口 |
| `DB_PATH` | `../data/new-alpha-trade.db` | SQLite 路径（与 trading-engine 共享） |
| `PLAYWRIGHT_HEADLESS` | `false` | 扫码窗口是否无头 |
