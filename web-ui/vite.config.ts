import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// 开发态：前端 :5173 → 反代 /api → trading-engine :7002, /api/qr → qr-service :7001
// 生产态：nginx 反代，本配置只影响 dev server
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/api/qr": { target: "http://127.0.0.1:7001", changeOrigin: true, rewrite: (p) => p.replace(/^\/api\/qr/, "") },
      "/api": { target: "http://127.0.0.1:7002", changeOrigin: true, rewrite: (p) => p.replace(/^\/api/, "") },
      "/ws":  { target: "ws://127.0.0.1:7002", ws: true },
    },
  },
});
