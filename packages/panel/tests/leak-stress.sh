#!/bin/bash
# The panel must not leave its pictures behind in the compositor.
#
# Every picture the panel shows is a wl_buffer in a memfd called "yggdrasil-panel"; the compositor maps each one while the buffer
# lives. Dropping a buffer without destroying it (found 2026-11-09: 819 of them, about 4 GB, kept by Hyprland after a day and a
# half) leaves the mapping in the compositor. This runs the panel in a throw-away headless compositor (wlr-layer-shell, no screen,
# no GPU), makes it build new pictures many times over (notification pop-ups, the calendar, the shortcuts card, the screen changing
# size), and counts the panel's memfd mappings in the compositor's /proc/PID/smaps. It fails when they grew past a small bound
# after the warm-up.
#
# Two compositors can host it: headless sway (the default), and Yggdrasil's own compositor in its headless mode
# (packages/compositor, `--headless`), which is to replace Hyprland one day. The test is the same on both.
#
#   packages/panel/tests/leak-stress.sh [--compositor sway|yggdrasil] [path/to/yggdrasil-panel]
#                          default panel: target/debug/yggdrasil-panel, then release; Yggdrasil's compositor is
#                          $YGGDRASIL_COMPOSITOR, else target/debug/yggdrasil-compositor, then release
#   packages/panel/tests/leak-stress.sh [--compositor ...] --podman [path/to/yggdrasil-panel]
#                          the same in a rootless podman container (Arch with sway; made on first use, which needs the network),
#                          for a machine without sway. The container gets no network, no display and no runtime folder of yours.
#   LEAK_CYCLES=40 ...     more rounds (default 20)
#
# Needs busctl (systemd), dbus-run-session (dbus) and a sans-serif font, and sway for the sway variant. It never touches the
# running desktop: it has its own runtime folder, Wayland socket, session bus and home folder, and it clears WAYLAND_DISPLAY,
# DISPLAY and the rest before anything starts. sway refuses to run as root; run it as a normal user (CI uses an account of its own).
# Exit status: 0 no leak, 1 a leak (or the panel died), 2 something needed is missing.
set -u

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)

compositor=${LEAK_COMPOSITOR:-sway}
if [ "${1:-}" = "--compositor" ]; then
    compositor=${2:-}
    shift 2
fi
case "$compositor" in
    sway | yggdrasil) ;;
    *) echo "leak-stress: --compositor is sway or yggdrasil, not '$compositor'" >&2; exit 2 ;;
esac

find_bin() {
    local p
    for p in "$repo/target/debug/$1" "$repo/target/release/$1"; do
        [ -x "$p" ] && { echo "$p"; return; }
    done
}
find_compositor() {
    if [ -n "${YGGDRASIL_COMPOSITOR:-}" ]; then echo "$YGGDRASIL_COMPOSITOR"; else find_bin yggdrasil-compositor; fi
}

if [ "${1:-}" = "--podman" ]; then
    panel=$(readlink -f "${2:-$(find_bin yggdrasil-panel)}")
    [ -x "$panel" ] || { echo "leak-stress: no panel binary (build it: cd packages/panel && cargo build)" >&2; exit 2; }
    command -v podman >/dev/null || { echo "leak-stress: podman is not installed" >&2; exit 2; }
    image=localhost/yggdrasil-leak-stress
    if ! podman image exists "$image"; then
        echo "leak-stress: making the container image $image (Arch with sway, about 1 GB) ..."
        podman rm -f ygg-leak-stress-build >/dev/null 2>&1
        podman run --name ygg-leak-stress-build docker.io/library/archlinux:base \
            sh -c 'pacman -Syu --noconfirm --needed sway ttf-dejavu fontconfig libxkbcommon >/dev/null && useradd -m ci' \
            && podman commit ygg-leak-stress-build "$image" >/dev/null
        podman rm -f ygg-leak-stress-build >/dev/null 2>&1
        podman image exists "$image" || { echo "leak-stress: could not make the container image" >&2; exit 2; }
    fi
    extra=()
    if [ "$compositor" = yggdrasil ]; then
        comp=$(readlink -f "$(find_compositor)")
        [ -x "$comp" ] || { echo "leak-stress: no compositor binary (build it: cd packages/compositor && cargo build)" >&2; exit 2; }
        extra=(-v "$(dirname "$comp"):/compositor:ro" -e YGGDRASIL_COMPOSITOR="/compositor/$(basename "$comp")")
    fi
    exec podman run --rm --network=none --user ci -e LEAK_CYCLES="${LEAK_CYCLES:-20}" -e LEAK_COMPOSITOR="$compositor" "${extra[@]}" \
        -v "$here:/leak-stress:ro" -v "$(dirname "$panel"):/panel:ro" "$image" /leak-stress/leak-stress.sh "/panel/$(basename "$panel")"
fi

