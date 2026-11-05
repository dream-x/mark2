# Kanban board web app

## Brief

Build a small kanban board web app in the current directory: an aiohttp
backend, a plain JavaScript page and an automated test. The board has three
columns, To Do, Doing and Done; cards can be added, moved between columns and
deleted, and the board is kept in a JSON file, so it survives a restart.

## Requirements

### Server

- `server.py`, on aiohttp, started as `python3 server.py`.
- Port: the `PORT` environment variable, default `8770`.
- Serves the page at `/`.

### JSON API

- `GET /api/cards` returns the list of all cards. A card is an object
  `{"id": ..., "title": ..., "column": ...}`: the id is unique, the column is
  `todo`, `doing` or `done`.
- `POST /api/cards` with `{"title": ...}` creates a card in `todo` and returns
  it. A blank title: 400.
- `PATCH /api/cards/<id>` with `{"column": ...}` moves the card and returns it.
  An unknown column: 400.
- `DELETE /api/cards/<id>` removes the card.
- `PATCH` and `DELETE` answer 404 for an id that does not exist.

### Storage

- Every change is saved, before the server answers, to a JSON file at the
  path from the `BOARD_FILE` environment variable, default `board.json`.
- No file yet means an empty board.
- The board survives a restart of the server, even a hard kill.

### Page

- Plain JavaScript: no frameworks, no build step, nothing loaded from the
  internet.
- A text input `#new-card` and a button `#add-btn` that adds a card with the
  typed title. Pressing Enter in the input does the same.
- Column elements `#todo`, `#doing` and `#done`, holding one `.card` element
  per card.
- Each `.card` shows its title and has the buttons `.move-left` and
  `.move-right` (move it to the neighbouring column; they may be missing or
  disabled where there is none) and `.delete`.
- The page changes the board only through the API, loads it when it opens and
  always shows it exactly as the server stores it.

### Test: `test_board.py`

- Starts `server.py` itself as a subprocess on a free port with its own
  temporary `BOARD_FILE`.
- Checks the API.
- Stops the server, even when a check fails.
- Finishes within 60 seconds and exits 0 only if everything works.
- You run it and make it pass.

### Environment

- `python3` on your PATH already has aiohttp: install nothing.
- Stop any server you started before you finish.

## Plan

Work in this order and finish each step before the next.

1. **Server and storage.** Write `server.py`: the port, the board loaded from
   `BOARD_FILE`, the four API routes with their 400 and 404 answers, a save on
   every change.
2. **Check the API by hand.** Start the server with a temporary `BOARD_FILE`
   and walk the API: create, a blank title, move, an unknown column, delete,
   an unknown id. Kill the server with SIGKILL, start it again on the same
   `BOARD_FILE` and confirm that the board is unchanged.
3. **Page.** Serve it at `/` with the required ids and classes. It loads the
   board when it opens; adding (button and Enter), moving left and right and
   deleting go through the API; after every action the page shows the board as
   the server stores it.
4. **Test.** Write `test_board.py` as required above.
5. **Run and fix.** Run `python3 test_board.py` until it passes, then go
   through the Definition of Done item by item and fix what fails.
6. **Clean up.** Stop every server you started.

## TODO

- [ ] `server.py` starts with `python3 server.py`, port from `PORT` (default 8770)
- [ ] `GET /` serves the page
- [ ] `GET /api/cards`
- [ ] `POST /api/cards`, 400 for a blank title
- [ ] `PATCH /api/cards/<id>`, 400 for an unknown column, 404 for an unknown id
- [ ] `DELETE /api/cards/<id>`, 404 for an unknown id
- [ ] Board saved to `BOARD_FILE` on every change, before answering; loaded at start; no file means an empty board
- [ ] Page: `#new-card` and `#add-btn`, Enter adds too
- [ ] Page: `#todo`, `#doing`, `#done` with one `.card` per card: title, `.move-left`, `.move-right`, `.delete`
- [ ] Page: loads the board on open, changes it only through the API, always shows it as the server stores it
- [ ] `test_board.py` written, run, passing
- [ ] Every Definition of Done item checked
- [ ] Every server you started is stopped

## Definition of Done

The task is done when all of the following hold.

API and storage, with the server started on a `BOARD_FILE` that does not exist yet:

1. `GET /` answers 200 with the page; `GET /api/cards` answers 200 with `[]`.
2. `POST /api/cards` with a title answers 2xx with the new card: an id, the
   same title, the column `todo`. Three cards get three different ids, and
   `GET /api/cards` lists all three in `todo`.
3. `POST /api/cards` with the title `""` or `"   "` answers 400 and creates
   nothing.
4. `PATCH /api/cards/<id>` to any of the three columns answers 2xx with the
   card in that column, and `GET /api/cards` agrees. With
   `{"column": "archive"}` it answers 400 and the card stays where it was.
5. `DELETE /api/cards/<id>` answers 2xx, and the card is gone from
   `GET /api/cards`.
6. `PATCH` and `DELETE` of an id that does not exist, or no longer exists,
   answer 404.
7. After a change the file at `BOARD_FILE` exists and holds valid JSON.
8. Killed with SIGKILL right after a change and started again on the same
   `BOARD_FILE`, the server returns exactly the same cards: ids, titles,
   columns. A card created after the restart gets an id no other card has, and
   it survives the next SIGKILL and restart as well.

Page, in a real browser, typing and clicking like a user:

9. Cards that are already stored show up when the page opens, each as one
   `.card` in its column (`#todo`, `#doing`, `#done`).
10. Typing a title into `#new-card` and pressing Enter adds the card to
    `#todo`, on the page and on the server. Typing a title and clicking
    `#add-btn` does the same.
11. Clicking `#add-btn` with an empty or blank title adds nothing, neither on
    the page nor on the server.
12. `.move-right` on a card in `#todo` moves it to `#doing`, once more to
    `#done`; `.move-left` on it in `#done` moves it back to `#doing`. After
    each click the card is in exactly one column on the page, and the server
    has it in the same column.
13. `.delete` removes the card from the page and from the server.
14. The page never shows more or fewer cards than the server has. Reloading
    it, or opening it in a second browser, shows the same board.

Test:

15. `python3 test_board.py`, with nothing running beforehand, exits 0 within
    60 seconds and leaves no server running.
16. If `server.py` cannot start (say, it exits at once), `python3 test_board.py`
    exits non-zero.
