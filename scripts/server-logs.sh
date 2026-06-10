#!/usr/bin/env bash
# 远程 tail journalctl 日志
# 用法: scripts/server-logs.sh <host> <service>
# service: qr-service | trading-engine | nginx

set -euo pipefail
HOST="${1:?usage: server-logs.sh <host> <service>}"
SERVICE="${2:?service name: qr-service | trading-engine | nginx}"
SSH_USER="${SSH_USER:-root}"

case "$SERVICE" in
  qr-service)     UNIT="nat-qr-service" ;;
  trading-engine) UNIT="nat-trading-engine" ;;
  nginx)          UNIT="nginx" ;;
  *) echo "unknown service: $SERVICE"; exit 1 ;;
esac

ssh "${SSH_USER}@${HOST}" "journalctl -u ${UNIT} -n 200 -f"
