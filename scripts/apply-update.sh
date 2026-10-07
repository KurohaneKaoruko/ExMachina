#!/bin/sh
# EXMACHINA 一键更新:由网关容器发起,经挂载的 docker.sock 在宿主引擎上执行。
# 前置:docker-compose.yml 将仓库根挂载到 /host,并挂载 /var/run/docker.sock。
# 流程:git pull → compose build(注入 GIT_HASH)→ compose up -d --force-recreate
set -eu

git config --global --add safe.directory /host
cd /host

echo "▸ 步骤 1/3:拉取最新代码"
git pull --ff-only

echo "▸ 步骤 2/3:构建镜像"
GIT_HASH=$(git rev-parse --short HEAD)
export GIT_HASH
docker compose -p exmachina build

echo "▸ 步骤 3/3:重启容器"
docker compose -p exmachina up -d --force-recreate

echo "▸ 更新完成"
