#!/usr/bin/env bash
# EXMACHINA 全自动安装脚本
# 用法：curl -fsSL https://raw.githubusercontent.com/KurohaneKaoruko/ExMachina/main/install.sh | bash
# 或：下载后运行 ./install.sh
set -e

REPO="https://github.com/KurohaneKaoruko/ExMachina"
DIR="ExMachina"

echo "═══════════════════════════════════════════════════════════"
echo "  EXMACHINA 智械体集群 · 全自动安装"
echo "═══════════════════════════════════════════════════════════"
echo ""

# ── 1. 安装 Rust（如果没有）─────────────────────────────────
if ! command -v cargo &>/dev/null; then
  echo "[1/5] 安装 Rust…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  source "$HOME/.cargo/env"
  echo "  ✓ Rust $(rustc --version | awk '{print $2}') 安装完成"
else
  echo "[1/5] ✓ Rust 已安装: $(rustc --version | awk '{print $2}')"
fi

# ── 2. 安装 Node.js（如果没有）──────────────────────────────
install_node() {
  echo "  正在通过 nvm 安装 Node.js…"
  if ! command -v nvm &>/dev/null && [ ! -s "$HOME/.nvm/nvm.sh" ]; then
    curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
  fi
  export NVM_DIR="$HOME/.nvm"
  [ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"
  nvm install --lts
  echo "  ✓ Node.js $(node --version) 安装完成"
}

if ! command -v node &>/dev/null; then
  echo "[2/5] 安装 Node.js…"
  install_node
else
  NV=$(node --version | sed 's/v//' | cut -d. -f1)
  if [ "$NV" -lt 22 ]; then
    echo "[2/5] Node.js 版本过低（$(node --version)），升级中…"
    install_node
  else
    echo "[2/5] ✓ Node.js $(node --version)"
  fi
fi

# ── 3. 克隆仓库 ─────────────────────────────────────────────
if [ -d "$DIR" ]; then
  echo "[3/5] ✓ 目录 $DIR 已存在，拉取最新代码…"
  cd "$DIR" && git pull --rebase 2>/dev/null || true
else
  echo "[3/5] 克隆仓库…"
  git clone --depth 1 "$REPO" "$DIR"
  cd "$DIR"
fi
echo "  ✓ 代码就绪"

# ── 4. 编译 + 构建 ─────────────────────────────────────────
echo "[4/5] 编译 Rust（首次约 3-5 分钟）…"
cargo build --release -p exm-gateway -p exm-cli 2>&1 | tail -3
echo "  ✓ Rust 编译完成"

echo "     构建 WebUI…"
npm install --silent 2>/dev/null
npm run build:webui 2>&1 | tail -3
echo "  ✓ WebUI 构建完成"

# ── 5. 启动 ────────────────────────────────────────────────
echo ""
echo "[5/5] 启动网关…"
echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  ✓ 安装完成！"
echo ""
echo "  浏览器打开  http://127.0.0.1:4173"
echo ""
echo "  首次使用："
echo "    1. 进入「模型提供商」页"
echo "    2. 点击「新增提供商」→ 选厂商预设 → 填入 API Key"
echo "    3. 点「设为全局默认」"
echo "    4. 回到「对话」页开始使用"
echo ""
echo "  终端对话："
echo "    cargo run -p exm-cli --release --bin exm -- chat \"你好\""
echo ""
echo "  停止：按 Ctrl+C"
echo "  下次启动：cd $DIR && cargo run --release -p exm-gateway"
echo "═══════════════════════════════════════════════════════════"
echo ""

exec cargo run --release -p exm-gateway
