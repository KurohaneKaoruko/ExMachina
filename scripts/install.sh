#!/usr/bin/env bash
# EXMACHINA 源码安装：Rust + Node 工具链 → 克隆仓库 → 编译 → 前台启动
#
# 用法：
#   curl -fsSL https://raw.githubusercontent.com/KurohaneKaoruko/ExMachina/main/scripts/install.sh | bash
#   ./install.sh [--port 4173]
#
# 兼容 macOS 自带 bash 3.2（POSIX 风格，未使用 bash4 特性）。
set -Eeuo pipefail

REPO="https://github.com/KurohaneKaoruko/ExMachina"
BRANCH="${EXM_BRANCH:-main}"
DIR="${EXM_DIR:-ExMachina}"
PORT="${EXM_PORT:-4173}"
NODE_MIN=22
RUST_MIN=56   # edition 2021 所需最低 minor（rustc 1.56+）
TOTAL=7

# ── 输出基建：NO_COLOR 或非 TTY 自动降级 ──────────────────────
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  C_G=$'\033[32m'; C_R=$'\033[31m'; C_Y=$'\033[33m'; C_B=$'\033[36m'; C_DIM=$'\033[2m'; C_0=$'\033[0m'
else
  C_G=''; C_R=''; C_Y=''; C_B=''; C_DIM=''; C_0=''
fi

ok()   { printf '  %s✓%s %s\n' "$C_G" "$C_0" "$*"; }
info() { printf '  %s·%s %s\n' "$C_B" "$C_0" "$*"; }
warn() { printf '  %s!%s %s\n' "$C_Y" "$C_0" "$*"; }
fail() {
  printf '  %s✗ %s%s\n' "$C_R" "$1" "$C_0" >&2
  if [ "${2:-}" != "" ]; then printf '    %s建议：%s%s\n' "$C_Y" "$2" "$C_0" >&2; fi
  exit 1
}
step() {
  STEP=$((STEP + 1))
  printf '\n%s[%d/%d]%s %s\n' "$C_B" "$STEP" "$TOTAL" "$C_0" "$1"
}
elapsed() { echo $(( $(date +%s) - $1 )); }

usage() {
  cat <<'EOF'
EXMACHINA 源码安装（Rust + Node.js 工具链 → 克隆 → 编译 → 前台启动）

用法：install.sh [--port N]

选项：
  --port N    网关监听端口（默认 4173）
  --help      显示本帮助（纯本地，不发起网络请求）

环境变量：
  EXM_BRANCH   克隆的分支（默认 main）
  EXM_DIR      安装目录（默认 ./ExMachina）
  EXM_PORT     网关监听端口（--port 可覆盖）
  NO_COLOR     置任意非空值禁用彩色输出

说明：
  - 需要磁盘剩余空间 ≥ 3GB（Rust 编译产物较大）
  - 首次编译约 3-5 分钟；WebUI 由 npm ci + build:webui 构建
  - 安装后浏览器打开 http://127.0.0.1:<端口>，到「模型设置」页配置 API Key
EOF
  exit 0
}

while [ $# -gt 0 ]; do
  case "$1" in
    --port)
      [ $# -ge 2 ] || fail "--port 需要一个端口号参数"
      PORT="$2"; shift 2 ;;
    --help|-h) usage ;;
    *) fail "未知参数：$1" "运行 install.sh --help 查看用法" ;;
  esac
done

trap 'fail "第 $LINENO 行执行失败" "重跑查看细节：bash -x install.sh --port $PORT"' ERR

banner() {
  printf '%s' "$C_B"
  cat <<'EOF'

  ┌──────────────────────────────────────────────
  │  EX·MACHINA — 智械体集群 · 源码安装
  └──────────────────────────────────────────────
EOF
  printf '%s\n' "$C_0"
}

disk_free_mb() { df -Pk . 2>/dev/null | awk 'NR==2 {print int($4 / 1024)}'; }
port_busy() {
  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
  elif command -v netstat >/dev/null 2>&1; then
    netstat -an 2>/dev/null | grep -Eq "[:.]$1[[:space:]].*LISTEN"
  else
    return 1
  fi
}
rust_ok() { [ "$(rustc -vV 2>/dev/null | awk '/^release:/ {split($2, a, "."); print a[2] + 0}')" -ge "$RUST_MIN" ]; }
node_ok() { [ "$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null)" -ge "$NODE_MIN" ]; }
install_node() {
  info "通过 nvm 安装 Node.js LTS…"
  if [ ! -s "$HOME/.nvm/nvm.sh" ]; then
    curl -fsSL --retry 3 --retry-delay 2 -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
  fi
  # shellcheck disable=SC1091
  export NVM_DIR="$HOME/.nvm"
  # shellcheck disable=SC1091
  . "$NVM_DIR/nvm.sh"
  nvm install --lts
  nvm use --lts
}

