"""Selftest fixture `variant`: a valid kanban built differently from golden.

Must PASS: verify judges the contract, not golden's design. Differences:
uuid string ids, POST answers 200 (not 201), DELETE answers a JSON body,
the store is a {id: card} mapping written whole on every change, the page
is inlined here instead of living in static/, listens on 0.0.0.0.
"""

import json
import os
import uuid
from pathlib import Path

from aiohttp import web

COLUMNS = ["todo", "doing", "done"]
BOARD_FILE = Path(os.environ.get("BOARD_FILE", "board.json"))
PAGE = (Path(__file__).resolve().parent / "page.html").read_text(encoding="utf-8")

cards = json.loads(BOARD_FILE.read_text(encoding="utf-8")) if BOARD_FILE.exists() else {}


def store():
    with open(BOARD_FILE, "w", encoding="utf-8") as f:
        json.dump(cards, f)
        f.flush()
        os.fsync(f.fileno())


async def body_of(request):
    try:
        data = await request.json()
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


routes = web.RouteTableDef()


@routes.get("/")
async def page(request):
    return web.Response(text=PAGE, content_type="text/html")


@routes.get("/api/cards")
async def list_cards(request):
    return web.json_response(list(cards.values()))


@routes.post("/api/cards")
async def create(request):
    title = (await body_of(request)).get("title")
    if not isinstance(title, str) or title.strip() == "":
        raise web.HTTPBadRequest(text="blank title")
    card = {"id": uuid.uuid4().hex, "title": title, "column": "todo"}
    cards[card["id"]] = card
    store()
    return web.json_response(card)


@routes.patch("/api/cards/{id}")
async def move(request):
    card = cards.get(request.match_info["id"])
    if card is None:
        raise web.HTTPNotFound()
    column = (await body_of(request)).get("column")
    if column not in COLUMNS:
        raise web.HTTPBadRequest(text="unknown column")
    card["column"] = column
    store()
    return web.json_response(card)


@routes.delete("/api/cards/{id}")
async def delete(request):
    if cards.pop(request.match_info["id"], None) is None:
        raise web.HTTPNotFound()
    store()
    return web.json_response({"deleted": request.match_info["id"]})


app = web.Application()
app.add_routes(routes)

if __name__ == "__main__":
    web.run_app(app, host="0.0.0.0", port=int(os.environ.get("PORT", "8770")))
