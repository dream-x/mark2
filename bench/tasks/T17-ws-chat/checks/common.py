"""Shared pieces of the T17 checks: the server under test and a WebSocket probe.

Every layer runs with the bench venv interpreter (bench/uitest/.venv) and
cwd = the run directory, where the model's server.py lives. Problems are
raised as `Failure` with a message that says what was expected and what was
seen; `run_layer` turns it into one `FAIL [...]` block on stderr and exit 1.
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

# Budget for server.py to start answering HTTP on its port.
SERVER_READY_TIMEOUT = 15.0
# Budget for any single expected reaction (a frame, a DOM update).
STEP_TIMEOUT = 5.0
# Output tails in failure messages: the runner keeps ~4000 chars of stderr.
TAIL_LINES = 12
TAIL_CHARS = 900


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


def kill_group(proc):
    """Stop a process started with start_new_session=True, children included."""
    for sig, grace in ((signal.SIGTERM, 3.0), (signal.SIGKILL, 3.0)):
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
    """`python server.py` on a free port, from the run directory.

    Use as a context manager: the process group is killed on exit whatever
    happens inside, so no server outlives the check.
    """

    def __init__(self, workdir="."):
        self.workdir = workdir
        self.port = None
        self.proc = None
        self.log = None

    @property
    def http_url(self):
        return f"http://127.0.0.1:{self.port}"

    @property
    def ws_url(self):
        return f"ws://127.0.0.1:{self.port}/ws"

    def output(self):
        if self.log is None:
            return b""
        self.log.flush()
        self.log.seek(0)
        return self.log.read()

    def __enter__(self):
        self.port = free_port()
        self.log = tempfile.TemporaryFile()
        env = dict(os.environ, PORT=str(self.port), PYTHONUNBUFFERED="1")
        self.proc = subprocess.Popen(
            [sys.executable, "server.py"],
            cwd=self.workdir,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=self.log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            self._wait_ready()
        except BaseException:
            self.__exit__(None, None, None)
            raise
        return self

    def _wait_ready(self):
        deadline = time.monotonic() + SERVER_READY_TIMEOUT
        while time.monotonic() < deadline:
            code = self.proc.poll()
            if code is not None:
                raise Failure(
                    f"server.py exited with code {code} before it was ready "
                    f"(started as `python server.py` with PORT={self.port}); "
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
            f"server.py did not answer HTTP on 127.0.0.1:{self.port} within "
            f"{SERVER_READY_TIMEOUT:.0f} s (does it read the PORT environment "
            f"variable?); its output:\n{tail(self.output())}"
        )

    def __exit__(self, *exc):
        if self.proc is not None:
            kill_group(self.proc)
        if self.log is not None:
            self.log.close()
            self.log = None
        return False


def show(frame, limit=200):
    text = frame if isinstance(frame, str) else json.dumps(frame, ensure_ascii=False)
    return text if len(text) <= limit else text[:limit] + "..."


def show_frames(frames, last=6):
    if not frames:
        return "nothing"
    shown = [show(f) for f in frames[-last:]]
    skipped = len(frames) - len(shown)
    head = f"{len(frames)} frame(s), last {len(shown)}: " if skipped else ""
    return head + "; ".join(shown)


class Client:
    """A WebSocket client that records every frame it receives.

    Frames are parsed JSON objects; anything else is kept as a "<...>" marker
    string so that it shows up in failure messages instead of crashing.
    `expect` looks only at frames after the last matched one.
    """

    def __init__(self, session, url, name):
        self.session = session
        self.url = url
        self.name = name
        self.ws = None
        self.frames = []
        self.cursor = 0
        self._changed = asyncio.Event()
        self._reader = None

    async def connect(self):
        try:
            self.ws = await asyncio.wait_for(self.session.ws_connect(self.url), STEP_TIMEOUT)
        except Exception as error:
            raise Failure(f"{self.name}: cannot open a WebSocket to {self.url}: {error!r}")
        self._reader = asyncio.create_task(self._read())
        return self

    async def _read(self):
        from aiohttp import WSMsgType

        try:
            async for msg in self.ws:
                if msg.type == WSMsgType.TEXT:
                    try:
                        frame = json.loads(msg.data)
                    except ValueError:
                        frame = f"<non-JSON text frame {msg.data[:120]!r}>"
                    if not isinstance(frame, dict):
                        frame = f"<JSON that is not an object: {show(frame)}>"
                else:
                    frame = f"<{msg.type.name} frame>"
                self.frames.append(frame)
                self._changed.set()
            self.frames.append("<connection closed>")
        except Exception as error:
            self.frames.append(f"<connection error {error!r}>")
        self._changed.set()

    async def send(self, obj):
        try:
            await self.ws.send_str(json.dumps(obj))
        except Exception as error:
            raise Failure(f"{self.name}: cannot send {show(obj)}: {error!r}")

    def mark(self):
        """Skip everything received so far."""
        self.cursor = len(self.frames)

    def since_mark(self):
        return self.frames[self.cursor :]

    async def expect(self, predicate, what, timeout=STEP_TIMEOUT):
        """The first new frame matching `predicate`, or a Failure naming `what`."""
        deadline = time.monotonic() + timeout
        while True:
            for index in range(self.cursor, len(self.frames)):
                frame = self.frames[index]
                if isinstance(frame, dict) and predicate(frame):
                    self.cursor = index + 1
                    return frame
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise Failure(
                    f"{self.name} expected {what} within {timeout:.0f} s; "
                    f"received instead: {show_frames(self.since_mark())}"
                )
            self._changed.clear()
            try:
                await asyncio.wait_for(self._changed.wait(), remaining)
            except asyncio.TimeoutError:
                pass

    async def close(self):
        if self.ws is not None and not self.ws.closed:
            await self.ws.close()
        if self._reader is not None:
            try:
                await asyncio.wait_for(self._reader, STEP_TIMEOUT)
            except Exception:
                self._reader.cancel()


def is_system_about(nick):
    return lambda f: f.get("type") == "system" and nick in str(f.get("text", ""))


def is_message(nick, text):
    return lambda f: f.get("type") == "message" and f.get("nick") == nick and f.get("text") == text


def check_ts(frame, who):
    ts = frame.get("ts")
    if isinstance(ts, bool) or not isinstance(ts, (int, float)) or ts <= 0:
        raise Failure(f"{who}: `ts` must be a positive number (Unix ms), got {show(frame)}")


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
