#!/usr/bin/env bash
# new-alpha-trade —— 一键更新（已部署过的服务器拉最新代码并重新部署）
#
# 用法（在已经跑过 server-install.sh 的服务器上，用当初那个 sudo 用户）：
#
#   curl -fsSL https://raw.githubusercontent.com/0xxue/new-alpha-trade/main/scripts/server-update.sh | bash
#
# 它会：git pull 最新代码 → 增量编译引擎 + 重建前端 → 换二进制/前端/qr 源码 → 重启服务。
# 比 server-install.sh 快很多（不重装 apt/rust/playwright）。
#
# 注意：会【重启引擎】，正在刷的 job 会自动 resume 续刷（中断约几秒）。
#       wear 基线若是旧口径(tune38 加总)会在重启后首次检查时自动校正，无需手动改库。

set -uo pipefail

APP_DIR="${APP_DIR:-/opt/new-alpha-trade}"
APP_USER="${APP_USER:-newalpha}"
SRC="${SRC:-$HOME/nat-src}"

step(){ echo; echo "==========> $*"; }
fail(){ echo "!! FAILED at: $*" >&2; exit 1; }

[ -d "$SRC/.git" ] || fail "没找到源码目录 $SRC（这台还没用 server-install.sh 部署过？先跑那个）"
sudo -n true 2>/dev/null || fail "当前用户没有免密 sudo"
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

step "1/5 拉取最新代码"
OLD=$(git -C "$SRC" rev-parse --short HEAD 2>/dev/null || echo none)
git -C "$SRC" pull --ff-only || fail gitpull
NEW=$(git -C "$SRC" rev-parse --short HEAD)
echo "  $OLD -> $NEW"

step "2/5 增量编译引擎"
( cd "$SRC/trading-engine" && cargo build --release ) || fail cargo-build

step "3/5 重建前端"
( cd "$SRC/web-ui" && npm install --no-audit --no-fund && npm run build ) || fail web-build
[ -f "$SRC/web-ui/dist/index.html" ] || fail web-dist-missing

step "4/5 换产物"
sudo cp -f "$SRC/trading-engine/target/release/trading-engine" "$APP_DIR/bin/trading-engine"
sudo rsync -a --delete --exclude '.venv' "$SRC/qr-service/" "$APP_DIR/qr-service/" || fail cp-qr
sudo rsync -a --delete "$SRC/web-ui/dist/" "$APP_DIR/web-ui/dist/" || fail cp-web
# qr 依赖若有变动也补一下（无变动会很快）
sudo "$APP_DIR/qr-service/.venv/bin/pip" install -q -e "$APP_DIR/qr-service" >/dev/null 2>&1 || true
sudo chown -R "$APP_USER:$APP_USER" "$APP_DIR"

step "5/5 重启服务"
sudo systemctl restart nat-trading-engine
sleep 4
sudo systemctl restart nat-qr-service
sudo systemctl reload nginx 2>/dev/null || true
sleep 3

EH=$(curl -s -m6 -o /dev/null -w '%{http_code}' http://127.0.0.1:7002/health)
QH=$(curl -s -m6 -o /dev/null -w '%{http_code}' http://127.0.0.1:7001/health)
echo
echo "============================================================"
if [ "$EH" = "200" ] && [ "$QH" = "200" ]; then
  echo "  ✅ 更新完成（$OLD -> $NEW），引擎/扫码 健康"
else
  echo "  ⚠️ 更新完成但健康检查异常: engine=$EH qr=$QH"
  echo "     看日志: sudo journalctl -u nat-trading-engine -n 50"
fi
echo "============================================================"
