"""Kanban layer B: the page, driven like a user in headless Chromium.

A fresh server.py on a free port with its own BOARD_FILE. Two cards are put
on the board through the API before the page opens (one in todo, one in
done), so the page has to load the board, not start from a blank template.
Then the browser types and clicks, and after every action two things must
agree: the DOM (the card sits in exactly one column, the right one) and the
server (GET /api/cards). The second half is what catches a page that only
shuffles DOM nodes: it looks alive in a screenshot, but nothing is stored.
At the end the page is reloaded and opened in a second browser: both must
show the board exactly as the server has it, card for card.
"""

import asyncio
import secrets
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from aiohttp import ClientSession
from playwright.async_api import Error as PlaywrightError
from playwright.async_api import async_playwright

from common import COLUMNS, STEP_TIMEOUT, Api, Failure, Server, run_layer

LABEL = "layer B ui"
IDS = ("new-card", "add-btn", "todo", "doing", "done")
CARDS = ", ".join(f"#{column} .card" for column in COLUMNS)
# How long a blank add gets to (wrongly) create a card before we look.
BLANK_SETTLE_SECONDS = 1.0
LOCAL_HOSTS = {"127.0.0.1", "localhost"}


class Page:
    """A browser page plus the errors it reported, for failure messages."""

    def __init__(self, page, name):
        self.page = page
        self.name = name
        self.errors = []
        page.on("pageerror", lambda error: self.errors.append(f"pageerror: {error}"))
        page.on("console", self._on_console)
        page.on(
            "requestfailed",
            lambda req: self.errors.append(f"request failed: {req.url} ({req.failure})"),
        )
        # A confirm("Delete?") is answered like a user who means it.
        page.on("dialog", lambda dialog: asyncio.ensure_future(dialog.accept()))

    def _on_console(self, msg):
        # The browser's own favicon probe 404s on most servers: not a page error.
        if msg.type == "error" and not msg.location.get("url", "").endswith("/favicon.ico"):
            self.errors.append(f"console.error: {msg.text}")

    def fail(self, message):
        details = ""
        if self.errors:
            details = "\n  page errors: " + "; ".join(e[:200] for e in self.errors[:5])
        return Failure(f"{self.name}: {message}{details}")

    async def act(self, what, coro):
        try:
            await coro
        except PlaywrightError as error:
            first = str(error).strip().splitlines()[0] if str(error).strip() else repr(error)
            raise self.fail(f"cannot {what}: {first}")

    async def type_into(self, selector, text):
        # Focus by a click, clear, then real key presses, as a user would type.
        locator = self.page.locator(selector)
        await self.act(f"click {selector}", locator.click())
        await self.act(f"clear {selector}", locator.fill(""))
        await self.act(f"type into {selector}", self.page.keyboard.type(text, delay=5))

    async def where(self, title):
        """{column: number of .card elements in it showing `title`}."""
        counts = {}
        for column in COLUMNS:
            try:
                counts[column] = await self.page.locator(f"#{column} .card", has_text=title).count()
            except PlaywrightError:
                counts[column] = 0
        return counts

    async def card_count(self):
        try:
            return await self.page.locator(CARDS).count()
        except PlaywrightError:
            return -1

    async def wait_card(self, title, column, why):
        """Exactly one .card showing `title`, inside #`column` (None: nowhere)."""
        want = {c: int(c == column) for c in COLUMNS}
        deadline = time.monotonic() + STEP_TIMEOUT
        while True:
            got = await self.where(title)
            if got == want:
                return
            if time.monotonic() >= deadline:
                seen = ", ".join(f"#{c}: {n}" for c, n in got.items())
                want_text = (
                    "no .card with it in any column"
                    if column is None
                    else f"exactly one .card with it, in #{column}"
                )
                raise self.fail(
                    f"{why}: for title {title!r} the page must show {want_text} within "
                    f"{STEP_TIMEOUT:.0f} s; .card elements with it now: {seen}"
                )
            await asyncio.sleep(0.1)

    async def click_card_button(self, title, column, button, why):
        card = self.page.locator(f"#{column} .card", has_text=title).first
        what = f"click .{button} on the card {title!r} in #{column} ({why})"
        # Hover first, as a mouse does: buttons may show only on hover.
        await self.act(what, card.hover())
        await self.act(what, card.locator(f".{button}").first.click())

    async def click_add_if_enabled(self):
        """A disabled #add-btn for a blank title is a valid way to add nothing."""
        button = self.page.locator("#add-btn")
        if await button.is_enabled():
            await self.act("click #add-btn", button.click())


async def block_external(route):
    if urlsplit(route.request.url).hostname in LOCAL_HOSTS:
        await route.continue_()
    else:
        await route.abort()


