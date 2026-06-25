#!/usr/bin/env bash
# new-alpha-trade —— 全新 Ubuntu 服务器一键部署 / 升级
#
# 在一台【全新 Ubuntu 22.04/24.04】上，用有 sudo 权限的普通用户（如 ubuntu）运行：
#
#   curl -fsSL https://raw.githubusercontent.com/0xxue/new-alpha-trade/main/scripts/server-install.sh -o nat-install.sh
#   BASIC_USER=admin BASIC_PASS=admin123456 bash nat-install.sh
#
# 想后台跑（防 SSH 掉线）：
#   BASIC_USER=admin BASIC_PASS=admin123456 nohup bash nat-install.sh > nat-install.log 2>&1 &
#   tail -f nat-install.log
#
# 它会：装系统依赖 + Rust + Node → 从 GitHub 拉代码 → 编译引擎 + 构建前端 →
#       配 Python venv + Playwright Chromium → systemd 托管 → nginx(:80)+Basic Auth → 起服务。
# 重复运行 = 拉最新代码重新部署（升级）。
#
# 注意：服务器要能访问 GitHub 和币安（海外机房 OK；大陆机房多半连不上币安，别部署）。

set -uo pipefail

REPO="${REPO:-https://github.com/0xxue/new-alpha-trade.git}"
APP_DIR="${APP_DIR:-/opt/new-alpha-trade}"
APP_USER="${APP_USER:-newalpha}"
SRC="${SRC:-$HOME/nat-src}"
BASIC_USER="${BASIC_USER:-admin}"
BASIC_PASS="${BASIC_PASS:-$(openssl rand -hex 6)}"

phase(){ echo; echo "==========> $*"; }
fail(){ echo "!! FAILED at: $*" >&2; exit 1; }

phase "0/11 检查环境"
command -v sudo >/dev/null || fail "需要 sudo"
sudo -n true 2>/dev/null || fail "当前用户没有免密 sudo"

phase "0.5 swap（防低内存编译 OOM）"
if ! sudo swapon --show | grep -q .; then
  sudo fallocate -l 2G /swapfile && sudo chmod 600 /swapfile && sudo mkswap /swapfile && sudo swapon /swapfile || echo "swap 跳过"
fi

phase "1/11 apt 系统依赖"
sudo DEBIAN_FRONTEND=noninteractive apt-get update -y >/dev/null || fail apt-update
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
  python3 python3-venv python3-pip nginx sqlite3 \
  build-essential pkg-config libssl-dev \
  curl git ca-certificates apache2-utils >/dev/null || fail apt-install

# Node.js 20（NodeSource）。Ubuntu 22.04 的 apt nodejs 是 v12，跑不了新版 tsc/vite，必须装新的。
if ! node -v 2>/dev/null | grep -qE '^v(18|20|22|24)'; then
  echo "  安装 Node.js 20 (NodeSource) ..."
  # 先清掉 apt 的旧 nodejs/npm，否则和 NodeSource 包抢文件导致 dpkg 冲突
  sudo apt-get purge -y nodejs npm libnode-dev libnode72 >/dev/null 2>&1 || true
  sudo apt-get autoremove -y >/dev/null 2>&1 || true
  curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash - >/dev/null 2>&1 || fail nodesource
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y nodejs >/dev/null 2>&1 \
    || { sudo dpkg --configure -a >/dev/null 2>&1; sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --fix-broken nodejs >/dev/null 2>&1; } \
    || fail node-install
fi
echo "node=$(node -v) npm=$(npm -v)"

