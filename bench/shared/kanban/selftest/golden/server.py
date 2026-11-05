"""Kanban board: aiohttp backend, JSON API, the board kept in a JSON file."""

import json
import os
from pathlib import Path

from aiohttp import web

COLUMNS = ("todo", "doing", "done")
HERE = Path(__file__).resolve().parent
STATIC = HERE / "static"


class Board:
    """Cards in memory, mirrored to `path` on every change."""

    def __init__(self, path):
        self.path = Path(path)
        self.cards = []
        self.next_id = 1
        if self.path.exists():
            data = json.loads(self.path.read_text(encoding="utf-8"))
            self.cards = data["cards"]
            self.next_id = data["next_id"]

    def save(self):
        # Write a sibling file and rename it over: a crash never leaves half a board.
        tmp = self.path.with_name(self.path.name + ".tmp")
        data = {"next_id": self.next_id, "cards": self.cards}
        tmp.write_text(json.dumps(data, ensure_ascii=False, indent=1), encoding="utf-8")
        os.replace(tmp, self.path)

    def find(self, card_id):
        for card in self.cards:
            if str(card["id"]) == card_id:
                return card
        return None


async def read_json(request):
    try:
        data = await request.json()
    except ValueError:
        return None
    return data if isinstance(data, dict) else None


def error(status, message):
    return web.json_response({"error": message}, status=status)


async def index(request):
    return web.FileResponse(STATIC / "index.html")


async def list_cards(request):
    return web.json_response(request.app["board"].cards)


async def create_card(request):
    board = request.app["board"]
    data = await read_json(request)
    title = data.get("title") if data else None
    if not isinstance(title, str) or not title.strip():
        return error(400, "title must be a non-blank string")
    card = {"id": board.next_id, "title": title.strip(), "column": "todo"}
    board.next_id += 1
    board.cards.append(card)
    board.save()
    return web.json_response(card, status=201)


async def update_card(request):
    board = request.app["board"]
    card = board.find(request.match_info["card_id"])
    if card is None:
        return error(404, "no such card")
    data = await read_json(request)
    column = data.get("column") if data else None
    if column not in COLUMNS:
        return error(400, f"column must be one of {', '.join(COLUMNS)}")
    card["column"] = column
    board.save()
    return web.json_response(card)


async def delete_card(request):
    board = request.app["board"]
    card = board.find(request.match_info["card_id"])
    if card is None:
        return error(404, "no such card")
    board.cards.remove(card)
    board.save()
    return web.Response(status=204)


def make_app(board_file):
    app = web.Application()
    app["board"] = Board(board_file)
    app.router.add_get("/", index)
    app.router.add_get("/api/cards", list_cards)
    app.router.add_post("/api/cards", create_card)
    app.router.add_patch("/api/cards/{card_id}", update_card)
    app.router.add_delete("/api/cards/{card_id}", delete_card)
    app.router.add_static("/static", STATIC)
    return app


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8770"))
    board_file = os.environ.get("BOARD_FILE", "board.json")
    web.run_app(make_app(board_file), host="127.0.0.1", port=port, print=None)