async def open_page(browser, server, name):
    context = await browser.new_context()
    await context.route("**/*", block_external)
    page = Page(await context.new_page(), name)
    page.page.set_default_timeout(STEP_TIMEOUT * 1000)
    try:
        response = await page.page.goto(server.http_url + "/", wait_until="load")
    except PlaywrightError as error:
        raise page.fail(f"GET / did not load: {error}")
    if response is None or response.status != 200:
        status = None if response is None else response.status
        raise page.fail(f"GET / must return 200 with the page, got status {status}")
    for element_id in IDS:
        if await page.page.locator(f"#{element_id}").count() == 0:
            raise page.fail(f"the page has no element with id={element_id!r}")
    return page


async def expect_matches_server(page, api, why):
    """The page shows every card of the server in its column, and nothing else."""
    board = await api.cards()
    for title, column in board.values():
        await page.wait_card(title, column, why)
    shown = await page.card_count()
    if shown != len(board):
        raise page.fail(
            f"{why}: the server has {len(board)} card(s) but the columns show {shown} "
            f".card element(s) (duplicates or leftovers?)"
        )


async def body():
    tag = secrets.token_hex(3)
    seeded_todo, seeded_done = f"Seeded todo {tag}", f"Seeded done {tag}"
    typed, clicked = f"Typed with Enter {tag}", f"Clicked add {tag}"

    with tempfile.TemporaryDirectory(prefix="kanban-ui-") as tmp:
        with Server(str(Path(tmp) / "board.json")) as server:
            async with ClientSession() as session, async_playwright() as playwright:
                api = Api(session, server)
                await api.create(seeded_todo)
                await api.move(await api.create(seeded_done), "done")
                # Otherwise "the page shows the stored board" below is vacuous.
                seeded = sorted((await api.cards()).values())
                want = sorted([(seeded_todo, "todo"), (seeded_done, "done")])
                if seeded != want:
                    raise Failure(
                        f"the API does not keep cards (see layer A): after creating two cards and "
                        f"moving one to done, GET /api/cards holds {seeded}, expected {want}"
                    )

                browser = await playwright.chromium.launch(headless=True)
                try:
                    first = await open_page(browser, server, "page 1")
                    await expect_matches_server(
                        first, api, "cards stored before the page opened (the page loads the board)"
                    )

                    # Add with the keyboard: type a title, press Enter.
                    await first.type_into("#new-card", typed)
                    await first.act("press Enter in #new-card", first.page.keyboard.press("Enter"))
                    why = "typing a title into #new-card and pressing Enter"
                    await first.wait_card(typed, "todo", why)
                    await api.wait_card(typed, "todo", why)

                    # Add with the mouse: type a title, click #add-btn.
                    await first.type_into("#new-card", clicked)
                    await first.act("click #add-btn", first.page.locator("#add-btn").click())
                    why = "typing a title into #new-card and clicking #add-btn"
                    await first.wait_card(clicked, "todo", why)
                    await api.wait_card(clicked, "todo", why)

                    # Blank titles add nothing, neither on the page nor on the server.
                    stored, shown = await api.cards(), await first.card_count()
                    await first.act("clear #new-card", first.page.locator("#new-card").fill(""))
                    await first.click_add_if_enabled()
                    await first.type_into("#new-card", "   ")
                    await first.click_add_if_enabled()
                    await asyncio.sleep(BLANK_SETTLE_SECONDS)
                    if await api.cards() != stored:
                        raise first.fail("clicking #add-btn with an empty or blank title created a card on the server")
                    if await first.card_count() != shown:
                        raise first.fail(
                            f"clicking #add-btn with an empty or blank title changed the columns "
                            f"from {shown} to {await first.card_count()} .card element(s)"
                        )

                    # Moves: right, right, left; each one stored and shown in one column.
                    for button, source, target in (
                        ("move-right", "todo", "doing"),
                        ("move-right", "doing", "done"),
                        ("move-left", "done", "doing"),
                    ):
                        why = f"clicking .{button} on a card in #{source}"
                        await first.click_card_button(typed, source, button, why)
                        await first.wait_card(typed, target, why)
                        await api.wait_card(typed, target, why)

                    why = "clicking .delete on a card"
                    await first.click_card_button(clicked, "todo", "delete", why)
                    await first.wait_card(clicked, None, why)
                    await api.wait_card(clicked, None, why)

                    await expect_matches_server(first, api, "after adding, moving and deleting")

                    await first.act("reload the page", first.page.reload(wait_until="load"))
                    await expect_matches_server(first, api, "after reloading the page")

                    second = await open_page(browser, server, "page 2 (a second browser)")
                    await expect_matches_server(second, api, "the same board opened in a second browser")
                finally:
                    await browser.close()


if __name__ == "__main__":
    sys.exit(run_layer(LABEL, body, deadline=90))
