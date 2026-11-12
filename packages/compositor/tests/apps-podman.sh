#!/usr/bin/env bash
# The compositor's headless tests with Yggdrasil's own programs (tests/apps.rs) and the protocol tests (tests/behaviour.rs), run in
# a rootless podman container: no devices (no GPU, no input), no network, no system bus, nothing of the running desktop. The way
# to run them on a desktop machine; CI runs the same tests in its own container (the compositor-headless job).
#
#   packages/compositor/tests/apps-podman.sh [cargo test filters...]
#
# Builds what it needs first (debug): the compositor and its tests, the panel, Settings, and the lock screen with --features
# test-auth in a folder of its own (target/test-auth), so the normal build of the lock is never the test one.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
target=$repo/target
image=localhost/yggdrasil-headless-test

command -v podman >/dev/null || { echo "apps-podman: podman is not installed" >&2; exit 2; }

(cd "$repo/packages/panel" && cargo build --locked)
(cd "$repo/packages/settings" && cargo build --locked)
(cd "$repo/packages/lock" && cargo build --locked --features test-auth --target-dir "$target/test-auth" --bin yggdrasil-lock)
cd "$repo/packages/compositor"
cargo build --locked
tests=$(cargo test --locked --no-run --test apps --test behaviour --message-format=json 2>/dev/null |
    grep -o '"executable":"[^"]*/deps/[^"]*"' | sed 's/"executable":"//; s/"$//')
[ -n "$tests" ] || { echo "apps-podman: the test programs were not built" >&2; exit 2; }

if ! podman image exists "$image"; then
    echo "apps-podman: making the container image $image (Arch with the libraries the programs load, about 1 GB) ..."
    podman rm -f ygg-headless-test-build >/dev/null 2>&1 || true
    podman run --name ygg-headless-test-build docker.io/library/archlinux:base \
        sh -c 'pacman -Syu --noconfirm --needed pixman wayland libxkbcommon fontconfig ttf-dejavu dbus mesa libglvnd grim >/dev/null && useradd -m ci' &&
        podman commit ygg-headless-test-build "$image" >/dev/null
    podman rm -f ygg-headless-test-build >/dev/null 2>&1 || true
    podman image exists "$image" || { echo "apps-podman: could not make the container image" >&2; exit 2; }
fi

status=0
for t in $tests; do
    rel=${t#"$target"/}
    echo "apps-podman: $(basename "$t")"
    podman run --rm --network=none --user ci -v "$target:/target:ro" \
        -e YGGDRASIL_COMPOSITOR=/target/debug/yggdrasil-compositor \
        -e YGGDRASIL_TEST_PANEL=/target/debug/yggdrasil-panel \
        -e YGGDRASIL_TEST_SETTINGS=/target/debug/yggdrasil-settings \
        -e YGGDRASIL_TEST_LOCK=/target/test-auth/debug/yggdrasil-lock \
        -e YGGDRASIL_TEST_APPS_REQUIRED=1 \
        "$image" "/target/$rel" --test-threads=2 "$@" || status=1
done
exit $status
