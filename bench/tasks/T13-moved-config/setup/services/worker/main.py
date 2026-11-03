"""Background worker that refreshes stale catalog entries."""

import configparser
import os

HERE = os.path.dirname(os.path.abspath(__file__))
WORKER_FILE = os.path.join(HERE, "conf", "worker.ini")


def load_worker_settings(path=WORKER_FILE):
    parser = configparser.ConfigParser()
    with open(path, encoding="utf-8") as handle:
        parser.read_file(handle)
    return parser


def run_once(fetch, store, keys):
    """Refresh every key in `keys` once: fetch a fresh value and store it."""
    refreshed = 0
    for key in keys:
        value = fetch(key)
        if value is not None:
            store(key, value)
            refreshed += 1
    return refreshed
