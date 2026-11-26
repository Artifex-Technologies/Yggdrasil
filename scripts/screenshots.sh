#!/bin/bash
# screenshots.sh: every screenshot in the README and docs/, made again from this repository's own build on Yggdrasil's headless
# compositor (packages/compositor --headless), with made-up data only. It never touches the running desktop: no window opens,
# nothing is drawn on a screen, and everything runs in a sandbox (bubblewrap) that cannot see the desktop's runtime folder, its
# session or system bus, your home, the network or any device.
#
#   scripts/screenshots.sh [--no-build] [--out DIR] [--only REGEX] [--list]
#
#   --no-build     use what is already built in target/ (debug builds: Settings reads its fixtures only in a debug build)
#   --out DIR      where the pictures go (default assets/screenshots)
#   --only REGEX   only the pictures whose name matches (e.g. 'settings-power' or '^settings-')
#   --list         the pictures it makes, and stop
#
# Each picture is 1920x1080: the panel (wallpaper, top bar, dock) with the app as a window, in the dark style (in the light style
# for a name ending in "-light"). The made-up system is in tests/screenshots: the account "alex" (Alex Example) on "example-pc",
# a home folder with a few files, stand-ins for the programs the apps run directly (tests/screenshots/bin/standin), a
# laptop's sensors and, for the desktop, a graphics card (tests/screenshots/drm: an AMD card's sysfs files, made-up numbers); Settings' pages get theirs from its fixtures (packages/settings/tests/fixtures: system.json, laptop.json,
# wifi.json, peripherals.json, displays-single.json). Two sandboxes are made, a
# desktop computer and a laptop (a battery, no fan controller).
#
# YGG_SHOT_FONTS=DIR adds the font files in DIR (the account's own fonts inside), for the Font pages' pictures on a computer
# that does not have the fonts to choose from installed (Labyrinth's font packages, unpacked).
#
# Needs bwrap (bubblewrap), dbus-daemon and busctl. In CI (CI=true, already a throw-away container) it runs without bwrap.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
data=$repo/tests/screenshots
fixtures=$repo/packages/settings/tests/fixtures
SIZE=1920x1080

