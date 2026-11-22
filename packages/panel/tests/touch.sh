#!/bin/bash
# The panel with a finger (YGG-79): every tap after a popover closed under the finger still counts, a popover goes when it is
# closed, and a long press on a dock app lists its windows without also tapping the app.
#
# Found on the laptop (2026-11-19 and 20): a tap outside a popover closes it while the finger is down, the compositor then sends
# no `up` for the gone surface, and the panel took the finger as still down and ignored the next touch (the dock "ignored
# clicks"); a long press on a dock app minimized it as its window list opened.
#
#   packages/panel/tests/touch.sh [path/to/yggdrasil-panel]
#       default panel: target/debug/yggdrasil-panel; Yggdrasil's compositor: $YGGDRASIL_COMPOSITOR, else
#       target/debug/yggdrasil-compositor
#   TOUCH_APP=/usr/bin/yggdrasil-calculator TOUCH_APP_X=1121 ... also the dock checks with that program's window (X: its icon)
#
# Runs in Yggdrasil's compositor in its headless mode with its own runtime folder, Wayland socket, session bus and home folder;
# WAYLAND_DISPLAY, DISPLAY and the rest are cleared first, so it never touches the running desktop. Needs dbus-daemon.
# Exit status: 0 passed, 1 failed, 2 something needed is missing.
set -u
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
panel=$(realpath -m "${1:-$repo/target/debug/yggdrasil-panel}")
comp=$(realpath -m "${YGGDRASIL_COMPOSITOR:-$repo/target/debug/yggdrasil-compositor}")
for b in "$panel" "$comp"; do [ -x "$b" ] || { echo "touch.sh: missing $b (cargo build it first)"; exit 2; }; done
command -v dbus-daemon > /dev/null || { echo "touch.sh: needs dbus-daemon"; exit 2; }

W=$(mktemp -d "${TMPDIR:-/tmp}/ygg-touch.XXXXXX")
mkdir -p "$W/run" "$W/home"
chmod 0700 "$W/run"
export HOME=$W/home XDG_RUNTIME_DIR=$W/run XDG_CONFIG_HOME=$W/home/.config
export LIBGL_ALWAYS_SOFTWARE=1 NO_AT_BRIDGE=1 XDG_CURRENT_DESKTOP=Yggdrasil XDG_SESSION_TYPE=wayland
unset WAYLAND_DISPLAY DISPLAY HYPRLAND_INSTANCE_SIGNATURE DBUS_SESSION_BUS_ADDRESS
export DBUS_SYSTEM_BUS_ADDRESS=unix:path=$W/no-system-bus
cat > "$W/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path=$W/bus</listen>
<policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>
EOF
dbus-daemon --config-file="$W/bus.conf" --nofork > /dev/null 2>&1 &
pids=($!)
trap 'kill "${pids[@]}" 2>/dev/null; wait 2>/dev/null; rm -rf "$W"' EXIT
for _ in $(seq 50); do [ -S "$W/bus" ] && break; sleep 0.1; done
export DBUS_SESSION_BUS_ADDRESS=unix:path=$W/bus

ctl() { "$comp" ctl "$@"; }
"$comp" --headless --size 1920x1080 > "$W/compositor.log" 2>&1 &
pids+=($!)
for _ in $(seq 100); do ctl outputs > /dev/null 2>&1 && break; sleep 0.1; done
for s in "$W"/run/wayland-*; do [[ $s =~ /wayland-[0-9]+$ ]] && export WAYLAND_DISPLAY=${s##*/}; done
[ -n "${WAYLAND_DISPLAY:-}" ] || { echo "touch.sh: the compositor did not start"; cat "$W/compositor.log"; exit 1; }
"$panel" > "$W/panel.log" 2>&1 &
pids+=($!)
for _ in $(seq 300); do ctl layers 2>/dev/null | grep -q yggdrasil-dock && break; sleep 0.1; done
ctl layers | grep -q yggdrasil-dock || { echo "touch.sh: the panel did not start"; tail -20 "$W/panel.log"; exit 1; }

fails=0
popup() { ctl layers | grep -q yggdrasil-popup && echo open || echo closed; }
expect() { # expect open|closed WHAT
    sleep 0.8
    local got
    got=$(popup)
    if [ "$got" = "$1" ]; then echo "ok    $2: popover $got"; else echo "FAIL  $2: popover $got, expected $1"; fails=$((fails + 1)); fi
}
tap() { ctl touch-down 0 "$1" "$2" > /dev/null; sleep 0.12; ctl touch-up 0 > /dev/null; }
hold() { ctl touch-down 0 "$1" "$2" > /dev/null; sleep 1.6; ctl touch-up 0 > /dev/null; }
pull() { ctl touch-down 0 960 10 > /dev/null; sleep 0.1; ctl touch-move 0 960 150 > /dev/null; sleep 0.1
    ctl touch-move 0 960 400 > /dev/null; sleep 0.1; ctl touch-up 0 > /dev/null; }

# the seat gets its touch screen with the first touch, and the panel starts listening a moment later: two on the empty
# desktop first
tap 300 600; sleep 1; tap 300 600; sleep 1
tap 22 14; expect open "a tap on the start button"
tap 1500 600; expect closed "a tap away (right of the start menu)"
tap 22 14; expect open "the next tap on the start button (the one that was ignored)"
tap 1500 600; expect closed "a tap away"
pull; expect open "pulling the notch down"
tap 300 600; expect closed "a tap away"
tap 960 12; expect open "a tap on the notch"
tap 300 600; expect closed "a tap away"

if [ -n "${TOUCH_APP:-}" ]; then
    "$TOUCH_APP" > /dev/null 2>&1 &
    pids+=($!)
    sleep 3
    x=${TOUCH_APP_X:?TOUCH_APP_X: where its dock icon is} y=${TOUCH_APP_Y:-1040}
    hold "$x" "$y"; expect open "a long press on its dock icon"
    # `ctl clients`: tab-separated, the window's states (maximized, minimized, ...) in the ninth column
    if [ "$(ctl clients | cut -f9 | grep -c minimized)" = 0 ]; then echo "ok    the long press left the app as it was"; else
        echo "FAIL  the long press also minimized the app"; fails=$((fails + 1)); fi
    tap 300 300; expect closed "a tap away"
    hold "$x" "$y"; expect open "the next long press"
    tap 300 300; expect closed "a tap away"
fi

if ! kill -0 "${pids[2]}" 2>/dev/null; then echo "FAIL  the panel died"; tail -20 "$W/panel.log"; exit 1; fi
if grep -q 'YGG-79' "$W/panel.log"; then echo "note  the panel removed a popup surface left with nothing to show:"; grep 'YGG-79' "$W/panel.log"; fi
[ "$fails" = 0 ] && { echo "touch.sh: passed"; exit 0; }
echo "touch.sh: $fails failed"
exit 1
