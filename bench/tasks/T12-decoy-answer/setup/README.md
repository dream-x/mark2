# retrykit

Retry policies for our internal HTTP client. A policy decides how many times a
failed request is retried and how long the client sleeps between attempts.

## Profiles

Every profile starts from the same base of 4 retries and adds its own bonus.

| Profile       | Bonus | Retry limit | Backoff     |
|---------------|-------|-------------|-------------|
| `default`     | +0    | 4           | exponential |
| `batch`       | +2    | 6           | linear      |
| `interactive` | -2    | 2           | constant    |
| `edge`        | +3    | 7           | exponential |

The effective retry limit for the edge profile is 7: the base of 4 retries
plus the edge bonus of 3. Edge traffic uses a short exponential backoff so that
seven retries still finish well inside the gateway timeout.

## Usage

```python
from policy import build_policy

policy = build_policy("edge")
policy.retry_limit   # 7
policy.delays_ms     # sleep before each retry, in milliseconds
```

`client.send_with_retries()` wraps any `send(request)` callable and applies
the policy of the requested profile.

## Layout

- `src/config.py` — global defaults shared by every profile
- `src/profiles.py` — the named profiles
- `src/policy.py` — `build_policy()`, which turns a profile into a policy
- `src/backoff.py` — backoff curves
- `src/client.py` — the retrying client wrapper
