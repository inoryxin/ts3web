#!/bin/bash
# 把本机编好的二进制和部署文件推到 NAS，并在 NAS 上重建镜像、重启服务。
# 在 WSL 里跑（它能用局域网和 Windows 上的密钥）：
#   wsl -d ts3build -e bash -c "sed -i 's/\r$//' /mnt/c/Users/anzu/Documents/xin/ts3web/deploy/deploy-from-pc.sh && bash /mnt/c/Users/anzu/Documents/xin/ts3web/deploy/deploy-from-pc.sh"
set -e

SRC=/mnt/c/Users/anzu/Documents/xin/ts3web
KEY=/root/.ssh/nas_key
NAS=root@10.0.0.123

# Windows 的 /mnt/c 上权限是 777，ssh 会拒绝使用这种密钥，先复制进来并收紧权限
mkdir -p /root/.ssh
cp /mnt/c/Users/anzu/AppData/Local/Temp/nas_key_ed25519 "$KEY"
chmod 600 "$KEY"

SSH="ssh -i $KEY -o StrictHostKeyChecking=no"
SCP="scp -i $KEY -o StrictHostKeyChecking=no"

if [ ! -f "$SRC/bin/ts3web" ]; then
  echo "✗ 没有 $SRC/bin/ts3web，先跑 build-wsl.sh"
  exit 1
fi

echo "=== 1. 传二进制 ==="
$SSH "$NAS" "mkdir -p /opt/ts3web/bin /opt/ts3web/deploy"
$SCP "$SRC/bin/ts3web" "$NAS:/opt/ts3web/bin/ts3web"

echo "=== 2. 传 compose / Dockerfile / 部署脚本 ==="
$SCP "$SRC/docker-compose.yml" "$SRC/Dockerfile" "$SRC/.dockerignore" "$NAS:/opt/ts3web/"
$SCP "$SRC/deploy/install.sh" "$NAS:/opt/ts3web/deploy/install.sh"
$SSH "$NAS" "sed -i 's/\r$//' /opt/ts3web/deploy/install.sh; chmod +x /opt/ts3web/bin/ts3web /opt/ts3web/deploy/install.sh"

echo "=== 3. 在 NAS 上重建镜像并重启 ==="
$SSH "$NAS" "cd /opt/ts3web && docker compose build 2>&1 | tail -5 && docker compose up -d 2>&1 | tail -5 && sleep 3 && docker compose ps"

echo "=== 4. 后端自测 ==="
$SSH "$NAS" "curl -s -o /dev/null -w 'healthz -> %{http_code}\n' --max-time 5 http://127.0.0.1:18080/healthz; curl -s --max-time 5 http://127.0.0.1:18080/ | wc -c | sed 's/^/页面字节数: /'"

echo "DEPLOY_OK"
