#!/bin/sh
# T08-write-run-script: `python3 fizzbuzz.py` must print the reference output.
set -eu

if [ ! -f fizzbuzz.py ]; then
    echo "FAIL: fizzbuzz.py does not exist" >&2
    exit 1
fi

expected=$(mktemp)
actual=$(mktemp)
trap 'rm -f "$expected" "$actual"' EXIT

cat > "$expected" <<'EOF'
1
2
Fizz
4
Buzz
Fizz
7
8
Fizz
Buzz
11
Fizz
13
14
FizzBuzz
EOF

if ! python3 fizzbuzz.py > "$actual" 2>/dev/null; then
    echo "FAIL: python3 fizzbuzz.py exited non-zero" >&2
    exit 1
fi

if ! cmp -s "$expected" "$actual"; then
    echo "FAIL: fizzbuzz.py output differs" >&2
    echo "--- expected ---" >&2
    cat "$expected" >&2
    echo "--- actual ---" >&2
    cat "$actual" >&2
    exit 1
fi

exit 0
