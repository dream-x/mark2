"""Selftest fixture `stub`: the server is up, every feature is a dummy.

The page has every required id, the API answers with plausible JSON and the
right status codes for happy paths, but nothing is stored: GET always says
the board is empty. Paired with a no-op static/app.js.
"""

import os
import random
from pathlib import Path

from aiohttp import web

STATIC = Path(__file__).resolve().parent / "static"


async def index(request):
    return web.FileResponse(STATIC / "index.html")


async def list_cards(request):
    return web.json_response([])


async def create_card(request):
    data = await request.json()
    card = {"id": random.randint(1, 10**9), "title": data.get("title"), "column": "todo"}
    return web.json_response(card, status=201)


async def update_card(request):
    data = await request.json()
    card = {"id": request.match_info["card_id"], "title": "?", "column": data.get("column")}
    return web.json_response(card)


async def delete_card(request):
    return web.Response(status=204)


app = web.Application()
app.router.add_get("/", index)
app.router.add_get("/api/cards", list_cards)
app.router.add_post("/api/cards", create_card)
app.router.add_patch("/api/cards/{card_id}", update_card)
app.router.add_delete("/api/cards/{card_id}", delete_card)
app.router.add_static("/static", STATIC)

if __name__ == "__main__":
    web.run_app(app, host="127.0.0.1", port=int(os.environ.get("PORT", "8770")), print=None)
