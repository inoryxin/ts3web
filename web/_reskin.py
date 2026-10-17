#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
重新装配 ts3web 页面：把导航页的 style.css 整份内联 + 我们的补丁样式 + 结构 + 逻辑。

- web/_nav-style.css   ：从 NAS 导航页复制过来的原版样式（要更新就重新 scp 一份）
- web/_overrides.css   ：在导航页样式之上补的部分（表单/二级菜单/频道列表…）
- web/_body.html       ：页面结构（用导航页的类名：.frame/.side/.char/.bubble/.btn-main…）
- web/_extra.js        ：时钟、立绘气泡、樱花关键帧
- <script>…</script>   ：原有语音逻辑，原样保留（元素 id 都没变）

重复执行是安全的：每次都会把上次追加的 _extra.js 去掉再重新拼。
"""
import re
import shutil
import sys
import time

WEB = "/mnt/c/Users/anzu/Documents/xin/ts3web/web"
P = f"{WEB}/index.html"
MARK = "// ================= ts3web："

nav = open(f"{WEB}/_nav-style.css", encoding="utf-8").read()
over = open(f"{WEB}/_overrides.css", encoding="utf-8").read()
body = open(f"{WEB}/_body.html", encoding="utf-8").read()
extra = open(f"{WEB}/_extra.js", encoding="utf-8").read()

src = open(P, encoding="utf-8").read()
m = re.search(r"<script>(.*?)</script>", src, re.S)
if not m:
    sys.exit("找不到 <script> 块")
script = m.group(1)
# 去掉上次追加的部分，保证可重复执行
script = script.split(MARK)[0].rstrip()

if len(nav) < 5000:
    sys.exit("导航页样式好像是空的，先重新复制一份 _nav-style.css")

bak = f"{P}.bak-{time.strftime('%Y%m%d-%H%M%S')}"
shutil.copy2(P, bak)
print(f"已备份: {bak}")

out = f"""<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="theme-color" content="#ffd9e8">
<meta name="darkreader-lock">
<title>✿ 许雪然的 TS3 语音频道</title>
<!-- ===== 导航页原版样式（内联，保证风格完全一致）===== -->
<style>
{nav}</style>
<!-- ===== ts3web 补丁样式 ===== -->
<style>
{over}</style>
</head>
<body>
{body}
<script>{script}
{MARK} 以下为页面装饰逻辑（时钟/气泡/樱花）=================
{extra}</script>
</body>
</html>
"""

open(P, "w", encoding="utf-8").write(out)
print(f"新页面: {len(out.encode())} 字节（导航页样式 {len(nav.encode())} + 补丁 {len(over.encode())} + 逻辑 {len(script.encode())}）")

need = ["nick","token","tokenRow","join","leave","mute","volume","volLabel","micName","micLevel",
        "stats","status","speakers","channels","hint","connBadge","optAec","optNs","optAgc",
        "optBalance","txVad","txCont","txPtt","vadRow","vadThresh","vadThreshLabel","vadState",
        "bubble","clockTime","clockSec","brandLogo"]
missing = [i for i in need if f'id="{i}"' not in out]
print("缺失的 id:", missing if missing else "无 ✓")
