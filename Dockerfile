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
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
# 依赖缓存层（无 lockfile 变更时命中）
RUN cargo build --release -p exm-cli -p exm-gateway

# ---------- 3) 编成数据（生成器需要 node；此处直接 COPY 源仓数据） ----------
FROM debian:bookworm-slim AS runtime
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=rust-builder /build/target/release/exm /usr/local/bin/exm
COPY --from=rust-builder /build/target/release/exmachina /usr/local/bin/exmachina
COPY --from=rust-builder /build/target/release/exm-gateway /usr/local/bin/exm-gateway
COPY --from=webui-builder /build/dist ./webui/dist
COPY agents ./agents
ENV EXM_LANG=zh
ENV RUST_LOG=info
EXPOSE 4173
VOLUME ["/app/.exmachina"]
CMD ["exm-gateway"]
