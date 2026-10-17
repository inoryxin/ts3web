#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""直接切频道的最简测试，配合 debug 日志看 TS3 到底收到什么"""
import asyncio
import json
import sys
import time

import websockets


async def main():
    url = sys.argv[1]
    target = sys.argv[2] if len(sys.argv) > 2 else "刑满释放"
    async with websockets.connect(url, max_size=None) as ws:
        await ws.send(json.dumps({"type": "hello", "nick": "调试"}))
        moved = False
        end = time.time() + 10
        while time.time() < end:
            try:
                msg = await asyncio.wait_for(ws.recv(), timeout=0.5)
            except asyncio.TimeoutError:
                continue
            if isinstance(msg, bytes):
                continue
            d = json.loads(msg)
            if d.get("type") == "state" and not moved:
                t = next((c for c in d["channels"] if c["name"] == target), None)
                if t:
                    print(f"-> 切换到 {target} (id={t['id']})")
                    await ws.send(json.dumps({"type": "move", "channelId": t["id"]}))
                    moved = True
            elif d.get("type") == "state" and moved:
                me = d["you"]
                cur = next((c for c in d["channels"] if c["id"] == me["channel"]), None)
                print(f"   现在在: {cur['name'] if cur else '?'}")
            elif d.get("type") == "error":
                print(f"   !! {d.get('message')}")
        print("done")


if __name__ == "__main__":
    asyncio.run(main())
