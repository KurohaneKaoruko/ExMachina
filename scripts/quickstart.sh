#!/usr/bin/env bash
# EXMACHINA 快速启动：下载预编译发行版 → 校验 → 解压 → 前台启动（无需 Rust / Node.js）
#
# 用法：
#   curl -fsSL https://raw.githubusercontent.com/KurohaneKaoruko/ExMachina/main/scripts/quickstart.sh | bash
#   ./quickstart.sh [--port 4173]
#
# 兼容 macOS 自带 bash 3.2（POSIX 风格，未使用 bash4 特性）。
set -Eeuo pipefail

REPO="KurohaneKaoruko/ExMachina"
DIR="exmachina"
PORT="${EXM_PORT:-4173}"
VER="${EXM_VERSION:-}"
TOTAL=5

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
EXMACHINA 快速启动（预编译发行版，无需 Rust / Node.js）

用法：quickstart.sh [--port N]

选项：
  --port N    网关监听端口（默认 4173）
  --help      显示本帮助（纯本地，不发起网络请求）

环境变量：
  EXM_VERSION   指定发行版版本（如 v1.2.0）；缺省取 GitHub Releases latest
  EXM_PORT      网关监听端口（--port 可覆盖）
  NO_COLOR      置任意非空值禁用彩色输出

说明：
  - 已安装目录再次运行 = 升级：覆盖程序文件，保留 .exmachina 数据目录
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
    *) fail "未知参数：$1" "运行 quickstart.sh --help 查看用法" ;;
  esac
done

trap 'fail "第 $LINENO 行执行失败" "重跑查看细节：bash -x quickstart.sh --port $PORT"' ERR

banner() {
  printf '%s' "$C_B"
  cat <<'EOF'

  ┌──────────────────────────────────────────────
  │  EX·MACHINA — 智械体集群 · 快速启动（预编译发行版）
  └──────────────────────────────────────────────
EOF
  printf '%s\n' "$C_0"
}

# ── 平台与依赖预检 ────────────────────────────────────────────
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

banner
STEP=0
T_ALL=$(date +%s)

step "环境预检"
for tool in curl tar; do
  command -v "$tool" >/dev/null 2>&1 || fail "缺少依赖：$tool" "请先安装 $tool（curl 与 tar 为下载解压所需）"
done
FREE=$(disk_free_mb)
[ "${FREE:-0}" -ge 512 ] || fail "磁盘剩余空间不足（${FREE}MB < 512MB）" "清理磁盘后重试，或更换到空间充足的工作目录"
ok "依赖齐备（curl / tar），磁盘剩余 ${FREE}MB"

step "解析版本与平台"
OS=$(uname -s)
ARCH=$(uname -m)
case "$OS:$ARCH" in
  Linux:x86_64 | Linux:amd64) FILE="exmachina-linux-x64.tar.gz";  PLAT="linux-x64" ;;
  Darwin:arm64)               FILE="exmachina-macos-arm64.tar.gz"; PLAT="macos-arm64" ;;
  Darwin:x86_64)              FILE="exmachina-macos-x64.tar.gz";   PLAT="macos-x64" ;;
  *)
    fail "不支持的平台：$OS $ARCH" "改用 install.sh 从源码安装（任意平台，需 Rust + Node.js）"
    ;;
esac
TAG="$VER"
if [ -z "$TAG" ]; then
  API_JSON=$(curl -fsSL --retry 3 --retry-delay 2 "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null) || true
  TAG=$(printf '%s' "$API_JSON" | grep -o '"tag_name":[[:space:]]*"[^"]*"' | head -1 | sed 's/.*: *//; s/"//g')
fi
[ -n "$TAG" ] || fail "无法获取最新版本号" "网络受限或 GitHub API 限流；可指定版本重试：EXM_VERSION=v1.2.0 bash quickstart.sh"
case "$TAG" in v*) ;; *) TAG="v$TAG" ;; esac
ok "版本 $TAG · 平台 $PLAT"

step "下载发行版（$FILE）"
DL_URL="https://github.com/$REPO/releases/download/$TAG/$FILE"
TMP=$(mktemp -d 2>/dev/null) || TMP="/tmp/exm-quickstart-$$"
mkdir -p "$TMP"
T_DL=$(date +%s)
if ! curl -fSL --retry 3 --retry-delay 2 -# -o "$TMP/$FILE" "$DL_URL"; then
  rm -rf "$TMP"
  fail "下载失败：$DL_URL" "检查网络；或指定版本重试：EXM_VERSION=v1.2.0 bash quickstart.sh"
fi
ok "下载完成（耗时 $(elapsed $T_DL)s）"

# 校验和：release 附带 .sha256 则校验，缺失则提示后跳过
if curl -fsSL --retry 2 -o "$TMP/$FILE.sha256" "$DL_URL.sha256" 2>/dev/null; then
  WANT=$(awk '{print $1}' "$TMP/$FILE.sha256")
  HAVE=$( { sha256sum "$TMP/$FILE" 2>/dev/null || shasum -a 256 "$TMP/$FILE"; } | awk '{print $1}')
  [ -n "$HAVE" ] || fail "本机缺少 sha256sum / shasum，无法校验" "安装 coreutils 或跳过校验（不推荐）"
  [ "$WANT" = "$HAVE" ] || fail "校验和不匹配（期望 $WANT，实际 $HAVE）" "下载可能损坏或被篡改；删除 $TMP 后重试"
  ok "校验和匹配"
else
  warn "该发行版未附带 .sha256，跳过校验"
fi

step "解压安装（目录：$DIR）"
if [ -d "$DIR" ]; then
  info "检测到已安装目录 → 升级路径（覆盖程序文件，保留 .exmachina 数据目录）"
fi
mkdir -p "$DIR"
STAGE="$DIR/.stage-$$"
mkdir -p "$STAGE"
tar -xzf "$TMP/$FILE" -C "$STAGE"
cp -Rf "$STAGE/." "$DIR/"
rm -rf "$STAGE"
rm -rf "$TMP"
ok "就绪（耗时含在下载计时内）"

step "启动网关（端口 $PORT）"
if port_busy "$PORT"; then
  fail "端口 $PORT 已被占用" "换端口启动：quickstart.sh --port 8173；或结束占用进程：lsof -ti :$PORT | xargs kill"
fi
chmod +x "$DIR"/exm-gateway "$DIR"/exm 2>/dev/null || true

printf '\n%s' "$C_G"
cat <<EOF
  ══════════════════════════════════════════════
  ✓ 安装完成
     版本   $TAG（$PLAT）
     目录   $(pwd)/$DIR
     端口   $PORT
  ══════════════════════════════════════════════
EOF
printf '%s\n\n' "$C_0"
info "浏览器打开  http://127.0.0.1:$PORT"
info "首次使用：到「模型设置」页配置 API Key（新增档案 → 选厂商 → 填 Key → 设为默认）"
info "停止：Ctrl+C"
printf '\n'

cd "$DIR"
exec env EXM_PORT="$PORT" ./exm-gateway