# ------------------------------------------------------------------ the pictures
# shot NAME MACHINE KIND ARGS...
#   settings PAGE FIXTURE [SCROLL]   yggdrasil-settings PAGE with packages/settings/tests/fixtures/FIXTURE.json (scrolled SCROLL px)
#   app PROGRAM [ARGS...]            one of target/debug's programs as a window
#   panel desktop|plain|start|calendar|fylgja N   the panel alone (with or without made-up notifications), its start menu,
#                                    calendar, or Fylgja on section N
#   lock                             the lock screen (its test-auth build)
# and what to do in the window before the picture, ";" between the steps: `tap X Y` (a click there), `wait S` (seconds) or any
# compositor control command (`yggdrasil-compositor ctl`, e.g. `type TEXT`); `agents-demo` writes the Agents item's made-up agents
declare -A ACTIONS=(
    [settings-cooling-curves]="tap 1408 273"
    [settings-displays-connect]="tap 1413 517"
    [settings-lock-screen-designs]="wait 30"
    [settings-wallpaper]="wait 30"
    [settings-font]="wait 20"
    [settings-fonts-installed]="wait 20"
    [taskmanager]="tap 474 265; wait 3; tap 735 430; wait 5"
    [minos-privacy-menu]="pointer 900 322; click right; wait 3"
    [calculator]="type 1299*12"
    [settings-software-update-available]="tap 816 425; wait 2"
    [settings-software-update-history]="tap 806 567; wait 3; tap 795 300; wait 2"
    [settings-network-heimdall]="tap 1435 364; wait 4; scroll 200; wait 1; scroll 200; wait 1; scroll 200; wait 1; scroll 150; wait 2"
    [agents]="agents-demo; wait 70; tap 1605 14; wait 3"
    [minos-top]="tap 657 354"
    [minos-top-light]="tap 657 354; tap 1224 251; wait 1; tap 1225 322; wait 1"
    [minos-properties]="tap 657 354; key alt+Return; wait 2"
    [minos-settings]="tap 382 155; wait 1; tap 404 341; wait 2"
    [settings-cloud-drives-add]="tap 769 603; wait 2; tap 940 653; wait 1"
    [settings-cloud-drive-page]="tap 1435 371; wait 2"
)
shots() {
    shot desktop                        desktop panel desktop
    shot start-menu                     desktop panel start
    shot calendar                       desktop panel calendar
    shot fylgja-stats                   desktop panel fylgja 2
    shot fylgja-notifications           desktop panel fylgja 3
    shot settings-general               desktop settings general system
    shot settings-accessibility         desktop settings accessibility system
    shot settings-storage               desktop settings storage system
    shot settings-software-update       desktop settings software system
    shot settings-software-update-available desktop settings software system
    shot settings-software-update-history desktop settings software system
    shot settings-language              desktop settings language system
    shot settings-network               desktop settings network wifi
    shot settings-network-heimdall      desktop settings network wifi
    shot settings-bluetooth             desktop settings bluetooth system
    shot settings-vpn                   desktop settings vpn system
    shot settings-vpn-add               desktop settings vpn:add system
    shot settings-displays              desktop settings displays system
    shot settings-displays-connect      desktop settings displays system
    shot settings-displays-single       desktop settings displays displays-single
    shot settings-sound                 desktop settings sound system
    shot settings-peripherals-mouse     desktop settings peripherals:mouse peripherals
    shot settings-peripherals-razer     desktop settings peripherals:mouse peripherals 600
    shot settings-peripherals-keyboard  desktop settings peripherals:keyboard peripherals
    shot settings-camera                desktop settings peripherals:camera system
    shot settings-microphone            desktop settings peripherals:microphone system
    shot settings-printers              desktop settings printers system
    shot settings-power                 desktop settings power system
    shot settings-power-laptop          laptop  settings power laptop
    shot settings-battery               laptop  settings battery laptop
    shot settings-cooling               desktop settings power:cooling system
    shot settings-cooling-curves        desktop settings power:cooling system 700
    shot settings-appearance            desktop settings appearance system
    shot settings-font                  desktop settings appearance:font system
    shot settings-font-text             desktop settings appearance:font-text system
    shot settings-font-code             desktop settings appearance:font-code system
    shot settings-fonts-installed       desktop settings appearance:font system 420
    shot settings-lock-screen-designs   desktop settings appearance:lock system
    shot settings-wallpaper             desktop settings appearance:wallpaper system
    shot settings-assistant             desktop settings assistant system
    shot settings-apps-list             desktop settings apps system
    shot settings-cloud-drives          desktop settings cloudstorage system
    shot settings-cloud-drives-light    desktop settings cloudstorage system
    shot settings-cloud-drives-add      desktop settings cloudstorage system
    shot settings-cloud-drive-page      desktop settings cloudstorage system
    shot settings-default-apps          desktop settings default system
    shot settings-games                 desktop settings games system
    shot settings-security              desktop settings security system
    shot settings-security-privacy      desktop settings security system 560
    shot settings-distress-open         desktop settings security:distress system
    shot settings-distress              desktop settings security:distress-demo system
    shot settings-distress-sections     desktop settings security:distress-demo-mid system
    shot settings-distress-warning      desktop settings security:distress-demo-warn system
    shot settings-firewall              desktop settings firewall system
    shot settings-users                 desktop settings users system
    shot settings-restore               desktop settings restore system
    shot settings-date-time             desktop settings datetime system
    shot settings-date-time-light       desktop settings datetime system
    shot settings-about                 desktop settings about system
    shot minos-explorer                 desktop app minos /home/alex/Documents
    shot minos-privacy-menu             desktop app minos /home/alex/Music
    shot minos-top                      desktop app minos /home/alex/Documents
    shot minos-top-light                desktop app minos /home/alex/Documents
    shot minos-properties               desktop app minos /home/alex/Documents
    shot minos-settings                 desktop app minos /home/alex/Documents
    shot minos-transfers                desktop app minos /home/alex/Documents --demo-transfers
    shot minos-transfers-collapsed      desktop app minos /home/alex/Documents --demo-transfers=collapsed
    shot minos-transfers-failed         desktop app minos /home/alex/Documents --demo-transfers=failed
    shot minos-transfers-light          desktop app minos /home/alex/Documents --demo-transfers
    shot taskmanager-processes          desktop app yggdrasil-taskmanager
    shot taskmanager                    desktop app yggdrasil-taskmanager
    shot marketplace                    desktop app agora
    shot calculator                     desktop app yggdrasil-calculator
    shot saga-md                        desktop app saga-md "/home/alex/Documents/Trip plan.md"
    shot installer-welcome              desktop app yggdrasil-installer --demo welcome
    shot installer-font                 desktop app yggdrasil-installer --demo font
    shot installer-network              desktop app yggdrasil-installer --demo network
    shot installer-network-hidden       desktop app yggdrasil-installer --demo network-hidden
    shot installer-keyboard             desktop app yggdrasil-installer --demo keyboard
    shot installer-timezone             desktop app yggdrasil-installer --demo timezone
    shot installer-timezone-light       desktop app yggdrasil-installer --demo timezone
    shot installer-disk-manual          desktop app yggdrasil-installer --demo disk-manual
    shot installer-account              desktop app yggdrasil-installer --demo account
    shot lock-screen                    desktop lock
    # last: the Agents pill stays in the bar once its made-up agents are written
    shot agents                         desktop panel plain
}

