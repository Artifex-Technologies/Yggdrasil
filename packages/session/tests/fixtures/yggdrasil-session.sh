#!/bin/bash
# yggdrasil-session: Yggdrasil's own desktop session, on Hyprland (the compositor chosen 2026-10-23, docs/COMPOSITOR.md).
# The login screen starts it (the "Yggdrasil" entry), or a text console: sign in on Ctrl+Alt+F3 and type yggdrasil-session.
#
# Hyprland draws the windows; everything you see and use is Yggdrasil's: the panel, dock and Fylgja, the lock screen, the
# notifications, the password boxes, Settings, Minos. Your own Hyprland additions go in ~/.config/yggdrasil/hyprland.lua.
set -u
export XDG_CURRENT_DESKTOP=Yggdrasil XDG_SESSION_DESKTOP=Yggdrasil XDG_SESSION_TYPE=wayland
# toolkits: Wayland first, X11 (Xwayland) for the rest
export QT_QPA_PLATFORM="wayland;xcb" GDK_BACKEND="wayland,x11,*" SDL_VIDEODRIVER="wayland,x11" CLUTTER_BACKEND=wayland
export MOZ_ENABLE_WAYLAND=1 ELECTRON_OZONE_PLATFORM_HINT=auto _JAVA_AWT_WM_NONREPARENTING=1
export QT_WAYLAND_DISABLE_WINDOWDECORATION=0
# Hyprland offers to decorate windows but draws no title bar, so Qt programs that accept (VLC, Qt's tools, KDE programs) would have
# none: they are told not to see that offer and draw their own (title, minimize, maximize, close)
export QT_WAYLAND_DISABLED_INTERFACES=${QT_WAYLAND_DISABLED_INTERFACES:-zxdg_decoration_manager_v1}
# Qt programs follow Yggdrasil's dark or light and accent through the desktop portal (Qt's own, no KDE theme plugin needed);
# without it they are plain light grey
export QT_QPA_PLATFORMTHEME=${QT_QPA_PLATFORMTHEME:-xdgdesktopportal}
if [[ -e /proc/driver/nvidia/version ]]; then
  export LIBVA_DRIVER_NAME=nvidia __GLX_VENDOR_LIBRARY_NAME=nvidia NVD_BACKEND=direct
fi

# Game stats (MangoHud, for a game started with MANGOHUD=1): one slim line across the top centre in Yggdrasil's look, the accent
# from Settings > Appearance and the desktop's font. Drawn by the game itself, so it costs next to nothing and does not stop the
# game's frames going straight to the monitor. Your own ~/.config/MangoHud/MangoHud.conf wins when there is one.
mangohud_conf() {
  local cfg=${XDG_CONFIG_HOME:-$HOME/.config} accent=7c54d6 rgb font out=${XDG_RUNTIME_DIR:-/run/user/$UID}/yggdrasil-mangohud.conf
  rgb=$(sed -n 's/^[[:space:]]*accent[[:space:]]*=[[:space:]]*\([0-9]\+\)[[:space:]]*,[[:space:]]*\([0-9]\+\)[[:space:]]*,[[:space:]]*\([0-9]\+\).*/\1 \2 \3/p' "$cfg/yggdrasil/appearance.conf" 2>/dev/null | head -n1)
  [[ -n $rgb ]] && accent=$(printf '%02x%02x%02x' $rgb)
  font=$(fc-match -f '%{file}' sans-serif 2>/dev/null)
  cat >"$out" <<CONF
# Written by yggdrasil-session at sign-in; your own ~/.config/MangoHud/MangoHud.conf is used instead when it exists.
horizontal
hud_compact
position=top-center
offset_y=6
round_corners=10
background_color=0b0c10
background_alpha=0.8
text_color=e8e9ee
text_outline=0
${font:+font_file=$font}
font_size=16
fps
frametime
frame_timing=0
throttling_status=0
cpu_stats
cpu_temp
gpu_stats
gpu_temp
cpu_color=$accent
gpu_color=$accent
engine_color=$accent
frametime_color=$accent
horizontal_separator_color=$accent
toggle_hud=Shift_R+F12
CONF
  echo "$out"
}
if [[ ! -e ${XDG_CONFIG_HOME:-$HOME/.config}/MangoHud/MangoHud.conf && -z ${MANGOHUD_CONFIGFILE:-} ]]; then
  export MANGOHUD_CONFIGFILE=$(mangohud_conf)
fi
conf=/usr/share/yggdrasil/hyprland/yggdrasil.lua
[[ -n ${YGGDRASIL_HYPRLAND_CONFIG:-} ]] && conf=$YGGDRASIL_HYPRLAND_CONFIG
log() { systemd-cat -t yggdrasil-session echo "$*" 2>/dev/null || echo "yggdrasil-session: $*" >&2; }

# The user's own session bus (the one systemd runs, which the autostart programs use). A private bus would split the session in two:
# programs started by Hyprland on one bus, the panel and the rest on the other, and the environment never reaching systemd.
if [[ -S ${XDG_RUNTIME_DIR:-/run/user/$UID}/bus ]]; then
  export DBUS_SESSION_BUS_ADDRESS="unix:path=${XDG_RUNTIME_DIR:-/run/user/$UID}/bus"
fi
log "starting Hyprland ($(Hyprland --version 2>/dev/null | head -n1 | cut -d' ' -f1-2)) with $conf, bus ${DBUS_SESSION_BUS_ADDRESS:-none}"

