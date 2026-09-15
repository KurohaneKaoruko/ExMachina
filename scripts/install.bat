@echo off
chcp 65001 >nul 2>&1
title EXMACHINA Installer

echo ================================================================
echo   EXMACHINA 智械体集群 · 全自动安装
echo ================================================================
echo.

set REPO=https://github.com/KurohaneKaoruko/ExMachina
set DIR=ExMachina

:: ── 1. 安装 Rust ──────────────────────────────────────────
where cargo >nul 2>&1
if %errorlevel% neq 0 (
  echo [1/5] 安装 Rust...
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o rustup-init.exe
  rustup-init.exe -y --default-toolchain stable
  del rustup-init.exe
  set PATH=%USERPROFILE%.cargoin;%PATH%
  echo   [OK] Rust 安装完成
) else (
  echo [1/5] [OK] Rust 已安装
)

:: ── 2. 安装 Node.js ────────────────────────────────────────
where node >nul 2>&1
if %errorlevel% neq 0 (
  echo [2/5] 安装 Node.js...
  echo   请从 https://nodejs.org 下载安装 Node.js ^>= 22
  echo   安装完成后重新运行此脚本
  start https://nodejs.org
  pause
  exit /b 1
) else (
  echo [2/5] [OK] Node.js 已安装
)

:: ── 3. 克隆仓库 ────────────────────────────────────────────
if exist %DIR% (
  echo [3/5] [OK] 目录 %DIR% 已存在，拉取最新...
  cd %DIR%
  git pull --rebase 2>nul
) else (
  echo [3/5] 克隆仓库...
  git clone --depth 1 %REPO% %DIR%
  cd %DIR%
)
if %errorlevel% neq 0 (
  echo   [X] 克隆失败，请检查网络
  pause
  exit /b 1
)
echo   [OK] 代码就绪

:: ── 4. 编译 + 构建 ─────────────────────────────────────────
echo [4/5] 编译 Rust（首次约 3-5 分钟）...
cargo build --release -p exm-gateway -p exm-cli
if %errorlevel% neq 0 (
  echo   [X] 编译失败
  pause
  exit /b 1
)
echo   [OK] Rust 编译完成

echo      构建 WebUI...
if not exist "webui\node_modules" (
  npm install --silent
)
call npm run build:webui
if %errorlevel% neq 0 (
  echo   [X] WebUI 构建失败
  pause
  exit /b 1
)
echo   [OK] WebUI 构建完成

:: ── 5. 启动 ────────────────────────────────────────────────
echo.
echo [5/5] 启动网关...
echo.
echo ================================================================
echo   [OK] 安装完成！
echo.
echo   浏览器打开  http://127.0.0.1:4173
echo.
echo   首次使用：
echo     1. 进入「模型提供商」页
echo     2. 点击「新增提供商」→ 选厂商预设 → 填入 API Key
echo     3. 点「设为全局默认」
echo     4. 回到「对话」页开始使用
echo.
echo   停止：按 Ctrl+C
echo   下次启动：cd %DIR% ^&^& cargo run --release -p exm-gateway
echo ================================================================
echo.

cargo run --release -p exm-gateway
pause
