# Multi-user WebSocket chat

## Brief

Build a small multi-user chat in the current working directory: a Python
backend on aiohttp, a plain JavaScript frontend and an automated test. Users
open the page, join under the nick they type and exchange messages over a
WebSocket; a newcomer sees the recent history. A checker will start your
server, talk to it over WebSocket, and drive the page in a headless browser
like a real user (typing and clicking). Follow the requirements below exactly.

## Requirements

### Files

- `server.py`
- `static/index.html`
- `static/app.js`
- `static/style.css`
- `test_chat.py`

### Server: `server.py`

- Python 3 with aiohttp. It is started as `python3 server.py` from the working
  directory and must keep running until it is killed.
- Port: the `PORT` environment variable, default `8765`. Listen on `0.0.0.0`
  or `127.0.0.1`: the checker connects to `127.0.0.1`.
- `GET /` returns `static/index.html`. Every file in `static/` is also served
  at `/static/<name>`, and `index.html` loads `/static/style.css` and
  `/static/app.js` from there.
- The WebSocket endpoint is `/ws` on the same port.

### Protocol

Every WebSocket message is a text frame holding one JSON object.

Client to server:

- `{"type": "join", "nick": "<nick>"}`
- `{"type": "message", "text": "<text>"}`

Server to client:

- `{"type": "history", "messages": [...]}`: sent only to the client that
  joined, right after its `join`. `messages` holds the last 50 chat messages,
  oldest first, each in the `message` form below. System notices are not part
  of the history.
- `{"type": "system", "text": "..."}`: sent to every joined client when a
  client joins and when a joined client leaves (its WebSocket closes). The
  text names the nick, e.g. `alice joined`, `alice left`. Whether the joining
  client also receives its own join notice is up to you.
- `{"type": "message", "nick": "...", "text": "...", "ts": <number>}`: when a
  joined client sends a message, the server broadcasts it to every joined
  client, the sender included. `nick` is the sender's nick, `ts` is the server
  time in Unix milliseconds.

A client that is connected but has not joined gets no broadcasts, and its
`message` frames are ignored.

### Page

- `static/index.html` has elements with exactly these ids: `nick` (text
  input), `join-btn` (button), `msg` (text input), `send-btn` (button),
  `messages` (the chat feed).
- No auto-join. Loading the page must never send `join` by itself, not even
  with a default or remembered nick: the user joins only by acting. Until then
  `#nick` and `#join-btn` are visible and enabled, and `#messages` is empty
  (no placeholder text).
- Clicking `#join-btn` joins with the nick typed into `#nick`. Pressing Enter
  in `#nick` does the same.
- After joining, `#msg` and `#send-btn` are visible and enabled (before
  joining they may be hidden or disabled). Pressing Enter in `#msg` or
  clicking `#send-btn` sends the typed text as a message.
- On join, `#messages` shows the received history. After that, every incoming
  `message` and `system` notice is appended to `#messages` with its text
  visible. A message is shown when its broadcast comes back from the server
  (the sender receives its own broadcast too).
- The page may open the WebSocket on load or when the user joins.
- Plain JavaScript: no frameworks, no build step, nothing loaded from the
  internet. The checker's browser can reach only your server.

### Test: `test_chat.py`

Write your own automated test and ship it with the code:

- It runs as `python3 test_chat.py` from the working directory, needs nothing
  already running, and finishes within 60 seconds.
- It starts `server.py` itself as a subprocess (use `sys.executable`), with
  `PORT` set to a free port it picks, waits until the server accepts
  connections, and stops the server at the end, also when a check fails.
- It checks the protocol with real WebSocket clients: join and history,
  broadcast between two clients, join and leave notices, history for a
  newcomer.
- It exits with status 0 only if every check passed, non-zero otherwise.

### Environment

- `python3` on your PATH has aiohttp available. It is the same Python the
  checker uses to run `server.py` and `test_chat.py`, so there is nothing to
  install. Do not install packages globally.
- You may start the server and run your test while you work. Stop the servers
  you started before you finish, each by the PID of the process you launched —
  never by matching names with pkill or killall: other processes on this
  machine are not yours.

## Plan

Work in this order and finish each step before the next.

1. **Server.** Write `server.py`: the port, `GET /` and `/static/<name>`, the
   `/ws` endpoint. On `join`: `history` to the joining client, a `system`
   notice to every joined client. On `message` from a joined client: the
   broadcast to every joined client, the sender included. On a joined
   client's disconnect: the leave notice. Clients that have not joined get
   nothing and are not heard.
