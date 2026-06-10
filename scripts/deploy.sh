#!/usr/bin/env bash
# 一键部署：推源码 → 服务器 build → 起服务 → 健康检查
#
# 前置：
#   1. bootstrap_ssh.py 推过 key（免密 ssh）
#   2. install-server.sh 跑过（系统依赖 + systemd + nginx 就位）
#
# 用法:
#   scripts/deploy.sh <host>
# 例:
#   scripts/deploy.sh <your-server-ip>
#
# 设计：
#   本机源码 tar 流 → 服务器解压 → 服务器 cargo build --release → npm build → pip install
#   → systemctl restart → curl /health 验证

set -euo pipefail

HOST="${1:?usage: deploy.sh <host>}"
SSH_USER="${SSH_USER:-root}"
APP_USER="${APP_USER:-newalpha}"
APP_DIR="${APP_DIR:-/opt/new-alpha-trade}"

PROJ_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${PROJ_ROOT}"

SSH_OPTS="-o StrictHostKeyChecking=accept-new -o BatchMode=yes"
SSH="ssh ${SSH_OPTS}"

echo "==============================================="
echo "  deploy.sh -> ${SSH_USER}@${HOST}:${APP_DIR}"
echo "==============================================="

# ----------------------------------------------------------------------
# 0. 本机构建 web-ui（生产 dist）
# ----------------------------------------------------------------------
echo
echo "[0/5] 本机构建 web-ui ..."
(cd web-ui && npm install --silent && npm run build) 2>&1 | tail -8

# ----------------------------------------------------------------------
# 1. 推源码（tar pipe，无 rsync 依赖）
# ----------------------------------------------------------------------
echo
echo "[1/5] 推源码（tar over ssh）..."
TAR_FILES=(
    qr-service
    trading-engine
    scripts
    docs
    .gitignore
    README.md
)
TAR_EXCLUDES=(
    --exclude='**/.venv'
    --exclude='**/target'
    --exclude='**/node_modules'
    --exclude='**/__pycache__'
    --exclude='**/*.pyc'
    --exclude='**/.pytest_cache'
    --exclude='**/.mypy_cache'
    --exclude='**/.ruff_cache'
    --exclude='**/*.egg-info'
)
tar -czf - "${TAR_EXCLUDES[@]}" "${TAR_FILES[@]}" | \
    ${SSH} "${SSH_USER}@${HOST}" "tar -xzf - -C ${APP_DIR}/ && chown -R ${APP_USER}:${APP_USER} ${APP_DIR}"

echo "[1.5/5] 推 web-ui/dist ..."
tar -czf - -C web-ui dist | \
    ${SSH} "${SSH_USER}@${HOST}" "tar -xzf - -C ${APP_DIR}/web-ui/ && chown -R ${APP_USER}:${APP_USER} ${APP_DIR}/web-ui/dist"

# ----------------------------------------------------------------------
# 2. 服务器：cargo build release
# ----------------------------------------------------------------------
echo
echo "[2/5] 服务器 cargo build --release（首次较慢 ~5-8min）..."
${SSH} "${SSH_USER}@${HOST}" bash <<EOF
set -euo pipefail
. /root/.cargo/env
cd ${APP_DIR}/trading-engine
# sqlx::migrate! 宏在编译时嵌入 migrations/，新增 .sql 文件 cargo 增量编译不会察觉
# touch 一下让 persistence crate 必重编译
touch crates/persistence/src/lib.rs
cargo build --release --quiet
# 把二进制移到 APP_DIR 顶层（systemd unit 路径）
cp -f target/release/trading-engine ${APP_DIR}/trading-engine.bin
mv -f ${APP_DIR}/trading-engine.bin ${APP_DIR}/trading-engine 2>/dev/null || true
# 实际上 trading-engine 既是目录又是二进制名冲突，统一放到 bin 子目录
mkdir -p ${APP_DIR}/bin
cp -f target/release/trading-engine ${APP_DIR}/bin/trading-engine
chmod +x ${APP_DIR}/bin/trading-engine
chown ${APP_USER}:${APP_USER} ${APP_DIR}/bin/trading-engine
echo "✓ engine built at ${APP_DIR}/bin/trading-engine"
EOF

# ----------------------------------------------------------------------
# 3. 服务器：qr-service Python venv + playwright
# ----------------------------------------------------------------------
echo
echo "[3/5] qr-service venv + playwright chromium ..."
${SSH} "${SSH_USER}@${HOST}" bash <<EOF
set -euo pipefail
cd ${APP_DIR}/qr-service
if [ ! -d .venv ]; then
    python3 -m venv .venv
fi
.venv/bin/pip install --quiet --upgrade pip
.venv/bin/pip install --quiet -e .
# Playwright 浏览器装到共享目录（systemd EnvironmentFile 已设 PLAYWRIGHT_BROWSERS_PATH）
PLAYWRIGHT_BROWSERS_PATH=${APP_DIR}/data/playwright-browsers \
    .venv/bin/playwright install chromium
.venv/bin/playwright install-deps chromium
chown -R ${APP_USER}:${APP_USER} ${APP_DIR}/qr-service ${APP_DIR}/data
echo "✓ qr-service ready"
EOF

# ----------------------------------------------------------------------
# 4. 修 systemd ExecStart 指向新二进制位置
# ----------------------------------------------------------------------
echo
echo "[4/5] 调整 systemd unit ExecStart ..."
${SSH} "${SSH_USER}@${HOST}" bash <<'EOF'
set -euo pipefail
sed -i 's|^ExecStart=/opt/new-alpha-trade/trading-engine$|ExecStart=/opt/new-alpha-trade/bin/trading-engine|' \
    /etc/systemd/system/nat-trading-engine.service
systemctl daemon-reload
EOF

# ----------------------------------------------------------------------
# 5. 起服务 + 健康检查
# ----------------------------------------------------------------------
echo
echo "[5/5] 起 systemd 服务 + 健康检查 ..."
${SSH} "${SSH_USER}@${HOST}" bash <<'EOF'
set -euo pipefail
# 先让 trading-engine 起来（它建 SQLite + migrations）
systemctl restart nat-trading-engine
sleep 2
systemctl restart nat-qr-service
sleep 2
systemctl --no-pager status nat-trading-engine | head -8
echo "---"
systemctl --no-pager status nat-qr-service | head -8
EOF

echo
echo "→ 健康检查 ..."
for i in 1 2 3 4 5; do
    if ${SSH} "${SSH_USER}@${HOST}" "curl -fsS http://127.0.0.1:7002/health && echo && curl -fsS http://127.0.0.1:7001/health" 2>/dev/null; then
        echo
        echo "✓ 两服务都健康"
        break
    fi
    [ $i -eq 5 ] && { echo "✗ 健康检查超时"; exit 1; }
    sleep 2
done

echo
echo "==============================================="
echo "  ✅ deploy.sh done."
echo "  浏览器打开: http://${HOST}/accounts"
echo "==============================================="