phase "2/11 Rust toolchain"
if [ ! -x "$HOME/.cargo/bin/cargo" ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal || fail rustup
fi
source "$HOME/.cargo/env"
cargo --version

phase "3/11 拉取代码（$REPO）"
if [ -d "$SRC/.git" ]; then git -C "$SRC" pull --ff-only || fail gitpull; else git clone --depth 1 "$REPO" "$SRC" || fail clone; fi

phase "4/11 编译引擎（cargo --release，首次约 5-15 分钟）"
( cd "$SRC/trading-engine" && cargo build --release ) || fail cargo-build

phase "5/11 构建前端（npm）"
( cd "$SRC/web-ui" && npm install --no-audit --no-fund && npm run build ) || fail web-build
[ -f "$SRC/web-ui/dist/index.html" ] || fail web-dist-missing

phase "6/11 应用用户 + 目录"
id -u "$APP_USER" >/dev/null 2>&1 || sudo useradd --system --create-home --shell /bin/bash "$APP_USER"
sudo mkdir -p "$APP_DIR"/bin "$APP_DIR"/qr-service "$APP_DIR"/web-ui/dist \
  "$APP_DIR"/data/qr "$APP_DIR"/data/playwright-state "$APP_DIR"/data/playwright-browsers /etc/new-alpha-trade

phase "7/11 拷贝产物"
sudo cp -f "$SRC/trading-engine/target/release/trading-engine" "$APP_DIR/bin/trading-engine"
sudo rsync -a --delete --exclude '.venv' "$SRC/qr-service/" "$APP_DIR/qr-service/" || fail cp-qr
sudo rsync -a --delete "$SRC/web-ui/dist/" "$APP_DIR/web-ui/dist/" || fail cp-web

phase "8/11 Python venv + Playwright Chromium"
[ -x "$APP_DIR/qr-service/.venv/bin/python" ] || sudo python3 -m venv "$APP_DIR/qr-service/.venv" || fail venv
sudo "$APP_DIR/qr-service/.venv/bin/pip" install -q --upgrade pip || fail pip-up
sudo "$APP_DIR/qr-service/.venv/bin/pip" install -q -e "$APP_DIR/qr-service" || fail pip-install
sudo PLAYWRIGHT_BROWSERS_PATH="$APP_DIR/data/playwright-browsers" \
  "$APP_DIR/qr-service/.venv/bin/python" -m playwright install chromium || fail pw-install
sudo "$APP_DIR/qr-service/.venv/bin/python" -m playwright install-deps chromium >/dev/null 2>&1 || true

phase "9/11 env + systemd + nginx + Basic Auth"
[ -f /etc/new-alpha-trade/trading-engine.env ] || sudo cp "$SRC/scripts/etc/trading-engine.env.example" /etc/new-alpha-trade/trading-engine.env
[ -f /etc/new-alpha-trade/qr-service.env ]     || sudo cp "$SRC/scripts/etc/qr-service.env.example" /etc/new-alpha-trade/qr-service.env
sudo cp -f "$SRC/scripts/systemd/nat-trading-engine.service" /etc/systemd/system/
sudo cp -f "$SRC/scripts/systemd/nat-qr-service.service" /etc/systemd/system/
sudo sed -i 's#ExecStart=/opt/new-alpha-trade/trading-engine$#ExecStart=/opt/new-alpha-trade/bin/trading-engine#' /etc/systemd/system/nat-trading-engine.service
sudo htpasswd -bc /etc/nginx/.htpasswd "$BASIC_USER" "$BASIC_PASS" >/dev/null 2>&1 || fail htpasswd
sudo tee /etc/nginx/sites-available/new-alpha-trade >/dev/null <<'NGINX'
server {
    listen 80 default_server;
    listen [::]:80 default_server;
    server_name _;
    client_max_body_size 8m;
    auth_basic "new-alpha-trade";
    auth_basic_user_file /etc/nginx/.htpasswd;
    location = /api/health    { auth_basic off; proxy_pass http://127.0.0.1:7002/health; proxy_set_header Host $host; }
    location = /api/qr/health { auth_basic off; proxy_pass http://127.0.0.1:7001/health; proxy_set_header Host $host; }
    root /opt/new-alpha-trade/web-ui/dist;
    index index.html;
    location / { try_files $uri $uri/ /index.html; }
    location /api/qr/ { rewrite ^/api/qr/(.*)$ /$1 break; proxy_pass http://127.0.0.1:7001; proxy_http_version 1.1; proxy_set_header Host $host; proxy_read_timeout 120s; }
    location /api/    { rewrite ^/api/(.*)$ /$1 break;    proxy_pass http://127.0.0.1:7002; proxy_http_version 1.1; proxy_set_header Host $host; proxy_read_timeout 30s; }
    location /ws/     { proxy_pass http://127.0.0.1:7002; proxy_http_version 1.1; proxy_set_header Upgrade $http_upgrade; proxy_set_header Connection "upgrade"; proxy_set_header Host $host; proxy_read_timeout 86400s; }
}
NGINX
sudo ln -sf /etc/nginx/sites-available/new-alpha-trade /etc/nginx/sites-enabled/new-alpha-trade
sudo rm -f /etc/nginx/sites-enabled/default
sudo chown -R "$APP_USER:$APP_USER" "$APP_DIR"

phase "10/11 起服务"
sudo systemctl daemon-reload
sudo nginx -t || fail nginx-conf
sudo systemctl enable nat-trading-engine nat-qr-service >/dev/null 2>&1
sudo systemctl restart nat-trading-engine; sleep 4
sudo systemctl restart nat-qr-service
sudo systemctl reload nginx 2>/dev/null || sudo systemctl restart nginx
sleep 4

phase "11/11 健康检查"
EH=$(curl -s -m6 -o /dev/null -w '%{http_code}' http://127.0.0.1:7002/health)
QH=$(curl -s -m6 -o /dev/null -w '%{http_code}' http://127.0.0.1:7001/health)
NH=$(curl -s -m6 -o /dev/null -w '%{http_code}' http://127.0.0.1/api/health)
IP=$(curl -s -m6 ifconfig.me 2>/dev/null || echo "<服务器IP>")
echo
echo "============================================================"
if [ "$EH" = "200" ] && [ "$QH" = "200" ] && [ "$NH" = "200" ]; then
  echo "  ✅ 部署成功"
else
  echo "  ⚠️ 部署完成但健康检查异常: engine=$EH qr=$QH nginx=$NH"
  echo "     看日志: sudo journalctl -u nat-trading-engine -n 50"
fi
echo "  访问:   http://$IP/"
echo "  账号:   $BASIC_USER"
echo "  密码:   $BASIC_PASS"
echo "  (确保云控制台安全组放行 80 端口)"
echo "============================================================"
