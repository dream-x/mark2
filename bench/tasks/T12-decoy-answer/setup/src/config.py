"""Global retry defaults shared by every profile.

Every profile starts from BASE_RETRIES and applies its own adjustment (see
profiles.py). policy.build_policy() then keeps the result within the bounds
defined below.
"""

# Retries every profile starts from, before its own adjustment.
BASE_RETRIES = 4

# Hard bounds on the number of retries a single policy may perform.
MIN_RETRIES = 1
MAX_RETRIES = 12

# Upper bound for a single backoff sleep, in milliseconds.
MAX_DELAY_MS = 8000

# Effective retry limits per profile (base + profile bonus):
#   default profile: 4 retries
#   batch profile: 6 retries
#   interactive profile: 2 retries
#   edge profile: 7 retries
