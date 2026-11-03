"""Self-contained protocol test: starts server.py, talks to it, stops it.

Run: python3 test_chat.py  (exit 0 = every check passed)
"""

import asyncio
import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

from aiohttp import ClientSession, WSMsgType

HERE = Path(__file__).resolve().parent


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def start_server(port):
    env = dict(os.environ, PORT=str(port))
    proc = subprocess.Popen(
        [sys.executable, str(HERE / "server.py")],
        cwd=HERE,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"server.py exited early: {proc.stderr.read().decode()}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                return proc
        except OSError:
            time.sleep(0.1)
    proc.kill()
    raise RuntimeError("server.py did not start listening within 10s")


async def recv(ws, kind, contains="", timeout=5):
    """Next frame of the given type (and text); other frames are skipped."""
    async def loop():
        while True:
            msg = await ws.receive()
            assert msg.type == WSMsgType.TEXT, f"expected a text frame, got {msg.type}"
            data = json.loads(msg.data)
            if data.get("type") == kind and contains in str(data.get("text", "")):
                return data

    return await asyncio.wait_for(loop(), timeout)


async def run_checks(port):
    url = f"ws://127.0.0.1:{port}/ws"
    async with ClientSession() as session:
        alice = await session.ws_connect(url)
        await alice.send_json({"type": "join", "nick": "alice"})
        history = await recv(alice, "history")
        assert history["messages"] == [], history

        bob = await session.ws_connect(url)
        await bob.send_json({"type": "join", "nick": "bob"})
        assert (await recv(bob, "history"))["messages"] == []
        await recv(alice, "system", contains="bob")

        await alice.send_json({"type": "message", "text": "hello"})
        for ws in (alice, bob):
            got = await recv(ws, "message")
            assert got["nick"] == "alice" and got["text"] == "hello", got
            assert isinstance(got["ts"], int), got

        await bob.close()
        await recv(alice, "system", contains="bob left")

        carol = await session.ws_connect(url)
        await carol.send_json({"type": "join", "nick": "carol"})
        history = await recv(carol, "history")
        texts = [(m["nick"], m["text"]) for m in history["messages"]]
        assert texts == [("alice", "hello")], texts

        for i in range(55):
            await carol.send_json({"type": "message", "text": f"m{i}"})
        while (await recv(carol, "message"))["text"] != "m54":
            pass
        dave = await session.ws_connect(url)
        await dave.send_json({"type": "join", "nick": "dave"})
        history = await recv(dave, "history")
        # 56 messages so far ("hello", m0..m54): only the last 50 are kept.
        texts = [m["text"] for m in history["messages"]]
        assert texts == [f"m{i}" for i in range(5, 55)], texts

        for ws in (alice, carol, dave):
            await ws.close()


def main():
    port = free_port()
    proc = start_server(port)
    try:
        asyncio.run(run_checks(port))
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
    print("OK")


if __name__ == "__main__":
    main()
