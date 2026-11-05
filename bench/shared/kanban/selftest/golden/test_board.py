"""Automated test of the kanban server: starts server.py, walks the API, stops it."""

import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Server:
    def __init__(self, board_file):
        self.board_file = board_file
        self.proc = None
        self.port = None

    def start(self):
        self.port = free_port()
        env = dict(os.environ, PORT=str(self.port), BOARD_FILE=self.board_file)
        self.proc = subprocess.Popen([sys.executable, "server.py"], cwd=HERE, env=env)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                raise AssertionError(f"server.py exited with {self.proc.returncode}")
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{self.port}/", timeout=1).close()
                return
            except (OSError, ValueError):
                time.sleep(0.1)
        raise AssertionError("server.py did not come up within 10 s")

    def stop(self, sig=signal.SIGTERM):
        if self.proc and self.proc.poll() is None:
            self.proc.send_signal(sig)
            self.proc.wait(timeout=5)

    def call(self, method, path, body=None):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(
            f"http://127.0.0.1:{self.port}{path}",
            data=data,
            method=method,
            headers={"Content-Type": "application/json"} if data else {},
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                text = resp.read().decode()
                return resp.status, json.loads(text) if text else None
        except urllib.error.HTTPError as err:
            return err.code, None


def board(server):
    status, cards = server.call("GET", "/api/cards")
    assert status == 200, status
    return {str(c["id"]): (c["title"], c["column"]) for c in cards}


def check(server):
    assert board(server) == {}, "a new board must be empty"

    status, a = server.call("POST", "/api/cards", {"title": "first"})
    assert 200 <= status < 300 and a["column"] == "todo", (status, a)
    status, b = server.call("POST", "/api/cards", {"title": "second"})
    assert 200 <= status < 300 and str(b["id"]) != str(a["id"]), (status, b)
    assert server.call("POST", "/api/cards", {"title": "  "})[0] == 400

    status, moved = server.call("PATCH", f"/api/cards/{a['id']}", {"column": "doing"})
    assert status == 200 and moved["column"] == "doing", (status, moved)
    assert server.call("PATCH", f"/api/cards/{a['id']}", {"column": "nope"})[0] == 400
    assert server.call("PATCH", "/api/cards/999999", {"column": "done"})[0] == 404

    status, _ = server.call("DELETE", f"/api/cards/{b['id']}")
    assert 200 <= status < 300, status
    assert server.call("DELETE", f"/api/cards/{b['id']}")[0] == 404

    want = {str(a["id"]): ("first", "doing")}
    assert board(server) == want, board(server)

    server.stop(signal.SIGKILL)
    server.start()
    assert board(server) == want, "the board must survive a hard restart"


def main():
    with tempfile.TemporaryDirectory() as tmp:
        server = Server(os.path.join(tmp, "board.json"))
        try:
            server.start()
            check(server)
        except AssertionError as err:
            print(f"FAIL: {err!r}")
            return 1
        finally:
            server.stop()
    print("OK: all kanban checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
