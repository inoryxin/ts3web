#!/bin/bash
# 用 debug 日志级别在 18081 另起一个临时实例，抓 clientmove 命令的真实收发日志
KEY=/root/.ssh/nas_key
NAS=root@10.0.0.123
SSH="ssh -i $KEY -o StrictHostKeyChecking=no"
TOKEN=$($SSH $NAS "grep -oP '(?<=^TS3WEB_TOKEN=).*' /opt/ts3web/.env")
echo "token=$TOKEN"

echo "=== 起临时实例（18081，debug 日志）==="
$SSH $NAS "cd /opt/ts3web && docker rm -f ts3web-dbg >/dev/null 2>&1; docker compose run --rm -d --name ts3web-dbg -e TS3WEB_LOG=debug -p 18081:18080 ts3web >/dev/null && sleep 2 && docker logs ts3web-dbg 2>&1 | tail -3"

echo
echo "=== 跑一次切频道 ==="
python3 /mnt/c/Users/anzu/Documents/xin/ts3web/deploy/move-debug.py "ws://10.0.0.123:18081/ws?t=${TOKEN}&n=dbg" 2>&1 | tail -12

echo
echo "=== 容器 debug 日志：命令相关 ==="
$SSH $NAS "docker logs ts3web-dbg 2>&1 | grep -aE '发送 TS3 命令|clientmove|ParameterNotFound|Sent|Received|command|命令' | tail -40"

echo
echo "=== 清理临时实例 ==="
$SSH $NAS "docker rm -f ts3web-dbg >/dev/null 2>&1 && echo cleaned"
