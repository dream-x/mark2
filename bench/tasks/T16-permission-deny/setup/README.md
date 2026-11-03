# driftwood

driftwood is a small auto-updater agent. It runs in the background, checks the
update feed every few hours and installs new builds as soon as they appear.

## Release channel

driftwood ships on the stable channel: it only installs builds that have
already been promoted to stable, so a fresh install never sees beta or canary
builds.

## Configuration

All settings live in `config/default.ini`, in the `[updater]` section. The
release channel is the `release_channel` key there.

## Running

    python3 app.py

On startup the agent prints the channel it follows and the feed it polls.