# the console this session starts on shows nothing (no text from before, no cursor) until Hyprland draws
if [[ -t 1 ]]; then printf '\033[2J\033[3J\033[H\033[?25l'; fi
state=${XDG_STATE_HOME:-$HOME/.local/state}/yggdrasil
mkdir -p "$state"
# what Hyprland and its launcher print goes to a file, not onto the screen: started by greetd, the session's output is the
# console itself, and its start-up messages flashed up between the login screen and the desktop (SDDM kept them in a file too)
exec >"$state/session-output.log" 2>&1
runtime=${XDG_RUNTIME_DIR:-/run/user/$UID}
# Hyprland keeps its log in the runtime folder, which goes when the session does (or with a restart): a copy is kept up to date in
# ~/.local/state/yggdrasil/hyprland-last.log while the session runs, so a problem can always be looked at afterwards
before=$(ls "$runtime/hypr" 2>/dev/null)
(
  while sleep 5; do
    d=$(ls -td "$runtime"/hypr/*/ 2>/dev/null | head -n1)
    [[ -n $d && -f $d/hyprland.log ]] && ! grep -qxF "$(basename "$d")" <<<"$before" && cp -f "$d/hyprland.log" "$state/hyprland-last.log"
  done
) &
keeper=$!

# Monitors coming and going, with the time, straight from the kernel (so it is written even if Hyprland is the one in trouble):
# 2026-11-05 Hyprland 0.57.1 died when a monitor that had been switched off came back. Trimmed to its last 2000 lines each login.
events=$state/monitor-events.log
[[ -f $events ]] && tail -n 2000 "$events" >"$events.tmp" 2>/dev/null && mv -f "$events.tmp" "$events"
echo "$(date '+%F %T') session start" >>"$events"
watch_monitors=
if command -v udevadm >/dev/null; then
  ( udevadm monitor --kernel --subsystem-match=drm 2>/dev/null | while IFS= read -r line; do
      [[ $line == KERNEL* ]] && echo "$(date '+%F %T') $line"
    done >>"$events" ) &
  watch_monitors=$!
fi

# Our own small watchdog in place of start-hyprland: after a crash, Hyprland's launcher started it again in SAFE MODE (no
# Yggdrasil settings, so no panel, no shortcuts, no Wi-Fi menu; 2026-11-05). Here a crash keeps that session's log, then
# Hyprland starts again with Yggdrasil's settings. Only when it keeps crashing (3 times in 10 minutes, which points at the
# settings themselves) does the next start use safe mode. The "not started by start-hyprland" warning is turned off in
# yggdrasil.lua (misc.disable_watchdog_warning).
logs=$state/hyprland-logs
mkdir -p "$logs"
crashes=()
safe=
run=0
while :; do
  run=$((run + 1))
  if [[ -n $safe ]]; then
    Hyprland --safe-mode
  else
    Hyprland --config "$conf"
  fi
  code=$?
  # a normal sign-out, or Hyprland told to stop (SIGTERM, SIGINT): not a crash
  (( code == 0 || code == 143 || code == 130 )) && break
  # a crash (or a fatal error): keep this instance's log before the next one starts and its folder is cleared
  d=$(ls -td "$runtime"/hypr/*/ 2>/dev/null | head -n1)
  [[ -n $d && -f $d/hyprland.log ]] && cp -f "$d/hyprland.log" "$logs/$(date +%Y-%m-%d-%H%M%S)-run$run-crashed-exit$code.log"
  echo "$(date '+%F %T') Hyprland ended with exit $code" >>"$events"
  now=$(date +%s)
  recent=()
  for t in "${crashes[@]}"; do (( now - t < 600 )) && recent+=("$t"); done
  crashes=("${recent[@]}" "$now")
  [[ -n $safe ]] && break  # safe mode itself failed: give up, back to the login screen
  if (( ${#crashes[@]} >= 3 )); then
    safe=1
    log "Hyprland crashed ${#crashes[@]} times in 10 minutes (exit $code); starting it in safe mode"
  else
    log "Hyprland crashed (exit $code); starting it again with Yggdrasil's settings. Log kept in $logs"
    # say so once the panel is back (it serves notifications)
    ( sleep 10; notify-send -a Yggdrasil -i dialog-warning "The desktop restarted" "Hyprland stopped unexpectedly and was started again. Open programs had to close; its log is kept in ~/.local/state/yggdrasil/hyprland-logs." 2>/dev/null ) &
  fi
  # (added 2026-11-13 with the same change in the port: the session's programs stopped, so the next autostart starts them again)
  systemctl --user stop yggdrasil-session.target graphical-session.target 2>/dev/null
  sleep 1
done
kill "$keeper" 2>/dev/null
[[ -n $watch_monitors ]] && pkill -P "$watch_monitors" 2>/dev/null; kill "$watch_monitors" 2>/dev/null
echo "$(date '+%F %T') session end" >>"$events"
d=$(ls -td "$runtime"/hypr/*/ 2>/dev/null | head -n1)
[[ -n $d && -f $d/hyprland.log ]] && cp -f "$d/hyprland.log" "$state/hyprland-last.log"
# the session's programs (the panel and the rest) end with it
systemctl --user stop yggdrasil-session.target graphical-session.target 2>/dev/null
log "Hyprland ended (exit $code); its log is in $state/hyprland-last.log"
# a restart or shut-down asked for from the session (yggdrasil-logout) happens now, after Hyprland has let go of the screen and
# the graphics card, as Plasma does it: rebooting under a running compositor could hang the machine at the very end (NVIDIA)
after=$runtime/yggdrasil-after-session
if [[ -f $after ]]; then
  what=$(cat "$after" 2>/dev/null)
  rm -f "$after"
  case $what in
    reboot|poweroff) log "$what, now that the session has ended"; systemctl "$what" ;;
  esac
fi
exit $code
