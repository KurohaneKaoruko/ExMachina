#!/usr/bin/env bash
# EXMACHINA 一键启动脚本：编译 → 构建 WebUI → 启动网关
# 用法：git clone 后运行 ./quickstart.sh
set -e

echo "═══════════════════════════════════════════════════════════"
echo "  EXMACHINA 智械体集群 · 一键启动"
echo "═══════════════════════════════════════════════════════════"

# ── 1. 环境检查 ──────────────────────────────────────────────
echo ""
echo "[1/4] 环境检查…"

check_cmd() {
  if ! command -v "$1" &>/dev/null; then
    echo "  ✗ 缺少 $1"
    echo ""
    echo "  安装方法："
    case "$1" in
      cargo) echo "    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" ;;
      node)  echo "    https://nodejs.org 或 nvm install --lts" ;;
      npm)   echo "    https://nodejs.org（npm 随 Node 安装）" ;;
    esac
    exit 1
  fi
  echo "  ✓ $1 $($1 --version 2>/dev/null | head -1)"
}

check_cmd cargo
check_cmd node
check_cmd npm

# ── 2. 编译 Rust 核心 + 网关 + CLI ──────────────────────────
echo ""
echo "[2/4] 编译 Rust（首次约 3-5 分钟）…"
cargo build --release -p exm-gateway -p exm-cli 2>&1 | tail -5
echo "  ✓ 编译完成"

# ── 3. 构建 WebUI ────────────────────────────────────────────
echo ""
echo "[3/4] 构建 WebUI…"
if [ ! -d "webui/node_modules" ]; then
  npm install 2>&1 | tail -3
fi
npm run build:webui 2>&1 | tail -3
echo "  ✓ WebUI 构建完成"

# ── 4. 启动网关 ─────────────────────────────────────────────
echo ""
echo "[4/4] 启动网关…"
echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  浏览器打开  http://127.0.0.1:4173"
echo "  首次使用建议到「模型提供商」页配置 API 密钥"
echo "  按 Ctrl+C 停止"
echo "═══════════════════════════════════════════════════════════"
echo ""

exec cargo run --release -p exm-gateway
