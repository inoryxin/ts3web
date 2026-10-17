#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""打印网页端拿到的频道顺序（用于确认和 TS3 客户端里一致）"""
import asyncio
import json
import sys

import websockets


async def main():
    url = sys.argv[1]
    async with websockets.connect(url) as ws:
        await ws.send(json.dumps({"type": "hello", "nick": "顺序测试"}))
        last = None
        deadline = asyncio.get_event_loop().time() + 6
        while asyncio.get_event_loop().time() < deadline:
            try:
                m = await asyncio.wait_for(ws.recv(), timeout=1)
            except asyncio.TimeoutError:
                continue
            if isinstance(m, str):
                d = json.loads(m)
                if d.get("type") == "state":
                    last = d
        if not last:
            print("没收到频道名单")
            return
        print(f"网页端显示顺序（共 {len(last['channels'])} 个频道，应与 TS3 客户端一致）：")
        for c in last["channels"]:
            mark = "🔊" if c["id"] == last["you"]["channel"] else "  "
            print(f"  {mark} {c['name']}   order={c.get('order')}  人数={len(c['members'])}")


if __name__ == "__main__":
    asyncio.run(main())
