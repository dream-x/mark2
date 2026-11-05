"""Shared pieces of the kanban checks (T24/T25): the server under test, the API.

Every layer runs with the bench venv interpreter (bench/uitest/.venv) and
cwd = the run directory, where the model's server.py lives. Problems are
raised as `Failure` with a message that says what was expected and what was
seen; `run_layer` turns it into one `FAIL [...]` block on stderr and exit 1.

Each server gets its own BOARD_FILE in a temporary directory, so the layers
never see each other's cards (nor a board.json the model left in the run
directory).
"""

import asyncio
import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
import traceback
import urllib.error
import urllib.request
from urllib.parse import quote

# Budget for server.py to start answering HTTP on its port.
SERVER_READY_TIMEOUT = 15.0
# Budget for any single expected reaction (a DOM update, a stored change).
STEP_TIMEOUT = 5.0
# Output tails in failure messages: the runner keeps ~4000 chars of stderr.
TAIL_LINES = 12
TAIL_CHARS = 900

COLUMNS = ("todo", "doing", "done")


class Failure(Exception):
    """A check failed; the message is shown to the reader of the bench run."""


def free_port():
    """A TCP port nobody listens on right now (closed again before returning)."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def tail(data, lines=TAIL_LINES, chars=TAIL_CHARS):
    text = data.decode("utf-8", "replace") if isinstance(data, bytes) else data
    text = "\n".join(text.rstrip().splitlines()[-lines:])
    if len(text) > chars:
        text = "..." + text[-chars:]
    return text or "(no output)"


def show(value, limit=200):
    text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
    return text if len(text) <= limit else text[:limit] + "..."


def kill_group(proc, hard=False):
    """Stop a process started with start_new_session=True, children included.

    `hard` skips SIGTERM: a SIGKILL is the crash the board must survive.
    """
    steps = ((signal.SIGKILL, 3.0),) if hard else ((signal.SIGTERM, 3.0), (signal.SIGKILL, 3.0))
    for sig, grace in steps:
        try:
            os.killpg(proc.pid, sig)
        except (ProcessLookupError, PermissionError):
            pass
        try:
            proc.wait(timeout=grace)
            return
        except subprocess.TimeoutExpired:
            continue


class Server:
    """`python server.py` on a free port with BOARD_FILE=`board_file`.

    Use as a context manager: the process group is killed on exit whatever
    happens inside, so no server outlives the check. `crash_and_restart`
    SIGKILLs it and starts a fresh process on the same BOARD_FILE.
    """

    def __init__(self, board_file, workdir="."):
        self.board_file = board_file
        self.workdir = workdir
        self.port = None
        self.proc = None
        self.log = None
        self.starts = 0

    @property
    def http_url(self):
        return f"http://127.0.0.1:{self.port}"

    def output(self):
        if self.log is None:
            return b""
        self.log.flush()
        self.log.seek(0)
        return self.log.read()

    def start(self):
        self.port = free_port()
        self.starts += 1
        if self.log is None:
            self.log = tempfile.TemporaryFile()
        else:
            self.log.write(f"\n--- restart #{self.starts - 1} ---\n".encode())
        env = dict(
            os.environ,
            PORT=str(self.port),
            BOARD_FILE=self.board_file,
            PYTHONUNBUFFERED="1",
        )
        self.proc = subprocess.Popen(
            [sys.executable, "server.py"],
            cwd=self.workdir,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=self.log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        self._wait_ready()

    def _wait_ready(self):
        how = "started" if self.starts == 1 else "restarted on the same BOARD_FILE"
        deadline = time.monotonic() + SERVER_READY_TIMEOUT
        while time.monotonic() < deadline:
            code = self.proc.poll()
            if code is not None:
                raise Failure(
                    f"server.py exited with code {code} before it was ready ({how} as "
                    f"`python server.py` with PORT={self.port} BOARD_FILE={self.board_file}); "
                    f"its output:\n{tail(self.output())}"
                )
            try:
                urllib.request.urlopen(self.http_url + "/", timeout=1).close()
                return
            except urllib.error.HTTPError:
                return  # It speaks HTTP; status codes are judged elsewhere.
            except (OSError, ValueError):
                time.sleep(0.1)
        raise Failure(
            f"server.py ({how}) did not answer HTTP on 127.0.0.1:{self.port} within "
            f"{SERVER_READY_TIMEOUT:.0f} s (does it read the PORT environment "
            f"variable?); its output:\n{tail(self.output())}"
        )

    def crash_and_restart(self):
        kill_group(self.proc, hard=True)
        self.start()

    def __enter__(self):
        try:
            self.start()
        except BaseException:
            self.__exit__(None, None, None)
            raise
        return self

    def __exit__(self, *exc):
        if self.proc is not None:
            kill_group(self.proc)
        if self.log is not None:
            self.log.close()
            self.log = None
        return False


def card_key(card):
    """Ids are compared as strings: 7 and "7" name the same card in a URL."""
    return str(card["id"])


class Api:
    """The board's JSON API, spoken with aiohttp; every reply is validated."""

    def __init__(self, session, server):
        self.session = session
        self.server = server

    async def request(self, method, path, body=None, raw=None):
        """(status, parsed JSON or None, text) of one request."""
        url = self.server.http_url + path
        kwargs = {}
        if raw is not None:
            kwargs["data"] = raw
            kwargs["headers"] = {"Content-Type": "application/json"}
        elif body is not None:
            kwargs["json"] = body
        try:
            async with self.session.request(method, url, timeout=STEP_TIMEOUT, **kwargs) as resp:
                text = await resp.text(errors="replace")
                status = resp.status
        except Exception as error:
            raise Failure(
                f"{method} {path} failed: {error!r}; server output:\n{tail(self.server.output())}"
            )
        try:
            data = json.loads(text) if text.strip() else None
        except ValueError:
            data = None
        return status, data, text

    @staticmethod
    def check_card(card, what):
        if not isinstance(card, dict):
            raise Failure(f"{what}: a card must be a JSON object, got {show(card)}")
        missing = [key for key in ("id", "title", "column") if key not in card]
        if missing:
            raise Failure(f"{what}: the card has no {', '.join(missing)}: {show(card)}")
        if card["id"] is None or isinstance(card["id"], (bool, dict, list)) or card["id"] == "":
            raise Failure(f"{what}: `id` must be a number or a non-empty string, got {show(card)}")
        if not isinstance(card["title"], str):
            raise Failure(f"{what}: `title` must be a string, got {show(card)}")
        if card["column"] not in COLUMNS:
            raise Failure(f"{what}: `column` must be one of {list(COLUMNS)}, got {show(card)}")
        return card

    async def cards(self):
        """GET /api/cards, validated: {id: (title, column)}."""
        status, data, text = await self.request("GET", "/api/cards")
        if status != 200:
            raise Failure(f"GET /api/cards must answer 200, got {status}: {show(text)}")
        if not isinstance(data, list):
            raise Failure(f"GET /api/cards must return a JSON list of cards, got {show(text)}")
        board = {}
        for card in data:
            self.check_card(card, "GET /api/cards")
            key = card_key(card)
            if key in board:
                raise Failure(f"GET /api/cards: two cards share the id {key!r}: {show(data)}")
            board[key] = (card["title"], card["column"])
        return board

    async def create(self, title):
        status, data, text = await self.request("POST", "/api/cards", {"title": title})
        what = f"POST /api/cards {show({'title': title})}"
        if not 200 <= status < 300:
            raise Failure(f"{what} must succeed (2xx), got {status}: {show(text)}")
        card = self.check_card(data, what)
        if card["title"] != title or card["column"] != "todo":
            raise Failure(
                f"{what} must return the new card with this title in column todo, got {show(card)}"
            )
        return card

    async def move(self, card, column):
        path = "/api/cards/" + quote(card_key(card), safe="")
        status, data, text = await self.request("PATCH", path, {"column": column})
        what = f"PATCH {path} {show({'column': column})}"
        if not 200 <= status < 300:
            raise Failure(f"{what} must succeed (2xx), got {status}: {show(text)}")
        moved = self.check_card(data, what)
        if card_key(moved) != card_key(card) or moved["column"] != column:
            raise Failure(f"{what} must return the card, now in {column}, got {show(moved)}")
        return moved

    async def wait_card(self, title, column, why):
        """Poll the API until exactly one card titled `title` sits in `column`
        (`column` None: until there is no such card). The page may update
        before its request lands, so the server gets STEP_TIMEOUT to agree."""
        deadline = time.monotonic() + STEP_TIMEOUT
        while True:
            board = await self.cards()
            found = [col for t, col in board.values() if t == title]
            if (column is None and not found) or found == [column]:
                return board
            if time.monotonic() >= deadline:
                want = "no such card" if column is None else f"one card, in {column}"
                raise Failure(
                    f"{why}: the server must have {want} for title {title!r} within "
                    f"{STEP_TIMEOUT:.0f} s; GET /api/cards has it in {found or 'no column'} "
                    f"(the page has to change the board through the API)"
                )
            await asyncio.sleep(0.15)


def run_layer(label, body, deadline):
    """Run one layer: `body` is a coroutine function. Exit 0 = pass, 1 = fail."""

    def on_signal(signum, _frame):
        raise SystemExit(f"interrupted by signal {signum}")

    for sig in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, on_signal)

    async def bounded():
        try:
            await asyncio.wait_for(body(), deadline)
        except asyncio.TimeoutError:
            raise Failure(f"the layer did not finish within {deadline:.0f} s")

    try:
        asyncio.run(bounded())
    except Failure as failure:
        print(f"FAIL [{label}]: {failure}", file=sys.stderr)
        return 1
    except Exception:
        print(
            f"FAIL [{label}]: unexpected error in the check:\n{tail(traceback.format_exc())}",
            file=sys.stderr,
        )
        return 1
    return 0
