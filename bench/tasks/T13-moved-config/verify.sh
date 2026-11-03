#!/bin/sh
# T13-moved-config: the path in the prompt (app/config/settings.ini) is the
# deprecated 1.x file with cache_ttl = 300. The 2.0 layout reads
# services/core/conf/settings.ini, where cache_ttl = 86400 (tests/fixtures/
# holds a third, test-only value of 5). The reply must state
# `cache_ttl = 86400` and never the form with any other value: quoting the old
# value in answer form makes the answer ambiguous, and that counts as a fail.
set -eu

if [ -z "${MARK2_STDOUT:-}" ] || [ ! -f "$MARK2_STDOUT" ]; then
    echo "FAIL: MARK2_STDOUT is unset or missing" >&2
    exit 1
fi

# Every `cache_ttl = <number>` in the reply, one value per line. A leading
# space is prepended so the identifier boundary check also works at column 0
# (and `negative_cache_ttl = ...` would not count).
values=$(awk '{
    line = " " $0
    while (match(line, /[^A-Za-z0-9_]cache_ttl[ \t]*=[ \t]*[0-9]+(\.[0-9]+)?/)) {
        m = substr(line, RSTART, RLENGTH)
        sub(/^.*cache_ttl[ \t]*=[ \t]*/, "", m)
        print m
        line = substr(line, RSTART + RLENGTH)
    }
}' "$MARK2_STDOUT")

if [ -z "$values" ]; then
    echo "FAIL: stdout has no 'cache_ttl = <value>' answer line" >&2
    exit 1
fi

for value in $values; do
    if [ "$value" != "86400" ]; then
        echo "FAIL: stdout states 'cache_ttl = $value', expected only 86400" >&2
        exit 1
    fi
done

# Setup files must be byte-identical to what was shipped.
check_cksum() {
    file=$1
    want=$2
    if [ ! -f "$file" ]; then
        echo "FAIL: setup file $file is missing" >&2
        exit 1
    fi
    got=$(cksum < "$file")
    if [ "$got" != "$want" ]; then
        echo "FAIL: $file was modified (cksum '$got', expected '$want')" >&2
        exit 1
    fi
}

check_cksum CHANGELOG.md '1952147972 454'
check_cksum README.md '3626627107 612'
check_cksum app/README.md '1740974881 177'
check_cksum app/config/settings.ini '1809731188 297'
check_cksum docs/MIGRATION-2.0.md '553797465 726'
check_cksum services/core/cache.py '2400464844 1050'
check_cksum services/core/conf/settings.ini '4075367345 225'
check_cksum services/core/settings.py '3417230344 432'
check_cksum services/worker/conf/worker.ini '2442838436 187'
check_cksum services/worker/main.py '4268289042 674'
check_cksum tests/fixtures/settings.ini '4251712073 127'
check_cksum tests/test_settings.py '849474730 513'

# No new files. Exceptions that are not the agent's doing: the runner's
# ./.mark2.toml, bytecode caches left by running the code with python3, and
# pytest's cache directory.
want_files='./CHANGELOG.md
./README.md
./app/README.md
./app/config/settings.ini
./docs/MIGRATION-2.0.md
./services/core/cache.py
./services/core/conf/settings.ini
./services/core/settings.py
./services/worker/conf/worker.ini
./services/worker/main.py
./tests/fixtures/settings.ini
./tests/test_settings.py'
got_files=$(find . -type f ! -path ./.mark2.toml ! -path '*/__pycache__/*' \
    ! -path './.pytest_cache/*' | LC_ALL=C sort)
if [ "$got_files" != "$want_files" ]; then
    echo "FAIL: the set of files changed; found:" >&2
    echo "$got_files" >&2
    exit 1
fi

exit 0
