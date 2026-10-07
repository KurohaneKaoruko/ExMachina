# EXMACHINA 多阶段构建：
#   1) node 阶段构建 WebUI（npm run build:webui 产物由网关静态托管）
#   2) rust 阶段编译 CLI + 网关（GNU 工具链）
#   3) 运行时镜像：仅二进制 + 静态资源 + 数据卷
# 部署语言：EXM_LANG=zh（默认）| en —— en 时指挥体提示词装载 exmachina-orchestrator.en.md

# ---------- 1) WebUI ----------
FROM node:22-bookworm-slim AS webui-builder
WORKDIR /build
COPY webui/package.json webui/package-lock.json* ./
RUN npm install --no-audit --no-fund
COPY webui/ ./
RUN npm run build

# ---------- 2) Rust 核心 ----------
FROM rust:1.92-bookworm AS rust-builder
WORKDIR /build
# libxdo-dev：enigo 键鼠控制链接 libxdo（dbus 已 vendored，无需系统包）
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libxdo-dev && rm -rf /var/lib/apt/lists/*
ARG GIT_HASH=dev
ENV EXM_GIT_HASH=${GIT_HASH}
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
# 依赖缓存层（无 lockfile 变更时命中）
RUN cargo build --release -p exm-cli -p exm-gateway

# ---------- 3) 编成数据（生成器需要 node；此处直接 COPY 源仓数据） ----------
FROM debian:bookworm-slim AS runtime
WORKDIR /app
# 运行时动态库：enigo/xcap 的 X11 栈（libxdo / X11 / xkbcommon；容器内无显示面，Computer Use 默认关闭，
# 但二进制加载即需这些 .so）+ CA 证书与 curl（健康检查）
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl libxdo3 libx11-6 libxcb1 libxext6 libxinerama1 libxtst6 libxkbcommon0 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust-builder /build/target/release/exm /usr/local/bin/exm
COPY --from=rust-builder /build/target/release/exmachina /usr/local/bin/exmachina
COPY --from=rust-builder /build/target/release/exm-gateway /usr/local/bin/exm-gateway
COPY --from=webui-builder /build/dist ./webui/dist
COPY entities ./entities
COPY skills ./skills
ENV EXM_LANG=zh
ENV RUST_LOG=info
EXPOSE 4173
VOLUME ["/app/.exmachina"]
CMD ["exm-gateway"]
