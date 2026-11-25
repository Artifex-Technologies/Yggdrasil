# yggdrasil-compositor

Yggdrasil's own Wayland compositor (Rust, on [Smithay](https://github.com/Smithay/smithay)): the piece that would let the screen
lock, the idle manager, the panel and the login screen all run on one compositor that Yggdrasil controls. Source: `packages/compositor`.

**Today (2026-11-04):** Yggdrasil's desktop runs as its own session on **Hyprland 0.57** (`packages/session`, `yggdrasil-session`,
configured in `packages/session/hyprland/yggdrasil.lua`). Hyprland is the real compositor; this one is still in progress and runs
nested in a window (below) or headless, with no screen at all, for tests (2026-11-10, next section), and since 2026-11-15 on the
screen of a virtual machine (the hardware backend's first milestone). KDE, and with it KWin, is gone
(2026-10-24).

Hyprland stays my daily compositor until this one passes the checks in packages/compositor/README.md ("Before it replaces
Hyprland") on my own PC. Our own desktop pieces are tested on it in CI, headless (since 2026-11-10). Hyprland's display crashes
(2026-11-14) brought the hardware backend forward: its first milestone (2026-11-15, next section) drives a virtual GPU and screen
in a virtual machine.

## The hardware backend: milestone 1 (2026-11-15)

    yggdrasil-compositor --drm [--run COMMAND ...]

Started from a text console, it drives the screens itself (`src/udev.rs`): libseat (logind or seatd), udev for the GPUs and
their connectors, OpenGL ES on GBM/EGL where the GPU has hardware 3D and pixman on dumb buffers where it has not, a Smithay
`DrmCompositor` per screen with frames paced by the page flip (and none drawn while nothing changes), libinput, Ctrl+Alt+Fn to
other virtual terminals and back, and a clean exit (`ctl quit` or a signal) that gives the console back. The control socket
moved to `src/control.rs` so both backends serve Yggdrasil's tools; the headless test commands stay headless-only.

Checked in a virtual machine of my own (`packages/compositor/tests/drm-vm.sh`: an Arch guest under plain QEMU as my user, on
Limbo's Arch cloud image, with a virtio GPU with and without virgl, a USB keyboard and tablet; Limbo's own guests have no display
and need root): the panel's bar, notch and dock and Settings drawn on the virtual screen, Settings > Displays naming the screen
from its EDID ("RHT QEMU Monitor (Virtual-1)", 1920x1080 at 75 Hz), a mode change from wlr-randr reaching the CRTC, foot typed
into and clicked through libinput, Ctrl+Alt+F2/F3/F4 and back with logind and with seatd, with OpenGL ES and with pixman, and
the console back after `ctl quit` and after SIGTERM. Idle (60 s, only the panel): 0.012 % of a core with OpenGL ES, 0.0074 % with
pixman (with Settings open too: 0.31-0.36 % until Settings stopped waking to look for changes on 2026-11-15, 0.008 % since); 62-64 MB resident with OpenGL ES (44 MB of it the GPU driver's libraries), 22.6 MB with pixman. Every number and what is
left: [packages/compositor/README.md](../packages/compositor/README.md#the-hardware-backend-milestone-1-2026-11-15).

What the VM taught: logind lets a session switch terminals only through polkit (`org.freedesktop.login1.chvt`, allowed for the
active session): without polkit the switch is refused; Labyrinth has polkit. QEMU's `screendump` has no surface for a virgl
picture under egl-headless; its VNC server reads it back, so the test machine takes screenshots through VNC.

Three things the first run found, fixed the same day: Settings drew a new picture every few seconds while idle (it now wakes
only for a change: docs/APPS_SETTINGS.md); the programs the compositor starts inherited SIGTERM, SIGINT and SIGHUP blocked from
its signalfd, so Settings never ended on SIGTERM (they are unblocked for them now, and Settings closes on them itself); and a
screen switched to a smaller mode left Settings partly off it (floating windows are now pulled back inside the free room of
their screen when its mode or scale changes or it goes, on the headless and hardware backends alike: `placement::inside`,
`tests/behaviour.rs`).

## The hardware backend: the laptop (built and first run on the laptop 2026-11-19)

What a laptop needs, for my ThinkPad X1 Yoga (Intel Iris Xe): the internal screen known by its connector (eDP, LVDS, DSI) and
given a CRTC first; the lid (closed with a monitor on: the panel leaves its CRTC as if unplugged; alone: dark, for logind to
suspend); the tablet-mode switch recorded; hotplug with CRTCs given back and docks' new MST connectors; a default scale from the
screen's size (1.25 for the laptop's 14-inch 1920x1200 panel, as I set it on Hyprland); waking from a suspend told by the kernel
(a timer cancelled on resume: no D-Bus) and every screen set up again; screens off as a CRTC switched off; the touchpad's and
TrackPoint's settings from input.conf; the touchscreen as wl_touch and the pen as a pointer; and screen copies drawn again on the
GPU (grim, the panel's previews). The rules have unit tests and the touchscreen a headless test; the rest is tried by hand on
the laptop's text console with `packages/compositor/tests/laptop-tty.sh`. What each part does and whether it has been checked:
[packages/compositor/README.md](../packages/compositor/README.md#the-laptop-milestone-2-built-and-first-run-2026-11-19).

## Headless: the hidden test mode (built 2026-11-10)

    yggdrasil-compositor --headless [--outputs N] [--size WxH] [--scale S] [--output WxH[@S]]... [--run COMMAND ...]

(`--backend headless` is the same.) No window, no screen, no GPU and no input devices: each output is a picture in memory, drawn with
Smithay's software renderer (pixman), so it runs in a CI container. Source: `src/headless.rs`.
- **Outputs**: `--outputs 2 --size 1600x900 --scale 1.5` gives two alike, `--output 1600x900 --output 1280x720@1.5` one of each;
  default one 1920x1080 at scale 1. They are named HEADLESS-1, HEADLESS-2 ... and sit side by side from 0,0. Each is a wl_output
  (version 4, with its name) and an xdg-output with its logical size and place.
- **Scale**: programs are told it through wp-fractional-scale (in 120ths), wl_surface's preferred buffer scale (version 6) and
  wl_output's whole-number scale (1.5 is sent as 2 there); the panels' first configure already carries it, and everything is drawn
  at the output's scale (a 1.5 output shows the panel sharp, not blown up).
- **Several outputs** (this needed changes beyond the backend): each output has its own panels (layer map), its own lock surface
  while locked, its own part of the scene, and the pointer finds the output it is on. The profile question shows on the first output, the
  window switcher on the screen of the window with the keyboard; windows open, maximize and go full screen there.
- **Control socket** in the compositor's own runtime folder (`$XDG_RUNTIME_DIR/yggdrasil-compositor-wayland-N.ctl`, mode 0600,
  printed as `YGGDRASIL_COMPOSITOR_CTL=...`), one line per connection; `yggdrasil-compositor ctl COMMAND` sends one:
  `outputs`, `resize HEADLESS-1 1280x720`, `scale HEADLESS-1 1.5`, `screenshot HEADLESS-1 /path/shot.png` (what is pending is drawn
  first), `windows`, `layers`, `quit`. The answer is the lines, then `ok` or `error: ...`.
- **Safety**: it refuses to start without `XDG_RUNTIME_DIR`, and refuses the desktop's own `/run/user/UID` unless
  `YGGDRASIL_COMPOSITOR_SHARED_RUNTIME=1`; WAYLAND_DISPLAY, DISPLAY, HYPRLAND_INSTANCE_SIGNATURE and the other compositors' variables
  are cleared before anything starts, so nothing it starts can reach the running desktop. XWayland is left out unless
  `YGGDRASIL_COMPOSITOR_XWAYLAND=1`. A socket path must fit in 108 bytes, so give it a short folder (the tests use `/tmp`).
- **Lightness**: as nested, nothing is drawn while nothing changes; a burst of commits is folded into at most one frame per 16 ms.
- **The panel runs on it**: the wallpaper, the top bar and the dock map and draw (wlr-layer-shell, xdg-output, wlr-foreign-toplevel,
  fractional scale, viewporter, cursor-shape are the same code as nested), at scale 1 and 1.5, and follow a size change. Its window
  previews come through ext-foreign-toplevel-list and ext-image-copy-capture since 2026-11-12 (below), as on Hyprland.

Tests (`cargo test` in `packages/compositor`; the CI's rust job runs them): 30 in all, 24 unit tests (5 new: the options, sizes
and scales, the refused runtime folder) and 6 end-to-end ones in `tests/headless.rs`, where a small Wayland client of the test's own
talks to a compositor started in a private folder: the outputs appear with the size, scale, name and place asked for (one and
several); a layer surface (a red bar) and a toplevel (a blue window) are mapped and listed, and the screenshot shows red and blue
exactly where they were put and the background elsewhere; a resize and a scale change reach the client (output events, a new
configure, the fractional scale 120 -> 240 -> 150); a bar drawn at buffer scale 2 on a scale 2 output covers exactly its rows of
the picture; `quit` ends it and leaves no socket behind; no runtime folder is refused.

**The panel's leak test runs on it** (`packages/panel/tests/leak-stress.sh --compositor yggdrasil`, CI job
`panel-leaks-own-compositor`; the sway variant and its job stay until this one has proven itself): the same notifications, pop-ups
and resizes, counting the panel's memfd mappings in this compositor's `/proc/PID/smaps` (Smithay keeps a shared-memory pool mapped
while a buffer made from it lives, as wlroots does). Proof, 20 rounds, 2026-11-10: the panel from main passes (9 mappings, 2.3 MB
after the warm-up and after 20 rounds); the leaky panel (the parent of c79d95f) fails (32 mappings after the warm-up, 376 after 20
rounds, about 490 MB). The same in the rootless podman container (`--podman`): main 9 mappings, pass; leaky 374, fail. For
comparison headless sway in that container: main 12, pass; leaky 617, fail.

**The panel with a finger** (`packages/panel/tests/touch.sh`, the same CI job, 2026-11-22): taps through the compositor's headless
`touch-down`/`touch-up` on the start button, the notch (tap and pull) and away from each popover, and with `TOUCH_APP` a long press
on that program's dock icon. It holds the laptop's touch bugs fixed: a tap outside a popover closes it while the finger is down,
the compositor sends no `up` for the gone surface, and the panel had taken the finger as still down and ignored the next touch; a
long press on a dock app minimized it as its window list opened. Proof: the panel before the fix fails "the next tap on the start
button" and "a tap on the notch"; with it all pass (and the dock checks with the Calculator).

**2026-11-12: Yggdrasil's own programs are tested on it** (details, the full list of commands and tests, what it matches of the
Hyprland session and the order to close the gap: [packages/compositor/README.md](../packages/compositor/README.md)). The control
socket now also makes input inside the compositor (`key`, `type`, `pointer`, `click`: no input device is opened) and reports
`focus`, `workspace`, `lock`, `free` and every window with its workspace and states. Headless it offers wlr-screencopy (grim and
yggdrasil-shot work) and wlr-output-power (yggdrasil-idle's screens off). Windows now open centred in the room the bar and the dock
leave free and keep clear of them, as in the Hyprland session (`placement.rs`, the cases of keep-clear.lua); a panel that asks for
the keyboard exclusively (the start menu) gets it at once and gives it back; a second locker is refused while the first runs; the media keys' shortcuts work on the lock screen, no others.
`tests/behaviour.rs` (14 tests, own small clients) and `tests/apps.rs` (the panel, Settings drawn in software, the lock screen's
test-auth build on two outputs) run in CI's `compositor-headless` job, and on a desktop through `tests/apps-podman.sh` in a
container with no devices and no network. Nothing switches: Hyprland stays the session. One test there is for a program of another
repository, the Argus app (argus-app against its fake Flipper on a socket): skipped unless its programs are named, even in CI;
Argus's `argus-tools/crates/argus-app/smoke-yggdrasil.sh` names them and runs it in the same container (2026-11-13).

Not built headless yet: GPU buffers (linux-dmabuf is not offered: shared memory only). Outputs added or removed while running and
placed other than side by side came on 2026-11-12 (below), and so did the window previews and the pointer:

**2026-11-12: the dock's window previews and the pointer** (gap items 6 and 7; details in
[packages/compositor/README.md](../packages/compositor/README.md)). The panel takes pictures of windows through
ext-foreign-toplevel-list, ext-image-capture-source and ext-image-copy-capture on Hyprland, so the compositor now offers them
(`src/capture.rs`; the list is Smithay's): each window drawn on its own at its screen's scale, covered, on another desktop or
minimized, never while the session is locked; the panel's card shows the window's picture on it (`tests/apps.rs`), and
`tests/capture.rs` checks the protocols with the tests' own client. The compositor draws the pointer (`src/pointer.rs`) with the
cursor theme as `ygg-theme cursor` stores and sets it (appearance.conf, the running compositor through yggdrasil-wm, XCURSOR_SIZE),
a program's own cursor surface and the shapes programs name, hides it while yggdrasil-idle has the screen dimmed (the pointer
timeout through yggdrasil-wm) and moves it onto a remaining screen when its screen goes; headless it is painted over the screen's
picture like a hardware cursor plane, so only `screenshot ... pointer`, grim -c and copies that ask for it show it
(`tests/pointer.rs`, with snapshots, Yggdrasil's own arrow among them). GPU buffers for the copies (dmabuf) come with the
hardware backend. The same day it took Settings > Peripherals' mouse and keyboard settings (`input.conf`, read with yggdrasil-wm's parser at
start and on reload, which yggdrasil-wm's `apply_input` asks for): key repeat for every program, and headless the virtual
mouse's speed, scrolling and left-handed buttons (`src/input.rs`, `tests/input.rs`). And, as the Hyprland session does since 2026-11-13, no floating
window opens bigger than its screen: one bigger than the free room is made as big as it less the outer gap and centred
(`placement.rs`'s `open`, keep-clear.lua's cases), remembered places are put back no bigger than the screen is now, and a dialog of
the same process counts as the same program for them.

**2026-11-12: Yggdrasil's own tools talk to it through yggdrasil-wm** (`packages/wm`, the layer they share): the panel's desktop
buttons, window zones, Snap Assist and the dock's minimize, `yggdrasil-snap`, `yggdrasil-close` and the idle manager's pointer find
whichever compositor runs (this one by its control socket for their Wayland display, else Hyprland's socket) and do the same on both.
The control socket got the desktop commands they use (`src/wmctl.rs`: windows with numbers of their own, desktops, screens, the
pointer, placing, maximizing, minimizing and closing windows); a window's title can no longer add lines to an answer, and programs it
starts never get Hyprland's signature. Proof on Hyprland with recorded Hyprland 0.57.1 answers, on this compositor in the headless
tests (`tests/tools.rs`, `tests/apps.rs`): details and what still calls hyprctl in [packages/compositor/README.md](../packages/compositor/README.md).
Later that day the rest moved too: the Task Manager, the clipboard's paste, `yggdrasil-logout`, Settings (Displays, reload, the
shortcuts list, focusing a pop-out), the game fixes, the pointer theme, the shortcuts card and config-error toast, the browser
warm-up, the portal, the screenshot tool and splash, xmove and the installer's keyboard try-out; the control socket gained the
commands for them (window processes, layers, bindings, reload, paste, the pointer theme, the keyboard layout, exit).

**2026-11-12: the session's shortcut table and window look** (gap items 2 and 4; details and what is left in
[packages/compositor/README.md](../packages/compositor/README.md)). The compositor's built-in shortcuts are now the table of
`yggdrasil.lua` (a unit test holds the two together), with the session's words and command lines, taps on a modifier (Super for
the start menu, Ctrl for Find my mouse) and repeat for held volume, brightness and magnifier keys; Super+Shift+M shows the
minimized windows. Windows get the session's look: the accent border, corners rounded at 10, the shadow, the gaps (a maximized
window keeps 8 all round, as on Hyprland), and the open, close and move animations (800 ms, Hyprland's default curve), none with
"reduce motion". `tests/keys.rs` (stand-ins on the compositor's PATH) and `tests/look.rs` (PNG snapshots, a stopped clock for the
frames in an animation) run in CI's `compositor-headless` job and in `tests/apps-podman.sh`.

**2026-11-12: the system's keyboard, the screens' settings and window places** (gap items 3 and 5; details in
[packages/compositor/README.md](../packages/compositor/README.md)). The keyboard layout is the system's, from the file the
installer and localectl write (`/etc/X11/xorg.conf.d/00-keyboard.conf`, then `/etc/vconsole.conf`), taken again when it changes.
The screens follow Settings > Displays' `~/.config/yggdrasil/monitors.lua`, the file Hyprland loads (modes, scales, places, screens
off), and the standard wlr-output-management protocol lists and changes them; headless, screens are plugged in and out while it
runs, and the windows on a screen that goes move to the first one. Windows come back where a program's last one closed (the
session's `window-places` file, shared with the Hyprland session), windows opening on a screen set to tile
(`~/.config/yggdrasil/tiling`) tile there, and Steam's own maximize request is ignored. `tests/outputs.rs` and `tests/places.rs`
(with two PNG snapshots) run in CI's `compositor-headless` job and in `tests/apps-podman.sh`. Settings itself still refuses to
change a screen on this compositor; that check is the one step left.

