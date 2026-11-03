#!/bin/sh
# Builds bench/uitest/.venv: the Python environment of the project-tier verify
# scripts (T17 and later). aiohttp drives WebSocket clients and runs the
# task's own server; Playwright with headless Chromium drives the UI layer.
#
# Idempotent. A finished venv carries a stamp with the checksum of
# requirements.txt; a matching stamp means there is nothing to do. A venv
# without the stamp (an interrupted bootstrap) or with a stale one
# (requirements changed) is rebuilt from scratch.
#
# Needs network (PyPI, and the Playwright CDN if the cached browser revision
# does not match). This is the only networked step of the bench: verify
# itself talks to localhost only.
#
# Usage: sh bench/uitest/bootstrap.sh [--force]
# Base interpreter: $MARK2_UITEST_PYTHON, default `python3` from PATH.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
venv="$here/.venv"
stamp="$venv/.bootstrap-stamp"
want=$(cksum < "$here/requirements.txt")
base=${MARK2_UITEST_PYTHON:-python3}

if [ "${1:-}" != "--force" ] && [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$want" ]; then
    exit 0
fi

echo "bootstrap: building $venv with $base ($("$base" --version 2>&1))" >&2
rm -rf "$venv"
"$base" -m venv "$venv"
py="$venv/bin/python"

"$py" -m pip install --disable-pip-version-check --quiet -r "$here/requirements.txt"

# Reuses ~/Library/Caches/ms-playwright when the revision matches the pinned
# Playwright, downloads the browser otherwise.
"$py" -m playwright install chromium

# Smoke test: the browser really launches headless and renders a page.
"$py" -E -s - <<'EOF'
from playwright.sync_api import sync_playwright

with sync_playwright() as p:
    browser = p.chromium.launch(headless=True)
    page = browser.new_page()
    page.set_content("<p id='x'>ok</p>")
    assert page.inner_text("#x") == "ok"
    browser.close()
EOF

printf '%s\n' "$want" > "$stamp"
echo "bootstrap: ready ($venv)" >&2
