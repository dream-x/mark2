"""T17 layer A: the WebSocket protocol, spoken directly by aiohttp clients.

A fresh server.py on a free port; four clients walk through the contract:
join -> history, join/leave notices, broadcast to every joined client
(sender included), history for a newcomer, and the 50-message history cap.
"""

import sys

from aiohttp import ClientSession

from common import (
    Client,
    Failure,
    Server,
    check_ts,
    is_message,
    is_system_about,
    run_layer,
    show,
)

LABEL = "layer A protocol"
HISTORY_LIMIT = 50
FLOOD = 55


def is_history(frame):
    return frame.get("type") == "history"


def history_entries(frame, who):
    messages = frame.get("messages")
    if not isinstance(messages, list):
        raise Failure(f"{who}: `history.messages` must be a list, got {show(frame)}")
    for entry in messages:
        if not isinstance(entry, dict) or entry.get("type") != "message":
            raise Failure(
                f"{who}: history may hold only `message` objects (system notices "
                f"are not part of it), found {show(entry)}"
            )
        check_ts(entry, who)
    return [(entry.get("nick"), entry.get("text")) for entry in messages]


async def join(session, server, nick):
    client = await Client(session, server.ws_url, nick).connect()
    await client.send({"type": "join", "nick": nick})
    history = await client.expect(is_history, f"a `history` frame in reply to its join")
    return client, history


async def say(sender, text, receivers):
    await sender.send({"type": "message", "text": text})
    for client in receivers:
        frame = await client.expect(
            is_message(sender.name, text),
            f'the broadcast {{"type":"message","nick":"{sender.name}","text":"{text}",...}}',
        )
        check_ts(frame, client.name)


async def body():
    with Server() as server:
        async with ClientSession() as session:
            alice, history = await join(session, server, "alice")
            if history_entries(history, "alice") != []:
                raise Failure(f"alice: history on a fresh server must be empty, got {show(history)}")

            bob, history = await join(session, server, "bob")
            history_entries(history, "bob")
            await alice.expect(is_system_about("bob"), "a `system` notice mentioning bob's join")

            await say(alice, "hello from alice", [alice, bob])
            await say(bob, "hi from bob", [alice, bob])

            await bob.close()
            await alice.expect(is_system_about("bob"), "a `system` notice mentioning that bob left")

            carol, history = await join(session, server, "carol")
            want = [("alice", "hello from alice"), ("bob", "hi from bob")]
            got = history_entries(history, "carol")
            if got != want:
                raise Failure(
                    f"carol (a newcomer) expected history {want} (oldest first), got {got}"
                )

            for i in range(FLOOD):
                await carol.send({"type": "message", "text": f"flood-{i:02d}"})
            await carol.expect(
                is_message("carol", f"flood-{FLOOD - 1:02d}"),
                f"the broadcast of its last flood message flood-{FLOOD - 1:02d}",
                timeout=10,
            )

            dave, history = await join(session, server, "dave")
            total = len(want) + FLOOD
            texts = [text for _, text in history_entries(history, "dave")]
            want_texts = [f"flood-{i:02d}" for i in range(total - HISTORY_LIMIT - len(want), FLOOD)]
            if texts != want_texts:
                raise Failure(
                    f"after {total} messages a newcomer's history must hold exactly the last "
                    f"{HISTORY_LIMIT} ({want_texts[0]} ... {want_texts[-1]}); got {len(texts)} "
                    f"entries: {texts[:2]} ... {texts[-2:]}"
                )

            for client in (alice, carol, dave):
                await client.close()


if __name__ == "__main__":
    sys.exit(run_layer(LABEL, body, deadline=40))
