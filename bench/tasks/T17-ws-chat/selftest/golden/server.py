import json
import os
import time
from pathlib import Path

from aiohttp import WSMsgType, web

HISTORY_LIMIT = 50
STATIC_DIR = Path(__file__).resolve().parent / "static"

# ws -> nick, only for clients that have joined.
members = {}
history = []


async def send_json(ws, obj):
    try:
        await ws.send_str(json.dumps(obj))
    except Exception:
        pass


async def broadcast(obj):
    for client in list(members):
        await send_json(client, obj)


async def websocket_handler(request):
    ws = web.WebSocketResponse()
    await ws.prepare(request)

    try:
        async for msg in ws:
            if msg.type != WSMsgType.TEXT:
                continue
            try:
                data = json.loads(msg.data)
            except ValueError:
                continue
            if not isinstance(data, dict):
                continue

            kind = data.get("type")
            if kind == "join" and ws not in members:
                nick = str(data.get("nick") or "").strip() or "anon"
                members[ws] = nick
                await send_json(ws, {"type": "history", "messages": list(history)})
                await broadcast({"type": "system", "text": f"{nick} joined"})
            elif kind == "message" and ws in members:
                text = str(data.get("text") or "")
                if not text:
                    continue
                entry = {
                    "type": "message",
                    "nick": members[ws],
                    "text": text,
                    "ts": int(time.time() * 1000),
                }
                history.append(entry)
                del history[:-HISTORY_LIMIT]
                await broadcast(entry)
    finally:
        nick = members.pop(ws, None)
        if nick is not None:
            await broadcast({"type": "system", "text": f"{nick} left"})
    return ws


async def index(request):
    return web.FileResponse(STATIC_DIR / "index.html")


def make_app():
    app = web.Application()
    app.router.add_get("/", index)
    app.router.add_get("/ws", websocket_handler)
    app.router.add_static("/static", STATIC_DIR)
    return app


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8765"))
    web.run_app(make_app(), host="0.0.0.0", port=port)
