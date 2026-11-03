"""T17 layer B: the UI, driven like a user in headless Chromium.

A fresh server.py on a free port. A probe WebSocket client joins first and
records everything the server announces, so the page's traffic is observed
from the server side, not taken on the page's word.

This layer exists for bugs the protocol layer cannot see. The one that
motivated it (flint:40b, 2026-11-03): app.js sent {"type":"join","nick":"anon"}
from ws.onopen on page load and disabled #nick and #join-btn, so a real user
could not join under their own nick at all, while a scripted two-client
protocol test was green.
"""

import asyncio
import secrets
import sys
import time
from urllib.parse import urlsplit

from aiohttp import ClientSession
from playwright.async_api import Error as PlaywrightError
from playwright.async_api import async_playwright

from common import STEP_TIMEOUT, Client, Failure, Server, is_message, is_system_about, run_layer, show_frames

LABEL = "layer B ui"
IDS = ("nick", "join-btn", "msg", "send-btn", "messages")
# How long the page is left alone after load before auto-join is ruled out.
IDLE_SECONDS = 1.5
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

    def _on_console(self, msg):
        # The browser's own favicon probe 404s on most servers: not a page error.
        if msg.type == "error" and not msg.location.get("url", "").endswith("/favicon.ico"):
            self.errors.append(f"console.error: {msg.text}")

    def fail(self, message):
        details = ""
        if self.errors:
            details = "\n  page errors: " + "; ".join(e[:200] for e in self.errors[:5])
        return Failure(f"{self.name}: {message}{details}")

    async def feed_text(self):
        try:
            return await self.page.locator("#messages").inner_text(timeout=1000)
        except PlaywrightError:
            return ""

    async def wait_feed(self, needle, why):
        deadline = time.monotonic() + STEP_TIMEOUT
        while time.monotonic() < deadline:
            if needle in await self.feed_text():
                return
            await asyncio.sleep(0.1)
        feed = (await self.feed_text()).strip().replace("\n", " | ")
        raise self.fail(
            f"{why}: {needle!r} did not appear in #messages within {STEP_TIMEOUT:.0f} s; "
            f"#messages shows: {feed[-300:]!r}"
        )

    async def act(self, what, coro):
        try:
            await coro
        except PlaywrightError as error:
            first = str(error).strip().splitlines()[0] if str(error).strip() else repr(error)
            raise self.fail(f"cannot {what}: {first}")

    async def type_into(self, selector, text):
        # Focus by a click, then real key presses, as a user would type.
        await self.act(f"click {selector}", self.page.locator(selector).click())
        await self.act(f"type into {selector}", self.page.keyboard.type(text, delay=10))


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
        raise page.fail(f"GET / must return 200 with static/index.html, got status {status}")
    for element_id in IDS:
        if await page.page.locator(f"#{element_id}").count() == 0:
            raise page.fail(f"the page has no element with id={element_id!r}")
    return page


async def assert_not_joined(page, probe):
    """Right after load, with no user input: nobody joined, the nick UI is usable."""
    await asyncio.sleep(IDLE_SECONDS)
    problems = []
    # Chat traffic only: a late notice about the probe's own join or a server
    # heartbeat is not the page's doing.
    own_notice = is_system_about(probe.name)
    announced = [
        f
        for f in probe.since_mark()
        if isinstance(f, dict) and f.get("type") in ("system", "message") and not own_notice(f)
    ]
    if announced:
        problems.append(
            f"the server announced activity although the user did nothing (auto-join?): "
            f"{show_frames(announced)}"
        )
    feed = (await page.feed_text()).strip()
    if feed:
        problems.append(f"#messages must be empty before the user joins, shows {feed[:200]!r}")
    nick = page.page.locator("#nick")
    if not (await nick.is_visible() and await nick.is_editable()):
        problems.append("#nick must be visible and editable before joining, it is not")
    join_btn = page.page.locator("#join-btn")
    if not (await join_btn.is_visible() and await join_btn.is_enabled()):
        problems.append("#join-btn must be visible and enabled before joining, it is not")
    if problems:
        raise page.fail(
            f"{IDLE_SECONDS} s after page load, with no user input: " + "; ".join(problems)
        )


async def send_from(page, probe, nick, text, via):
    await page.type_into("#msg", text)
    if via == "enter":
        await page.act("press Enter in #msg", page.page.keyboard.press("Enter"))
        how = "typing into #msg and pressing Enter"
    else:
        await page.act("click #send-btn", page.page.locator("#send-btn").click())
        how = "typing into #msg and clicking #send-btn"
    try:
        await probe.expect(is_message(nick, text), f"the broadcast of {text!r} from {nick}")
    except Failure as failure:
        raise page.fail(f"{how} did not send the message: {failure}")


async def body():
    tag = secrets.token_hex(3)
    nick1, nick2 = f"ann{tag}", f"ben{tag}"
    text1, text2, text3 = f"typed-enter-{tag}", f"clicked-send-{tag}", f"reply-{tag}"

    with Server() as server:
        async with ClientSession() as session, async_playwright() as playwright:
            probe = await Client(session, server.ws_url, f"probe{tag}").connect()
            await probe.send({"type": "join", "nick": probe.name})
            await probe.expect(lambda f: f.get("type") == "history", "a `history` frame")
            await asyncio.sleep(0.3)
            probe.mark()

            browser = await playwright.chromium.launch(headless=True)
            try:
                first = await open_page(browser, server, "page 1")
                await assert_not_joined(first, probe)
                probe.mark()

                # Join with the keyboard: type the nick, press Enter.
                await first.act("clear #nick", first.page.locator("#nick").fill(""))
                await first.type_into("#nick", nick1)
                await first.act("press Enter in #nick", first.page.keyboard.press("Enter"))
                try:
                    await probe.expect(is_system_about(nick1), f"a `system` notice that {nick1} joined")
                except Failure as failure:
                    raise first.fail(f"typing a nick into #nick and pressing Enter did not join: {failure}")

                await send_from(first, probe, nick1, text1, via="enter")
                await first.wait_feed(text1, "own message sent with Enter")
                await send_from(first, probe, nick1, text2, via="click")
                await first.wait_feed(text2, "own message sent with #send-btn")

                # A second user joins with a click and must see the history.
                second = await open_page(browser, server, "page 2")
                await second.act("clear #nick", second.page.locator("#nick").fill(""))
                await second.type_into("#nick", nick2)
                await second.act("click #join-btn", second.page.locator("#join-btn").click())
                try:
                    await probe.expect(is_system_about(nick2), f"a `system` notice that {nick2} joined")
                except Failure as failure:
                    raise second.fail(f"typing a nick and clicking #join-btn did not join: {failure}")
                await second.wait_feed(text1, "history shown to a newcomer")
                await second.wait_feed(text2, "history shown to a newcomer")
                await first.wait_feed(nick2, "system notice about another user joining")

                await send_from(second, probe, nick2, text3, via="enter")
                await first.wait_feed(text3, "incoming message from another user")
            finally:
                await browser.close()
            await probe.close()


if __name__ == "__main__":
    sys.exit(run_layer(LABEL, body, deadline=75))
