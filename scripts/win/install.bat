@echo off
chcp 65001 >nul
setlocal
title new-alpha-trade 一键安装

echo ============================================================
echo   new-alpha-trade  Windows 一键安装
echo ------------------------------------------------------------
echo   会用 winget 自动安装 Python / Git / Node.js，
echo   下载预编译引擎，配好依赖并生成启动脚本。
echo   全程无需手动装 Rust / Visual Studio。
echo ============================================================
echo.

rem ---------- 0. 检查 winget ----------
where winget >nul 2>nul
if errorlevel 1 (
  echo [错误] 没找到 winget。请把 Windows 更新到 10 版本 2004+ 或 Windows 11，
  echo        或从 Microsoft Store 安装 "应用安装程序"（App Installer）后重试。
  pause
  exit /b 1
)

rem ---------- 1. Python ----------
where py >nul 2>nul
if errorlevel 1 where python >nul 2>nul
if errorlevel 1 (
  echo [安装] Python 3 ...
  winget install -e --id Python.Python.3.13 --silent --accept-package-agreements --accept-source-agreements
)

rem ---------- 2. Git ----------
where git >nul 2>nul
if errorlevel 1 (
  echo [安装] Git ...
  winget install -e --id Git.Git --silent --accept-package-agreements --accept-source-agreements
)

rem ---------- 3. Node.js LTS ----------
where npm >nul 2>nul
if errorlevel 1 (
  echo [安装] Node.js LTS ...
  winget install -e --id OpenJS.NodeJS.LTS --silent --accept-package-agreements --accept-source-agreements
)

rem ---------- 刷新 PATH（winget 装完当前窗口不会自动更新）----------
for /f "skip=2 tokens=2,*" %%A in ('reg query "HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment" /v Path 2^>nul') do set "MPATH=%%B"
for /f "skip=2 tokens=2,*" %%A in ('reg query "HKCU\Environment" /v Path 2^>nul') do set "UPATH=%%B"
set "PATH=%MPATH%;%UPATH%"

rem ---------- 选 Python 命令 ----------
set "PYCMD="
where py >nul 2>nul && set "PYCMD=py"
if not defined PYCMD where python >nul 2>nul && set "PYCMD=python"
if not defined PYCMD (
  echo.
  echo [提示] 刚装好的 Python 还没进 PATH。请【关掉本窗口，重新双击 install.bat】即可。
  pause
  exit /b 1
)

rem ---------- clone / 更新仓库 ----------
set "REPO=https://github.com/0xxue/new-alpha-trade.git"
set "DIR=%~dp0new-alpha-trade"
if exist "%DIR%\.git" (
  echo [更新] 仓库已存在，git pull ...
  git -C "%DIR%" pull --ff-only
) else (
  echo [克隆] %REPO%
  git clone --depth 1 "%REPO%" "%DIR%"
)
if not exist "%DIR%\.git" (
  echo [错误] 仓库下载失败，请检查网络或 git 是否安装成功后重试。
  pause
  exit /b 1
)

rem ---------- 跑 Python 安装器 ----------
echo.
echo [运行] Python 安装器 ...
%PYCMD% "%DIR%\scripts\win\install.py"
if errorlevel 1 (
  echo.
  echo [错误] 安装器执行失败，请把上面的报错截图发出来。
  pause
  exit /b 1
)

echo.
echo ============================================================
echo   全部完成！双击下面这个文件启动：
echo   %DIR%\start.bat
echo ============================================================
pause
exit /b 0
