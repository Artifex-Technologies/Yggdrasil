#!/bin/bash
# Makes Yggdrasil's own compositor the login screen's default session on this computer, from this checkout's release builds,
# keeping the Yggdrasil session on Hyprland as the way back (F2 on the login screen). Every step looks first and says what it
# does; nothing is deleted that a package owns.
#
#   install-session.sh --check    what it would do, changing nothing (no password)
#   install-session.sh            do it (asks for the administrator's password once)
#   install-session.sh --undo     back to Hyprland as the default, and the files it added taken out again
#
# What it puts where:
#   /usr/local/lib/yggdrasil-own/yggdrasil-compositor, yggdrasil-panel   the release builds, copied (the checkout may change)
#   /usr/local/bin/yggdrasil-compositor                                  a link to the compositor (the session finds it there)
#   /usr/local/bin/yggdrasil-compositor-session                          the session's start (this folder)
#   /usr/share/wayland-sessions/yggdrasil-compositor.desktop             the login screen's entry (this folder)
#   (the panel build runs only in the own compositor's session: its start stops the autostart folder's panel and runs this
#   one in its place, so the Yggdrasil session on Hyprland, the way back, keeps the packaged panel exactly as it was)
#   /etc/yggdrasil/greeter.conf                                          `session = yggdrasil-compositor`; the file as it was is
#                                                                        kept beside it, greeter.conf.before-own-compositor
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
BUILD=${YGG_BUILD:-$HERE/../../../target/release}
LIB=/usr/local/lib/yggdrasil-own
ENTRY=/usr/share/wayland-sessions/yggdrasil-compositor.desktop
GREETER=/etc/yggdrasil/greeter.conf
BACKUP=$GREETER.before-own-compositor
MODE=${1:-do}

case "$MODE" in do|--check|--undo) ;; *) echo "usage: $0 [--check|--undo]" >&2; exit 2 ;; esac
step() { printf '\n== %s\n' "$*"; }
run() { # a root step: shown with --check, done otherwise
  if [ "$MODE" = --check ]; then printf '   would run: %s\n' "$*"; else printf '   running: %s\n' "$*"; sudo "$@"; fi
}
user_run() {
  if [ "$MODE" = --check ]; then printf '   would run: %s\n' "$*"; else printf '   running: %s\n' "$*"; "$@"; fi
}

if [ "$MODE" = --undo ]; then
  sudo -v
  step "the login screen's default back to what it was"
  if [ -f "$BACKUP" ]; then run cp -p "$BACKUP" "$GREETER"; else echo "   no backup ($BACKUP): left as it is"; grep -n '^session' "$GREETER" || true; fi
  step "the files this added"
  run rm -f "$ENTRY" /usr/local/bin/yggdrasil-compositor-session /usr/local/bin/yggdrasil-compositor
  run rm -rf "$LIB"
  echo; echo "Done. Sign out: the login screen starts the Yggdrasil session on Hyprland again."
  exit 0
fi

step "the release builds in $BUILD"
for b in yggdrasil-compositor yggdrasil-panel; do
  [ -x "$BUILD/$b" ] || { echo "   missing $BUILD/$b: build it first (cargo build --release in packages/compositor and packages/panel)"; exit 1; }
  echo "   $b: $(stat -c '%y' "$BUILD/$b" | cut -d. -f1), $(du -h "$BUILD/$b" | cut -f1)"
  if ldd "$BUILD/$b" | grep -q 'not found'; then echo "   $b needs a library that is not installed:"; ldd "$BUILD/$b" | grep 'not found'; exit 1; fi
done
echo "   built from: $(git -C "$HERE" log -1 --format='%h %s' 2>/dev/null | cut -c1-90)"

step "the compositor starts (headless, 3 seconds, no screen taken)"
rt=$(mktemp -d)   # a runtime folder of its own, so the test never touches the running desktop's sockets
out=$(XDG_RUNTIME_DIR=$rt timeout 3 "$BUILD/yggdrasil-compositor" --headless 2>&1 || true)
rm -rf "$rt"
if printf '%s' "$out" | grep -q '^WAYLAND_DISPLAY='; then echo "   it started and made its socket"; else echo "   it did not start:"; printf '%s\n' "$out" | tail -5; exit 1; fi

step "the session's start and the login screen's entry"
sh -n "$HERE/yggdrasil-compositor-session" && echo "   yggdrasil-compositor-session: the shell reads it"
grep -q '^Exec=/usr/local/bin/yggdrasil-compositor-session$' "$HERE/yggdrasil-compositor.desktop" && echo "   yggdrasil-compositor.desktop: Exec as expected"
[ -x /usr/bin/yggdrasil-session ] && echo "   the way back, /usr/bin/yggdrasil-session (Hyprland), is installed" || { echo "   /usr/bin/yggdrasil-session is missing: no way back, stopping"; exit 1; }

[ "$MODE" = --check ] || sudo -v

step "copy the builds to $LIB and link the compositor into /usr/local/bin"
run install -d -m755 "$LIB"
run install -m755 "$BUILD/yggdrasil-compositor" "$LIB/yggdrasil-compositor"
run install -m755 "$BUILD/yggdrasil-panel" "$LIB/yggdrasil-panel"
run ln -sf "$LIB/yggdrasil-compositor" /usr/local/bin/yggdrasil-compositor

step "the session's start and the login screen's entry"
run install -m755 "$HERE/yggdrasil-compositor-session" /usr/local/bin/yggdrasil-compositor-session
run install -m644 "$HERE/yggdrasil-compositor.desktop" "$ENTRY"

step "the panel"
echo "   $LIB/yggdrasil-panel runs only in the own compositor's session (its start swaps it in); the Hyprland session keeps the packaged panel"

step "the login screen's default session ($GREETER)"
echo "   now: $(grep '^session' "$GREETER" || echo 'no session line')"
[ -f "$BACKUP" ] && echo "   a backup is there already: $BACKUP (kept)" || run cp -p "$GREETER" "$BACKUP"
if grep -q '^session *=' "$GREETER"; then
  run sed -i 's/^session *=.*/session = yggdrasil-compositor/' "$GREETER"
else
  run sh -c "echo 'session = yggdrasil-compositor' >> '$GREETER'"
fi

step "after"
if [ "$MODE" = --check ]; then
  echo "   nothing was changed. Run it without --check to do it."
else
  echo "   now: $(grep '^session' "$GREETER")"
  cat <<'EOF'

What to do now:
  1. Save your work, then sign out of Hyprland (the panel's power menu, Sign out). This ends the Claude Code session in it.
  2. On the login screen, the line under the password should read "Yggdrasil (own compositor) session · F2 to change".
     F2 (Fn+F2 on this laptop) switches to "Yggdrasil" (on Hyprland) for this one sign-in.
  3. Sign in. If the compositor fails within its first 15 seconds you land in Hyprland's session by itself.
  4. Back to Hyprland for good: run this script with --undo and sign out.
EOF
fi
