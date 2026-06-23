#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""new-alpha-trade Windows 安装器。

由同目录的 install.bat 调用（install.bat 负责装好 Python/Git/Node 并 clone 仓库），
也可在已 clone 的仓库里单独运行：  python scripts\\win\\install.py

它做这些事（全部幂等，可重复跑）：
  1. 从 GitHub Release 下载预编译的 trading-engine.exe  -> bin\\trading-engine.exe
  2. 给 qr-service 建 Python 虚拟环境 + 装依赖 + 下载 Playwright Chromium
  3. 给 web-ui 跑 npm install
  4. 生成 start.bat / stop.bat（一键起停三服务）

引擎本身是 Rust 预编译好的，朋友的电脑【不需要】装 Rust / Visual Studio。
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

# ----------------------------------------------------------------------------
REPO = "0xxue/new-alpha-trade"          # GitHub 仓库
ENGINE_ASSET = "trading-engine.exe"     # Release 里预编译引擎的资产名
ROOT = Path(__file__).resolve().parents[2]   # 仓库根目录
DATA = ROOT / "data"
BIN = ROOT / "bin"
# ----------------------------------------------------------------------------


def step(msg: str) -> None:
    print(f"\n==> {msg}", flush=True)


def log(msg: str) -> None:
    print(f"    {msg}", flush=True)


def run(cmd, cwd: str | None = None) -> None:
    shown = cmd if isinstance(cmd, str) else " ".join(map(str, cmd))
    log("$ " + shown)
    subprocess.run(cmd, cwd=cwd, check=True, shell=isinstance(cmd, str))


def download_engine() -> None:
    step(f"下载预编译引擎 {ENGINE_ASSET}（GitHub Release，无需装 Rust）")
    BIN.mkdir(exist_ok=True)
    api = f"https://api.github.com/repos/{REPO}/releases/latest"
    req = urllib.request.Request(api, headers={"User-Agent": "nat-installer"})
    with urllib.request.urlopen(req, timeout=30) as r:
        rel = json.load(r)
    tag = rel.get("tag_name", "?")
    asset = next((a for a in rel.get("assets", []) if a["name"] == ENGINE_ASSET), None)
    if asset is None:
        raise SystemExit(
            f"[错误] 最新 Release（{tag}）里没有 {ENGINE_ASSET}。\n"
            f"        请确认 https://github.com/{REPO}/releases 里上传了该文件。"
        )
    url = asset["browser_download_url"]
    dst = BIN / ENGINE_ASSET
    log(f"{url}")
    req2 = urllib.request.Request(url, headers={"User-Agent": "nat-installer"})
    with urllib.request.urlopen(req2, timeout=600) as r, open(dst, "wb") as f:
        shutil.copyfileobj(r, f)
    log(f"完成：{dst}  ({dst.stat().st_size / 1e6:.1f} MB, 版本 {tag})")


def setup_qr_service() -> None:
    step("配置扫码服务（Python 虚拟环境 + 依赖 + Playwright 浏览器）")
    qr = ROOT / "qr-service"
    venv = qr / ".venv"
    if not (venv / "Scripts" / "python.exe").exists():
        run([sys.executable, "-m", "venv", str(venv)])
    py = str(venv / "Scripts" / "python.exe")
    run([py, "-m", "pip", "install", "--quiet", "--upgrade", "pip"])
    run([py, "-m", "pip", "install", "--quiet", "-e", "."], cwd=str(qr))
    # 下载无头浏览器（约 150MB，首次较慢）
    run([py, "-m", "playwright", "install", "chromium"])


def setup_web_ui() -> None:
    step("配置前端（npm install，首次较慢）")
    web = ROOT / "web-ui"
    npm = "npm.cmd" if os.name == "nt" else "npm"
    run([npm, "install"], cwd=str(web))


def write_launchers() -> None:
    step("生成启动脚本 start.bat / stop.bat")
    DATA.mkdir(exist_ok=True)
    (ROOT / "start.bat").write_text(START_BAT, encoding="utf-8")
    (ROOT / "stop.bat").write_text(STOP_BAT, encoding="utf-8")
    log(str(ROOT / "start.bat"))
    log(str(ROOT / "stop.bat"))


START_BAT = r"""@echo off
chcp 65001 >nul
setlocal
set "ROOT=%~dp0"
if not exist "%ROOT%data" mkdir "%ROOT%data"

rem ===== 共享环境变量（绝对路径，保证引擎和扫码服务用同一个数据库）=====
set "DB_PATH=%ROOT%data\new-alpha-trade.db"
set "QR_DIR=%ROOT%data\qr"
set "FACE_QR_DIR=%ROOT%data\face_qr"
set "PLAYWRIGHT_STATE_DIR=%ROOT%data\playwright-state"
set "PLAYWRIGHT_HEADLESS=true"
set "QR_SERVICE_URL=http://127.0.0.1:7001"
set "PORT=7002"

echo [1/3] 启动交易引擎  http://127.0.0.1:7002 ...
start "nat-engine" cmd /k ""%ROOT%bin\trading-engine.exe""
timeout /t 4 /nobreak >nul

echo [2/3] 启动扫码服务  http://127.0.0.1:7001 ...
start "nat-qr" cmd /k ""%ROOT%qr-service\.venv\Scripts\python.exe" -m uvicorn qr_service.main:app --host 127.0.0.1 --port 7001"
timeout /t 3 /nobreak >nul

echo [3/3] 启动前端界面  http://localhost:5173 ...
start "nat-web" cmd /k "cd /d "%ROOT%web-ui" && npm run dev"
timeout /t 6 /nobreak >nul

start "" http://localhost:5173
echo.
echo ============================================================
echo   已启动三个窗口：nat-engine / nat-qr / nat-web
echo   浏览器已打开  http://localhost:5173
echo   先去【账户】页扫码登录，再到【交易】页刷量。
echo   停止：双击 stop.bat，或直接关掉那三个黑窗口。
echo ============================================================
pause
"""

STOP_BAT = r"""@echo off
chcp 65001 >nul
echo 正在停止 new-alpha-trade ...
taskkill /F /IM trading-engine.exe >nul 2>nul
taskkill /F /T /FI "WINDOWTITLE eq nat-engine*" >nul 2>nul
taskkill /F /T /FI "WINDOWTITLE eq nat-qr*" >nul 2>nul
taskkill /F /T /FI "WINDOWTITLE eq nat-web*" >nul 2>nul
echo 已停止。
timeout /t 2 >nul
"""


def main() -> None:
    print("============================================================")
    print("  new-alpha-trade Windows 安装器")
    print(f"  仓库目录: {ROOT}")
    print("============================================================")
    download_engine()
    setup_qr_service()
    setup_web_ui()
    write_launchers()
    print("\n============================================================")
    print("  ✅ 安装完成！")
    print(f"  双击启动:  {ROOT / 'start.bat'}")
    print("============================================================")


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as e:
        print(f"\n[错误] 命令执行失败（退出码 {e.returncode}）：{e.cmd}", file=sys.stderr)
        sys.exit(1)
