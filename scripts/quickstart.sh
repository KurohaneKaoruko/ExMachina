#!/usr/bin/env bash
# EXMACHINA 快速启动：下载最新发行版 → 解压 → 启动（无需 Rust / Node.js）
# 用法：curl -fsSL https://raw.githubusercontent.com/KurohaneKaoruko/ExMachina/main/quickstart.sh | bash
set -e

REPO="KurohaneKaoruko/ExMachina"
API="https://api.github.com/repos/$REPO/releases/latest"
DIR="exmachina"

echo "═══════════════════════════════════════════════════════════"
echo "  EXMACHINA 智械体集群 · 快速启动（预编译发行版）"
echo "═══════════════════════════════════════════════════════════"

# ── 1. 获取最新版本号 ───────────────────────────────────────
echo "[1/3] 获取最新版本…"
TAG=$(curl -fsSL "$API" | grep -o '"tag_name": *"[^"]*"' | sed 's/.*"v/v/;s/"//')
if [ -z "$TAG" ]; then
  echo "  ✗ 无法获取版本号（网络问题？）"
  exit 1
fi
echo "  ✓ 版本 v$TAG"

# ── 2. 下载并解压 ──────────────────────────────────────────
OS=$(uname -s)
ARCH=$(uname -m)
if [ "$OS" = "Linux" ] && [ "$ARCH" = "x86_64" ]; then
  FILE="exmachina-linux-x64.tar.gz"
elif [ "$OS" = "Darwin" ]; then
  FILE="exmachina-macos-arm64.tar.gz"
else
  echo "  ✗ 不支持的系统：$OS $ARCH"
  echo "  请使用 install.sh 从源码安装"
  exit 1
fi

echo "[2/3] 下载 $FILE…"
mkdir -p "$DIR"
curl -fsSL "https://github.com/$REPO/releases/download/v$TAG/$FILE" -o "/tmp/$FILE"
tar -xzf "/tmp/$FILE" -C "$DIR"
echo "  ✓ 解压完成"

# ── 3. 启动 ────────────────────────────────────────────────
echo "[3/3] 启动网关…"
echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  浏览器打开  http://127.0.0.1:4173"
echo "  首次使用到「模型提供商」页配置 API 密钥"
echo "  停止：Ctrl+C"
echo "═══════════════════════════════════════════════════════════"
cd "$DIR"
chmod +x exm-gateway exm exmachina 2>/dev/null
exec ./exm-gateway
