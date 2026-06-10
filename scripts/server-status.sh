#!/usr/bin/env bash
# 远程拉取三服务状态
# 用法: scripts/server-status.sh <host>

set -euo pipefail
HOST="${1:?usage: server-status.sh <host>}"
SSH_USER="${SSH_USER:-root}"

ssh "${SSH_USER}@${HOST}" bash -s <<'EOF'
echo "==== systemd ===="
systemctl is-active nat-qr-service 2>/dev/null || echo "nat-qr-service: not installed"
systemctl is-active nat-trading-engine 2>/dev/null || echo "nat-trading-engine: not installed"
systemctl is-active nginx 2>/dev/null || echo "nginx: not installed"
echo ""
echo "==== health ===="
curl -fsS http://127.0.0.1:7001/health 2>/dev/null || echo "qr-service: down"
echo ""
curl -fsS http://127.0.0.1:7002/health 2>/dev/null || echo "trading-engine: down"
echo ""
echo "==== disk ===="
df -h /opt 2>/dev/null || true
EOF
