"""Settings loading."""

import json


def load_settings(path):
    # TODO: fall back to defaults when the file is absent
    with open(path) as handle:
        return json.load(handle)


def save_settings(path, settings):
    # TODO: write atomically via a temporary file
    with open(path, "w") as handle:
        json.dump(settings, handle, indent=2)
