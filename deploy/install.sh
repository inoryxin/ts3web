#!/bin/bash
# ============================================================
#  在 NAS 上安装/更新 ts3web
#
#  做的事：
#    1. 生成访问口令写进 .env（已存在就不覆盖）
#    2. docker compose up -d 起服务
#    3. 调用你自己的 /root/add-https-service.sh 开 18080 的 HTTPS 入口
#    4. 打印可用的链接和自测命令
#
#  用法： bash /opt/ts3web/deploy/install.sh
#         bash /opt/ts3web/deploy/install.sh --skip-https   （只更新容器）
# ============================================================
set -uo pipefail
cd /opt/ts3web || exit 1

PORT=18080
SKIP_HTTPS=0
[ "${1:-}" = "--skip-https" ] && SKIP_HTTPS=1

echo "=== 1. 访问口令 ==="
if [ -f .env ] && grep -q '^TS3WEB_TOKEN=' .env; then
  echo "  .env 已存在，沿用现有口令"
else
  TOKEN=$(head -c 24 /dev/urandom | base64 | tr -d '/+=' | head -c 24)
  {
    echo "TS3WEB_TOKEN=$TOKEN"
    echo "TS3WEB_CHANNEL=匡扶汉室"
  } > .env
  chmod 600 .env
  echo "  已生成 .env"
fi
# shellcheck disable=SC1091
. ./.env
echo "  口令: $TS3WEB_TOKEN"
echo "  频道: ${TS3WEB_CHANNEL:-（默认频道）}"

echo
echo "=== 2. 启动容器 ==="
docker compose up -d 2>&1 | sed 's/^/  /'
sleep 3
docker compose ps

echo
echo "=== 3. 容器内自测（HTTP 是否可用）==="
if curl -s -o /dev/null -w '  本机 http://127.0.0.1:%{http_code}\n' --max-time 5 "http://127.0.0.1:$PORT/healthz"; then
  :
else
  echo "  ! 后端没响应，先看日志: docker compose logs --tail=50"
fi
echo "  页面大小: $(curl -s --max-time 5 "http://127.0.0.1:$PORT/" | wc -c) 字节"

if [ "$SKIP_HTTPS" = "0" ]; then
  echo
  echo "=== 4. 加 HTTPS 入口（用你自己的脚本）==="
  if grep -qE "listen[[:space:]]+10\.0\.0\.123:$PORT[[:space:]]+ssl;" \
       /opt/1panel/www/conf.d/zz-https-ports.conf; then
    echo "  18080 入口已存在，跳过"
  else
    /root/add-https-service.sh "$PORT" "http://127.0.0.1:$PORT" "ts3web 浏览器加入 TS3 语音" 2>&1 | sed 's/^/  /'
  fi
fi

echo
echo "=== 完成 ==="
echo "  内网  https://home.inory.xin:$PORT/?t=$TS3WEB_TOKEN&n=测试"
echo "  外网  https://inory.xin:$PORT/?t=$TS3WEB_TOKEN&n=测试   （需要在路由器加：外部 $PORT → 10.0.0.123:$PORT）"
echo
echo "  TS3 侧自测（发 15 秒 440Hz 测试音进频道，验证发言权）："
echo "    cd /opt/ts3web && docker compose run --rm -e TS3WEB_SELFTEST=15 ts3web"
