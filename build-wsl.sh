#!/bin/bash
# 在本机 WSL 里编译 ts3web（32 核，一两分钟），产物拷贝到仓库的 bin/ 目录，
# 之后只用把 bin/ts3web 传到 NAS、在 NAS 上打个镜像即可。
#
# 在 Windows 上这样调用：
#   wsl -d ts3build -e bash -c "sed -i 's/\r$//' /mnt/c/Users/anzu/Documents/xin/ts3web/build-wsl.sh && bash /mnt/c/Users/anzu/Documents/xin/ts3web/build-wsl.sh"
set -e
set -o pipefail

SRC=/mnt/c/Users/anzu/Documents/xin/ts3web
WORK=/root/ts3web-build
export PATH=/root/.cargo/bin:$PATH

# 静态链接 opus 与 openssl：这样运行镜像（debian:bookworm-slim）里不用再装任何库
export OPUS_STATIC=1
export OPENSSL_STATIC=1

echo "=== 同步源码到 WSL: $WORK ==="
mkdir -p "$WORK"
( cd "$SRC" && tar cf - --exclude=target --exclude=bin --exclude=.git . ) | ( cd "$WORK" && tar xf - )
echo "  文件数: $(find "$WORK/src" -name '*.rs' | wc -l) 个 rs"

echo "=== 编译（release）==="
cd "$WORK"
time cargo build --release 2>&1 | tail -30

echo "=== 拷回仓库 bin/ ==="
mkdir -p "$SRC/bin"
cp target/release/ts3web "$SRC/bin/ts3web"
ls -la "$SRC/bin/ts3web"

echo "=== 动态库依赖检查（确认运行时镜像里够不够）==="
ldd target/release/ts3web | sed 's/^/  /'

echo "BUILD_OK"