if [ -z "${LEAK_STRESS_INNER:-}" ]; then
    panel=${1:-$(find_bin yggdrasil-panel)}
    [ -n "$panel" ] && [ -x "$panel" ] || { echo "leak-stress: no panel binary (build it: cd packages/panel && cargo build)" >&2; exit 2; }
    tools="busctl dbus-run-session fc-match"
    comp=""
    if [ "$compositor" = sway ]; then
        tools="sway swaymsg $tools"
        if [ "$(id -u)" = 0 ]; then
            echo "leak-stress: sway does not run as root; run this as a normal user" >&2
            exit 2
        fi
    else
        comp=$(find_compositor)
        [ -n "$comp" ] && [ -x "$comp" ] || { echo "leak-stress: no compositor binary (build it: cd packages/compositor && cargo build)" >&2; exit 2; }
        comp=$(readlink -f "$comp")
    fi
    for tool in $tools; do
        command -v "$tool" >/dev/null || { echo "leak-stress: $tool is not installed" >&2; exit 2; }
    done
    # short: a Wayland socket's path must fit in 108 bytes
    work=$(mktemp -d "${TMPDIR:-/tmp}/ygg-leak.XXXXXX")
    mkdir -p "$work/run" "$work/home"
    chmod 700 "$work/run"
    # nothing of the running session comes along: a private runtime folder, home and session bus (dbus-run-session)
    exec env -u WAYLAND_DISPLAY -u DISPLAY -u SWAYSOCK -u I3SOCK -u HYPRLAND_INSTANCE_SIGNATURE -u DBUS_SESSION_BUS_ADDRESS \
        -u XDG_CURRENT_DESKTOP -u XDG_SESSION_DESKTOP -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME \
        -u WAYLAND_SOCKET -u NIRI_SOCKET -u YGGDRASIL_COMPOSITOR_CTL \
        XDG_RUNTIME_DIR="$work/run" HOME="$work/home" LEAK_STRESS_INNER="$work" LEAK_STRESS_PANEL="$(readlink -f "$panel")" \
        LEAK_COMPOSITOR="$compositor" LEAK_STRESS_COMPOSITOR="$comp" \
        dbus-run-session -- "$0"
fi

work=$LEAK_STRESS_INNER
panel=$LEAK_STRESS_PANEL
cycles=${LEAK_CYCLES:-20}
cycles=$(((cycles + 1) / 2 * 2)) # even: the screen ends at the size it had after the warm-up
# after the warm-up the count may wobble by a few (a surface holds one to four buffers, depending on what the compositor has let
# go of yet); a leak adds at least one per pop-up, per toast and per surface at each size change, so dozens over the rounds
slack_count=${LEAK_SLACK_COUNT:-8}
slack_kb=${LEAK_SLACK_KB:-16384}
comp_pid=""
panel_pid=""

cleanup() {
    [ -n "$panel_pid" ] && kill "$panel_pid" 2>/dev/null
    [ -n "$comp_pid" ] && kill "$comp_pid" 2>/dev/null
    wait 2>/dev/null
    [ -z "${LEAK_KEEP:-}" ] && rm -rf "$work"
}
trap cleanup EXIT

