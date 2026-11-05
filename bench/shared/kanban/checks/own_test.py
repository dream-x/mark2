"""Kanban layer C: the model's own automated test, `python3 test_board.py`.

Two runs, each with the 60 s budget both prompts give:

1. In the run directory: the test must exit 0 against the model's own code.
2. In a scratch copy of the run directory where server.py is a stub that
   exits at once: the test must exit non-zero. Both prompts say the test
   starts server.py itself and exits 0 only if everything works, so a test
   that stays green while the server cannot even start checks nothing
   (`sys.exit(0)` passes run 1). This is the only judgement of the test's
   quality; how thorough it is beyond that is not measured, layers A and B
   stay the ground truth for the app itself.
"""

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from common import Failure, kill_group, run_layer, tail

LABEL = "layer C own test_board.py"
TEST_FILE = "test_board.py"
TEST_TIMEOUT = 60.0
DEAD_SERVER = (
    "import sys\n"
    "sys.stderr.write('server.py replaced by the kanban verify: this server never starts\\n')\n"
    "sys.exit(3)\n"
)
# Not copied into the scratch run: environments and caches, not the project.
SKIP = shutil.ignore_patterns("__pycache__", ".venv", "venv", "node_modules", ".git")


def run_test(cwd):
    """(exit code or None on timeout, output tail) of `python3 test_board.py`."""
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen(
            [sys.executable, TEST_FILE],
            cwd=cwd,
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
        return code, tail(log.read())


async def body():
    if not Path(TEST_FILE).is_file():
        raise Failure(f"{TEST_FILE} is missing: the task requires a self-contained test")

    code, output = run_test(".")
    if code is None:
        raise Failure(
            f"`python3 {TEST_FILE}` did not finish within {TEST_TIMEOUT:.0f} s "
            f"(does it stop its server?); its output:\n{output}"
        )
    if code != 0:
        raise Failure(f"`python3 {TEST_FILE}` exited with code {code}; its output:\n{output}")

    with tempfile.TemporaryDirectory(prefix="kanban-dead-server-") as tmp:
        copy = Path(tmp) / "run"
        shutil.copytree(".", copy, ignore=SKIP, symlinks=True)
        (copy / "server.py").write_text(DEAD_SERVER, encoding="utf-8")
        code, output = run_test(copy)
    if code is None:
        raise Failure(
            f"with server.py replaced by a stub that exits at once, `python3 {TEST_FILE}` did "
            f"not finish within {TEST_TIMEOUT:.0f} s (it must notice the server never came up); "
            f"its output:\n{output}"
        )
    if code == 0:
        raise Failure(
            f"with server.py replaced by a stub that exits at once, `python3 {TEST_FILE}` "
            f"still exited 0: the test does not really start and check the server; "
            f"its output:\n{output}"
        )


if __name__ == "__main__":
    sys.exit(run_layer(LABEL, body, deadline=2 * TEST_TIMEOUT + 20))
