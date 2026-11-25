#!/bin/bash
# The panel for screen readers: its accessibility tree read over AT-SPI the way Orca finds it, and a button pressed through it.
#
# The panel describes its bar, dock, notification pop-ups and kept notifications to screen readers (packages/panel/src/a11y.rs,
# AccessKit over AT-SPI). This runs it on Yggdrasil's compositor in its headless mode with an accessibility bus of its own
# (at-spi2-core's bus launcher and registry), switches accessibility on as a screen reader does, sends it a notification and
# reads the tree from the registry's desktop down (walk_atspi.py). It checks that the bar has the start button, the first
# desktop, the clock (which opens the calendar) and the notifications button; the dock its pinned app and the trash; the pop-up
# and the kept list the notification; every button the "click" action; and that a reader's click on the start button opens the
# start menu.
#
#   tests/a11y/panel_a11y.sh [path/to/yggdrasil-panel]
#       default panel: target/debug/yggdrasil-panel; Yggdrasil's compositor: $YGGDRASIL_COMPOSITOR, else
#       target/debug/yggdrasil-compositor (cargo build in packages/panel and packages/compositor first)
#
# It never touches the running desktop: every program starts from an empty environment with the test's own runtime folder,
# Wayland socket, home folder, session bus (no service activation) and accessibility bus, no system bus, GSettings in memory
# (never dconf), and an empty folder in place of /dev/input (YGGDRASIL_PANEL_INPUT: the panel never reads the computer's mice).
# Whatever is left running at the end (a bar add-on the panel started) is found by its runtime folder and ended.
# Needs dbus-daemon, busctl, at-spi2-core and python-dbus.
# Exit status: 0 passed, 1 failed, 2 something needed is missing.
set -u
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
panel=$(realpath -m "${1:-$repo/target/debug/yggdrasil-panel}")
comp=$(realpath -m "${YGGDRASIL_COMPOSITOR:-$repo/target/debug/yggdrasil-compositor}")
for b in "$panel" "$comp"; do [ -x "$b" ] || { echo "panel_a11y.sh: missing $b (cargo build it first)"; exit 2; }; done
libexec() { local d; for d in /usr/lib /usr/libexec /usr/lib/at-spi2-core; do [ -x "$d/$1" ] && { echo "$d/$1"; return; }; done; }
launcher=$(libexec at-spi-bus-launcher)
registry=$(libexec at-spi2-registryd)
[ -n "$launcher" ] && [ -n "$registry" ] || { echo "panel_a11y.sh: needs at-spi2-core (at-spi-bus-launcher, at-spi2-registryd)"; exit 2; }
for t in dbus-daemon busctl python3; do command -v "$t" > /dev/null || { echo "panel_a11y.sh: needs $t"; exit 2; }; done
python3 -I -c 'import dbus' 2> /dev/null || { echo "panel_a11y.sh: needs python-dbus"; exit 2; }

# short: a socket's path must fit in 108 bytes
W=$(mktemp -d "${TMPDIR:-/tmp}/ygg-a11y.XXXXXX")
mkdir -p "$W/run" "$W/home/.config/yggdrasil" "$W/home/.local/share/applications" "$W/input"
chmod 0700 "$W/run"
S=unix:path=$W/bus
# an array, not a function: a program started in the background with it has its own pid in $! (a function would run in a
# subshell of its own, and killing that subshell would leave the program running)
in_test=(env -i PATH=/usr/bin LANG=C.UTF-8 HOME="$W/home" XDG_RUNTIME_DIR="$W/run" DBUS_SESSION_BUS_ADDRESS="$S"
    DBUS_SYSTEM_BUS_ADDRESS="unix:path=$W/no-system-bus" GSETTINGS_BACKEND=memory)