case "$XDG_RUNTIME_DIR" in
    "$work"/*) ;;
    *) echo "leak-stress: refusing to start: the runtime folder is not the private one" >&2; exit 2 ;;
esac

if [ "$LEAK_COMPOSITOR" = sway ]; then
    # sway in Arch carries a file capability (cap_sys_nice) that a container without that capability refuses to run; a plain copy runs
    cp "$(command -v sway)" "$work/sway"
    cat > "$work/sway.cfg" <<'EOF'
xwayland disable
output HEADLESS-1 mode --custom 1600x900
EOF
    WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1 "$work/sway" -c "$work/sway.cfg" >"$work/compositor.log" 2>&1 &
    comp_pid=$!
    for _ in $(seq 100); do
        sock=$(ls "$XDG_RUNTIME_DIR" 2>/dev/null | grep -E '^wayland-[0-9]+$' | head -1)
        ipc=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*.sock 2>/dev/null | head -1)
        [ -n "$sock" ] && [ -n "$ipc" ] && break
        sleep 0.1
    done
    [ -n "${sock:-}" ] && [ -n "${ipc:-}" ] && export SWAYSOCK=$ipc
    set_mode() { swaymsg -- output HEADLESS-1 mode --custom "$1" >/dev/null; }
else
    # Yggdrasil's own compositor, headless: one 1600x900 output drawn in memory, sizes changed through its control socket
    YGGDRASIL_COMPOSITOR_XWAYLAND=0 "$LEAK_STRESS_COMPOSITOR" --headless --size 1600x900 >"$work/compositor.log" 2>&1 &
    comp_pid=$!
    ctl=""
    for _ in $(seq 100); do
        sock=$(sed -n 's/^WAYLAND_DISPLAY=//p' "$work/compositor.log" 2>/dev/null | head -1)
        ctl=$(sed -n 's/^YGGDRASIL_COMPOSITOR_CTL=//p' "$work/compositor.log" 2>/dev/null | head -1)
        [ -n "$sock" ] && [ -n "$ctl" ] && [ -S "$ctl" ] && break
        sleep 0.1
    done
    ipc=$ctl
    set_mode() { "$LEAK_STRESS_COMPOSITOR" ctl --socket "$ctl" resize HEADLESS-1 "$1" >/dev/null; }
fi
if [ -z "${sock:-}" ] || [ -z "${ipc:-}" ]; then
    echo "leak-stress: the compositor ($LEAK_COMPOSITOR) did not start" >&2
    cat "$work/compositor.log" >&2
    exit 2
fi
export WAYLAND_DISPLAY=$sock
echo "leak-stress: on $LEAK_COMPOSITOR"

# The panel's memfd mappings in the compositor: "count kB".
mappings() {
    awk '/^[0-9a-f]+-[0-9a-f]+ / { mine = ($0 ~ /memfd:yggdrasil-panel/); if (mine) n++ } mine && $1 == "Size:" { kb += $2 } END { printf "%d %d\n", n, kb }' "/proc/$comp_pid/smaps"
}

YGGDRASIL_A11Y=off "$panel" >"$work/panel.log" 2>&1 &
panel_pid=$!

notify() {
    busctl --user call org.freedesktop.Notifications /org/freedesktop/Notifications org.freedesktop.Notifications Notify \
        'susssasa{sv}i' leak-stress 0 "" "Round $1" "A pop-up that comes and goes" 0 0 5000 2>/dev/null | awk '{print $2}'
}
close_notif() {
    busctl --user call org.freedesktop.Notifications /org/freedesktop/Notifications org.freedesktop.Notifications CloseNotification u "$1" >/dev/null 2>&1
}
panel_call() {
    busctl --user call org.yggdrasil.Panel /Panel org.yggdrasil.Panel "$1" >/dev/null 2>&1
}
fail() {
    echo "leak-stress: FAIL: $*" >&2
    echo "--- panel log" >&2
    tail -40 "$work/panel.log" >&2
    echo "--- compositor log" >&2
    tail -20 "$work/compositor.log" >&2
    exit 1
}

# up: the panel serves notifications and its own bus name, and the bar is on the screen
ready=""
for _ in $(seq 300); do
    kill -0 "$panel_pid" 2>/dev/null || fail "the panel ended at start-up"
    kill -0 "$comp_pid" 2>/dev/null || fail "the compositor ended at start-up"
    if busctl --user status org.freedesktop.Notifications >/dev/null 2>&1 && busctl --user status org.yggdrasil.Panel >/dev/null 2>&1; then
        read -r n _ < <(mappings)
        [ "$n" -gt 0 ] && { ready=1; break; }
    fi
    sleep 0.1
done
[ -n "$ready" ] || fail "the panel did not come up within 30 s"

modes=(1280x720 1600x900)
round() {
    local i=$1 id
    id=$(notify "$i")
    sleep 0.4
    [ -n "$id" ] && close_notif "$id"
    sleep 0.3
    panel_call ToggleCalendar
    sleep 0.4
    panel_call ToggleCalendar
    sleep 0.3
    panel_call ToggleShortcuts
    sleep 0.4
    panel_call ToggleShortcuts
    sleep 0.3
    set_mode "${modes[$((i % 2))]}"
    sleep 0.6
}

# warm-up: every kind of surface made once, the screen back at its first size, then a moment to settle
for i in 1 2; do round "$i"; done
sleep 1.5
read -r base_n base_kb < <(mappings)
echo "leak-stress: after the warm-up: $base_n mappings, $base_kb kB"

for i in $(seq 3 $((cycles + 2))); do
    round "$i"
    kill -0 "$panel_pid" 2>/dev/null || fail "the panel ended during round $i"
    kill -0 "$comp_pid" 2>/dev/null || fail "the compositor ended during round $i"
    if [ $((i % 5)) = 0 ]; then
        read -r n kb < <(mappings)
        echo "leak-stress: round $i: $n mappings, $kb kB"
    fi
done
sleep 1.5
read -r end_n end_kb < <(mappings)
echo "leak-stress: after $cycles rounds: $end_n mappings, $end_kb kB (allowed: $((base_n + slack_count)) mappings, $((base_kb + slack_kb)) kB)"
[ "$end_n" -le $((base_n + slack_count)) ] || fail "the compositor holds $((end_n - base_n)) more of the panel's buffers than after the warm-up"
[ "$end_kb" -le $((base_kb + slack_kb)) ] || fail "the compositor maps $(((end_kb - base_kb) / 1024)) MB more of the panel's memory than after the warm-up"
echo "leak-stress: PASS"
