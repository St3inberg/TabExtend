#!/bin/sh
# Run TabExtend's real-desktop test inside a headless desktop container.
# Usage: tests/desktops/run.sh [sway]   (default: every desktop)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
test_name=real_desktop_streams_takes_input_and_cleans_up

binary=$(cd "$root" && cargo test --release -p tabextend-host --test end_to_end --no-run \
    --message-format=json 2>/dev/null \
    | sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -1)
[ -n "$binary" ] || { echo "could not build the test binary" >&2; exit 1; }

status=0
for desktop in ${1:-sway}; do
    echo "== $desktop"
    podman build -q -t "tabextend-$desktop" "$here/$desktop" >/dev/null
    if podman run --rm \
        -v "$binary:/tabextend-test:ro,Z" \
        -v "$here/$desktop/session.sh:/session.sh:ro,Z" \
        "tabextend-$desktop" sh /session.sh \
        /tabextend-test --ignored --exact "$test_name" --nocapture --test-threads=1; then
        echo "== $desktop: PASS"
    else
        echo "== $desktop: FAIL"
        status=1
    fi
done
exit $status
