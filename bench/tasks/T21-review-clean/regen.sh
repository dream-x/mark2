#!/bin/sh
# Regenerate prompt.md of T21-review-clean: the exact prompt scripts/review.sh
# hands to mark2 when it reviews this task's change.
#
#   sh bench/tasks/T21-review-clean/regen.sh
#
# prompt.md was generated with prompts/review.md as of commit 1048370
# (cksum 2487656210 3924) and scripts/review.sh cksum 1909377913 12321. Rerun
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
# The "before" version is the same project as in T20-review-bug before its
# change; this change keeps every behavior.
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

# The change under review, behavior-preserving: a local variable renamed and
# the bounds check folded into one chained comparison in check_page(), a
# reworded docstring of page_bounds() (same inclusive contract), local
# variables renamed in one test.
# An empty context line of a diff is a single space, which editors strip as
# trailing whitespace: sed puts it back (the patch has no truly empty line).
sed 's/^$/ /' >"$work/change.patch" <<'END_OF_PATCH'
diff --git a/lib/pager.py b/lib/pager.py
index 0805e66..457c091 100644
--- a/lib/pager.py
+++ b/lib/pager.py
@@ -26,15 +26,15 @@ def page_count(total, per_page=DEFAULT_PER_PAGE):


 def check_page(page, total, per_page=DEFAULT_PER_PAGE):
-    """Raise PageError unless ``page`` exists for ``total`` items."""
-    count = page_count(total, per_page)
-    if page < 1 or page > count:
-        raise PageError("page %d is out of range 1..%d" % (page, count))
+    """Raise PageError unless ``page`` is one of the pages ``total`` items fill."""
+    pages = page_count(total, per_page)
+    if not 1 <= page <= pages:
+        raise PageError("page %d is out of range 1..%d" % (page, pages))


 def page_bounds(page, total, per_page=DEFAULT_PER_PAGE):
-    """Return ``(first, last)``, the indexes of the first and the last item
-    on ``page``; both ends are inclusive.
+    """Return ``(first, last)``: the index of the first item on ``page`` and
+    the index of its last item, both inclusive.

     The only page of an empty ledger is empty: ``last == first - 1``.
     """
diff --git a/tests/test_pager.py b/tests/test_pager.py
index ca13f73..d934171 100644
--- a/tests/test_pager.py
+++ b/tests/test_pager.py
@@ -54,10 +54,10 @@ class PageBoundsTest(unittest.TestCase):

 class PaginateTest(unittest.TestCase):
     def test_pages_cover_every_item_once(self):
-        items = list(range(45))
-        chunks = [paginate(items, page, 20) for page in (1, 2, 3)]
-        self.assertEqual([len(chunk) for chunk in chunks], [20, 20, 5])
-        self.assertEqual(sum(chunks, []), items)
+        ledger = list(range(45))
+        pages = [paginate(ledger, page, 20) for page in (1, 2, 3)]
+        self.assertEqual([len(rows) for rows in pages], [20, 20, 5])
+        self.assertEqual(sum(pages, []), ledger)

     def test_empty_ledger(self):
         self.assertEqual(paginate([], 1, 20), [])
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

cp "$work/prompt" "$here/prompt.md"
printf 'regen: wrote %s (%s bytes)\n' \
    "$here/prompt.md" "$(wc -c <"$here/prompt.md" | tr -d ' ')"
printf 'regen: prompts/review.md cksum now: %s\n' "$(cksum <"$root/prompts/review.md")"
