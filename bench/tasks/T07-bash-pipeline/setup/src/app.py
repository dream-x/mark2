"""Application entry point."""

from core.engine import Engine
from util.io import load_settings


def build_engine(path):
    # TODO: validate the settings file before constructing the engine
    settings = load_settings(path)
    return Engine(settings)


def run(path):
    engine = build_engine(path)
    # TODO: add a dry-run mode
    engine.start()
    try:
        engine.drain()
    finally:
        # TODO: report drain statistics on shutdown
        engine.stop()


def main():
    # TODO: read the settings path from argv
    run("settings.json")


if __name__ == "__main__":
    main()