## Window switching: Alt + Tab (2026-11-19)

Switching windows should feel as good as on Windows 11, in Yggdrasil's own look (`src/switcher.rs`, `src/icons.rs`):
- **Alt + Tab** opens a panel of cards in rows that wrap and stay centred on the screen of the window with the keyboard. Each card
  is a live picture of its window at the window's own shape (wide windows wide, narrow ones narrow, every picture in the grid
  the same height; 160 px, lower when the rows do not fit, and at 64 px only the rows around the chosen card), under a title bar
  with the app's icon and the window's title. The window used last comes first and the first Tab chooses the one before it;
  Alt + Shift + Tab opens on the window used longest ago.
- While Alt is held: Tab moves on and Shift + Tab back (both round the ends), the arrow keys move in the grid (Left and Right
  to the card before and after, Up and Down to the nearest card of the row above or below, all round the ends), Enter or letting go of Alt switches,
  a click on a card switches to it, a click outside the panel or Escape closes it and nothing changes. Switching raises the
  window and gives it the keyboard; a minimized window comes back, a window on another desktop is shown there.
- **Group cards.** The compositor has no grouping of its own; what it groups is what shares a screen. On each screen of the
  desktop showing, the windows snapped to its two halves (Super + Left and Super + Right; one on each side at least) are one
  card, and the windows tiled there (a tiling desktop, or a screen set to tile in Settings) are one card when there are two or
  more. The group card shows their pictures where they are on the screen, side by side, titled "Group | TITLE and N other
  windows" with the icon of the one used last; choosing it brings them all forward, the one used last in front with the
  keyboard. Every other window, minimized ones and those on other desktops included, is a card of its own.
