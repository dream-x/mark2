#!/bin/sh
# Verify of the chat pair T26-chat-freeform / T27-chat-planned: T17's ws-chat
# contract asked for once as a plain paragraph (T26) and once as a brief +
# plan + TODO + Definition of Done (T27). The verdict must be the same
# function of the run directory for both and for T17 itself, so this wrapper
# runs T17's verify.sh, unchanged (three layers: A protocol, B ui in headless
# Chromium, C the model's own test_chat.py; see the header there). T17 is the
# frozen historical baseline: its verify finds checks/ and bench/uitest from
# its own path, not from the caller's, and its verdict lines keep the "T17"
# label, so one grep reads T17, T26 and T27 runs alike. This wrapper is
# byte-identical in both task directories (crates/bench task.rs tests assert
# it): only prompt.md differs.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
exec sh "$here/../T17-ws-chat/verify.sh"
