#!/bin/sh
# Regenerate prompt.md of T20-review-bug: the exact prompt scripts/review.sh
# hands to mark2 when it reviews this task's change.
#
#   sh bench/tasks/T20-review-bug/regen.sh
#
# prompt.md was generated with prompts/review.md as of the grep files_only
# change (cksum 956005854 4031) and scripts/review.sh cksum 1909377913 12321. Rerun
# after any change to either file; then update these numbers.
#
# How: setup/ is the tree AFTER the change, the working tree `git diff HEAD`
# sees. The change itself is the patch below. The script copies setup/ into a
# scratch git repository, reverts the patch (git apply -R) and commits the
# result as HEAD, the "before" version, applies the patch again, checks that
# the working tree is byte-identical to setup/ and that `git diff HEAD` is the
# patch, and runs scripts/review.sh with no arguments (git diff HEAD) and a fake
# mark2 that only saves the prompt it gets on stdin. That saved prompt becomes
# prompt.md, so the hunk lines carry the same numbers the real wrapper adds.
#
# It also checks what verify.sh depends on: the prompt never names report.py,
# so a reply that names it found the broken caller by searching the
# repository, not by reading it off the diff.
set -eu

die() {
    printf 'regen: %s\n' "$*" >&2
    exit 1
}

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$here/../../.." && pwd)
review=$root/scripts/review.sh
[ -r "$review" ] || die "not found: $review"
[ -d "$here/setup" ] || die "not found: $here/setup"
if [ -n "$(find "$here/setup" -name __pycache__ -o -name '*.pyc' -o -name .DS_Store)" ]; then
    die "setup/ has build or OS junk (__pycache__, *.pyc, .DS_Store); remove it first"
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/mark2-regen.XXXXXX")
trap 'rm -rf "$work"' EXIT
case $work in
    *[[:space:]]*) die "temporary directory has whitespace in its path: $work" ;;
esac

# Isolate git from the user's config (diff algorithm, prefixes, hooks), and
# pin the --stat width, which git takes from COLUMNS.
GIT_CONFIG_NOSYSTEM=1
GIT_CONFIG_GLOBAL=/dev/null
COLUMNS=80
export GIT_CONFIG_NOSYSTEM GIT_CONFIG_GLOBAL COLUMNS
unset MARK2_REVIEW_MODEL MARK2_REVIEW_TEMPERATURE MARK2_REVIEW_BASE_URL \
    MARK2_REVIEW_MAX_DIFF_BYTES MARK2_REVIEW_TIMEOUT_SECS

# The change under review: page_bounds() switches from an inclusive
# (first, last) pair to a half-open (start, stop) range. Its docstring, its
# caller paginate() and its tests follow; report.py, also a caller, does not.
# An empty context line of a diff is a single space, which editors strip as
# trailing whitespace: sed puts it back (the patch has no truly empty line).
sed 's/^$/ /' >"$work/change.patch" <<'END_OF_PATCH'
diff --git a/lib/pager.py b/lib/pager.py
index 0805e66..155546f 100644
--- a/lib/pager.py
+++ b/lib/pager.py
@@ -33,21 +33,21 @@ def check_page(page, total, per_page=DEFAULT_PER_PAGE):


 def page_bounds(page, total, per_page=DEFAULT_PER_PAGE):
-    """Return ``(first, last)``, the indexes of the first and the last item
-    on ``page``; both ends are inclusive.
+    """Return ``(start, stop)``, the half-open index range of the items on
+    ``page``, ready for slicing: ``items[start:stop]``.

-    The only page of an empty ledger is empty: ``last == first - 1``.
+    The only page of an empty ledger is empty: ``start == stop``.
     """
     check_page(page, total, per_page)
-    first = (page - 1) * per_page
-    last = min(first + per_page, total) - 1
-    return first, last
+    start = (page - 1) * per_page
+    stop = min(start + per_page, total)
+    return start, stop


 def paginate(items, page, per_page=DEFAULT_PER_PAGE):
     """Return the items shown on ``page``."""
-    first, last = page_bounds(page, len(items), per_page)
-    return items[first:last + 1]
+    start, stop = page_bounds(page, len(items), per_page)
+    return items[start:stop]


 def clamp_page(page, total, per_page=DEFAULT_PER_PAGE):
diff --git a/tests/test_pager.py b/tests/test_pager.py
index ca13f73..c2e833c 100644
--- a/tests/test_pager.py
+++ b/tests/test_pager.py
@@ -42,14 +42,14 @@ class CheckPageTest(unittest.TestCase):

 class PageBoundsTest(unittest.TestCase):
     def test_full_page(self):
-        self.assertEqual(page_bounds(1, 45, 20), (0, 19))
-        self.assertEqual(page_bounds(2, 45, 20), (20, 39))
+        self.assertEqual(page_bounds(1, 45, 20), (0, 20))
+        self.assertEqual(page_bounds(2, 45, 20), (20, 40))

     def test_last_page_is_short(self):
-        self.assertEqual(page_bounds(3, 45, 20), (40, 44))
+        self.assertEqual(page_bounds(3, 45, 20), (40, 45))

     def test_empty_ledger(self):
-        self.assertEqual(page_bounds(1, 0, 20), (0, -1))
+        self.assertEqual(page_bounds(1, 0, 20), (0, 0))


 class PaginateTest(unittest.TestCase):
END_OF_PATCH

repo=$work/repo
mkdir "$repo"
cp -R "$here/setup/." "$repo/"
g() { git -C "$repo" -c user.name=bench -c user.email=bench@example.invalid "$@"; }
g init -q
g apply -R "$work/change.patch" || die "the patch does not revert cleanly from setup/"
g add -A
g commit -q -m before
g apply "$work/change.patch" || die "the patch does not apply to the reverted tree"
diff -r -x .git "$here/setup" "$repo" >/dev/null ||
    die "the working tree differs from setup/ after the patch"
g -c core.quotePath=false diff --no-color --no-ext-diff --src-prefix=a/ \
    --dst-prefix=b/ HEAD -- >"$work/actual.patch"
cmp -s "$work/change.patch" "$work/actual.patch" ||
    die "git diff HEAD is not the embedded patch (setup/ and the patch disagree)"

# Fake mark2: saves the prompt from stdin and approves, so review.sh exits 0.
fake=$work/fake-mark2
cat >"$fake" <<'EOF'
#!/bin/sh
cat >"$REGEN_PROMPT"
printf 'No issues found.\n\nVERDICT: approve\n'
EOF
chmod +x "$fake"
REGEN_PROMPT=$work/prompt
MARK2_REVIEW_BIN=$fake
export REGEN_PROMPT MARK2_REVIEW_BIN

if ! (cd "$repo" && sh "$review") >"$work/out" 2>"$work/err" </dev/null; then
    cat "$work/err" >&2
    die "scripts/review.sh failed"
fi
[ -s "$work/prompt" ] || die "the fake mark2 got no prompt"

if grep -q 'report\.py' "$work/prompt"; then
    die "the prompt names report.py: the broken caller would be visible from the diff"
fi

cp "$work/prompt" "$here/prompt.md"
printf 'regen: wrote %s (%s bytes); report.py is not in the prompt\n' \
    "$here/prompt.md" "$(wc -c <"$here/prompt.md" | tr -d ' ')"
printf 'regen: prompts/review.md cksum now: %s\n' "$(cksum <"$root/prompts/review.md")"