# ------------------------------------------------------------------ the outside: build, then one sandbox per machine
outside() {
    local build=1 out=$repo/assets/screenshots only='' list=0
    while [ $# -gt 0 ]; do
        case "$1" in
        --no-build) build=0 ;;
        --out) out=$(mkdir -p "$2" && cd "$2" && pwd); shift ;;
        --only) only=$2; shift ;;
        --list) list=1 ;;
        *) sed -n '2,21p' "$0"; exit 2 ;;
        esac
        shift
    done
    if [ $list = 1 ]; then
        shot() { [[ $1 =~ ${only:-.} ]] && echo "$1.png"; return 0; }
        shots
        exit 0
    fi
    if [ $build = 1 ]; then
        local p
        for p in compositor panel settings minos taskmanager market calculator saga-md installer lock agent-usage; do
            (cd "$repo/packages/$p" && cargo build --locked)
        done
        # the lock screen's test build (a stand-in password check) goes to a folder of its own, never mixed with the real one
        (cd "$repo/packages/lock" && cargo build --locked --features test-auth --target-dir "$repo/target/test-auth" --bin yggdrasil-lock)
    fi
    local machine
    for machine in desktop laptop; do
        shot() { [[ $1 =~ ${only:-.} ]] && [ "$2" = "$machine" ] && echo "$1"; return 0; }
        [ -n "$(shots)" ] || continue
        if [ "${CI:-}" = true ]; then
            OUT=$out ONLY=$only "$0" --inside "$machine"
        else
            sandbox "$machine" "$out" "$only"
        fi
    done
    # the compositor writes quick, large PNGs: the same pixels again, smaller (about half), when Pillow is there
    shot() { [[ $1 =~ ${only:-.} ]] && echo "$out/$1.png"; return 0; }
    # shellcheck disable=SC2046 # one file name per line, none with a space
    python3 -I -c 'import sys; from PIL import Image
for f in sys.argv[1:]: Image.open(f).convert("RGB").save(f, optimize=True)' $(shots) 2>/dev/null || true
}

