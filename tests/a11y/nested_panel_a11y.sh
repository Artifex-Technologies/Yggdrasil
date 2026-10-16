#!/bin/bash
# Reads the panel's accessibility tree through AT-SPI, without touching your real desktop: a nested Hyprland and a private D-Bus session.
# Run: dbus-run-session -- bash tests/a11y/nested_panel_a11y.sh   (needs `cargo build` in packages/panel, Hyprland, python-dbus)
# Expect: a "Top bar" with Start menu / Desktop / Clock ..., a "Dock" with the apps, "Kept notifications", each button with actions=click.
printf 'monitor = , 1280x720, 0x0, 1\ngeneral { gaps_in = 0; gaps_out = 0 }\nmisc { disable_hyprland_logo = true }\n' > /tmp/hypr-a11y-test.conf
# Runs inside dbus-run-session: nested Hyprland, our panel in it, then a read of the accessibility tree.
export HYPRLAND_NO_RT=1
cd "$(dirname "$0")/../../packages/panel"
WAYLAND_DISPLAY=wayland-0 Hyprland --config /tmp/hypr-a11y-test.conf >/tmp/hypr-a11y.log 2>&1 &
HP=$!
sleep 5
ls $XDG_RUNTIME_DIR | grep wayland
WAYLAND_DISPLAY=wayland-1 ./target/debug/labyrinth-panel >/tmp/panel-a11y.log 2>&1 &
PP=$!
sleep 6
/usr/lib/at-spi2-registryd --use-gnome-session >/tmp/registryd.log 2>&1 &
busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled b true
busctl --user get-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled
sleep 4
ADDR=$(busctl --user call org.a11y.Bus /org/a11y/bus org.a11y.Bus GetAddress | cut -d"\"" -f2); echo "a11y address: $ADDR"; ls -la ${ADDR#unix:path=} 2>&1 | head -2; export AT_SPI_BUS_ADDRESS="$ADDR"
timeout 30 python3 -u "$(dirname "$0")/walk_atspi.py"; echo "(done)"
: <<'PY'
import gi
gi.require_version('Atspi','2.0')
from gi.repository import Atspi
def walk(n, d=0):
    try:
        role = n.get_role_name(); name = n.get_name(); desc = n.get_description()
    except Exception as e:
        print(' '*d + '?', e); return
    acts = ''
    try:
        ai = n.get_action_iface()
        if ai: acts = ' actions=' + ','.join(ai.get_action_name(i) for i in range(ai.get_n_actions()))
    except Exception: pass
    print(' '*d + '%s "%s"%s%s' % (role, name, (' — ' + desc) if desc else '', acts))
    for i in range(n.get_child_count()):
        walk(n.get_child_at_index(i), d+2)
desk = Atspi.get_desktop(0)
print('apps on this bus:', [desk.get_child_at_index(i).get_name() for i in range(desk.get_child_count())])
for i in range(desk.get_child_count()):
    a = desk.get_child_at_index(i)
    if 'labyrinth' in (a.get_name() or '').lower() or 'panel' in (a.get_name() or '').lower():
        walk(a)
PY
kill $PP $HP 2>/dev/null
wait 2>/dev/null
