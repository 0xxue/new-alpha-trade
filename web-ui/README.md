# web-ui

new-alpha-trade 控制面板（React + Vite + TypeScript + Tailwind）。

## 本地开发

```bash
npm install
npm run dev
```

访问 http://localhost:5173

Vite dev server 已配代理：
- `/api/qr/*` → `http://127.0.0.1:7001`（qr-service）
- `/api/*` → `http://127.0.0.1:7002`（trading-engine）
- `/ws/*` → `ws://127.0.0.1:7002`

## 构建

```bash
npm run build
# 产物在 dist/，部署时 rsync 到服务器，nginx serve
```

## 页面规划

| 路径 | 内容 | 阶段 |
|---|---|---|
| `/` | Dashboard（服务健康检查） | P0 ✅ |
| `/accounts` | 账户管理 / 扫码登录 | P1 |
| `/trade` | 交易任务 / 实时订单流 | P4 |
| `/strategy` | 策略选择 / 参数配置 | P4+ |