# bubblewrap: the host's programs and libraries read-only; a blank /home, /run, /tmp and /dev (so neither the desktop's runtime
# folder nor its buses are there); the repository at /run/ygg/src and the output folder at /run/ygg/out; no network, no input
# devices, no batteries; a made-up processor, memory, host name and accounts; the stand-ins over the programs the apps run directly
sandbox() {
    local machine=$1 out=$2 only=$3
    command -v bwrap >/dev/null || { echo "screenshots: needs bwrap (the bubblewrap package)" >&2; exit 2; }
    local args=(
        --die-with-parent --new-session --unshare-all --hostname example-pc --clearenv
        --ro-bind / / --proc /proc --dev /dev
        --tmpfs /home --tmpfs /root --tmpfs /run --tmpfs /tmp
        --ro-bind "$repo" /run/ygg/src --bind "$out" /run/ygg/out
        --ro-bind "$data/proc/cpuinfo" /proc/cpuinfo --ro-bind "$data/proc/meminfo" /proc/meminfo
        --ro-bind "$data/etc/hostname" /etc/hostname --ro-bind "$data/etc/passwd" /etc/passwd --ro-bind "$data/etc/group" /etc/group
    )
    local d
    [ -n "${YGG_SHOT_FONTS:-}" ] && args+=(--ro-bind "$YGG_SHOT_FONTS" /run/ygg/fonts)
    for d in /var/tmp /media /mnt /srv /var/lib/AccountsService /sys/class/input; do [ -d "$d" ] && args+=(--tmpfs "$d"); done
    # this repository's own bar items for the pictures (the Agents item), beside the system's
    [ -d /usr/share/yggdrasil/integrations ] && args+=(--overlay-src /usr/share/yggdrasil/integrations --overlay-src "$data/integrations"
        --ro-overlay /usr/share/yggdrasil/integrations)
    # no batteries (a wireless mouse's would show its name; Settings' laptop battery comes from its fixture)
    args+=(--tmpfs /sys/class/power_supply)
    if [ "$machine" = laptop ]; then
        # no fan controller (Settings shows a laptop's own fans then), and a laptop's sensors
        [ -d /usr/lib/yggdrasil ] && args+=(--tmpfs /usr/lib/yggdrasil)
        args+=(--ro-bind "$data/laptop/hwmon" /sys/class/hwmon)
    else
        # a made-up graphics card instead of the real ones (the Task Manager reads /sys/class/drm; /dev has no NVIDIA nodes, so
        # NVML finds nothing)
        args+=(--ro-bind "$data/drm" /sys/class/drm)
    fi
    local p
    for p in /usr/bin/yggdrasil-gpu-run /usr/lib/yggdrasil/game-fix /usr/bin/labyrinth-sarina-setting /usr/bin/labyrinth-quarantine \
        /usr/bin/labyrinth-security-setting; do
        [ -e "$p" ] && { [ "$machine" = desktop ] || [[ $p != /usr/lib/yggdrasil/* ]]; } && args+=(--ro-bind "$data/bin/standin" "$p")
    done
    bwrap "${args[@]}" --setenv PATH /usr/bin --setenv OUT /run/ygg/out --setenv ONLY "$only" --setenv REPO /run/ygg/src \
        /usr/bin/bash /run/ygg/src/scripts/screenshots.sh --inside "$machine"
}

# ------------------------------------------------------------------ the inside: compositor, panel, each app, each picture
inside() {
    local machine=$1 repo=${REPO:-$repo}
    local target=$repo/target data=$repo/tests/screenshots fixtures=$repo/packages/settings/tests/fixtures
    # global: the EXIT trap uses them
    work=$(mktemp -d /tmp/ygg-shots.XXXXXX)
    export HOME=/home/alex USER=alex LOGNAME=alex LANG=en_US.UTF-8 TZ=${TZ:-America/Denver}
    if [ "${CI:-}" = true ]; then HOME=$work/home; fi
    mkdir -p "$HOME" "$work/run" "$work/bin" "$work/input"
    chmod 0700 "$work/run"
    cp -r "$data/home/." "$HOME/"
    # extra fonts for the Font pages (YGG_SHOT_FONTS outside)
    if [ -d /run/ygg/fonts ]; then mkdir -p "$HOME/.local/share/fonts" && cp -r /run/ygg/fonts/. "$HOME/.local/share/fonts/"; fi
    # the operating system's wallpaper as the chosen one (the lock screen draws only a chosen wallpaper)
    local wall
    wall=$(sed -n 's/^wallpaper *= *//p' /usr/share/yggdrasil/integrations/*.conf 2>/dev/null | head -1) || true
    if [ -f "$wall" ]; then echo "$wall" > "$HOME/.config/yggdrasil/wallpaper"; fi
    # programs found by name: the stand-ins, and this build's lock screen for Settings' lock screen previews
    local n
    for n in snapper systemctl heimdall-strip; do ln -s "$data/bin/standin" "$work/bin/$n"; done
    ln -s "$target/debug/yggdrasil-lock" "$work/bin/yggdrasil-lock"
    # the Agents item asks this stand-in instead of the assistant (outside PATH: nothing else finds it)
    mkdir -p "$work/agents"
    ln -s "$data/bin/standin" "$work/agents/sarina"
    export YGGDRASIL_AGENTS_SARINA=$work/agents/sarina
    export PATH=$work/bin:/usr/bin XDG_RUNTIME_DIR=$work/run XDG_CURRENT_DESKTOP=Yggdrasil XDG_SESSION_TYPE=wayland
    export LIBGL_ALWAYS_SOFTWARE=1 NO_AT_BRIDGE=1
    unset WAYLAND_DISPLAY DISPLAY HYPRLAND_INSTANCE_SIGNATURE DBUS_SESSION_BUS_ADDRESS
    export DBUS_SYSTEM_BUS_ADDRESS=unix:path=$work/no-system-bus

    # a session bus of its own, with no services to start
    cat > "$work/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path=$work/bus</listen>
<policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>
EOF
    /usr/bin/dbus-daemon --config-file="$work/bus.conf" --nofork > /dev/null 2>&1 &
    pids=($!)
    trap 'kill "${pids[@]}" 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$work"' EXIT
    until [ -S "$work/bus" ]; do pause 0.1; done
    export DBUS_SESSION_BUS_ADDRESS=unix:path=$work/bus

    "$target/debug/yggdrasil-compositor" --headless --size $SIZE > "$work/compositor.log" 2>&1 &
    pids+=($!)
    local i
    for i in $(seq 100); do ctl outputs > /dev/null 2>&1 && break; pause 0.1; done
    ctl outputs > /dev/null || { cat "$work/compositor.log"; exit 1; }
    local sock
    for sock in "$work"/run/wayland-*; do [[ $sock =~ /wayland-[0-9]+$ ]] && export WAYLAND_DISPLAY=${sock##*/}; done
    # the idle pointer is never in the picture: screenshots without `pointer` leave it out

    # an empty folder in place of /dev/input (YGGDRASIL_PANEL_INPUT): the panel never reads the computer's mice
    YGGDRASIL_PANEL_INPUT=$work/input "$target/debug/yggdrasil-panel" > "$work/panel.log" 2>&1 &
    pids+=($!)
    for i in $(seq 300); do ctl layers | grep -q yggdrasil-dock && break; pause 0.1; done
    ctl layers | grep -q yggdrasil-dock || { echo "the panel did not start:"; tail -20 "$work/panel.log"; exit 1; }
    pause 2

    shot() {
        local name=$1 m=$2 kind=$3
        shift 3
        local what=${1:-}
        [ "$m" = "$machine" ] && [[ $name =~ ${ONLY:-.} ]] || return 0
        echo "screenshots: $name"
        # the style: set before the app starts (the panel follows the file too), back to dark after the picture
        local style=dark
        [[ $name == *-light ]] && style=light
        sed -i "s/^style = .*/style = $style/" "$HOME/.config/yggdrasil/appearance.conf"
        [ $style = dark ] || pause 2
        local app=''
        case "$kind" in
        settings)
            local env=(YGGDRASIL_SETTINGS_FIXTURE="$fixtures/$2.json")
            [ -n "${3:-}" ] && env+=(YGGDRASIL_SETTINGS_SCROLL="$3")
            env "${env[@]}" "$target/debug/yggdrasil-settings" "$1" > "$work/$name.log" 2>&1 &
            app=$! ;;
        app)
            local prog=$1
            shift
            "$target/debug/$prog" "$@" > "$work/$name.log" 2>&1 &
            app=$! ;;
        panel)
            case "$1" in
            desktop) notifications ;;
            plain) ;;
            start) panel_call ToggleStart ;;
            calendar) panel_call ToggleCalendar ;;
            fylgja)
                # the Notifications section lists the desktop picture's notifications (sent here when that picture was not made)
                if [ "$2" = 3 ] && [ "$notified" = 0 ]; then notifications; pause 10; fi
                panel_call ShowFylgja u "$2" ;;
            esac ;;
        lock)
            # only the test-auth build checks the password against this variable (any other asks PAM): refuse anything else
            grep -qa YGGDRASIL_LOCK_TEST_PASSWORD "$target/test-auth/debug/yggdrasil-lock" || { echo "not a test-auth lock" >&2; return 1; }
            YGGDRASIL_LOCK_TEST_PASSWORD=example "$target/test-auth/debug/yggdrasil-lock" > "$work/$name.log" 2>&1 &
            app=$! ;;
        esac
        if [ "$kind" = settings ] || [ "$kind" = app ]; then
            for i in $(seq 300); do [ -n "$(ctl windows)" ] && break; kill -0 "$app" 2>/dev/null || break; pause 0.1; done
            [ -n "$(ctl windows)" ] || { echo "screenshots: $name opened no window:"; tail -20 "$work/$name.log"; return 1; }
        elif [ "$kind" = lock ]; then
            for i in $(seq 300); do [ "$(ctl lock | head -1)" = locked ] && break; pause 0.1; done
            # it draws once the wallpaper is read and blurred (seconds in a debug build): black until then
            pause 8
        fi
        local action
        IFS=';' read -ra action <<< "${ACTIONS[$name]:-}"
        # the page laid out and its data read before anything is clicked
        [ ${#action[@]} = 0 ] || pause 4
        for i in "${action[@]}"; do
            # shellcheck disable=SC2086 # a command and its words
            set -- $i
            case "${1:-}" in
            '') ;;
            wait) pause "$2" ;;
            agents-demo) agents_demo ;;
            # the pointer is put near and moved there by the virtual mouse (a program sees it arrive), then clicked
            tap) ctl pointer $(($2 - 14)) $(($3 - 10)) > /dev/null && ctl pointer-move 14 10 > /dev/null && pause 0.3 && ctl click > /dev/null ;;
            *) ctl "$@" > /dev/null ;;
            esac || { echo "screenshots: $name: \"$i\" failed" >&2; return 1; }
            pause 0.5
        done
        steady "${OUT:?}/$name.png"
        if [ $style != dark ]; then sed -i "s/^style = .*/style = dark/" "$HOME/.config/yggdrasil/appearance.conf"; fi
        # back to the plain desktop for the next one
        case "$kind" in
        lock)
            # killing a lock leaves the screen locked: unlock it as a person would
            ctl type example > /dev/null
            ctl key Return > /dev/null
            wait "$app" 2>/dev/null || true
            [ "$(ctl lock | head -1)" = unlocked ] || { echo "screenshots: the lock screen did not unlock" >&2; return 1; } ;;
        settings | app)
            kill "$app" 2>/dev/null; wait "$app" 2>/dev/null || true
            for i in $(seq 100); do [ -z "$(ctl windows)" ] && break; pause 0.1; done ;;
        panel)
            case "$what" in
            start) panel_call ToggleStart ;;
            calendar) panel_call ToggleCalendar ;;
            fylgja) ctl key escape > /dev/null ;;
            esac
            pause 1 ;;
        esac
    }
    shots
}

