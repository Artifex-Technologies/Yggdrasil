#!/bin/bash
# Writes the fixture /proc trees the tests read (tests/fixtures): DIR/PID/cmdline (NUL-separated argv) and DIR/PID/comm.
# Run from packages/gametime: tests/make-fixtures.sh tests/fixtures
set -euo pipefail
out=$1
rm -rf "$out"
proc() {   # proc TREE PID COMM [ARG...]   (no ARG: an empty cmdline, as a kernel thread or a zombie has)
  local tree=$1 pid=$2 comm=$3; shift 3
  mkdir -p "$out/$tree/$pid"
  printf '%s\n' "$comm" > "$out/$tree/$pid/comm"
  : > "$out/$tree/$pid/cmdline"
  for a in "$@"; do printf '%s\0' "$a" >> "$out/$tree/$pid/cmdline"; done
}
S=/home/player/.local/share/Steam

# a desktop with no game: also programs that merely look busy
for t in idle steam gamescope lutris quoting; do
  proc $t 1 systemd /usr/lib/systemd/systemd --switched-root --system --deserialize=40
  proc $t 2 kthreadd
  proc $t 812 Hyprland Hyprland
  proc $t 901 yggdrasil-panel /usr/bin/yggdrasil-panel
  proc $t 1500 ollama /usr/bin/ollama serve
done
proc idle 2200 vivaldi-bin /opt/vivaldi/vivaldi-bin --type=renderer
proc idle 2300 steam $S/ubuntu12_32/steam -srt-logger-opened

# Steam running Cyberpunk 2077 through Proton; a shell asking pgrep about it comes first and is not the game
proc steam 3050 steam $S/ubuntu12_32/steam -srt-logger-opened
proc steam 4100 bash /bin/bash -c "pgrep -a -f 'SteamLaunch AppId='"
proc steam 4101 pgrep pgrep -a -f "SteamLaunch AppId="
proc steam 4210 reaper $S/ubuntu12_32/reaper SteamLaunch AppId=1091500 -- $S/ubuntu12_32/steam-launch-wrapper -- \
  $S/steamapps/common/SteamLinuxRuntime_sniper/_v2-entry-point --verb=waitforexitandrun -- \
  "$S/steamapps/common/Proton 9.0 (Beta)/proton" waitforexitandrun "$S/steamapps/common/Cyberpunk 2077/bin/x64/Cyberpunk2077.exe"
proc steam 4400 Cyberpunk2077.e 'Z:\home\player\.local\share\Steam\steamapps\common\Cyberpunk 2077\bin\x64\Cyberpunk2077.exe'

# a game in gamescope (a grep for it comes first)
proc gamescope 5000 grep grep -r gamescope /etc
proc gamescope 5100 gamescope /usr/bin/gamescope -w 2560 -h 1440 -W 3440 -H 1440 -f -- /usr/bin/supertuxkart

# Lutris open, and running a game through its wrapper (a Python script)
proc lutris 6000 lutris python3 /usr/bin/lutris
proc lutris 6010 less less /usr/share/lutris/bin/lutris-wrapper
proc lutris 6020 python3 python3 /usr/share/lutris/bin/lutris-wrapper "Diablo II" 0 0 /usr/bin/wine "C:\\Games\\Diablo II\\Game.exe"

# only mentions: shells, searches and editors whose command lines name the words, our own status, and a zombie gamescope
proc quoting 7000 bash /bin/bash -c "reaper SteamLaunch AppId=10 -- true"
proc quoting 7001 pgrep pgrep -af "reaper SteamLaunch AppId=10"
proc quoting 7002 grep grep -rn gamescope /usr/share/doc
proc quoting 7003 vim vim /usr/bin/gamescope
proc quoting 7004 less less /usr/share/lutris/bin/lutris-wrapper
proc quoting 7005 sh sh -c "lutris-wrapper: Diablo II"
proc quoting 7006 yggdrasil-gamet /usr/bin/yggdrasil-gametime status
proc quoting 7007 gamescope
proc quoting 7008 journalctl journalctl -g "SteamLaunch AppId=1091500"
