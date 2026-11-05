"""Kanban layer A: the JSON API and its persistence, spoken directly over HTTP.

A fresh server.py on a free port with BOARD_FILE pointing at a file that
does not exist yet. The walk follows the contract of both prompts:
an empty board, the page at /, create (and the 400 for blank titles), move
(400 for an unknown column), delete, 404 for unknown ids, and then two hard
crashes (SIGKILL, no chance to save on shutdown) with a restart on the same
BOARD_FILE: the board must come back exactly as it was answered.
"""

import json
import os
import sys
import tempfile
from urllib.parse import quote

from aiohttp import ClientSession

from common import Api, Failure, Server, card_key, run_layer, show

LABEL = "layer A api"


def path_of(key):
    return "/api/cards/" + quote(key, safe="")


async def expect_board(api, want, when):
    got = await api.cards()
    if got != want:
        raise Failure(
            f"{when}: GET /api/cards must hold exactly {show(want, 400)}, got {show(got, 400)}"
        )


async def expect_status(api, method, path, body, want, why):
    status, _, text = await api.request(method, path, body)
    if status != want:
        raise Failure(f"{method} {path} {show(body) if body is not None else ''} must answer "
                      f"{want} ({why}), got {status}: {show(text)}")


def unknown_key(board):
    key = "424242"
    while key in board:
        key += "0"
    return key


async def body():
    with tempfile.TemporaryDirectory(prefix="kanban-api-") as tmp:
        board_file = os.path.join(tmp, "board.json")
        with Server(board_file) as server:
            async with ClientSession() as session:
                api = Api(session, server)

                status, _, text = await api.request("GET", "/")
                if status != 200 or "<" not in text:
                    raise Failure(f"GET / must answer 200 with the page, got {status}: {show(text)}")

                await expect_board(api, {}, "with a missing BOARD_FILE the board starts empty")

                titles = ["Write spec", "Fix login bug", "Купить молоко ☕"]
                cards = [await api.create(title) for title in titles]
                keys = [card_key(card) for card in cards]
                if len(set(keys)) != len(keys):
                    raise Failure(f"three created cards must get three different ids, got {keys}")
                a, b, c = keys
                board = {key: (title, "todo") for key, title in zip(keys, titles)}
                await expect_board(api, board, "after creating three cards")

                for blank in ("", "   "):
                    await expect_status(api, "POST", "/api/cards", {"title": blank}, 400,
                                        "a blank title is rejected")
                await expect_board(api, board, "after two rejected blank titles")

                await api.move(cards[0], "doing")
                await api.move(cards[1], "done")
                await api.move(cards[1], "doing")
                board[a] = (titles[0], "doing")
                board[b] = (titles[1], "doing")
                await expect_board(api, board, "after moving cards between columns")

                await expect_status(api, "PATCH", path_of(c), {"column": "archive"}, 400,
                                    "unknown column")
                await expect_board(api, board, "after a move to an unknown column was rejected")

                missing = unknown_key(board)
                await expect_status(api, "PATCH", path_of(missing), {"column": "done"}, 404,
                                    "unknown id")
                await expect_status(api, "DELETE", path_of(missing), None, 404, "unknown id")

                status, _, text = await api.request("DELETE", path_of(c))
                if not 200 <= status < 300:
                    raise Failure(f"DELETE {path_of(c)} must succeed (2xx), got {status}: {show(text)}")
                del board[c]
                await expect_board(api, board, "after deleting a card")
                await expect_status(api, "DELETE", path_of(c), None, 404, "the card is already deleted")
                await expect_status(api, "PATCH", path_of(c), {"column": "done"}, 404,
                                    "the card is already deleted")

                try:
                    with open(board_file, encoding="utf-8") as f:
                        json.load(f)
                except FileNotFoundError:
                    raise Failure(
                        f"after several changes the BOARD_FILE {board_file} does not exist: every "
                        f"change must be saved, before the server answers, to the path from the "
                        f"BOARD_FILE environment variable"
                    )
                except ValueError as error:
                    raise Failure(f"the BOARD_FILE {board_file} is not valid JSON: {error}")

                server.crash_and_restart()
                await expect_board(
                    api, board, "after SIGKILL and a restart on the same BOARD_FILE (every change "
                    "must be saved before the server answers)"
                )

                fresh = await api.create("After restart")
                key = card_key(fresh)
                if key in board:
                    raise Failure(
                        f"a card created after the restart got id {key!r}, which an existing card "
                        f"already has: {show(board)}"
                    )
                await api.move(fresh, "done")
                board[key] = ("After restart", "done")

                server.crash_and_restart()
                await expect_board(api, board, "after a second SIGKILL and restart")


if __name__ == "__main__":
    sys.exit(run_layer(LABEL, body, deadline=60))
