"""Tiny HTTP client wrapper that retries failed requests according to a policy."""

import time

from policy import build_policy

RETRYABLE_STATUS = frozenset({429, 502, 503, 504})


class RetriesExhausted(Exception):
    """Raised when every attempt allowed by the policy has failed."""


def send_with_retries(send, request, profile="default", sleep=time.sleep):
    """Call `send(request)` until it returns a non-retryable status.

    `send` must return an object with a `status` attribute. Between attempts
    the client sleeps according to the backoff schedule of the policy.
    """
    policy = build_policy(profile)
    last = None
    for attempt in range(policy.max_attempts):
        last = send(request)
        if last.status not in RETRYABLE_STATUS:
            return last
        if attempt < policy.retry_limit:
            sleep(policy.delays_ms[attempt] / 1000.0)
    raise RetriesExhausted(
        "%s: gave up after %d attempts (last status %s)"
        % (profile, policy.max_attempts, last.status)
    )
