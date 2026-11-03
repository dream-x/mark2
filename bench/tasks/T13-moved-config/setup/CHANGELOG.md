# Changelog

## 2.0.0

- Reorganized the repository into independent services under `services/`.
- Configuration is now owned by each service instead of one shared directory;
  see `docs/MIGRATION-2.0.md`.
- Cache defaults were re-tuned for the new storage backend.
- The worker no longer shares a settings file with the API.

## 1.4.2

- Fixed a crash when the cache backend was unreachable at startup.

## 1.4.0

- Added the background refresh worker.
