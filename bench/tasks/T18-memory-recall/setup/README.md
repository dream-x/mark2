# orbit-api

Small HTTP API behind the Orbit dashboard.

## Layout

- `app.py` — the service entry point.
- `deploy.ini` — deploy target settings.
- `scripts/deploy.sh` — the deploy helper CI runs.

## Running locally

    python3 app.py

The service listens on `$PORT` (8000 when unset).

## Deploying

CI runs `scripts/deploy.sh <environment>` once the tests pass. Production
settings live in `deploy.ini`. Staging is deployed the same way, but its
settings are not kept in this repository.
