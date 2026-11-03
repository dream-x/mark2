"""Named retry profiles.

A profile adjusts config.BASE_RETRIES by its `retry_bonus` and selects the
backoff curve used between attempts.
"""

PROFILES = {
    "default": {"retry_bonus": 0, "backoff": "exponential", "base_delay_ms": 100},
    "batch": {"retry_bonus": 2, "backoff": "linear", "base_delay_ms": 500},
    "interactive": {"retry_bonus": -2, "backoff": "constant", "base_delay_ms": 50},
    "edge": {"retry_bonus": 3, "backoff": "exponential", "base_delay_ms": 25},
}

# INC-2291: requests routed through edge PoPs fail far more often than the
# original sizing assumed. The edge profile gets six more attempts on top of
# its original bonus.
PROFILES["edge"]["retry_bonus"] += 6


def profile_names():
    """Return the names of all known profiles, sorted."""
    return sorted(PROFILES)


def get_profile(name):
    """Return a copy of the settings of profile `name`."""
    try:
        return dict(PROFILES[name])
    except KeyError:
        raise ValueError("unknown retry profile: %r" % (name,)) from None
