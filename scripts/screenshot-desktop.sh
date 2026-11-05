#!/bin/bash
# screenshot-desktop.sh: README screenshots (1920x1080, an app as a window on the Yggdrasil desktop) from a running Hyprland
# session, without touching it.
#
#   scripts/screenshot-desktop.sh start                 a nested Hyprland at 1920x1080 (the installed session's looks and window
#                                                       rules, no autostart, nothing written to your session) with the panel on a
#                                                       private D-Bus; prints its display
#   scripts/screenshot-desktop.sh run CMD [ARGS...]     a program on it (e.g. yggdrasil-settings power:fans)
#   scripts/screenshot-desktop.sh shot FILE.png         the nested screen, exactly 1920x1080
#   scripts/screenshot-desktop.sh show | hide           its window on your screen (to click in it) or back in the corner
#   scripts/screenshot-desktop.sh stop                  everything it started, private-bus helpers first (killing Hyprland first
#                                                       makes them report "Hyprland crashed" on your session)
#
# The nested window opens in a corner of your screen, mostly off it; it still draws. To click inside it, `show` moves it to
# 100,100 so a point (x, y) in the picture is (x + 100, y + 100) on your screen. Its pointer stays where it last was: park it in
# a corner before `shot` (an idle pointer is hidden after a second anyway).
set -eu
STATE=${XDG_RUNTIME_DIR:?}/yggdrasil-screenshot
SESSION=/usr/share/yggdrasil/hyprland/yggdrasil.lua
mkdir -p "$STATE"

nested_pid() {
  for p in $(pgrep -x Hyprland); do
    { tr '\0' ' ' < "/proc/$p/cmdline"; } 2>/dev/null | grep -q "$STATE/nested.lua" && echo "$p"
  done
}

case "${1:-}" in
start)
  [ -n "$(nested_pid)" ] && { echo "already running on $(cat "$STATE/display")"; exit 0; }
  # the session's helpers and looks (up to the monitor rule), a fixed screen, then hl.config and the float rule; no hooks that
  # remember window places, no autostart, no environment exported to systemd
  {
    awk '/^hl.monitor\(/{exit} {print}' "$SESSION"
    echo 'hl.monitor({ output = "", mode = "1920x1080@60", position = "0x0", scale = 1 })'
    awk '/^hl.config\(\{/{p=1} p{print} p && /^\}\)$/{exit}' "$SESSION"
    grep '"windows-float"' "$SESSION"
    # an idle pointer is hidden, so it is not in the picture
    echo 'hl.config({ cursor = { inactive_timeout = 1 } })'
  } > "$STATE/nested.lua"
  before=$(ls "$XDG_RUNTIME_DIR" | grep -E '^wayland-[0-9]+$' | sort)
  env -u ELECTRON_RUN_AS_NODE -u ELECTRON_NO_ATTACH_CONSOLE HYPRLAND_NO_CRASHREPORTER=1 \
    setsid Hyprland --config "$STATE/nested.lua" > "$STATE/hyprland.log" 2>&1 < /dev/null &
  for _ in $(seq 1 40); do
    new=$(comm -13 <(echo "$before") <(ls "$XDG_RUNTIME_DIR" | grep -E '^wayland-[0-9]+$' | sort) | head -1)
    [ -n "$new" ] && break
    sleep 0.25
  done
  [ -n "${new:-}" ] || { echo "the nested Hyprland did not start: see $STATE/hyprland.log"; exit 1; }
  echo "$new" > "$STATE/display"
  # one private D-Bus for everything shown on it (the panel owns names the real panel owns too)
  setsid dbus-run-session -- sh -c 'echo "$DBUS_SESSION_BUS_ADDRESS" > "$1/bus"; sleep infinity' _ "$STATE" > /dev/null 2>&1 < /dev/null &
  sleep 1
  "$0" run /usr/bin/yggdrasil-panel
  # Hyprland's "started without start-hyprland" banner goes away by itself after a few seconds
  sleep 16
  echo "nested desktop on $new"
  ;;
run)
  shift
  env -u ELECTRON_RUN_AS_NODE -u ELECTRON_NO_ATTACH_CONSOLE -u DISPLAY WAYLAND_DISPLAY="$(cat "$STATE/display")" \
    DBUS_SESSION_BUS_ADDRESS="$(cat "$STATE/bus")" XDG_CURRENT_DESKTOP=Yggdrasil XDG_SESSION_TYPE=wayland \
    setsid "$@" >> "$STATE/programs.log" 2>&1 < /dev/null &
  ;;
shot)
  WAYLAND_DISPLAY="$(cat "$STATE/display")" grim "${2:?a file name}"
  ;;
show | hide)
  where=$([ "$1" = show ] && echo "x = 100, y = 100" || echo "x = 3430, y = 1430")
  addr=$(hyprctl -j clients | python3 -c 'import json,sys; print(next((c["address"] for c in json.load(sys.stdin) if c["class"] == "aquamarine"), ""))')
  [ -n "$addr" ] && hyprctl dispatch "hl.dsp.window.move({ $where, window = 'address:$addr' })" > /dev/null
  ;;
stop)
  bus=$(cat "$STATE/bus" 2>/dev/null || true)
  if [ -n "$bus" ]; then
    for p in $(pgrep -u "$(id -u)"); do
      { tr '\0' '\n' < "/proc/$p/environ"; } 2>/dev/null | grep -qxF "DBUS_SESSION_BUS_ADDRESS=$bus" && kill "$p" 2>/dev/null || true
    done
    sleep 1
  fi
  for p in $(pgrep -x dbus-run-sessio); do   # the process name is cut at 15 characters
    tr '\0' ' ' < "/proc/$p/cmdline" | grep -q "$STATE" && kill "$p" 2>/dev/null || true
  done
  p=$(nested_pid); [ -n "$p" ] && kill "$p"
  rm -f "$STATE/bus" "$STATE/display"
  ;;
*)
  sed -n '2,19p' "$0"; exit 2
  ;;
esac