- **The look**: Yggdrasil's dark panel and cards, the accent chosen in Settings > Appearance (else the operating system's, else
  Yggdrasil's purple) as the chosen card's outline, rounded corners as on the windows. The icon is the one the app's desktop
  entry names (found from the window's app id, then the entry's StartupWMClass, as the dock finds it), from the icon pack chosen
  in Settings, hicolor or Liki; an app without one gets the first letter of its name in an accent circle.
- **What each renderer does.** Both renderers draw the cards the same way: each picture is the window's own texture, scaled
  while it is drawn (with the renderer's linear filter), clipped to the card's rounded rectangle; nothing is copied. The
  damage of a window is scaled with it, so a card is redrawn only where its window changed. With OpenGL ES that is a few small
  textured quads per card; with pixman (the software renderer of the headless mode and of machines without a usable GPU) the
  same pictures are scaled in software. I measured that the software renderer needs no snapshot cards (below), so it draws
  them live too. The card frames, titles and icons are small pictures drawn once per opening (again only for a card whose
  title or choice changed); the panel is four corner pictures and three solid bands, opaque, so nothing behind it is drawn.
- **Lightness**: nothing of the switcher runs, is kept or asks for a frame while it is closed; open, it draws one frame and then
  only when a window changes. Measured 2026-11-19 headless (release build, pixman, 1920x1080, four 800x600 windows, 5 s each,
  two rounds): closed and still, 0 ms of CPU and no frame; open over still windows, 10 ms (the opening frame: the cards, the
  icons looked up, the panel) and 1 frame; one window drawing about 20 times a second, 90 ms closed and 110-120 ms open (2.2-2.4 %
  of a core), the same number of frames either way.
- Win + Tab (the deck) is not built; the grid's layout and moves are plain functions of the cards' shapes and the screen's
  size (`switcher::grid`), for its flat grid to use too.

Proof: `tests/keys.rs`, four tests in CI's `rust` and `compositor-headless` jobs: the order (most recent first), Tab and
Shift + Tab going round, Alt + Shift + Tab opening on the oldest, Escape cancelling, the arrows and Enter, switching on Alt's
release; a minimized window brought back, a click on a card, a click outside; the snapped pair as one group card (wider than a
tall window's, each window's colour on its side, a PNG snapshot `tests/snapshots/switcher-grid.png`) brought forward together;
the frames drawn (the control socket's `frames`): none while open over still windows, a window's new picture showing on its
card, one frame to close it and none after. The headless tests run without fonts (fontconfig is given an empty configuration),
so the snapshot is the same on every machine; the titles are drawn when a font is found.

Known limitations: there is no blur or dimming behind the panel (too dear for the software renderer); minimized windows and
windows on other desktops show the last picture they drew (they are not asked for new ones); hovering a card does not choose
it; it has not yet been tried by hand nested or in the test machine (the host keeps Alt + Tab nested).

## Full screen and the windows over it: one stacking order (2026-11-19)

On Hyprland I know this bug: a game full screen, a smaller window opened over it, and then switching back is refused, or the small
window stays drawn while clicks pass through it into the game. It comes from the drawing order and the input order (what the
pointer hits, which window has the keyboard) coming from different places. Before I move the laptop to this compositor it has to
be proven not to do that.

**How it works.** Smithay's `Space` holds the windows back to front, and it is the only order there is
(`packages/compositor/src/state/stacking.rs`):
- **Drawing** (`scene`) walks it from the front: X11 menus, the Overlay layer, the Top layer (the bar and the dock), each window
  (its controls, its picture and its menus, its border), then the Bottom and Background layers.
- **The pointer and touch** (`under`, behind `surface_under`) walk the same list in the same order, so what a click or a
  finger lands on is what is drawn there; the window controls and the resize grips are found in the same walk (a grip of a window
  behind another takes nothing).
- **A click or a tap** gives the keyboard to what `under` found: a window is raised, told it is active and given the keyboard
  in one step; a panel only when it is in front there and asks for the keyboard.
- **Raising** happens only together with the keyboard: a click, Alt + Tab, the dock, `activate`, a new window opening, a
  minimized one restored. Everything that only moves or resizes a window (maximize, full screen and back, snapping, tiling,
  keeping windows clear of the bar and dock, screens changing size) keeps its place in the stack (`relocate`).
- **Full screen** covers the whole screen. While the window in front on a screen is full screen, that screen's bar and dock go
  behind the windows, for the eye and for the pointer alike; another screen keeps its bar. A window opened over a full screen
  one is in front of it with the keyboard (and the bar shows again over the game while it is); a click on the game outside it
  brings the game forward and hides the bar again. A full screen window asking for full screen again, or leaving it, from
  behind stays behind.
- **The keyboard** goes, when the window with it closes or is minimized, to the window then in front, which is told it is
  active. While the screen is locked no window takes the keyboard from the lock.

Before (found by these tests and fixed 2026-11-19): maximize, full screen, restore, snapping, tiling, keep-clear and a screen
changing size raised windows without the keyboard (a window behind a full screen game came out in front of it when the dock
changed, or the game came out in front of the window with the keyboard when the screen changed size, when it asked for full
screen again or left it); a full screen window on one screen decided the bar of every screen (a bar on the other screen was
drawn but took no click); a click on the bar's place over a full screen game could give a panel the keyboard; a hidden window's
resize grip took clicks meant for the full screen window in front; the window under a new one was never told it was no longer
active, nor told it was again when the new one closed; a window closing behind the lock took the keyboard from it; a menu
(xdg_popup) never got its first configure and so never showed. Opening, clicking, Alt + Tab and `activate` were already right.

**Proof**: `packages/compositor/tests/fullscreen.rs` (9 tests, CI's `rust` job). Each check looks at four things at once: the
pixel at a probe point of the screen, the surface a click there reaches, the surface a key reaches, and which window the
programs were told is active. Covered: a window opened over a full screen one; switching back and forth by Alt + Tab, by
clicking and by the control socket's `activate`; minimize and restore; closing the window with the keyboard; leaving full
screen and asking for it again, in front and behind; a menu that reaches past its window over the game, and a dialog with its
parent; two screens; the dock growing; the screen getting smaller; a decorated window's hidden grips; the lock while windows
open and close behind it; a finger tapping the small window, the game outside it, where it was and where the hidden bar is.

Known limitations: a menu has no grab (a click outside it does not close it, and it does not take the keyboard); there is no
"always on top" or layer of its own for dialogs (a dialog is a window like any other, in front because it opened last); the
window a workspace shows with the keyboard is the one used last there, which is also the one in front unless a tool raised
another without it. Not tried headless: real games through Proton and XWayland (X11's own stacking is kept with
`raise_window` when a window is raised, and a full screen X11 window is drawn opaque).

## Status (2026-10-18): first version, runs nested in a window only

It opens a window on the running desktop (winit backend) and serves its own Wayland socket; programs started with `--run` use it:

    yggdrasil-compositor --run /usr/bin/yggdrasil-settings --run dbus-run-session -- /usr/bin/yggdrasil-panel

It cannot touch the real session: it is just another window on the running desktop (that was KWin when the checks below were made; today it is Hyprland, where it has not been tried again). Nothing about the real session changes until the hardware backend exists.

What works (each checked by hand in the nested window):
- **Windows** (xdg-shell): toplevels open cascaded, a click raises one and gives it the keyboard, closing hands the keyboard to the next; popups are placed.
- **Panels and backgrounds** (wlr-layer-shell): the whole Labyrinth desktop runs in it: the wallpaper, the top bar, Fylgja's notch and the dock from `yggdrasil-panel` drew correctly beside a Settings window.
- **Screen lock** (ext-session-lock): `yggdrasil-lock` locked, typed the test password and unlocked (exit 0). While locked only the locker's own picture is drawn and only it gets input.
- **Idle** (ext-idle-notify): `yggdrasil-idle` was told when the user went idle, and its dimming overlay (layer-shell + viewporter) was accepted.
- **Moving a window** by its own title bar (the xdg `move` request): dragging Settings' title area moved the window; checked with a real pointer drag. **Resizing, maximize and full screen** (the matching requests, remembering and restoring the old size) are written but not yet tried by hand. Since 2026-11-24 a program's own move and resize requests are followed only while the button is held on one of that program's own surfaces (a subsurface or a menu of it counts): Smithay hands the request on with nothing but its serial, and every program can learn the serials. A request made with another program's press moves and resizes nothing (`tests/grabs.rs`), nor does one made with the compositor's own Super+drag or grip, which starts on no program's surface.
- **Window list for the panel** (`wlr-foreign-toplevel-management`, written here because Smithay does not ship it): the panel found it, the top bar showed the active window's title ("Settings") and the dock lit a dot under its icon. Activate, close, minimize, maximize and full screen from the panel are written (minimized windows are kept aside and brought back by activate) but not yet tried by hand.
- **Shortcuts handled by the compositor** (`shortcuts.rs`; when this was written they were KDE's `kglobalacceld`, today Hyprland binds the keys): the Super key on its own (start menu, through the panel's D-Bus service), Super+E/I/L/V/S/C/F/Q, Alt+F4, Alt+Tab, Super+arrows (maximize, restore or minimize, snap left and right), Super+Return. `~/.config/labyrinth/shortcuts.conf` changes or removes any of them. Checked in the nested window with test bindings (the host desktop keeps the Super key and Alt+Tab to itself): Ctrl+Shift+Left snapped the focused window to the left half, and Ctrl+Shift+Space opened the panel's start menu. The Super tap, Alt+Tab and the rest are written but could not be tried there.
- **Window controls** drawn by the compositor for programs that ask for server-side decoration (`xdg-decoration`, mostly Qt programs): no title bar, only a grip and the round minimize, maximize and close buttons in the top right corner of the window with no background of their own (they take on the look of the program under them), level with the program's own menu or tool bar; the program keeps the whole height of its window. Drag by the grip (double click maximizes); or hold Super and drag anywhere with the left button to move, the right button to resize; grips just outside the edges resize too. A program that asks to draw its own controls (Minos, Settings, the Marketplace) gets none. `YGGDRASIL_COMPOSITOR_DECORATE=all` gives every window the pill (for testing). Checked in the nested window with Wireshark: the pill sat beside its menu row. Moving by the grip, Super+drag and the edge grips are written but were not tried by hand.
- **Window switcher** (Alt+Tab): holding Alt shows a card for each window, most recently used first; Tab moves on, Shift+Tab goes back, Escape cancels, letting go of Alt picks. Checked with Ctrl+Tab in the nested window (the host keeps Alt+Tab).
- **Workspaces** (four): Super+Ctrl+1..4 and Super+Ctrl+Left/Right switch, Super+Ctrl+Shift+1..4 send the focused window to another one; the dock still lists every window and activating one on another workspace goes there. Checked: sending a window away, and showing only it on the other workspace. The panel's workspace buttons follow these too since 2026-11-12 (through yggdrasil-wm and the control socket, headless; tested).
- **Multimedia keys** are bound to `yggdrasil-mediakey` (in the panel package) (volume, mute, microphone, the running player, brightness). They could not be tried in the nested window, because the host desktop takes those keys; held keys do not repeat yet.
- **Workspace profiles** (`tiling.rs`): each workspace is **standard** (windows float) or **tiling** (Hyprland-style "dwindle": the first window takes the left half, the next the top right, and so on, with gaps). `~/.config/labyrinth/workspaces.conf` sets a default (standard, tiling, or **ask** each time an empty workspace is opened) and an exception per workspace; Settings > Appearance > Workspaces writes it and the compositor reads it again when it changes. Super+T floats or tiles one window. Checked in the nested window: with "ask" the question appeared on the empty workspace, Right and Enter chose Tiling, and the three windows that opened afterwards were laid out side by side.
- Pointer and keyboard (from the host window), wheel, wl_output with xdg-output, clipboard (data device), viewporter, shm.

**X11 programs (XWayland), built 2026-11-04, tried by hand 2026-11-05** (xterm, glxgears, the panel inside the compositor; see
"Tried by hand" below) (`src/xwayland.rs`; for Steam's client, games through Proton
or Wine, and older programs). The X11 display is bound when the compositor starts and printed as `DISPLAY=:n`; programs given with
`--run` and programs started by shortcuts get that DISPLAY (never the host's), and XWayland itself starts when the first X11 program
connects (since 2026-11-24, below). `YGGDRASIL_COMPOSITOR_XWAYLAND=0` leaves it out. What is written:
- An X11 window is placed, tiled, listed for the panel (dock, top bar), shown in Alt+Tab and given the keyboard like a Wayland
  window; the keyboard also sets the X input focus, without which XWayland passes keys to none of its windows. It gets the window
  controls unless it says it draws its own (Motif hints: Steam and most games do). Activate, minimize, maximize, full screen,
  snap, close, the window controls, Super+drag and the program's own move and resize requests use the same code as for Wayland
  windows. An X11 program's own move or resize request (`_NET_WM_MOVERESIZE`) is followed only while the button is held on
  that X11 window itself; any other is ignored, as one sent while the button is held on a Wayland window (`tests/grabs.rs`),
  on the desktop or on the lock screen. Known limitation: X11 does not tell which X11 program sent the request, so while the
  button is held on an X11 window, another X11 program may still move that window. Hiding or destroying it is the same
  clean-up as a closed Wayland window.
- Menus and tooltips (override-redirect windows), and menus, tooltips, notifications and splash screens that go through the window
  manager, are drawn where they ask, on top, never get the keyboard and are not listed; a click on one goes to it.
- A window the compositor placed (tiled, maximized, full screen, snapped) keeps its place when it asks for another; others may change
  their size but not move themselves.
- **What a window asked for before it was shown** (2026-11-05): a game that wants to start full screen, or a program that wants
  to start maximized, sets `_NET_WM_STATE` before its window is shown. Smithay's window manager reads it when the window is
  mapped (on Smithay main), and the window is full screen or maximized from its first frame (`src/xwayland.rs`,
  map_window_request); when that ends, it goes back to the place it would have had. Steam's is ignored, as its maximize requests
  are (below). A window that asks to start minimized (WM_HINTS' initial state) is shown, and told so. A game (below) as big as
  the screen that said nothing is still taken as going full screen; other programs no longer are (`tests/x11.rs`).
- Clipboard and middle-click selection both ways between X11 and Wayland programs; an X11 program can read them only while one of
  its windows has the keyboard. The program with the keyboard is now the one offered the clipboard (this was missing for Wayland
  programs too: Smithay offers it only to the focused program).
- Full screen X11 windows are drawn opaque (a shader that ignores the alpha channel), so a game that leaves junk in it no longer
  shows the desktop through in patches. X11 windows without an alpha visual were already opaque (XWayland hands them over as XRGB).
- **Games are drawn opaque windowed too** (2026-11-05): a Steam game (X11 class `steam_app_<id>`) or a Windows program through Wine
  or Proton (`<name>.exe`), whatever its alpha channel holds. Anything else that needs it goes in `~/.config/yggdrasil/opaque.conf`,
  one app id (X11 class) per line, read again when it changes.
- **Dock icons for X11 programs** (in the panel, so the Hyprland session has them too): a window is matched to its desktop entry by
  its class (`StartupWMClass`), and a Steam game's window (`steam_app_<id>`) to the shortcut Steam made (`steam://rungameid/<id>`);
  a game without a shortcut gets the icon Steam saved (`steam_icon_<id>`). A native Linux game names its window after itself, not
  `steam_app_<id>`: when no desktop entry matches a window, the panel takes the window's process from the compositor's window list
  and reads the `SteamAppId` Steam started it with (only that variable), then matches as above (2026-11-22; checked on the
  headless compositor with a made-up game entry and icon).

**Tried by hand (2026-11-05, nested window on the Hyprland session):**
- xterm shows with the window controls; typing reaches it (keyboard focus and the X input focus).
- Clipboard both ways: `wl-copy` on the compositor's display was read by `xclip` on its X display, and the other way round.
- `xterm -fullscreen` (it sets `_NET_WM_STATE_FULLSCREEN` before its window shows, as games do) opened full screen.
- glxgears (OpenGL through XWayland) ran at about 90 frames a second, drawn and moved like any window.
- Close on glxgears, which does not take part in WM_DELETE_WINDOW: Smithay destroyed its window and left it running with no
  window. **Fixed:** a program that takes part is asked politely and one that does not has its connection ended (XKillClient,
  as KWin and Hyprland do); glxgears then ended with "fatal IO error", as under KWin. Since 2026-11-24 this is fixed in our
  Smithay fork itself (X11Surface::close; the fork is described below), with another of Smithay's X11 bugs: every X11 event was
  dropped after the window manager set the primary screen until it made another request (it ignored that request's sequence
  number, which every following event carries). A third, a new window's first place and size converted with 1.25 taken as 1,
  is fixed in Smithay's main branch itself. The compositor's workarounds for all three are gone (`tests/x11.rs` fails without
  the fixes and passes with them).
- Minimize: the window went away and showed `_NET_WM_STATE_HIDDEN`, but kept `_NET_WM_STATE_FOCUSED`. **Fixed:** a minimized
  window is marked inactive and the window now on top becomes the active one.
- The panel inside the compositor: the minimized xterm was in the dock; a click brought it back with the keyboard, and the top
  bar named it.
- **Steam and games (2026-11-05):** Steam's client runs: its window, its menus (in place, with hover), its pop-ups (closed with
  their own buttons), the dock entry with a dot per window, the top bar's name.
  - Steam asked for a 1373x1080 window on the 1280x800 screen, and its launch-options question, centred on that window, landed
    off the edge where nobody could answer it, so the game never started. **Fixed:** a window is never bigger than the space the
    panels leave free and is placed wholly on it (`xwayland::fit_in`, at map and when it asks to grow).
  - Poly Bridge 2 (native, Unity): full screen as asked, covering the panels, no see-through patches; the mouse reached it.
  - Dead Space 2 through Proton 11: the Wine dialogs ("Activation successful!") showed with the window controls; the game's
    window (`steam_app_47780`) went full screen and minimized itself when Steam took the focus (old DirectX 9 games do), the
    dock showed it with the game's own icon and brought it back full screen with the keyboard. It then drew only a thin strip
    at the top left. The same run in a nested Hyprland at the same size drew the whole picture (Hyprland ignores a program's
    own minimize, so the game never minimized there). The strip was the game's picture cut to a minimized window's size: Wine
    learns a window was restored from ICCCM's WM_STATE going from Iconic back to Normal, and Smithay 0.7 wrote WM_STATE only
    on map and unmap. **Fixed:** minimizing sets WM_STATE to Iconic and restoring sets it to Normal (`x11asked.rs`); the game
    now comes back whole, and keys reach it. Smithay main's `set_hidden`, which minimizing and restoring call too, writes the
    same WM_STATE itself; the compositor's own write is kept, sent at once, as the one tried with the game (Smithay main's has
    not been tried with Wine).
  - Two small gaps, closed 2026-11-24: Proton's own dialogs (class `steam_proton`) go with Steam in the dock, with Steam's icon
    (`packages/panel`, apps.rs); and the dock's window list, with the pointer on it, goes when a game goes full screen
    (`tests/apps.rs`, with an X11 window asking through `_NET_WM_STATE` as games do).
- Not tried by hand: drag and drop (built 2026-11-24 and tested headless: `tests/dnd.rs`, below) and resizing an X11 window
  from its edges (`tests/x11.rs` drags the compositor's grip).
- Small gaps seen: the dock shows a plain icon for xterm (its desktop entry names an XPM picture the panel does not load). The
  dock's hover preview of an X11 window, once empty, shows it (tests/x11.rs and tests/apps.rs, 2026-11-24).

X11 programs are told the desktop's pointer theme and size (Xcursor.theme and Xcursor.size in XWayland's X resources, at
X11's scale, set again when the theme changes; `tests/x11.rs`). X11 programs draw at the screens' resolution (since
2026-11-22): XWayland is told the screens in pixels and Xft.dpi to match, so on a 1920x1200 screen at 125 % a game is offered
1920x1200 and a GTK program keeps its size; `stretch = true` in `~/.config/yggdrasil/xwayland.conf` keeps the old way (the
scaled-down 1536x960 stretched, Xft.dpi 96); `tests/x11.rs` checks both. XWayland starts when the first X11 program connects
(since 2026-11-24): the X11 display is bound when the compositor starts, so DISPLAY is known at once (`DISPLAY=:n` is printed
and given to the programs it starts), and nothing of XWayland runs until a program connects to it; then XWayland starts on
those sockets and accepts the program once its window manager is ready, so even a window mapped at once is managed
(`tests/x11.rs`). This needs a change to Smithay (`X11Sockets` and `XWayland::spawn_with_sockets`: the sockets bound apart
from starting XWayland), carried in our Smithay fork (below). An X11 program's own icon (`_NET_WM_ICON`) shows in the
dock when no desktop entry matches its window (a Windows program through Wine, say; since 2026-11-24): the compositor keeps the
best size of it as a PNG for each window class in the runtime folder's `yggdrasil-window-icons` (read on a thread of its own
when the window shows, and again when its title changes), and the panel looks there for an icon its theme does not have and
watches the folder, so the icon shows a moment after the window (`tests/x11.rs`, `tests/apps.rs`). When XWayland goes away
(it crashed or was killed), its windows go and the next X11 program starts a new one on the same display, so programs already
told DISPLAY reach it (since 2026-11-24, `tests/x11.rs`), whichever of the display's two sockets the first program came
through: the abstract one, which programs on libxcb try first, or the socket file, the only one x11rb uses since its 0.14 (the
tests' own programs). The compositor watches both until a program connects, then leaves both to XWayland. Before 2026-11-25 the
watch on the other socket stayed, and after a first program through the file it held the abstract socket's name, so the display
could not be bound again (`tests/x11.rs`, `tests/dnd.rs`). A paste or a drop still being handed over when it goes ends with it:
the clipboard or the middle-click selection in either direction, and a drop's data. A Wayland program writing into its pipe
finds it closed (EPIPE); one reading gets what was already in the pipe, then its end; one whose drop on an X11 window was not
finished yet is told the drop was cancelled. This holds also when the compositor was busy and an X11 program had asked for the
same paste again. The compositor goes on, and the next X11 program starts XWayland again (`tests/dnd.rs`). Before, the
compositor ended as soon as such a pipe became ready: Smithay's window manager kept each transfer as an event source of its
own, and its handler asked for the window manager that was gone. A drop's dragging program also waited for an end that never
came. The fix is in our Smithay fork (below). The X11 display has XSETTINGS (since 2026-11-24; kept by Smithay's
window manager, fed by the compositor): GTK programs under X11 get their DPI (Xft/DPI, matching Xft.dpi), the pointer's theme and size, and GTK's theme,
icon theme, font and font settings from GTK's settings.ini, and follow a change at once (a new pointer theme, or settings.ini
rewritten by ygg-theme or Settings, seen as soon as it is written: the compositor watches its config files; `tests/x11.rs`).

An X11 window that draws its own shadow inside its X window (GTK and Chromium do once the window manager says it reads
`_GTK_FRAME_EXTENTS`, as Smithay's does on its main branch) is placed as any X11 window, and the compositor keeps the part
inside the shadow as the window, as a Wayland window's geometry: it is listed, drawn, clicked and maximized as that part,
and when the shadow goes or comes back (maximized, restored) the window keeps its place and size and its X window is placed
around it. Its surface takes clicks only 10 of the program's own (X) pixels out into the shadow, where GTK keeps its resize
border (GTK 3 takes clicks 10 X pixels out, GTK 4 12). X11 programs draw at XWayland's scale (above), so the band is 10
pixels divided by that scale in logical pixels: 8 at 125 %, 5 at 200 %. A click further out reaches what is behind, as the
program's X input shape says, which XWayland does not pass on (`tests/x11.rs`, also at 125 %). When the compositor draws the
window's controls, its own grips take the edge of the shadow. A shadow is not believed when one side is wider than 256 logical pixels
(a program may write any number there; our Smithay fork reads each side as 0 to 65535), or when it leaves nothing of the X
window: left and right, or top and bottom, together as wide as the X window or wider (60 on each side of a 100x100 X window,
say). The window is then all of its X window for everything: placed, listed, drawn, clicked (the compositor's controls and
grips go around all of it), shown in the window switcher and copied (the dock's preview). The shadow is judged against the X
window as it is when the window is placed (shown, moved, resized, maximized, tiled, its shadow changed, the scale changed),
and again when the program resizes its X window itself: the window keeps its place, and its X window is placed around it with
the shadow judged then. The compositor has one geometry per window (`state::window_geometry`): Smithay's for a Wayland
window; for an X11 window, its picture less the shadow it was placed with (`state::placed_shadow`), which is also its place in
the window list. Smithay's own X11 geometry (the picture less the program's whole `_GTK_FRAME_EXTENTS`) is used only inside
Smithay's `Space`, to work out which outputs a window is on (`tests/x11.rs`). Known limitations: for a window whose shadow is
not believed, Smithay's `Space` may count it on no output or the wrong one: its frame callbacks, and with several screens the output column of the control
socket's `clients` and the focused screen of `screens`, then go by that screen, or by the main one when there is none; and a shadow
that did not fit the X window when the window was shown is believed once the window is placed in an X window big enough for
it (maximized, then restored): its X window then goes around the window, which looks the same.

**Drag and drop** works between Wayland programs (wl_data_device) and between X11 and Wayland programs, both ways (since
2026-11-24, with Smithay main's XDND). Only the program whose window the button or finger went down on may turn that press
into a drag: Smithay checks only the drag's serial, and every program can learn the serials. A program that asks with another
program's press or finger, or with the compositor's own Super+drag or grip, is told its drag is cancelled, and nothing else
changes: no closed hand, and the press, the finger or the move goes on. This check is the compositor's own (`dnd_requested`,
not in our Smithay fork); an X11 program's drag is checked by the fork's first XDND fix (below). A drag's closed hand and its
icon last exactly as long as the drag: they end when it is dropped, and when it ends without a drop (a touchscreen's cancel,
another grab taking the button, or the dragging program going away), which Smithay main reports through
`DndGrabHandler::cancelled`. Nothing else ends them, so a finger's drag keeps its closed hand while the mouse moves
(`tests/dnd.rs`). A Wayland program's drag over an X11 window is told to it in XDND by Smithay's window manager, which asks
the Wayland program for the data when the X11 one does; while an X11 program drags, the window manager covers the screen with
a window of its own that takes XDND and stands in for the Wayland windows, and steps aside over X11 windows, so two X11
programs still speak XDND to each other. An X11 program's drag icon under the pointer is passed over while a button is held
(only then can a drag go on, so X11 menus take the pointer again after a drag was given up), and XWayland going away in the
middle of a drag ends only its own part: a Wayland program's drag goes on to the Wayland windows, and a Wayland program whose drop on an X11 window was not finished yet is told the drop was cancelled (`tests/dnd.rs`).
Smithay main's drag and drop needed three fixes, carried in our fork: only a drag started on XWayland's surfaces can become
an X11 program's drag (so an X11 program can no longer take over a press on a Wayland window, or the compositor's own move or
resize; X11 programs are not told apart there, so one can still take over a press on another's window), a drop target that
goes away ends the drag for good, and a drag the X11 program gave up ends with the button. A fourth fix in the fork ends
XWayland's transfers with its X connection (above): when it closes, the window manager's clipboard, middle-click selection
and XdndSelection transfers end before the compositor lets the window manager go; a stale transfer replaced by a new request
is removed before it is answered, so a failed answer can no longer leave its pipe watched; and a drop on an X11 window that
was not finished yet is cancelled (`tests/dnd.rs`). Known limitations: the drag's icon and closed hand are drawn at the mouse
pointer, also during a finger's drag; and while a Wayland program's drag is over an X11 window (or dropped there and not
finished yet), any X11 program can ask for its data, since X11 cannot tell programs apart. X11 programs that read a drag's
data before the drop (Wine; Chromium, Electron and CEF programs on X11) can take drops this way.

**Our Smithay fork** (github.com/Artifex-Technologies/smithay-yggdrasil, branch yggdrasil/smithay-main, pinned by commit in
packages/compositor/Cargo.toml): since 2026-11-24, Smithay's main branch (54cf9c10) with six commits of ours, each for
something the compositor's own code cannot do well, and a seventh that only formats a line. They bind the X11 sockets apart from
starting XWayland (XWayland on the first X11 program, above); fix two X11 window manager bugs (close, and the events dropped
after setting the primary screen: "Tried by hand", above); clamp a layer surface's size at i32::MAX and `_GTK_FRAME_EXTENTS`
at what an X window can hold (a program's huge values ended a debug build: a negative size, an overflow in
`X11Surface::geometry`; `tests/hostile.rs`, `tests/x11.rs`); fix the three drag and drop bugs above; and end the window
manager's transfers, and a drop not finished yet, when XWayland's X connection closes (above; `tests/dnd.rs`). The rest of
what our fork of Smithay 0.7 carried is Smithay main's own: a new X11 window's first place and size at fractional scales; the
session lock's checks (a program refused as a second locker can neither unlock the session nor show its lock screen in place
of the real one, which would get the password; the compositor keeps its own check of which lock's surfaces it shows:
docs/LOCKSCREEN.md); and the checks of what programs send (a region of negative size ended a debug build; negative size hints
and window geometry of no size are protocol errors), with `tests/hostile.rs`: programs sending what the protocols forbid,
which the compositor must survive.

To try it: `yggdrasil-compositor --run xterm` (typing, its menus on Ctrl+click, moving, the window controls), then Steam
(`--run steam`: its menus, the clipboard both ways with a Wayland program, minimize and restore from the dock), then a Proton
game full screen (no see-through patches, keys and mouse reach it). `YGGDRASIL_COMPOSITOR_DEBUG=1` also shows XWayland's own messages.

Known limitations (2026-11-19): it does not run the real session; Hyprland does. Resize, maximize, full screen and minimize,
a full screen window covering the top bar and dock, GPU clients (linux-dmabuf), idle inhibit, primary selection, cursor
shapes and xdg-activation are built but not yet tried by hand. X11 programs were tried by hand on Smithay 0.7 (xterm,
glxgears, Steam and two games, 2026-11-05, above); on Smithay main (2026-11-24) only xterm and glxgears have been, in the test
machine, not yet Steam or a game. It has four workspaces, and up to nine
when more are used (other programs see, switch and add them through `ext-workspace`; the panel follows them through yggdrasil-wm). On the hardware
backend, hotplug has not been tried on real hardware; explicit sync for NVIDIA is built and checked nested on an RTX 2080 Ti
but not yet on the hardware backend, and a machine with two GPUs picks the one with the screens (not tried on two cards);
VRR (while a full screen window covers the screen) is built but not yet seen with a game; no window goes to a plane (direct
scanout): every window is drawn through an element of the compositor's own (rounded corners, animations, a full screen X11
window drawn opaque), which offers Smithay's DRM compositor no buffer, so a full screen game is always composited
(packages/compositor/README.md, "Games on the hardware backend"); there is no tearing control (Smithay has no asynchronous
page flips yet). Screen sharing through the portal (ScreenCast:
yggdrasil-portal, the share picker and yggdrasil-screencast, packages/screencast) is built and tested headless, whole screens in
shared memory, but not yet tried with a browser or OBS; screenshots and picked colours through the portal (the same backend)
are tested headless but not yet tried with a program by hand. It is developed in a virtual machine, never on the live session, and the
headless tests run in CI the whole time.

Debugging: `YGGDRASIL_COMPOSITOR_DEBUG=1` prints pointer positions, button presses and move requests. To drive it from a script, move the host pointer
with `ydotool` in small steps (the host accelerates movement by about 1.7x) and read the positions it prints.

## Lightness (a rule for all of Yggdrasil)
Yggdrasil must stay as light and quick as the features allow, on any machine, including old and small ones. For the compositor that means: nothing is drawn while
nothing changes (a frame is drawn only after a program committed a picture, input arrived or a setting changed, and a frame with no change is not even sent on);
no polling (the config files are watched with inotify and read only when one of them changes, since 2026-11-25; until then a look at them every 2 seconds); pictures such as the window controls and the switcher are drawn once and kept until what they show changes.
Measured 2026-10-18 on this computer, nested: the compositor used about 2 % of a core while idle because it redrew 60 times a second; after the change it uses 0 %.
Measured 2026-11-12 headless (release build, software drawing, 1920x1080, a bar, a dock and three 800x600 windows), before and after
the window look: idle 0.004 % and 0.005 % of a core, 25.6 and 26.3 MB resident; a window opening costs about 0.1 s of CPU spread
over its 800 ms animation, and nothing is drawn once it has settled. After the keyboard, screens and window places (gap items 3
and 5, the same set): idle 0.006 % of a core, 26.4-26.5 MB resident (before: 0.005-0.006 %, 26.2-26.3 MB). After the keyboard, screens and window places (gap items 3 and
5, the same day): idle 0.006 % of a core, 26.4-26.5 MB resident (from 26.3). After the window previews and the pointer (gap items
6 and 7, the same set, five runs): idle 0.006 % of a core (560-621 µs in 10 s, before 547-572), 26.5-26.8 MB resident (before
26.6-26.7); a window's picture for the dock costs 1.1-1.2 ms of CPU (800x600), and the pointer costs nothing while it rests. On the hardware (the test machine, 2026-11-15, 60 s with the panel): 0.012 % of a core with OpenGL ES (62-64 MB resident, 44
MB of it the GPU driver's libraries), 0.0074 % with pixman (22.6 MB); its only wake-ups then are the 2-second look at the config
files. XWayland on the first X11 program (2026-11-24, headless at 1920x1080, three runs each, debug builds): before, an idle
compositor with XWayland on ran an Xwayland of 90 MB resident (23 MB proportional to what it shares) beside its own 38 MB; after,
no Xwayland until an X11 program connects, and the compositor 35 MB. Its memory nested (about 117 MB) is mostly the graphics driver's context; the panel is about 89 MB, the idle manager 3 MB, the polkit agent 8 MB.
Measured 2026-11-24 headless (release builds, software drawing, this PC): the time from starting a program until the compositor
has its first picture, on a fresh compositor each time (the median of three; the first start after a build is up to 80 ms
slower): Calculator 91 ms, 2048 92 ms, Player 93 ms, Settings 105 ms, Sudoku 106 ms, Saga MD 108 ms, Task Manager 111 ms, Agora
124 ms (without network), Minos 170 ms. With four 1270x710 windows on a 2560x1440 screen (Minos, Settings, Saga MD, Calculator)
the compositor drew nothing and used no CPU in 30 s idle, at 53 MB resident; dragging one of them over the other three cost
about 1 ms of CPU a frame (300 ms for 302 frames in 6.8 s). The egui apps themselves draw nothing while idle since YGG-17:
0 frames in 60 s for Minos and the Calculator (before: Minos about 28 a second, the Calculator 6). The window switcher (2026-11-19): nothing while closed; open, one frame
and then only what changes (numbers in "Window switching" above).
Smithay main against Smithay 0.7 (2026-11-24, the builds side by side): in the test machine (release builds, 1920x1080 at 75 Hz,
the own session's environment, every thread of the compositor) idle and a full screen program cost the same within the runs'
spread: 60 s with the panel took 6.26 against 5.47 ms of CPU with OpenGL ES (0.010 and 0.009 % of a core, 33 wake-ups each,
62.5 and 62.1 MB resident) and 4.29 against 3.52 ms in software without 3D (48.9 and 47.8 MB); a full screen glxgears at 75
frames a second, 2.45-2.56 against 2.33-2.62 % of a core. A window drawing 75 frames a second (a 300x300 eglgears, alternating
runs of 20 s) costs a little more: 11.29 against 10.71 % of a core (11.11-11.48 and 10.55-10.87), about 5 % more a frame.
Headless (debug builds, `tests/look.rs idle_cost`, two runs each): idle 3243-3279 against 3229-3352 µs in 10 s, an 800x600
window drawn anew 1.93-1.95 against 1.84 ms, 44.6 against 42.6-43.1 MB resident (about 2 MB more).
The config files watched instead of looked at every 2 seconds (2026-11-25, `packages/compositor/src/configs.rs`), measured
headless at 1920x1080 on this PC, the builds before and after side by side: with nothing on the screen, 30 s of idling took 15
passes over the screen, 15 wake-ups of the compositor's main thread and 1.0-1.1 ms of CPU before (release builds; 3.0 ms in debug
builds), and none of the three after, at the same memory (23.0-23.3 and 23.1-23.2 MB resident). With a bar, a dock and three
800x600 windows (`tests/look.rs idle_cost`, two runs each): idle 708-782 against 0 µs of CPU in 10 s (release builds; debug
builds 3569-3837 against 0), 30.5 against 30.6-30.7 MB resident, an 800x600 window drawn anew 0.92-0.98 against 0.92-1.09 ms a
frame (within the runs' spread). A change to one of the files is in force at once, and nothing wakes the compositor while none
changes (`tests/headless.rs`).
The panel's window zones and the mouse's left button (2026-11-25, `packages/panel/src/zones.rs`, docs/ZONES.md), measured
headless at 1600x900 on this PC, debug builds of the panel before and after, alternating, three runs each: wake-ups of the
panel's main thread (its voluntary context switches) and its time on the CPU, with a FIFO in its input folder standing in for
the mouse (the build before took only real devices, so a measurement-only `LD_PRELOAD` shim, loaded into both, answered their
device questions for the FIFO and counted them). With window zones off and the button held, 148 wake-ups in 3 s in each run
before (its 60 ms look for a drag, asking the compositor each time; 6.3-7.0 ms of CPU) and 0-1 after, no more than idle: the
mouse is not even open while zones are off. With zones on and the button held without a drag, 148 in 3 s both (the look for a
drag, unchanged). With the drag's sheet open and the pointer still, 536-641 wake-ups and 18.2-20.1 ms of CPU in 4 s before, and
271-329 and 12.0-12.9 ms after: the sheet's 30 ms look asks the compositor for the pointer alone (before, for the window too, to
see whether it still followed the pointer), and before the sheet also opened the mouse a second time and asked it 131-132 times
in those 4 s (`EVIOCGKEY`, 33 a second); after, it holds the mouse once and asks it nothing. With the pointer moving within one
zone (2 px every 20 ms), 3.18-3.37 s of CPU in 4 s before and 14.6-15.0 ms after: the sheet, the bar and the dock were drawn
again on each look after the pointer moved, and now only when it goes to another zone (wake-ups 221-230 before and 278-291
after: the drawing had slowed the looks to about one every 60 ms). The drop was seen 9-27 ms after the release before (at the
next 30 ms look) and at once after (0 ms, the release's own event). Idle, 0-1 wake-ups in 3 s both. `tests/apps.rs` holds the
drag, the drop, the pointer moving within a zone and a held button with zones off to this.
Settings > Power's root services (tune, casefan, gpufan) are Rust since 2026-11-09 (docs/APPS_SETTINGS.md): about 2.8 MB each on
their own, plus about 26 MB while libnvidia-ml is initialised (the NVIDIA driver's own cost, the same in any language), and tune
reads /dev/kmsg instead of keeping a journalctl child (about 46 MB). The Python versions were about 40 MB each with swap counted.


## The bake-off (set up 2026-10-23)

Decision rule (owner, 2026-10-23): the compositor is chosen by measurement on this machine's real games and a CAD model, and the
shell stays on standard protocols whichever wins. `tools/bakeoff/yggdrasil-bakeoff` adds one login entry per candidate, each
starting the compositor with only Steam in it. Round 1 also had KWin alone and the full Plasma desktop as the baseline; Hyprland
won, KDE is gone, and those two entries were removed from the tool (2026-10-26). Their results stay in `~/bakeoff` for the report.
The table is round 1 as it was set up:

| Entry | Compositor | Notes |
| --- | --- | --- |
| Bake-off: KWin alone | `kwin_wayland --xwayland --exit-with-session=steam` | KDE's compositor without Plasma; removed with KDE |
| Bake-off: Hyprland | Hyprland (0.56 then), a generated Lua config | games may tear (`immediate` rule), VRR per `yggdrasil-bakeoff vrr` |
| Bake-off: Yggdrasil's own compositor | `yggdrasil-compositor --drm --run steam` (added 2026-11-23), the screen at `highrr` through `YGGDRASIL_COMPOSITOR_MONITORS` | VRR per `yggdrasil-bakeoff vrr` (`YGGDRASIL_COMPOSITOR_VRR`); games always composited (no window goes to a plane); no tearing control yet; the session ends when Steam does |
| Bake-off: COSMIC compositor | cosmic-comp 1.0 (Smithay, Rust) | Steam is started into it once its Wayland and X sockets exist |
| Bake-off: gamescope | Valve's, `gamescope -e -f -- steam -gamepadui` | the Steam Deck way; a candidate for a game session, not for the desktop |
| Bake-off: Plasma | the full KDE desktop | baseline; removed with KDE |

MangoHud measures every Vulkan game started there (Proton games are): 20 s after the game starts, 90 s of frame times, no overlay,
into `~/bakeoff/NAME-vrrX/`. `yggdrasil-bakeoff report` compares average FPS, 1% and 0.1% lows, frame-time spread and stutters.
Round 1 is with variable refresh off everywhere; round 2 (`yggdrasil-bakeoff vrr on`) with it on. Not measured: input latency (it needs
a camera or a photodiode), and the CAD model in Daedalus; Yggdrasil's own compositor has not been through the bake-off.

## Title bars in the Hyprland session (2026-10-24)

Hyprland offers every program server-side decoration (`xdg-decoration`) but draws no title bar. Programs that draw their own
anyway are unaffected: Yggdrasil's apps (`with_decorations(false)`), GTK programs, Ghostty, Firefox, Chromium and Electron.
Qt programs accept the offer and would have no title bar, so `yggdrasil-session` sets
`QT_WAYLAND_DISABLED_INTERFACES=zxdg_decoration_manager_v1`: Qt does not see the offer and draws its own title bar with minimize,
maximize and close. Checked in a nested Hyprland with Qt Linguist (title bar present) and VLC before the change (none).
