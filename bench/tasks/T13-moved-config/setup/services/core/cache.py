"""Catalog cache of the core service."""

import time

from . import settings


class Cache:
    """A minimal TTL cache; entries expire after `ttl` seconds."""

    def __init__(self, ttl, max_entries, clock=time.monotonic):
        self.ttl = ttl
        self.max_entries = max_entries
        self._clock = clock
        self._entries = {}

    def get(self, key):
        item = self._entries.get(key)
        if item is None:
            return None
        value, expires_at = item
        if self._clock() >= expires_at:
            del self._entries[key]
            return None
        return value

    def put(self, key, value):
        if len(self._entries) >= self.max_entries:
            self._entries.pop(next(iter(self._entries)))
        self._entries[key] = (value, self._clock() + self.ttl)


def from_settings():
    """Build the cache configured in the service settings."""
    section = settings.load()["cache"]
    return Cache(
        ttl=section.getint("cache_ttl"),
        max_entries=section.getint("max_entries"),
    )
