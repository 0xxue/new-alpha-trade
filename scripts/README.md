# scripts/

部署 & 运维脚本。详细架构见 [`docs/design/new-alpha-trade.md` 附录 B](../docs/design/new-alpha-trade.md)。

## 速查

| 脚本 | 用途 | 频率 |
|---|---|---|
| `install-server.sh <host>` | 首次初始化（apt deps + 用户 + 防火墙） | 每台机一次 |
| `deploy.sh <host>` | 推代码 + 重启服务 | 每次发版 |
| `server-status.sh <host>` | 看 systemctl 状态 + curl 健康检查 | 随时 |
| `server-logs.sh <host> <service>` | tail journalctl 日志 | 随时 |
| `config/servers.json.example` | 服务器清单模板（复制成 `servers.local.json` 用） | 编辑一次 |

## 当前阶段（P0）

所有脚本是**骨架**。`install-server.sh` 只装系统依赖；`deploy.sh` 只 rsync 文件（systemctl 暂时跳过，因为 P0 还没装 systemd unit）。

## 阶段对接

| 阶段 | 这里要补什么 |
|---|---|
| P1 | systemd unit 文件、`install-server.sh` 加 `systemctl enable` |
| P2 | `deploy.sh` 加 `systemctl restart nat-trading-engine` 和健康检查 |
| P3 | `deploy.sh` 加 qr-service playwright deps 安装 |
| P5 | `install-server.sh` 加 nginx 配置 |
| P6 | 加 `secrets.env` 推送（**不走 rsync，单独走 scp 且权限 600**） |

## 安全基线

部署机和服务器之间用 SSH key（默认路径 `~/.ssh/new-alpha-trade.key`），不要用明文密码。
首次用 `bootstrap_ssh.py` 推一次 key，之后禁用密码登录。