2. **Test.** Write `test_chat.py` as required above: it starts the server on a
   free port, checks join and history, broadcast between two clients, join and
   leave notices and history for a newcomer, and stops the server whatever
   happens.
3. **Run and fix the server.** Run `python3 test_chat.py` until it passes.
4. **Page.** Write `static/index.html`, `static/app.js` and `static/style.css`:
   the five ids, join by click and by Enter, send by click and by Enter, the
   history on join and every incoming `message` and `system` notice appended
   to `#messages`.
5. **Walk the page as a user.** The checker uses the page in a real browser,
   so go through `static/app.js` step by step the way a user meets it: the
   page loads and nobody touches it; a nick is typed and Enter is pressed; a
   message is typed and Enter is pressed; another one is typed and
   `#send-btn` is clicked; a second user opens the page, types a nick and
   clicks `#join-btn`. After each step the page and the server must be in the
   state the Page requirements describe.
6. **Check the Definition of Done.** Go through it item by item and fix what
   fails.
7. **Clean up.** Stop every server you started.

## TODO

- [ ] `server.py` starts with `python3 server.py`, port from `PORT` (default 8765), keeps running until killed
- [ ] `GET /` serves `static/index.html`; every file in `static/` is served at `/static/<name>`
- [ ] `/ws`: `join` gets `history` (the last 50 chat messages, oldest first, no system notices), sent only to the joining client
- [ ] `system` notice naming the nick to every joined client on a join and on a leave
- [ ] `message` broadcast to every joined client, the sender included, with `nick` and `ts` (Unix milliseconds)
- [ ] A client that has not joined gets no broadcasts, and its `message` frames are ignored
- [ ] Page: ids `nick`, `join-btn`, `msg`, `send-btn`, `messages`
- [ ] Page: no auto-join; before joining `#nick` and `#join-btn` are visible and enabled, `#messages` is empty
- [ ] Page: join by clicking `#join-btn` or pressing Enter in `#nick`
- [ ] Page: after joining, `#msg` and `#send-btn` are visible and enabled; send by pressing Enter in `#msg` or clicking `#send-btn`
- [ ] Page: history on join, then every incoming `message` and `system` notice appended to `#messages`
- [ ] Page: plain JavaScript, no frameworks, no build step, nothing loaded from the internet
- [ ] `test_chat.py` written, run, passing
- [ ] Every Definition of Done item checked
- [ ] Every server you started is stopped

## Definition of Done

The task is done when all of the following hold.

1. `server.py`, `static/index.html`, `static/app.js`, `static/style.css` and
   `test_chat.py` exist in the working directory.

Protocol, with the server started as `python3 server.py` and `PORT` set to a
free port:

2. The server answers HTTP on `127.0.0.1` at that port and keeps running.
3. A client that opens a WebSocket at `/ws` and sends `join` receives a
   `history` frame; on a fresh server its `messages` is empty.
4. When a second client joins, it receives its own `history`, and the first
   client receives a `system` notice whose text names the second client's
   nick.
5. A `message` sent by a joined client reaches every joined client, the sender
   included, as a `message` frame with the sender's `nick`, the same `text`
   and a `ts` that is a positive number.
6. When a joined client's WebSocket closes, the joined clients that remain
   receive a `system` notice that names its nick.
7. A client that joins later receives in its `history` exactly the chat
   messages sent so far, oldest first, each with its `nick`, `text` and `ts`,
   and no system notices.
8. After more than 50 chat messages, many of them sent in a quick burst, every
   one of them is broadcast, and a newcomer's `history` holds exactly the last
   50, oldest first.

Page, in a real browser that can reach only your server, typing and clicking
like a user:

9. `GET /` answers 200 with a page that has elements with the ids `nick`,
   `join-btn`, `msg`, `send-btn` and `messages`.
10. Left alone after it loads, the page joins nobody: the server announces no
    join and no message, `#messages` is empty, `#nick` is visible and
    editable, `#join-btn` is visible and enabled.
11. Typing a nick into `#nick` and pressing Enter joins under that nick: the
    other joined clients receive a `system` notice that names it.
12. Typing a text into `#msg` and pressing Enter sends it: every joined client
    receives its broadcast with the page's nick, and the text appears in the
    page's own `#messages`.
13. Typing a text into `#msg` and clicking `#send-btn` does the same.
14. On a second page, typing a nick into `#nick` and clicking `#join-btn`
    joins: its `#messages` shows the texts of the messages sent before, and
    the first page's `#messages` shows a notice that names the newcomer.
15. A message sent from the second page appears in the first page's
    `#messages`.

Test:

16. `python3 test_chat.py`, with nothing running beforehand, exits 0 within 60
    seconds and leaves no server running.
