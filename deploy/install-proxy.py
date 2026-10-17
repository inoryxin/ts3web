#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
把 ts3web 的网页和 WebSocket 反代挂到 1Panel 现有站点（inory.xin，443）下面。

为什么这样做：443 已经在路由器上放通、证书也现成，临时队友拿到的链接
https://inory.xin/voice/?t=<口令> 直接就能用，不需要再动路由器。

做的事：
  1. 把 web/index.html 放到站点目录 /opt/1panel/www/sites/inory.xin/index/voice/
  2. 备份并修改 /opt/1panel/www/conf.d/inory.xin.conf，插入两个 location：
       /voice/      -> 静态页面
       /voice/ws    -> 反代到 127.0.0.1:18080/ws （ts3web 容器）
  3. nginx -t 校验，通过才 reload；不通过就回滚

可重复执行（幂等）：已插入过就跳过。
"""
import os
import re
import shutil
import subprocess
import sys
import time

SITE_CONF = "/opt/1panel/www/conf.d/inory.xin.conf"
SITE_ROOT = "/opt/1panel/www/sites/inory.xin/index"
PAGE_SRC = "/opt/ts3web/web/index.html"
PAGE_DST = os.path.join(SITE_ROOT, "voice", "index.html")
BACKEND = "127.0.0.1:18080"
MARK = "# >>> ts3web (managed)"

SNIPPET = f"""
    {MARK} >>>
    # 网页版 TS3 语音：静态页面 + WebSocket 反代
    location ^~ /voice/ws {{
        proxy_pass http://{BACKEND}/ws;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
        proxy_buffering off;
    }}
    location ^~ /voice/ {{
        alias {SITE_ROOT.replace('/opt/1panel/www', '/www')}/voice/;
        index index.html;
        try_files $uri $uri/ /voice/index.html;
    }}
    {MARK} <<<
"""


def sh(cmd, check=True):
    print(f"$ {cmd}")
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    out = (r.stdout or "") + (r.stderr or "")
    if out.strip():
        print(out.strip())
    if check and r.returncode != 0:
        raise SystemExit(f"命令失败（退出码 {r.returncode}）: {cmd}")
    return r


def main():
    # ---- 1. 页面 ----
    if not os.path.exists(PAGE_SRC):
        raise SystemExit(f"找不到 {PAGE_SRC}")
    os.makedirs(os.path.dirname(PAGE_DST), exist_ok=True)
    shutil.copy2(PAGE_SRC, PAGE_DST)
    print(f"页面已就位: {PAGE_DST}")

    # ---- 2. 站点配置 ----
    if not os.path.exists(SITE_CONF):
        raise SystemExit(f"找不到站点配置 {SITE_CONF}")
    with open(SITE_CONF, "r", encoding="utf-8") as f:
        conf = f.read()

    if MARK in conf:
        print("站点配置里已有 ts3web 段，跳过修改（只更新了页面）")
    else:
        idx = conf.rfind("}")
        if idx < 0:
            raise SystemExit("站点配置里找不到结尾的 }")
        backup = f"{SITE_CONF}.bak-ts3web-{time.strftime('%Y%m%d-%H%M%S')}"
        shutil.copy2(SITE_CONF, backup)
        print(f"已备份: {backup}")

        new_conf = conf[:idx] + SNIPPET + conf[idx:]
        with open(SITE_CONF, "w", encoding="utf-8") as f:
            f.write(new_conf)
        print("已插入 /voice/ 与 /voice/ws 两个 location")

    # ---- 3. 校验 + 重载 ----
    r = sh("docker exec 1Panel-openresty-CM7c nginx -t", check=False)
    if r.returncode != 0 or "successful" not in ((r.stdout or "") + (r.stderr or "")):
        print("!! nginx 配置校验失败，尝试回滚")
        baks = sorted(
            [p for p in os.listdir(os.path.dirname(SITE_CONF)) if ".bak-ts3web-" in p]
        )
        if baks:
            latest = os.path.join(os.path.dirname(SITE_CONF), baks[-1])
            shutil.copy2(latest, SITE_CONF)
            print(f"已回滚到 {latest}")
        raise SystemExit(1)

    sh("docker exec 1Panel-openresty-CM7c nginx -s reload")
    print("\n完成。网页地址： https://inory.xin/voice/?t=<口令>&n=<昵称>")


if __name__ == "__main__":
    main()