bus() { "${in_test[@]}" busctl --address="$S" "$@"; }
# the test's processes still running: they carry its runtime folder
ours() {
    local p
    for p in /proc/[0-9]*; do grep -qzxF "XDG_RUNTIME_DIR=$W/run" "$p/environ" 2> /dev/null && echo "${p#/proc/}"; done
}
pids=()
cleanup() {
    local i left
    for ((i = ${#pids[@]} - 1; i >= 0; i--)); do kill "${pids[i]}" 2> /dev/null; wait "${pids[i]}" 2> /dev/null; done
    left=$(ours)
    if [ -n "$left" ]; then
        echo "note  ended what was left running: $(for p in $left; do cat "/proc/$p/comm"; done 2> /dev/null | tr '\n' ' ')"
        kill $left 2> /dev/null
        for _ in $(seq 20); do [ -z "$(ours)" ] && break; sleep 0.1; done
    fi
    rm -rf "$W"
}
trap cleanup EXIT

cat > "$W/bus.conf" << EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>$S</listen>
<policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>
EOF
"${in_test[@]}" dbus-daemon --config-file="$W/bus.conf" --nofork > /dev/null 2>&1 &
pids+=($!)
for _ in $(seq 50); do [ -S "$W/bus" ] && break; sleep 0.1; done
[ -S "$W/bus" ] || { echo "panel_a11y.sh: the session bus did not start"; exit 1; }

# the accessibility bus, as a desktop session has it: the launcher owns org.a11y.Bus (GetAddress, IsEnabled) and starts the
# bus itself (a plain dbus-daemon, its socket in the runtime folder); the registry is the desktop that screen readers start from
"${in_test[@]}" ATSPI_DBUS_IMPLEMENTATION=dbus-daemon "$launcher" --launch-immediately > "$W/launcher.log" 2>&1 &
pids+=($!)
for _ in $(seq 50); do bus status org.a11y.Bus > /dev/null 2>&1 && break; sleep 0.1; done
A=$(bus call org.a11y.Bus /org/a11y/bus org.a11y.Bus GetAddress 2> /dev/null | sed -n 's/^s "\(.*\)"$/\1/p')
case "$A" in
    "unix:path=$W/"*) ;;
    *) echo "panel_a11y.sh: no accessibility bus of the test's own (got '$A')"; cat "$W/launcher.log"; exit 1 ;;
esac
a11y() { "${in_test[@]}" busctl --address="$A" "$@"; }
"${in_test[@]}" AT_SPI_BUS_ADDRESS="$A" "$registry" > "$W/registry.log" 2>&1 &
pids+=($!)
for _ in $(seq 50); do a11y status org.a11y.atspi.Registry > /dev/null 2>&1 && break; sleep 0.1; done
a11y status org.a11y.atspi.Registry > /dev/null 2>&1 || { echo "panel_a11y.sh: the registry did not start"; cat "$W/registry.log"; exit 1; }

"${in_test[@]}" YGGDRASIL_COMPOSITOR_XWAYLAND=0 "$comp" --headless --size 1920x1080 > "$W/compositor.log" 2>&1 &
pids+=($!)
up=""
for _ in $(seq 100); do
    sock=$(sed -n 's/^WAYLAND_DISPLAY=//p' "$W/compositor.log" | head -1)
    ctl_sock=$(sed -n 's/^YGGDRASIL_COMPOSITOR_CTL=//p' "$W/compositor.log" | head -1)
    [ -n "$sock" ] && [ -S "$W/run/$sock" ] && [[ $ctl_sock == "$W/run/"* ]] && [ -S "$ctl_sock" ] && { up=1; break; }
    sleep 0.1
done
[ -n "$up" ] || { echo "panel_a11y.sh: the compositor did not start"; cat "$W/compositor.log"; exit 1; }
ctl() { "${in_test[@]}" "$comp" ctl --socket "$ctl_sock" "$@"; }

# the dock: one app pinned, the test's own
printf 'ygg-a11y-test\n' > "$W/home/.config/yggdrasil/dock"
printf '[Desktop Entry]\nType=Application\nName=Test App\nExec=true\n' > "$W/home/.local/share/applications/ygg-a11y-test.desktop"
"${in_test[@]}" WAYLAND_DISPLAY="$sock" YGGDRASIL_PANEL_INPUT="$W/input" XDG_CURRENT_DESKTOP=Yggdrasil "$panel" > "$W/panel.log" 2>&1 &
panel_pid=$!
pids+=($!)
up=""
for _ in $(seq 300); do
    ctl layers 2> /dev/null | awk -F'\t' '$2 == "yggdrasil-dock" && $5 != "0x0" { f = 1 } END { exit !f }' &&
        bus status org.freedesktop.Notifications > /dev/null 2>&1 && { up=1; break; }
    sleep 0.1
done
[ -n "$up" ] || { echo "panel_a11y.sh: the panel did not start"; tail -20 "$W/panel.log"; exit 1; }

# a screen reader starting switches accessibility on; the panel then joins the registry and sends its tree
bus set-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled b true
# critical, so its pop-up stays 14 s, time enough to read it; no sound
bus call org.freedesktop.Notifications /org/freedesktop/Notifications org.freedesktop.Notifications Notify 'susssasa{sv}i' \
    "A11y test" 0 "" "Hello" "A notification for the screen reader" 0 2 urgency y 2 suppress-sound b true 0 > /dev/null
# AccessKit names the application after the program's file
walk() { "${in_test[@]}" AT_SPI_BUS_ADDRESS="$A" python3 -I "$here/walk_atspi.py" "$(basename "$panel")" "$@"; }
for _ in $(seq 100); do
    walk > "$W/tree" 2> "$W/walk.log" && grep -q '^      button "Start menu"' "$W/tree" &&
        grep -q '^      button "Dismiss notification"' "$W/tree" && break
    sleep 0.2
done
echo "What a screen reader finds of the panel:"
cat "$W/tree"
[ -s "$W/walk.log" ] && tail -5 "$W/walk.log"

fails=0
check() { # check WHAT COMMAND...: "ok" when the command succeeds, else "FAIL"
    local what=$1
    shift
    if "$@"; then echo "ok    $what"; else echo "FAIL  $what"; fails=$((fails + 1)); fi
}
# under PART LINE: a node right under one part of the panel's window (PART: the part's line, four spaces in; LINE: an extended
# regular expression for the node's whole line)
under() { awk -v head="    $1" '$0 == head { on = 1; next } /^    [^ ]/ { on = 0 } on' "$W/tree" | grep -qxE -- "      $2"; }
check "the registry lists the panel, with its window" grep -qxF '  frame "Desktop"' "$W/tree"
check "the bar's start button" under 'tool bar "Top bar"' 'button "Start menu" actions=click'
check "the bar's first desktop" under 'tool bar "Top bar"' 'button "Desktop 1" actions=click'
check "the bar's clock, which opens the calendar" under 'tool bar "Top bar"' 'button "Clock, [^"]+" - Opens the calendar actions=click'
check "the bar's notifications button, one unread" under 'tool bar "Top bar"' 'button "Notifications, 1 unread" actions=click'
check "the dock's pinned app" under 'tool bar "Dock"' 'button "Test App" actions=click'
check "the dock's trash" under 'tool bar "Dock"' 'button "Trash" actions=click'
check "the pop-up's notification" under 'log "New notifications"' \
    'list item "A11y test: Hello" - A notification for the screen reader actions=click'
check "the pop-up's dismiss button" under 'log "New notifications"' 'button "Dismiss notification" actions=click'
check "the kept notification" under 'list "Kept notifications"' 'list item "A11y test: Hello" - A notification for the screen reader'
buttons=$(grep -c '^ *button ' "$W/tree")
unclickable=$(grep '^ *button ' "$W/tree" | grep -cv ' actions=click$')
clickable() { [ "$buttons" -gt 0 ] && [ "$unclickable" = 0 ]; }
check "every button can be clicked ($buttons buttons, $unclickable without the action)" clickable

# a reader's click is the same as one with the mouse: the start menu opens and takes the keyboard
walk --press "Start menu"
start_menu=$'layer\tyggdrasil-popup'
for _ in $(seq 50); do
    focus=$(ctl focus | cut -f1,2)
    [ "$focus" = "$start_menu" ] && break
    sleep 0.1
done
check "a reader's click on the start button opens the start menu (the keyboard: ${focus//$'\t'/ })" [ "$focus" = "$start_menu" ]

running() { kill -0 "$panel_pid" 2> /dev/null; }
check "the panel is still running" running
[ "$fails" = 0 ] && { echo "panel_a11y.sh: passed"; exit 0; }
echo "panel_a11y.sh: $fails failed; the panel's log:"
tail -20 "$W/panel.log"
exit 1