banner
STEP=0
T_ALL=$(date +%s)

step "环境预检"
for tool in git curl tar; do
  command -v "$tool" >/dev/null 2>&1 || fail "缺少依赖：$tool" "请先安装 $tool 并确保在 PATH 中"
done
FREE=$(disk_free_mb)
[ "${FREE:-0}" -ge 3072 ] || fail "磁盘剩余空间不足（${FREE}MB < 3072MB）" "Rust 编译产物较大，清理磁盘或更换工作目录后重试"
ok "依赖齐备（git / curl / tar），磁盘剩余 ${FREE}MB"

step "Rust 工具链"
if ! command -v cargo >/dev/null 2>&1; then
  info "未检测到 cargo → 安装 rustup（stable）…"
  curl --proto '=https' --tlsv1.2 -sSf --retry 3 https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  # shellcheck disable=SC1091
  if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
elif ! rust_ok; then
  RV=$(rustc --version | awk '{print $2}')
  info "rustc $RV 低于 edition 2021 要求 → rustup update…"
  rustup update stable
  # shellcheck disable=SC1091
  if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
fi
rust_ok || fail "rustc 版本不满足 edition 2021（需 ≥1.$RUST_MIN）" "手动执行 rustup update stable 后重试"
ok "Rust $(rustc --version | awk '{print $2}')"

step "Node.js（≥ $NODE_MIN）"
if ! command -v node >/dev/null 2>&1; then
  install_node
elif ! node_ok; then
  info "Node.js $(node --version) 版本过低 → 升级…"
  install_node
fi
node_ok || fail "Node.js 版本仍不满足（需 ≥$NODE_MIN）" "检查 nvm 安装输出；或手动安装 Node.js $NODE_MIN+ 后重开终端重试"
ok "Node.js $(node --version)"

step "获取源码（分支 $BRANCH，目录 $DIR）"
if [ -d "$DIR/.git" ]; then
  info "已存在 → 拉取最新代码…"
  git -C "$DIR" pull --rebase --autostash
else
  git clone --depth 1 -b "$BRANCH" "$REPO" "$DIR"
fi
cd "$DIR"
ok "代码就绪（$(git rev-parse --short HEAD)）"

step "编译 Rust（首次约 3-5 分钟，流式输出）"
BUILD_LOG=$(mktemp 2>/dev/null) || BUILD_LOG="/tmp/exm-build-$$.log"
T_B=$(date +%s)
if ! cargo build --release -p exm-gateway -p exm-cli 2>&1 | tee "$BUILD_LOG"; then
  fail "Rust 编译失败（完整日志已在上文与 $BUILD_LOG）" "常见原因：磁盘满 / rustc 版本旧 / 依赖源不可达；处理后重跑本脚本"
fi
rm -f "$BUILD_LOG"
ok "编译完成（耗时 $(elapsed $T_B)s）"

step "构建 WebUI（npm ci + build:webui，流式输出）"
T_W=$(date +%s)
NPM_LOG=$(mktemp 2>/dev/null) || NPM_LOG="/tmp/exm-npm-$$.log"
if ! npm ci --no-audit --no-fund 2>&1 | tee "$NPM_LOG"; then
  fail "依赖安装（npm ci）失败（完整日志见 $NPM_LOG）" "常见原因：Node 版本低 / npm 源不可达；可执行 npm config set registry https://registry.npmmirror.com 后重试"
fi
if ! npm run build:webui 2>&1 | tee -a "$NPM_LOG"; then
  fail "WebUI 构建失败（完整日志见 $NPM_LOG）" "常见原因：Node 版本低 / 内存不足；处理后重跑本脚本"
fi
rm -f "$NPM_LOG"
ok "WebUI 构建完成（耗时 $(elapsed $T_W)s）"

step "启动网关（端口 $PORT）"
if port_busy "$PORT"; then
  fail "端口 $PORT 已被占用" "换端口：install.sh --port 8173；或结束占用进程：lsof -ti :$PORT | xargs kill"
fi

printf '\n%s' "$C_G"
cat <<EOF
  ══════════════════════════════════════════════
  ✓ 安装完成（总耗时 $(elapsed $T_ALL)s）
     分支   $BRANCH（$(git rev-parse --short HEAD)）
     目录   $(pwd)
     端口   $PORT
  ══════════════════════════════════════════════
EOF
printf '%s\n\n' "$C_0"
info "浏览器打开  http://127.0.0.1:$PORT"
info "首次使用：到「模型设置」页配置 API Key（新增档案 → 选厂商 → 填 Key → 设为默认）"
info "终端对话：cargo run --release -p exm-cli -- chat \"你好\""
info "停止：Ctrl+C；下次启动：cd $DIR && cargo run --release -p exm-gateway"
printf '\n'

exec env EXM_PORT="$PORT" cargo run --release -p exm-gateway
