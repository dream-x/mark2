"""T17 layer C: the model's own automated test, `python3 test_chat.py`.

Measures self-verification discipline: the test must exist, be
self-contained (start and stop server.py itself) and pass against the
model's own code. A trivial test passes too; layers A and B stay the ground
truth, this layer only shows that the shipped test does not lie.
"""

import subprocess
import sys
import tempfile
from pathlib import Path

from common import Failure, kill_group, run_layer, tail

LABEL = "layer C own test_chat.py"
TEST_TIMEOUT = 60.0


async def body():
    if not Path("test_chat.py").is_file():
        raise Failure("test_chat.py is missing: the task requires a self-contained test")
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen(
            [sys.executable, "test_chat.py"],
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            code = proc.wait(timeout=TEST_TIMEOUT)
        except subprocess.TimeoutExpired:
            code = None
        finally:
            # Also reaps a server the test left behind in its process group.
            kill_group(proc)
        log.seek(0)
        output = tail(log.read())
    if code is None:
        raise Failure(
            f"`python3 test_chat.py` did not finish within {TEST_TIMEOUT:.0f} s "
            f"(does it stop its server?); its output:\n{output}"
        )
    if code != 0:
        raise Failure(f"`python3 test_chat.py` exited with code {code}; its output:\n{output}")


if __name__ == "__main__":
    sys.exit(run_layer(LABEL, body, deadline=TEST_TIMEOUT + 15))