ctl() { "$target/debug/yggdrasil-compositor" ctl "$@"; }
# sleep without starting a program (a `sleep` would show in Fylgja's top apps and the Task Manager)
pause() { read -rt "$1" <> <(:) || true; }
panel_call() { /usr/bin/busctl --address="$DBUS_SESSION_BUS_ADDRESS" call org.yggdrasil.Panel /Panel org.yggdrasil.Panel "$@" > /dev/null; }

# a few made-up notifications for the desktop picture (their pop-ups go after 8 s; the Notification Center keeps them)
notified=0
notifications() {
    local n
    notified=1
    for n in "Weather|Rain from 4 pm|Take an umbrella: 80 % chance until the evening." "Agora|Updates installed|12 packages were brought up to date." \
        "Calendar|Dentist at 3 pm|In 30 minutes"; do
        IFS='|' read -r app title body <<< "$n"
        /usr/bin/busctl --address="$DBUS_SESSION_BUS_ADDRESS" call org.freedesktop.Notifications /org/freedesktop/Notifications \
            org.freedesktop.Notifications Notify "susssasa{sv}i" "$app" 0 "" "$title" "$body" 0 0 8000 > /dev/null
    done
}

# the Agents item's made-up agents: Claude's limits (a fresh cached reading and a made-up sign-in, so nothing is asked of
# Anthropic), two Claude Code sessions named after processes that run during the pictures (so they count as running), and the
# assistant's fleet (the stand-in prints it): the assistant idle, one agent with a limit, one whose record is old
agents_demo() {
    local now sess=$HOME/.claude/sessions state=$HOME/.local/state/agents-demo
    printf -v now '%(%s)T' -1
    mkdir -p "$sess" "$HOME/.cache/yggdrasil/agent-usage" "$state"
    printf '{"claudeAiOauth":{"accessToken":"made-up","expiresAt":%s}}\n' $(((now + 86400) * 1000)) > "$HOME/.claude/.credentials.json"
    printf '{"fetched_at":%s,"payload":{"seven_day":{"utilization":38,"resets_at":%s},"five_hour":{"utilization":64,"resets_at":%s}}}\n' \
        "$now" $((now + 4 * 86400)) $((now + 7200)) > "$HOME/.cache/yggdrasil/agent-usage/claude.json"
    utc() { TZ=UTC printf "%(%Y-%m-%dT%H:%M:%SZ)T" "$1"; }
    printf '{"agents":[{"id":"codex","name":"Codex","ready":true,"stale":false,"updatedAt":"%s","limits":[{"label":"Weekly","percentUsed":23,"resetsAt":"%s"}],"statusText":""},{"id":"gemini","name":"Gemini","ready":true,"stale":true,"updatedAt":"%s","limits":[],"statusText":"no limit data"}],"self":{"id":"sarina","name":"Sarina","running":true,"lastActive":"%s"}}\n' \
        "$(utc $((now - 240)))" "$(utc $((now + 3 * 86400)))" "$(utc $((now - 3 * 3600)))" "$(utc $((now - 1500)))" > "$state/fleet.json"
    local pid status name ago st
    for pid in "${pids[@]:0:2}"; do
        if [ "$pid" = "${pids[0]}" ]; then status=busy name=photo-site ago=20; else status=idle name=trip-planner ago=840; fi
        read -r -a st < "/proc/$pid/stat"
        printf '{"pid":%s,"procStart":"%s","status":"%s","name":"%s","updatedAt":%s,"kind":"interactive"}\n' \
            "$pid" "${st[21]}" "$status" "$name" $(((now - ago) * 1000)) > "$sess/$pid.json"
    done
}

# the picture once it has stopped changing (two copies a second apart alike), after at least two seconds
steady() {
    local file=$1 a=$work/a.png b=$work/b.png i
    pause 2
    ctl screenshot HEADLESS-1 "$a" > /dev/null
    for i in $(seq 15); do
        pause 1
        ctl screenshot HEADLESS-1 "$b" > /dev/null
        [ "$(cksum < "$a")" = "$(cksum < "$b")" ] && break
        mv "$b" "$a"
    done
    cp "$a" "$file"
}

if [ "${1:-}" = --inside ]; then
    target=$repo/target
    [ -n "${REPO:-}" ] && target=$REPO/target
    inside "$2"
else
    outside "$@"
fi
