# config/

INI files that `app.py` reads at startup.

- `default.ini` holds the defaults that ship with every release. Upgrades
  replace it, so it is not meant to be edited.
- `override.ini` holds the settings of this particular installation.
  Upgrades never touch it.

All keys belong to the `[updater]` section:

- `release_channel`: the build stream to follow, one of `stable`, `beta` or
  `canary`.
- `update_url`: template of the update feed URL; `{channel}` is replaced with
  the release channel.
- `check_interval_minutes`: how often the feed is polled.
- `download_retries`: download attempts before giving up until the next check.
- `log_level`: `debug`, `info`, `warning` or `error`.
