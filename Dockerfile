# 运行镜像：只装编译好的二进制 + 它的动态库依赖。
#
# 编译不在 NAS 上做 —— 它只有 1 个核。本机 WSL（32 核）编好后放进 bin/：
#   bash build-wsl.sh         # 本机编译，产物落到 bin/ts3web
#   bash deploy/deploy-from-pc.sh   # 传到 NAS 并在 NAS 上打包、重启
#
# 二进制动态依赖 libssl3(含 libcrypto) 和 libopus0，所以这里装一次；
# 走阿里源，避免 deb.debian.org 在国内太慢。
FROM debian:bookworm-slim

RUN sed -i 's|deb.debian.org|mirrors.aliyun.com|g; s|security.debian.org|mirrors.aliyun.com|g' \
        /etc/apt/sources.list /etc/apt/sources.list.d/*.sources 2>/dev/null || true \
    && apt-get update \
    && apt-get install -y --no-install-recommends libssl3 libopus0 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY bin/ts3web /usr/local/bin/ts3web

EXPOSE 18080
ENTRYPOINT ["/usr/local/bin/ts3web"]
