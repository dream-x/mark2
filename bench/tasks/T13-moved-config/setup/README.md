# larder

Larder is a small catalog service: an HTTP API in front of a cache and a
background worker that refreshes catalog entries.

Version 2.0 split the old monolithic `app/` tree into independent services
under `services/`. If you are upgrading from 1.x, read
`docs/MIGRATION-2.0.md` first.

## Configuration

Each service reads its own settings file at startup. Nothing is configured
through environment variables.

## Repository layout

- `services/` — the 2.0 services
- `app/` — the pre-2.0 layout, kept for reference
- `docs/` — operator documentation
- `tests/` — unit tests and their fixtures
