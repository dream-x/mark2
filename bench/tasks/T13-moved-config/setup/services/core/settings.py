"""Load the settings of the core service."""

import configparser
import os

HERE = os.path.dirname(os.path.abspath(__file__))
SETTINGS_FILE = os.path.join(HERE, "conf", "settings.ini")


def load(path=SETTINGS_FILE):
    """Parse the settings file at `path` and return the ConfigParser."""
    parser = configparser.ConfigParser()
    with open(path, encoding="utf-8") as handle:
        parser.read_file(handle)
    return parser
