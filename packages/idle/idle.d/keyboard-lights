#!/bin/sh
# yggdrasil-idle runs this with "lights-off" when the screen dims, goes off or shows the screensaver, with "lights-on" when you
# are back, and with "lights-on" once when it starts (Settings > Peripherals > Keyboard: "Keyboard and mouse lights off while dimmed"). It turns
# off, and then back to how they were:
#   * Razer keyboards, mice, keypads and mouse mats, through openrazer's daemon (if it runs): each lighting zone it has (the
#     whole device, logo, scroll wheel, backlight, left and right strips)
#   * a laptop's keyboard backlight (/sys/class/leds/*kbd_backlight*), through logind, which lets the signed-in user set it
# Only what was lit is turned off and remembered, so nothing is changed that was already off.
#
# What to put back is kept in ~/.local/state, not in the session's runtime folder: a session that ended while the lights were off
# (log-out, reboot, crash) used to lose it, and the lights stayed at 0 for good; the next start puts them back. The runs take turns
# (a lock): "lights-on" right after "lights-off" used to read the list while it was still being written and leave lights off.
dir=${XDG_STATE_HOME:-$HOME/.local/state}/yggdrasil
state=$dir/lights-off
mkdir -p "$dir" || exit 0
exec 9>"$dir/lights.lock"
flock 9 || exit 0

quoted() { sed -n 's/^[a-z]* [0-9]* *//p' | tr ' ' '\n' | tr -d '"'; }
num() { sed -n 's/^[a-z] //p'; }
# zone interface, its getter and setter (openrazer's razer.device.lighting.<interface>)
zones='brightness getBrightness setBrightness
logo getLogoBrightness setLogoBrightness
scroll getScrollBrightness setScrollBrightness
backlight getBacklightBrightness setBacklightBrightness
left getLeftBrightness setLeftBrightness
right getRightBrightness setRightBrightness'
safe() { case $1 in *[!A-Za-z0-9_-]*|'') return 1 ;; esac; }
dev() { echo "/org/razer/device/$1"; }

case ${1:-} in
lights-off)
  [ -e "$state" ] && exit 0
  : >"$state.new"
  if busctl --user status org.razer >/dev/null 2>&1; then
    for s in $(busctl --user call org.razer /org/razer razer.devices getDevices 2>/dev/null | quoted); do
      safe "$s" || continue
      case $(busctl --user call org.razer "$(dev "$s")" razer.device.misc getDeviceType 2>/dev/null) in
        's "keyboard"'|'s "mouse"'|'s "keypad"'|'s "mousemat"') ;;
        *) continue ;;
      esac
      echo "$zones" | while read -r zone get set; do
        b=$(busctl --user call org.razer "$(dev "$s")" "razer.device.lighting.$zone" "$get" 2>/dev/null | num)
        [ -n "$b" ] && [ "${b%.*}" != 0 ] || continue
        echo "razer $s $zone $set $b" >>"$state.new"
      done
    done
  fi
  for d in /sys/class/leds/*kbd_backlight*; do
    [ -r "$d/brightness" ] || continue
    b=$(cat "$d/brightness")
    [ "$b" -gt 0 ] 2>/dev/null || continue
    echo "leds ${d##*/} - - $b" >>"$state.new"
  done
  # the list is complete before anything goes dark: a run cut short leaves the lights on, never a list missing what it turned off
  mv -f "$state.new" "$state"
  while read -r kind id zone set b; do
    case $kind in
      razer) busctl --user call org.razer "$(dev "$id")" "razer.device.lighting.$zone" "$set" d 0 >/dev/null 2>&1 ;;
      leds) busctl call org.freedesktop.login1 /org/freedesktop/login1/session/auto org.freedesktop.login1.Session SetBrightness ssu leds "$id" 0 >/dev/null 2>&1 ;;
    esac
  done <"$state"
  ;;
lights-on)
  [ -e "$state" ] || exit 0
  # at the start of a session openrazer's daemon may not be up yet: wait for it (up to 30 s) rather than lose the list
  if grep -q '^razer ' "$state"; then
    i=0
    until busctl --user status org.razer >/dev/null 2>&1 || [ $i -ge 30 ]; do sleep 1; i=$((i + 1)); done
  fi
  while read -r kind id zone set b; do
    case $b in *[!0-9.]*|'') continue ;; esac
    case $kind in
      razer)
        safe "$id" && safe "$zone" && safe "$set" || continue
        busctl --user call org.razer "$(dev "$id")" "razer.device.lighting.$zone" "$set" d "$b" >/dev/null 2>&1 ;;
      leds)
        safe "$id" || continue
        busctl call org.freedesktop.login1 /org/freedesktop/login1/session/auto org.freedesktop.login1.Session SetBrightness ssu leds "$id" "$b" >/dev/null 2>&1 ;;
    esac
  done <"$state"
  rm -f "$state"
  ;;
esac
exit 0
