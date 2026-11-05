#!/bin/sh
# Verify of the kanban pair T24-app-freeform / T25-app-planned: one app, two
# prompt styles (a plain paragraph vs a brief + plan + TODO + DoD). The checks
# must be the same for both, so they live once in bench/shared/kanban/ (three
# layers: A api, B ui in headless Chromium, C the model's own test_board.py;
# see the header of verify.sh there). This wrapper is byte-identical in both
# task directories (crates/bench task.rs tests assert it): only prompt.md
# differs. The task ID from the directory name only labels the verdicts.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
name=$(basename "$here")
exec sh "$here/../../shared/kanban/verify.sh" "${name%%-*}"
