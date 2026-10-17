#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""连上网关，依次发若干条 TS3 原始命令，打印 TS3 的应答（用于定位 clientmove 为什么失败）"""
import asyncio
import json
import sys
import time

import websockets

PROBES = [
    "whoami",
    "clientupdate client_description=dsh-probe",
    "clientmove cid=1",
    "clientmove cid=1 cpw=",
    "clientmove cid=2 cpw=",
]


async def main():
    url = sys.argv[1]
    async with websockets.connect(url, max_size=None) as ws:
        await ws.send(json.dumps({"type": "hello", "nick": "探针"}))
        me = None
        state = None

        # 等第一个 state（拿到自己的 client id）
        end = time.time() + 8
        while time.time() < end and state is None:
            try:
                msg = await asyncio.wait_for(ws.recv(), timeout=1)
            except asyncio.TimeoutError:
                continue
            if isinstance(msg, str):
                d = json.loads(msg)
                if d.get("type") == "state":
                    state = d
                    me = d["you"]
                    print(f"我是 id={me['id']} nick={me['name']} 频道={me['channel']}")
                    break

        probes = list(PROBES)
        if me:
            probes.insert(3, f"clientmove clid={me['id']} cid=1")

        for p in probes:
            print(f"\n>>> {p}")
            await ws.send(json.dumps({"type": "raw", "cmd": p}))
            got = False
            end = time.time() + 4
            while time.time() < end:
                try:
                    msg = await asyncio.wait_for(ws.recv(), timeout=1)
                except asyncio.TimeoutError:
                    continue
                if isinstance(msg, bytes):
                    continue
                d = json.loads(msg)
                if d.get("type") == "raw_result" and d.get("cmd") == p:
                    ok = d.get("ok")
                    err = d.get("error") or ""
                    print(f"    <- {'OK' if ok else 'FAIL'}  {err}")
                    got = True
                    break
                if d.get("type") == "error":
                    print(f"    <- 服务器: {d.get('message')}")
            if not got:
                print("    <- （没有应答，超时）")
            await asyncio.sleep(0.4)


if __name__ == "__main__":
    asyncio.run(main())
