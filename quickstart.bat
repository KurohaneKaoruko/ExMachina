@echo off
chcp 65001 >nul 2>&1
title EXMACHINA Quickstart

echo ================================================================
echo   EXMACHINA 智械体集群 · 一键启动
echo ================================================================

echo.
echo [1/4] 环境检查...

where cargo >nul 2>&1
if %errorlevel% neq 0 (
  echo   [X] 缺少 cargo — 请安装 Rust: https://rustup.rs
  pause & exit /b 1
)
echo   [OK] cargo

where node >nul 2>&1
if %errorlevel% neq 0 (
  echo   [X] 缺少 Node.js — https://nodejs.org
  pause & exit /b 1
)
echo   [OK] node

where npm >nul 2>&1
if %errorlevel% neq 0 (
  echo   [X] 缺少 npm — https://nodejs.org
  pause & exit /b 1
)
echo   [OK] npm

echo.
echo [2/4] 编译 Rust（首次约 3-5 分钟）...
cargo build --release -p exm-gateway -p exm-cli
if %errorlevel% neq 0 (
  echo   [X] 编译失败
  pause & exit /b 1
)
echo   [OK] 编译完成

echo.
echo [3/4] 构建 WebUI...
if not exist "webui\node_modules" (
  npm install
)
call npm run build:webui
if %errorlevel% neq 0 (
  echo   [X] WebUI 构建失败
  pause & exit /b 1
)
echo   [OK] WebUI 构建完成

echo.
echo [4/4] 启动网关...
echo.
echo ================================================================
echo   浏览器打开  http://127.0.0.1:4173
echo   首次使用建议到「模型提供商」页配置 API 密钥
echo   按 Ctrl+C 停止
echo ================================================================
echo.

cargo run --release -p exm-gateway
pause
