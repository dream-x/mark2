Build a small multi-user chat in the current working directory: a Python
backend on aiohttp, a plain JavaScript frontend, and an automated test for
it. A checker will start your server, talk to it over WebSocket, and drive
the page in a headless browser like a real user (typing and clicking).
Follow the contract below exactly.

## Files

- `server.py`
- `static/index.html`
- `static/app.js`
- `static/style.css`
- `test_chat.py`

## Server (`server.py`)

- Python 3 with aiohttp. It is started as `python3 server.py` from the
  working directory and must keep running until it is killed.
- Port: the `PORT` environment variable, default `8765`. Listen on `0.0.0.0`
  or `127.0.0.1`: the checker connects to `127.0.0.1`.
- `GET /` returns `static/index.html`. Every file in `static/` is also served
  at `/static/<name>`, and `index.html` loads `/static/style.css` and
  `/static/app.js` from there.
- The WebSocket endpoint is `/ws` on the same port.

## Protocol

Every WebSocket message is a text frame holding one JSON object.

Client to server:

- `{"type": "join", "nick": "<nick>"}`
- `{"type": "message", "text": "<text>"}`

Server to client:

- `{"type": "history", "messages": [...]}`: sent only to the client that
  joined, right after its `join`. `messages` holds the last 50 chat messages,
  oldest first, each in the `message` form below. System notices are not
  part of the history.
- `{"type": "system", "text": "..."}`: sent to every joined client when a
  client joins and when a joined client leaves (its WebSocket closes). The
  text names the nick, e.g. `alice joined`, `alice left`. Whether the joining
  client also receives its own join notice is up to you.
- `{"type": "message", "nick": "...", "text": "...", "ts": <number>}`: when a
  joined client sends a message, the server broadcasts it to every joined
  client, the sender included. `nick` is the sender's nick, `ts` is the
  server time in Unix milliseconds.

A client that is connected but has not joined gets no broadcasts, and its
`message` frames are ignored.

## Page

- `static/index.html` has elements with exactly these ids: `nick` (text
  input), `join-btn` (button), `msg` (text input), `send-btn` (button),
  `messages` (the chat feed).
- No auto-join. Loading the page must never send `join` by itself, not even
  with a default or remembered nick: the user joins only by acting. Until
  then `#nick` and `#join-btn` are visible and enabled, and `#messages` is
  empty (no placeholder text).
- Clicking `#join-btn` joins with the nick typed into `#nick`. Pressing Enter
  in `#nick` does the same.
- After joining, `#msg` and `#send-btn` are visible and enabled (before
  joining they may be hidden or disabled). Pressing Enter in `#msg` or
  clicking `#send-btn` sends the typed text as a message.
- On join, `#messages` shows the received history. After that, every
  incoming `message` and `system` notice is appended to `#messages` with its
  text visible. A message is shown when its broadcast comes back from the
  server (the sender receives its own broadcast too).
- The page may open the WebSocket on load or when the user joins.
- Plain JavaScript: no frameworks, no build step, nothing loaded from the
  internet. The checker's browser can reach only your server.

## Test (`test_chat.py`)

Write your own automated test and ship it with the code:

- It runs as `python3 test_chat.py` from the working directory, needs
  nothing already running, and finishes within 60 seconds.
- It starts `server.py` itself as a subprocess (use `sys.executable`), with
  `PORT` set to a free port it picks, waits until the server accepts
  connections, and stops the server at the end, also when a check fails.
- It checks the protocol with real WebSocket clients: join and history,
  broadcast between two clients, join and leave notices, history for a
  newcomer.
- It exits with status 0 only if every check passed, non-zero otherwise.

## Environment

`python3` on your PATH has aiohttp available. It is the same Python the
checker uses to run `server.py` and `test_chat.py`, so there is nothing to
install. Do not install packages globally. You may start the server and run
your test while you work, but you don't have to. Stop any server you started
before you finish.
