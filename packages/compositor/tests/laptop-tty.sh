#!/bin/sh
# The hardware backend on the laptop itself, for trying the laptop milestone by hand (README, "The laptop"): run it on a text
# console (Ctrl+Alt+F3, log in as yourself), never inside a desktop. It builds nothing: build first, from the desktop,
#
#   cd packages/compositor && cargo build --release
#
# and it starts that build (this checkout's target/release/yggdrasil-compositor) with --drm, the panel, Settings and a terminal,
# YGGDRASIL_COMPOSITOR_DEBUG=1, everything it prints in ~/.cache/yggdrasil-compositor/laptop-DATE.log. It prints the Wayland
# socket and the control socket and writes them to ~/.cache/yggdrasil-compositor/laptop-current.env, for screenshots and
# commands from the other session (Ctrl+Alt+F7 or wherever the desktop is, or ssh):
#
#   . ~/.cache/yggdrasil-compositor/laptop-current.env
#   WAYLAND_DISPLAY=$YGG_WAYLAND grim /tmp/shot.png
#   yggdrasil-compositor ctl --socket "$YGG_CTL" outputs
#
# The programs run on a D-Bus session bus of their own (dbus-run-session), so the panel's notifications and tray never reach
# the desktop's session. Settings writes the same files the desktop reads (~/.config/yggdrasil), so a change made here is the
# desktop's too. It ends with the panel's "Sign out", `ctl quit` from the other session, or Ctrl+C here before the screen is
# taken; the console comes back when it ends.
#
#   laptop-tty.sh                 the compositor, the panel, Settings and a terminal (foot, else ghostty)
#   laptop-tty.sh -- ARGS ...     more for the compositor (--run COMMAND ...)
# Environment: YGG_PANEL, YGG_SETTINGS, YGG_TERMINAL (the programs, by name or path; empty: left out),
# YGGDRASIL_COMPOSITOR_RENDERER=pixman (software drawing), YGG_NO_DBUS=1 (no bus of its own).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=${CARGO_TARGET_DIR:-$HERE/../../../target}/release/yggdrasil-compositor
LOGDIR=${XDG_CACHE_HOME:-$HOME/.cache}/yggdrasil-compositor
LOG=$LOGDIR/laptop-$(date +%Y-%m-%d-%H%M%S).log
ENVFILE=$LOGDIR/laptop-current.env

[ "${1:-}" = "--" ] && shift

tty_now=$(tty 2>/dev/null || true)
case "$tty_now" in
  /dev/tty[0-9]*) ;;
  *) echo "laptop-tty.sh: run it on a text console (Ctrl+Alt+F3, log in), not in a terminal window ($tty_now)" >&2; exit 2 ;;
esac
for v in WAYLAND_DISPLAY DISPLAY HYPRLAND_INSTANCE_SIGNATURE; do
  if [ -n "$(printenv "$v" 2>/dev/null || true)" ]; then
    echo "laptop-tty.sh: $v is set: this is inside a desktop; run it on a text console" >&2
    exit 2
  fi
done
[ -x "$BIN" ] || { echo "laptop-tty.sh: no release build at $BIN (cd packages/compositor && cargo build --release)" >&2; exit 2; }
[ -n "${XDG_RUNTIME_DIR:-}" ] || { echo "laptop-tty.sh: no XDG_RUNTIME_DIR: log in on the console (a login session)" >&2; exit 2; }

pick() { # the first of these programs there is
  for p in "$@"; do command -v "$p" >/dev/null 2>&1 && { command -v "$p"; return; }; done
}
# this checkout's panel when it has been built (a fix made with the compositor's is tried with it), else the installed one
PANEL=${YGG_PANEL-$(pick "${BIN%/*}/yggdrasil-panel" yggdrasil-panel)}
SETTINGS=${YGG_SETTINGS-$(pick yggdrasil-settings)}
TERMINAL=${YGG_TERMINAL-$(pick foot ghostty)}

# first, the session bus of its own learns where the screen is, so the services it starts on demand (the GTK portal: GTK's
# window buttons, dark or light) can open it; not --systemd, which would change the desktop's own user manager
if [ -z "${YGG_NO_DBUS:-}" ] && command -v dbus-update-activation-environment >/dev/null 2>&1; then
  set -- "$@" --run dbus-update-activation-environment WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE
fi
[ -n "$PANEL" ] && set -- "$@" --run "$PANEL"
[ -n "$SETTINGS" ] && set -- "$@" --run "$SETTINGS"
[ -n "$TERMINAL" ] && set -- "$@" --run "$TERMINAL"

mkdir -p "$LOGDIR"
rm -f "$ENVFILE"
{
  echo "laptop-tty.sh $(date -Iseconds) on $tty_now"
  echo "compositor: $BIN ($(stat -c %y "$BIN" 2>/dev/null || true))"
  echo "programs: ${PANEL:-no panel} ${SETTINGS:-no Settings} ${TERMINAL:-no terminal}"
  uname -r
} > "$LOG"

# the sockets, as the compositor prints them, to the console and the env file
watch_sockets() {
  i=0
  while [ $i -lt 100 ]; do
    w=$(sed -n 's/^WAYLAND_DISPLAY=//p' "$LOG" | head -n1)
    c=$(sed -n 's/^YGGDRASIL_COMPOSITOR_CTL=//p' "$LOG" | head -n1)
    if [ -n "$w" ] && [ -n "$c" ]; then
      printf 'YGG_WAYLAND=%s\nYGG_WAYLAND_PATH=%s/%s\nYGG_CTL=%s\nYGG_LOG=%s\n' "$w" "$XDG_RUNTIME_DIR" "$w" "$c" "$LOG" > "$ENVFILE"
      printf 'Wayland socket: %s/%s\ncontrol socket: %s\nlog: %s\n' "$XDG_RUNTIME_DIR" "$w" "$c" "$LOG"
      return
    fi
    i=$((i + 1))
    sleep 0.1
  done
  echo "laptop-tty.sh: the compositor printed no sockets (see $LOG)"
}

echo "starting the compositor (log: $LOG)"
watch_sockets &
watcher=$!
status=0
if [ -z "${YGG_NO_DBUS:-}" ] && command -v dbus-run-session >/dev/null 2>&1; then
  env YGGDRASIL_COMPOSITOR_DEBUG=1 XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=Yggdrasil \
    dbus-run-session -- "$BIN" --drm "$@" >> "$LOG" 2>&1 || status=$?
else
  env YGGDRASIL_COMPOSITOR_DEBUG=1 XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=Yggdrasil \
    "$BIN" --drm "$@" >> "$LOG" 2>&1 || status=$?
fi
kill "$watcher" 2>/dev/null || true
rm -f "$ENVFILE"
# the console as it was (a program that ended badly may have left it in another mode)
stty sane 2>/dev/null || true
printf '\033[0m\033[?25h'
echo "the compositor ended (status $status); its log: $LOG"
grep -v '^\(frame\|pointer\|button\) ' "$LOG" | tail -n 15
exit "$status"
