#!/bin/bash
# 起一个允许发原始命令的调试实例，跑命令探针，然后清理
KEY=/root/.ssh/nas_key
NAS=root@10.0.0.123
SSH="ssh -i $KEY -o StrictHostKeyChecking=no"
TOKEN=$($SSH $NAS "grep -oP '(?<=^TS3WEB_TOKEN=).*' /opt/ts3web/.env")

echo "=== 起调试实例（18081, ALLOW_RAW=1, debug 日志）==="
$SSH $NAS "cd /opt/ts3web && docker rm -f ts3web-dbg >/dev/null 2>&1; docker compose run --rm -d --name ts3web-dbg -e TS3WEB_LOG=debug -e TS3WEB_ALLOW_RAW=1 -p 18081:18080 ts3web >/dev/null && sleep 2 && echo started"

echo
echo "=== 命令探针 ==="
python3 /mnt/c/Users/anzu/Documents/xin/ts3web/deploy/raw-probe.py "ws://10.0.0.123:18081/ws?t=${TOKEN}&n=probe" 2>&1 | tail -40

echo
echo "=== 容器日志（debug，看 tsclientlib 记录的命令）==="
$SSH $NAS "docker logs ts3web-dbg 2>&1 | grep -aE '调试命令|命令应答|clientmove|Command|command|Send|Recv' | tail -30"

echo
echo "=== 清理 ==="
$SSH $NAS "docker rm -f ts3web-dbg >/dev/null 2>&1 && echo cleaned"
