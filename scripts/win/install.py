#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""new-alpha-trade Windows installer.

Called by scripts/win/install.bat (which installs Python/Git/Node and clones the
repo first). Can also be run inside an already-cloned repo:
    python scripts\\win\\install.py

Idempotent. It:
  1. Downloads the prebuilt trading-engine.exe from the latest GitHub Release
     -> bin\\trading-engine.exe   (no Rust / Visual Studio needed)
  2. Creates the qr-service Python venv + installs deps + Playwright Chromium
  3. Runs npm install for web-ui
  4. Writes start.bat / stop.bat

NOTE: all console output here is ASCII English on purpose. Chinese text inside a
.bat/.py run through cmd on a non-UTF-8 console gets mangled, so we keep the
runnable scripts ASCII-only; the docs (README) stay bilingual.
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
REPO = "0xxue/new-alpha-trade"          # GitHub repo
ENGINE_ASSET = "trading-engine.exe"     # prebuilt engine asset name in Releases
ROOT = Path(__file__).resolve().parents[2]   # repo root
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
    step(f"Downloading prebuilt engine {ENGINE_ASSET} (GitHub Release, no Rust needed)")
    BIN.mkdir(exist_ok=True)
    api = f"https://api.github.com/repos/{REPO}/releases/latest"
    req = urllib.request.Request(api, headers={"User-Agent": "nat-installer"})
    with urllib.request.urlopen(req, timeout=30) as r:
        rel = json.load(r)
    tag = rel.get("tag_name", "?")
    asset = next((a for a in rel.get("assets", []) if a["name"] == ENGINE_ASSET), None)
    if asset is None:
        raise SystemExit(
            f"[ERROR] latest Release ({tag}) has no {ENGINE_ASSET}.\n"
            f"        Check https://github.com/{REPO}/releases"
        )
    url = asset["browser_download_url"]
    dst = BIN / ENGINE_ASSET
    log(url)
    req2 = urllib.request.Request(url, headers={"User-Agent": "nat-installer"})
    with urllib.request.urlopen(req2, timeout=600) as r, open(dst, "wb") as f:
        shutil.copyfileobj(r, f)
    log(f"done: {dst}  ({dst.stat().st_size / 1e6:.1f} MB, version {tag})")


def setup_qr_service() -> None:
    step("Setting up qr-service (Python venv + deps + Playwright Chromium)")
    qr = ROOT / "qr-service"
    venv = qr / ".venv"
    if not (venv / "Scripts" / "python.exe").exists():
        run([sys.executable, "-m", "venv", str(venv)])
    py = str(venv / "Scripts" / "python.exe")
    run([py, "-m", "pip", "install", "--quiet", "--upgrade", "pip"])
    run([py, "-m", "pip", "install", "--quiet", "-e", "."], cwd=str(qr))
    # downloads the headless browser (~150MB, slow on first run)
    run([py, "-m", "playwright", "install", "chromium"])


def setup_web_ui() -> None:
    step("Setting up web-ui (npm install, slow on first run)")
    web = ROOT / "web-ui"
    npm = "npm.cmd" if os.name == "nt" else "npm"
    run([npm, "install"], cwd=str(web))


def write_launchers() -> None:
    step("Writing start.bat / stop.bat")
    DATA.mkdir(exist_ok=True)
    (ROOT / "start.bat").write_text(START_BAT, encoding="ascii")
    (ROOT / "stop.bat").write_text(STOP_BAT, encoding="ascii")
    log(str(ROOT / "start.bat"))
    log(str(ROOT / "stop.bat"))


START_BAT = r"""@echo off
setlocal
set "ROOT=%~dp0"
if not exist "%ROOT%data" mkdir "%ROOT%data"

rem ===== shared env (absolute paths so engine and qr-service share one DB) =====
set "DB_PATH=%ROOT%data\new-alpha-trade.db"
set "QR_DIR=%ROOT%data\qr"
set "FACE_QR_DIR=%ROOT%data\face_qr"
set "PLAYWRIGHT_STATE_DIR=%ROOT%data\playwright-state"
set "PLAYWRIGHT_HEADLESS=true"
set "QR_SERVICE_URL=http://127.0.0.1:7001"
set "PORT=7002"

echo [1/3] starting trading engine  http://127.0.0.1:7002 ...
start "nat-engine" cmd /k ""%ROOT%bin\trading-engine.exe""
timeout /t 4 /nobreak >nul

echo [2/3] starting qr service      http://127.0.0.1:7001 ...
start "nat-qr" cmd /k ""%ROOT%qr-service\.venv\Scripts\python.exe" -m uvicorn qr_service.main:app --host 127.0.0.1 --port 7001"
timeout /t 3 /nobreak >nul

echo [3/3] starting web ui          http://localhost:5173 ...
start "nat-web" cmd /k "cd /d "%ROOT%web-ui" && npm run dev"
timeout /t 6 /nobreak >nul

start "" http://localhost:5173
echo.
echo ============================================================
echo   Started 3 windows: nat-engine / nat-qr / nat-web
echo   Browser opened at  http://localhost:5173
echo   Scan QR on the Accounts page, then trade on the Trade page.
echo   To stop: run stop.bat, or just close the 3 black windows.
echo ============================================================
pause
"""

STOP_BAT = r"""@echo off
echo Stopping new-alpha-trade ...
taskkill /F /IM trading-engine.exe >nul 2>nul
taskkill /F /T /FI "WINDOWTITLE eq nat-engine*" >nul 2>nul
taskkill /F /T /FI "WINDOWTITLE eq nat-qr*" >nul 2>nul
taskkill /F /T /FI "WINDOWTITLE eq nat-web*" >nul 2>nul
echo Stopped.
timeout /t 2 >nul
"""


def main() -> None:
    print("============================================================")
    print("  new-alpha-trade Windows installer")
    print(f"  repo dir: {ROOT}")
    print("============================================================")
    download_engine()
    setup_qr_service()
    setup_web_ui()
    write_launchers()
    print("\n============================================================")
    print("  DONE.")
    print(f"  Double-click to start:  {ROOT / 'start.bat'}")
    print("============================================================")


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as e:
        print(f"\n[ERROR] command failed (exit {e.returncode}): {e.cmd}", file=sys.stderr)
        sys.exit(1)
