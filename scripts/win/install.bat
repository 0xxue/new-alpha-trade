@echo off
setlocal
title new-alpha-trade installer

echo ============================================================
echo   new-alpha-trade  Windows one-click installer
echo ------------------------------------------------------------
echo   Installs Python / Git / Node.js via winget, downloads the
echo   prebuilt engine, sets up deps and writes start.bat.
echo   No Rust / Visual Studio needed.
echo ============================================================
echo.

rem ---------- 0. winget check ----------
where winget >nul 2>nul
if errorlevel 1 (
  echo [ERROR] winget not found. Update Windows to 10 build 2004+ / Windows 11,
  echo         or install "App Installer" from the Microsoft Store, then retry.
  pause
  exit /b 1
)

rem ---------- 1. Python ----------
set "HAVEPY="
where py >nul 2>nul && set "HAVEPY=1"
if not defined HAVEPY where python >nul 2>nul && set "HAVEPY=1"
if not defined HAVEPY (
  echo [INSTALL] Python 3 ...
  winget install -e --id Python.Python.3.13 --silent --accept-package-agreements --accept-source-agreements
)

rem ---------- 2. Git ----------
where git >nul 2>nul
if errorlevel 1 (
  echo [INSTALL] Git ...
  winget install -e --id Git.Git --silent --accept-package-agreements --accept-source-agreements
)

rem ---------- 3. Node.js LTS ----------
where npm >nul 2>nul
if errorlevel 1 (
  echo [INSTALL] Node.js LTS ...
  winget install -e --id OpenJS.NodeJS.LTS --silent --accept-package-agreements --accept-source-agreements
)

rem ---------- refresh PATH (winget does not update the current window) ----------
for /f "skip=2 tokens=2,*" %%A in ('reg query "HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment" /v Path 2^>nul') do set "MPATH=%%B"
for /f "skip=2 tokens=2,*" %%A in ('reg query "HKCU\Environment" /v Path 2^>nul') do set "UPATH=%%B"
set "PATH=%MPATH%;%UPATH%"

rem ---------- pick python command ----------
set "PYCMD="
where py >nul 2>nul && set "PYCMD=py"
if not defined PYCMD where python >nul 2>nul && set "PYCMD=python"
if not defined PYCMD (
  echo.
  echo [NOTE] Python was just installed but is not on PATH yet.
  echo        Please CLOSE this window and double-click install.bat AGAIN.
  pause
  exit /b 1
)

rem ---------- clone / update repo ----------
set "REPO=https://github.com/0xxue/new-alpha-trade.git"
set "DIR=%~dp0new-alpha-trade"
if exist "%DIR%\.git" (
  echo [UPDATE] repo exists, git pull ...
  git -C "%DIR%" pull --ff-only
) else (
  echo [CLONE] %REPO%
  git clone --depth 1 "%REPO%" "%DIR%"
)
if not exist "%DIR%\.git" (
  echo [ERROR] repo download failed. Check network / git install and retry.
  pause
  exit /b 1
)

rem ---------- run python installer ----------
echo.
echo [RUN] python installer ...
%PYCMD% "%DIR%\scripts\win\install.py"
if errorlevel 1 (
  echo.
  echo [ERROR] installer failed. Please screenshot the messages above.
  pause
  exit /b 1
)

echo.
echo ============================================================
echo   DONE. Double-click this file to start:
echo   %DIR%\start.bat
echo ============================================================
pause
exit /b 0
