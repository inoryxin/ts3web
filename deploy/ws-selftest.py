#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
网页端链路自检（不用浏览器）：
  1. hello 握手 -> ready
  2. 收频道/成员名单 (state)
  3. 按 20ms 一帧发假 Opus 包（验证上行）
  4. 可选：切到指定频道，确认 state 里的"我"换了位置
  5. 统计从 TS3 收到的语音帧

用法：
    python3 ws-selftest.py <wss-url> [持续秒数] [要切过去的频道名]
"""
import asyncio
import json
import sys
import time

import websockets

SENT = 0
RECV_AUDIO = 0
RECV_BYTES = 0
READY = False
STATES = []
SPEAKERS = {}


async def main():
    global SENT, RECV_AUDIO, RECV_BYTES, READY
    url = sys.argv[1]
    duration = float(sys.argv[2]) if len(sys.argv) > 2 else 8.0
    want_channel = sys.argv[3] if len(sys.argv) > 3 else None

    print(f"连接 {url.split('?')[0]} ...")
    async with websockets.connect(url, max_size=None) as ws:
        await ws.send(json.dumps({"type": "hello", "nick": "自检"}))

        end_at = time.time() + duration
        next_frame = time.time()
        moved = False
        last_state = None

        while time.time() < end_at:
            if time.time() >= next_frame:
                await ws.send(bytes([0x78]) + bytes(24))
                SENT += 1
                next_frame += 0.02

            if want_channel and last_state and not moved:
                target = next((c for c in last_state["channels"] if c["name"] == want_channel), None)
                if target:
                    print(f"  请求切换频道 -> 「{want_channel}」(id={target['id']})")
                    await ws.send(json.dumps({"type": "move", "channelId": target["id"]}))
                    moved = True

            try:
                msg = await asyncio.wait_for(ws.recv(), timeout=0.02)
            except asyncio.TimeoutError:
                continue

            if isinstance(msg, bytes):
                RECV_AUDIO += 1
                RECV_BYTES += len(msg)
                sid = int.from_bytes(msg[0:2], "little")
                SPEAKERS[sid] = SPEAKERS.get(sid, 0) + 1
            else:
                data = json.loads(msg)
                t = data.get("type")
                if t == "ready":
                    READY = True
                    print(f"  ready: nick={data.get('nick')} 频道={data.get('channel')}")
                elif t == "state":
                    last_state = data
                    STATES.append(data)
                    me = data["you"]
                    cur = next((c for c in data["channels"] if c["id"] == me["channel"]), None)
                    print(f"  state: 我在「{cur['name'] if cur else '?'}」  频道数={len(data['channels'])}")
                    for c in data["channels"]:
                        if c["members"]:
                            who = ", ".join(m["name"] for m in c["members"])
                            print(f"        - {c['name']}: {who}")
                elif t == "error":
                    print(f"  !! 服务器报错: {data.get('message')}")

    print()
    print(f"结果: ready={READY}  发出音频帧={SENT}  收到语音帧={RECV_AUDIO} ({RECV_BYTES} 字节)")
    if SPEAKERS:
        print(f"      各说话人帧数: {SPEAKERS}")
    if len(STATES) >= 2:
        f = next((c["name"] for c in STATES[0]["channels"] if c["id"] == STATES[0]["you"]["channel"]), "?")
        l = STATES[-1]
        lc = next((c["name"] for c in l["channels"] if c["id"] == l["you"]["channel"]), "?")
        print(f"      频道变化: 开始「{f}」 -> 最后「{lc}」（收到 {len(STATES)} 次名单更新）")
    else:
        print(f"      收到名单更新 {len(STATES)} 次")


if __name__ == "__main__":
    asyncio.run(main())
