#!/usr/bin/env bash
# 首次初始化服务器：装系统依赖 + 应用用户 + systemd unit + nginx 配置
#
# 前置：先跑过 `python scripts/bootstrap_ssh.py <host>` 配好免密。
#
# 用法:
#   scripts/install-server.sh <host>
# 例:
#   scripts/install-server.sh <your-server-ip>

set -euo pipefail

HOST="${1:?usage: install-server.sh <host>}"
SSH_USER="${SSH_USER:-root}"
APP_USER="${APP_USER:-newalpha}"
APP_DIR="${APP_DIR:-/opt/new-alpha-trade}"

PROJ_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# 统一 SSH 选项：首次自动接受 host key，禁交互密码（密码登录已用 bootstrap_ssh.py 走完）
SSH_OPTS="-o StrictHostKeyChecking=accept-new -o BatchMode=yes"
SSH="ssh ${SSH_OPTS}"
SCP="scp ${SSH_OPTS}"

echo "==============================================="
echo "  install-server.sh -> ${SSH_USER}@${HOST}"
echo "  APP_DIR=${APP_DIR}, APP_USER=${APP_USER}"
echo "==============================================="

# ----------------------------------------------------------------------
# 1. 远端：apt 装系统依赖
# ----------------------------------------------------------------------
echo
echo "[1/5] 装系统依赖（python/nginx/sqlite/build deps/curl）..."
${SSH} "${SSH_USER}@${HOST}" bash -s <<EOF
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y \\
    python3 python3-venv python3-pip \\
    nginx sqlite3 \\
    build-essential pkg-config libssl-dev \\
    curl rsync git ca-certificates \\
    ufw
echo "✓ system deps installed"
EOF

# ----------------------------------------------------------------------
# 2. 远端：装 Rust（按 newalpha 用户跑 cargo build；先用 root 装在 /opt/rust）
# ----------------------------------------------------------------------
echo
echo "[2/5] 装 Rust toolchain ..."
${SSH} "${SSH_USER}@${HOST}" bash -s <<'EOF'
set -euo pipefail
if [ ! -x "/root/.cargo/bin/cargo" ]; then
    echo "→ rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
fi
. /root/.cargo/env
cargo --version
EOF

# ----------------------------------------------------------------------
# 3. 远端：应用用户 + 目录布局
# ----------------------------------------------------------------------
echo
echo "[3/5] 应用用户 + 目录..."
${SSH} "${SSH_USER}@${HOST}" bash -s <<EOF
set -euo pipefail
if ! id -u ${APP_USER} >/dev/null 2>&1; then
    useradd --system --create-home --shell /bin/bash ${APP_USER}
    echo "✓ user ${APP_USER} created"
else
    echo "✓ user ${APP_USER} exists"
fi
mkdir -p ${APP_DIR}/{qr-service,web-ui/dist,data/logs,data/qr,data/playwright-state,data/playwright-browsers}
mkdir -p /etc/new-alpha-trade
chown -R ${APP_USER}:${APP_USER} ${APP_DIR}
chmod 700 /etc/new-alpha-trade
EOF

# ----------------------------------------------------------------------
# 4. 推 systemd unit + nginx + env 模板（用 scp，无 rsync 依赖）
# ----------------------------------------------------------------------
echo
echo "[4/5] 推 systemd unit / nginx / env ..."
${SCP} -q \
    "${PROJ_ROOT}/scripts/systemd/nat-qr-service.service" \
    "${PROJ_ROOT}/scripts/systemd/nat-trading-engine.service" \
    "${SSH_USER}@${HOST}:/etc/systemd/system/"

${SCP} -q \
    "${PROJ_ROOT}/scripts/nginx/new-alpha-trade.conf" \
    "${SSH_USER}@${HOST}:/etc/nginx/sites-available/new-alpha-trade"

# env 文件：如不存在才推（避免覆盖用户已改的）
if ! ${SSH} "${SSH_USER}@${HOST}" "test -f /etc/new-alpha-trade/qr-service.env"; then
    ${SCP} -q \
        "${PROJ_ROOT}/scripts/etc/qr-service.env.example" \
        "${SSH_USER}@${HOST}:/etc/new-alpha-trade/qr-service.env"
fi

if ! ${SSH} "${SSH_USER}@${HOST}" "test -f /etc/new-alpha-trade/trading-engine.env"; then
    ${SCP} -q \
        "${PROJ_ROOT}/scripts/etc/trading-engine.env.example" \
        "${SSH_USER}@${HOST}:/etc/new-alpha-trade/trading-engine.env"
fi

${SSH} "${SSH_USER}@${HOST}" bash -s <<'EOF'
set -euo pipefail
# nginx 启用 site
ln -sf /etc/nginx/sites-available/new-alpha-trade /etc/nginx/sites-enabled/new-alpha-trade
rm -f /etc/nginx/sites-enabled/default
nginx -t
systemctl reload nginx
systemctl enable nginx >/dev/null 2>&1 || true

# systemd reload + enable（先 enable 还不 start，等代码就位后 deploy.sh 再 start）
systemctl daemon-reload
systemctl enable nat-qr-service nat-trading-engine >/dev/null 2>&1 || true
echo "✓ systemd + nginx ready (services not started yet)"
EOF

# ----------------------------------------------------------------------
# 5. 防火墙基线
# ----------------------------------------------------------------------
echo
echo "[5/5] 防火墙 ..."
${SSH} "${SSH_USER}@${HOST}" bash -s <<'EOF'
set -euo pipefail
ufw allow 22/tcp >/dev/null 2>&1 || true
ufw allow 80/tcp >/dev/null 2>&1 || true
ufw allow 443/tcp >/dev/null 2>&1 || true
ufw --force enable >/dev/null 2>&1 || true
ufw status | head -5
EOF

echo
echo "==============================================="
echo "  ✅ install-server.sh done."
echo "  下一步: bash scripts/deploy.sh ${HOST}"
echo "==============================================="
