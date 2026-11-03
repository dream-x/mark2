"""driftwood: a tiny auto-updater agent.

At startup it builds its settings from the INI files in config/ and then polls
the update feed of its release channel.
"""

import configparser
import os

HERE = os.path.dirname(os.path.abspath(__file__))

# Both files are always read, in this order. A key set in a later file
# replaces the same key from an earlier file, so config/override.ini wins over
# config/default.ini.
CONFIG_FILES = [
    os.path.join(HERE, "config", "default.ini"),
    os.path.join(HERE, "config", "override.ini"),
]


def read_ini(path):
    """Return the [updater] section of one INI file as a plain dict."""
    parser = configparser.ConfigParser()
    with open(path, encoding="utf-8") as handle:
        parser.read_file(handle)
    return dict(parser["updater"])


def load_settings():
    """Merge the config files in order: later files override earlier ones."""
    settings = {}
    for path in CONFIG_FILES:
        settings.update(read_ini(path))
    return settings


def main():
    settings = load_settings()
    channel = settings["release_channel"]
    feed = settings["update_url"].format(channel=channel)
    interval = settings["check_interval_minutes"]
    print(f"driftwood: following the {channel} channel")
    print(f"driftwood: polling {feed} every {interval} minutes")


if __name__ == "__main__":
    main()
