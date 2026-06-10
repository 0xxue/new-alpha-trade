# -*- coding: utf-8 -*-
"""一次性把本机 SSH 公钥推到远程服务器的 authorized_keys。

之后 ssh root@<host> 免密，scripts/deploy.sh 这种 bash 脚本就能直接走。

用法:
    python scripts/bootstrap_ssh.py <host> [--user root] [--password ...]

如果不传 --password，从 stdin 读（不回显），或从环境变量 SSH_PASSWORD 读。

如果本机没有 ~/.ssh/id_rsa.pub，会先用 ssh-keygen 生成一对（无 passphrase）。
"""
from __future__ import annotations

import argparse
import getpass
import io
import os
import subprocess
import sys
from pathlib import Path

if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding="utf-8")

try:
    import paramiko
except ImportError:
    print("✗ paramiko 未安装。请先: pip install paramiko", file=sys.stderr)
    sys.exit(1)


def ensure_local_keypair() -> Path:
    """返回本机公钥 path，缺则生成。"""
    ssh_dir = Path.home() / ".ssh"
    ssh_dir.mkdir(mode=0o700, exist_ok=True)
    pub = ssh_dir / "id_rsa.pub"
    priv = ssh_dir / "id_rsa"
    if pub.exists():
        print(f"✓ 找到本机公钥: {pub}")
        return pub
    print(f"⚠ 本机没有 {priv}，正在生成 RSA-4096 keypair（无 passphrase）...")
    subprocess.run(
        ["ssh-keygen", "-t", "rsa", "-b", "4096", "-f", str(priv), "-N", "", "-q"],
        check=True,
    )
    print(f"✓ 已生成: {priv} / {pub}")
    return pub


def push_key(host: str, user: str, password: str, pub_path: Path) -> None:
    pub_text = pub_path.read_text(encoding="utf-8").strip()
    if not pub_text:
        print(f"✗ 公钥文件为空: {pub_path}", file=sys.stderr)
        sys.exit(1)

    print(f"→ 用密码 SSH 登录 {user}@{host} ...")
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    try:
        client.connect(
            host,
            port=22,
            username=user,
            password=password,
            timeout=15,
            allow_agent=False,
            look_for_keys=False,
        )
    except paramiko.AuthenticationException:
        print(f"✗ 密码错误", file=sys.stderr)
        sys.exit(1)
    except Exception as e:  # noqa: BLE001
        print(f"✗ 连接失败: {e}", file=sys.stderr)
        sys.exit(1)
    print("✓ 已登录")

    # 用 shell 一次性把 key 安装到 authorized_keys（如未存在）
    home = "/root" if user == "root" else f"/home/{user}"
    cmd = (
        f'mkdir -p {home}/.ssh && '
        f'chmod 700 {home}/.ssh && '
        f'touch {home}/.ssh/authorized_keys && '
        f'chmod 600 {home}/.ssh/authorized_keys && '
        f'grep -qxF {shellquote(pub_text)} {home}/.ssh/authorized_keys '
        f'|| echo {shellquote(pub_text)} >> {home}/.ssh/authorized_keys && '
        f'echo OK'
    )
    print("→ 安装公钥 ...")
    stdin, stdout, stderr = client.exec_command(cmd, timeout=15)
    out = stdout.read().decode(errors="ignore").strip()
    err = stderr.read().decode(errors="ignore").strip()
    if "OK" not in out:
        print(f"✗ 远端命令失败")
        if out:
            print(f"  stdout: {out}")
        if err:
            print(f"  stderr: {err}")
        client.close()
        sys.exit(1)
    client.close()
    print("✓ 公钥已安装")

    # 验证免密登录
    print("→ 验证免密登录 ...")
    verify = paramiko.SSHClient()
    verify.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    try:
        verify.connect(
            host,
            port=22,
            username=user,
            key_filename=str(pub_path.with_suffix("")),  # 私钥路径 = pub 去 .pub
            timeout=10,
            allow_agent=False,
            look_for_keys=False,
        )
        verify.close()
        print("✓ 免密登录可用！之后所有 deploy.sh 可直接走。")
    except Exception as e:  # noqa: BLE001
        print(f"⚠ 公钥安装了但免密验证失败: {e}")
        print(f"   可能是 sshd_config 禁了 PubkeyAuthentication，先手动确认。")


def shellquote(s: str) -> str:
    """单引号安全转义。"""
    return "'" + s.replace("'", "'\\''") + "'"


def main() -> None:
    ap = argparse.ArgumentParser(description="把本机 SSH 公钥推到远程服务器")
    ap.add_argument("host")
    ap.add_argument("--user", default="root")
    ap.add_argument("--password", default=None, help="SSH 密码（不推荐命令行传，会进 shell 历史）")
    args = ap.parse_args()

    password = args.password or os.environ.get("SSH_PASSWORD")
    if not password:
        password = getpass.getpass(f"SSH password for {args.user}@{args.host}: ")
    if not password:
        print("✗ 密码必填", file=sys.stderr)
        sys.exit(1)

    pub = ensure_local_keypair()
    push_key(args.host, args.user, password, pub)


if __name__ == "__main__":
    main()
