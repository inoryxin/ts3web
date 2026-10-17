#!/bin/bash
# 在 WSL 里跑网页端链路自检（不用浏览器）
#   bash run-ws-test.sh [秒数] [要切过去的频道名]
set -e
SCRIPT=/mnt/c/Users/anzu/Documents/xin/ts3web/deploy/ws-selftest.py
DURATION="${1:-8}"
WANT_CHANNEL="${2:-}"

if ! python3 -c "import websockets" 2>/dev/null; then
  echo "=== 安装 python3-websockets ==="
  apt-get install -y -qq --no-install-recommends python3 python3-websockets
fi

KEY=/root/.ssh/nas_key
if [ ! -f "$KEY" ]; then
  mkdir -p /root/.ssh
  cp /mnt/c/Users/anzu/AppData/Local/Temp/nas_key_ed25519 "$KEY"
  chmod 600 "$KEY"
fi

TOKEN=$(ssh -i "$KEY" -o StrictHostKeyChecking=no root@10.0.0.123 "grep -oP '(?<=^TS3WEB_TOKEN=).*' /opt/ts3web/.env")
CHAN=$(ssh -i "$KEY" -o StrictHostKeyChecking=no root@10.0.0.123 "grep -oP '(?<=^TS3WEB_CHANNEL=).*' /opt/ts3web/.env")
echo "口令: $TOKEN   默认频道: $CHAN"
echo

URL="wss://home.inory.xin:18080/ws?t=${TOKEN}&n=%E8%87%AA%E6%A3%80"
python3 "$SCRIPT" "$URL" "$DURATION" "$WANT_CHANNEL"
