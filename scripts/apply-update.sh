#!/bin/sh
# EXMACHINA 一键更新:由网关容器发起,经挂载的 docker.sock 在宿主引擎上执行。
# 前置:docker-compose.yml 将仓库根挂载到 /host,并挂载 /var/run/docker.sock。
# 流程:git pull → compose build → compose up -d(经一次性辅助容器,与被重启的 exmachina 项目隔离)。
#      直接在本容器跑 up -d 会让 compose 客户端随旧容器一起被杀,recreate 做一半——故步骤 3 必须借道辅助容器。
set -eu

HELPER_IMAGE="docker:27.5.1-cli"

git config --global --add safe.directory /host
cd /host

# docker cli 需要 config.json(拉取镜像时解析仓库认证用);容器内默认没有,造一个空的
export DOCKER_CONFIG="${DOCKER_CONFIG:-/tmp/.docker}"
mkdir -p "$DOCKER_CONFIG"
[ -f "$DOCKER_CONFIG/config.json" ] || echo '{}' > "$DOCKER_CONFIG/config.json"

echo "▸ 步骤 1/3:拉取最新代码"
git pull --ff-only

echo "▸ 步骤 2/3:构建镜像"
GIT_HASH=$(git rev-parse --short HEAD)
export GIT_HASH
docker compose -p exmachina build

echo "▸ 步骤 3/3:重启容器(经一次性辅助容器执行,独立于本容器生命周期)"
docker pull -q "$HELPER_IMAGE"
docker rm -f exmachina-updater 2>/dev/null || true
docker run --rm --name exmachina-updater \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v /host:/host -w /host \
  "$HELPER_IMAGE" compose -p exmachina up -d --force-recreate

echo "▸ 更新完成"
