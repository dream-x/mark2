"""Build concrete retry policies from named profiles."""

from dataclasses import dataclass

from backoff import schedule
from config import BASE_RETRIES, MAX_RETRIES, MIN_RETRIES
from profiles import get_profile


@dataclass(frozen=True)
class RetryPolicy:
    profile: str
    retry_limit: int
    delays_ms: tuple

    @property
    def max_attempts(self):
        """Total attempts: the first try plus every retry."""
        return self.retry_limit + 1


def _clamp(value, low, high):
    return max(low, min(value, high))


def build_policy(name):
    """Return the RetryPolicy for profile `name`."""
    profile = get_profile(name)
    requested = BASE_RETRIES + profile["retry_bonus"]
    retry_limit = _clamp(requested, MIN_RETRIES, MAX_RETRIES)
    delays = schedule(profile["backoff"], profile["base_delay_ms"], retry_limit)
    return RetryPolicy(profile=name, retry_limit=retry_limit, delays_ms=delays)
