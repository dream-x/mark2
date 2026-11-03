"""Backoff curves: how long to sleep before each retry."""

from config import MAX_DELAY_MS


def _constant(base_ms, attempt):
    return base_ms


def _linear(base_ms, attempt):
    return base_ms * (attempt + 1)


def _exponential(base_ms, attempt):
    return base_ms * (2 ** attempt)


CURVES = {
    "constant": _constant,
    "linear": _linear,
    "exponential": _exponential,
}


def schedule(curve, base_ms, retries):
    """Return the sleep before each of `retries` retries, in milliseconds."""
    try:
        step = CURVES[curve]
    except KeyError:
        raise ValueError("unknown backoff curve: %r" % (curve,)) from None
    return tuple(min(step(base_ms, attempt), MAX_DELAY_MS) for attempt in range(retries))
