@echo off
chcp 65001 >nul 2>&1
title EXMACHINA Quickstart

echo ================================================================
echo   EXMACHINA 快速启动（预编译发行版）
echo ================================================================

set REPO=KurohaneKaoruko/ExMachina
set DIR=exmachina
set FILE=exmachina-windows-x64.zip

:: ── 1. 获取最新版本 ────────────────────────────────────────
echo [1/3] 获取最新版本...
curl -fsSL "https://api.github.com/repos/%REPO%/releases/latest" -o "%TEMP%\exm_ver.json"
for /f "tokens=4 delims=:, " %%a in ('findstr "tag_name" "%TEMP%\exm_ver.json"') do set TAG=%%a
set TAG=%TAG:"=%
if "%TAG%"=="" (
  echo   [X] 无法获取版本号
  pause
  exit /b 1
)
echo   [OK] 版本 %TAG%

:: ── 2. 下载并解压 ─────────────────────────────────────────
echo [2/3] 下载 %FILE%...
if not exist %DIR% mkdir %DIR%
curl -fsSL "https://github.com/%REPO%/releases/download/%TAG%/%FILE%" -o "%TEMP%\exm_pkg.zip"
if %errorlevel% neq 0 (
  echo   [X] 下载失败，请检查网络
  pause
  exit /b 1
)
powershell -Command "Expand-Archive -Path '%TEMP%\exm_pkg.zip' -DestinationPath '%DIR%' -Force"
echo   [OK] 解压完成

:: ── 3. 启动 ────────────────────────────────────────────────
echo [3/3] 启动网关...
echo.
echo ================================================================
echo   浏览器打开  http://127.0.0.1:4173
echo   首次使用到「模型提供商」页配置 API 密钥
echo   停止：Ctrl+C
echo ================================================================
cd %DIR%
exm-gateway.exe
pause
