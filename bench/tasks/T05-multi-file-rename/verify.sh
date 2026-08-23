#!/bin/sh
# T05-multi-file-rename: old name gone everywhere, behaviour unchanged.
set -eu

for f in lib.py app.py; do
    if [ ! -f "$f" ]; then
        echo "FAIL: $f is missing" >&2
        exit 1
    fi
done

# 1. The old name must be gone from every Python file in the tree.
hits=$(find . -name '*.py' -type f -exec grep -lF 'calc_total' {} + 2>/dev/null || true)
if [ -n "$hits" ]; then
    echo "FAIL: 'calc_total' still present in:" >&2
    echo "$hits" >&2
    exit 1
fi

# 2. The new name must actually be defined and used.
if ! grep -qF 'def compute_total(' lib.py; then
    echo "FAIL: lib.py does not define compute_total" >&2
    exit 1
fi
if ! grep -qF 'compute_total' app.py; then
    echo "FAIL: app.py does not use compute_total" >&2
    exit 1
fi

# 3. Behaviour must be identical.
expected=$(mktemp)
actual=$(mktemp)
trap 'rm -f "$expected" "$actual"' EXIT

cat > "$expected" <<'EOF'
units: 9
subtotal: 114.00
with tax: 125.40
avg unit: 12.67
empty subtotal: 0.00
EOF

if ! python3 app.py > "$actual" 2>/dev/null; then
    echo "FAIL: python3 app.py exited non-zero" >&2
    exit 1
fi

if ! cmp -s "$expected" "$actual"; then
    echo "FAIL: python3 app.py output differs" >&2
    echo "--- expected ---" >&2
    cat "$expected" >&2
    echo "--- actual ---" >&2
    cat "$actual" >&2
    exit 1
fi

exit 0
