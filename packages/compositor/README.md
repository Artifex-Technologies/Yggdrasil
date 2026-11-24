# yggdrasil-compositor

Yggdrasil's own Wayland compositor, in Rust on [Smithay](https://github.com/Smithay/smithay). **Nothing switches to it.** My
plan (2026-11-10) was to give it a hardware (DRM/KMS) backend only after the rest of Yggdrasil is ported to Rust; Hyprland's
display crashes changed that on 2026-11-14: the hardware backend is the next big project, and its first milestone (below) runs on
a virtual GPU and screen in a virtual machine. It replaces Hyprland only once it passes the checks at the end of this file on
my own PC. Until then my desktop stays on Hyprland; the compositor runs nested in a window, headless (where Yggdrasil's own
programs are tested on it) or in the test machine. The longer history (nested mode, XWayland, what was tried by hand) is in [docs/COMPOSITOR.md](../../docs/COMPOSITOR.md).

## The headless test mode

    yggdrasil-compositor --headless [--outputs N] [--size WxH] [--scale S] [--output WxH[@S]]... [--run COMMAND ...]

No window, no screen, no GPU and no input device: every output is a picture in memory drawn by pixman (software). It refuses
the desktop's own runtime folder and drops WAYLAND_DISPLAY, DISPLAY, HYPRLAND_INSTANCE_SIGNATURE and the desktop's session bus,
so nothing it starts can reach the running desktop. A control socket (`yggdrasil-compositor ctl COMMAND`) drives and reads it:

| Command | What it does |
| --- | --- |
| `outputs`, `resize NAME WxH`, `scale NAME S`, `power NAME on\|off` | the virtual outputs (one switched off by Settings or a program says `disabled`) |
| `add-output WxH[@S] [NAME]`, `remove-output NAME` | a screen plugged in (the next name, HEADLESS-N, or NAME as the kernel names a real one: eDP-1 is a laptop's own screen, listed first and main; placed as Settings' file says or to the right) and unplugged (its panels closed, its windows moved to the first screen; the last one stays) |
| `unplug NAME`, `replug NAME` | a screen's link goes and comes back, as the hardware backend sees a connector: the screen is kept 2 s with its windows and panels, and removed only if it has not come back by then |
| `main` | the main screen (`display::pick`: Settings' choice, else a laptop's own screen, else the top-left one); on every backend |
| `screenshot NAME FILE.png [pointer]` | the output's picture; with `pointer`, the pointer painted over it as the screen shows it |
| `pointer-look` | what the pointer looks like: `theme NAME SIZE SHAPE`, `surface WxH X,Y` (a program's own picture and its hotspot), `hidden resting` or `hidden program` |
| `windows` | every window: app id, title, place, size, workspace, states (active, floating/tiled, maximized, fullscreen, left, right, minimized) |
| `layers`, `free` | the layer-shell surfaces per output, with what each asks of the keyboard (`none`, `exclusive`, `on-demand`); the room they leave free for windows |
| `focus`, `workspace`, `lock` | what has the keyboard (`window APP TITLE`, `layer NAMESPACE OUTPUT`, `lock OUTPUT`, `none`; on every backend, and `YGGDRASIL_COMPOSITOR_DEBUG=1` logs each change); the workspace showing; the lock and its surfaces |
| `switcher` | the window switcher while it is open: one line per card, `*` (chosen) or `-`, the app ids of its windows, its place and size (empty when out of sight) |
| `frames` | `drawn N` (frames with a change on a screen since the start) and `passes N`: for tests that nothing is drawn while nothing changes |
| `key super+ctrl+2`, `key-down COMBO`, `key-up COMBO`, `type TEXT`, `pointer X Y`, `click [left\|right\|middle]` | input made inside the compositor (US layout); a key held down repeats its shortcut |
| `pointer-move DX DY`, `scroll DY [DX]`, `input` | the virtual mouse moved (at its speed in input.conf) and its wheel turned (scroll factor, natural scrolling); `click` is its buttons (left-handed: swapped); `input` gives the mouse and keyboard settings in force |
| `touch-down ID X Y`, `touch-move ID X Y`, `touch-up ID` | a finger (ID 0 to 15) on a touchscreen at X,Y of the touchscreen's screen (the laptop's own, else the main one), as wl_touch to the program under it (`src/touch.rs`) |
| `clock`, `clock stop`, `clock +MS`, `clock run` | the clock the animations run on: stopped and moved on by hand, a frame in the middle of an animation can be looked at |
| `quit` | end it |
| `clients`, `active`, `workspaces`, `workspace N`, `activate`/`close`/`minimize ID`, `maximize ID on\|off\|toggle`, `send ID N`, `place ID X Y W H`, `cursor`, `screens`, `screens all`, `layers`, `binds`, `reload`, `pointer-timeout [S]`, `zoom [FACTOR]`, `cursor-theme [NAME SIZE]`, `keyboard [LAYOUT [VARIANT]]`, `devices`, `modifiers`, `paste ctrl\|ctrl+shift`, `exit` | the desktop commands Yggdrasil's own tools use through yggdrasil-wm (`src/wmctl.rs`; below): nothing outside the compositor; the only key presses are `paste`'s Ctrl+V into the window with the keyboard (as Hyprland's sendshortcut), refused while locked |

Headless it also offers wlr-screencopy (grim, yggdrasil-shot, Sunshine's shared-memory path), ext-image-copy-capture with
ext-image-capture-source (single windows and screens: the panel's window previews) and wlr-output-power (yggdrasil-idle), served
from the pictures in memory, and wlr-output-management (wlr-randr, kanshi: the screens listed with their modes, and changed). The
standard window list, ext-foreign-toplevel-list, is offered nested too.

### The tests

| File | What it checks | Runs |
| --- | --- | --- |
| `src/*` unit tests (78) | options, sizes and scales, the hardware backend's (its options, a screen's make and model from its EDID, a refresh's length, Ctrl+Alt+Fn as virtual terminals; the laptop's: eDP, LVDS and DSI known as its own screen, the default scale from a screen's size, the lid's rule, the switches, a resume told from a clock set, input devices named as Hyprland names them and known by kind, a touchpad's and a TrackPoint's settings with the session's defaults, the pen's buttons, a touch on a turned screen landing under the finger (every turn), a colour matrix made into a gamma table for each colour, the lid and tablet-mode switches known from the start, a touchpad scroll stopping only the axis that stopped, pixels read back from the GPU turned into XRGB8888 and a copy drawn with OpenGL ES upright (Mesa's software OpenGL ES; skipped without it)), the refused runtime folder and session bus, shortcuts (the table held to the session's `yggdrasil.lua`: every bind, its words, release and repeat), tiling with the session's gaps, placement (the same cases as `keep-clear.lua`, its `open()` too; a window pulled back inside a screen that changed), the programs it starts taking SIGTERM, SIGINT and SIGHUP although its own thread blocks them, the look (the session's values, Hyprland's curve, opening, moving and closing, the corners' pixels), the keyboard files (the installer's, localectl's, vconsole.conf), Settings' monitors.lua (its lines, the last rule naming a screen winning, a mode matched within half a hertz, a turn), the window-places lines, the pointer (appearance.conf's `cursor`, XCURSOR_SIZE, painting it over a picture, the plain arrow), input.conf (a mouse's section over every mouse's, the defaults, buttons, scrolling, speed), the window switcher's grid (one centred row at the windows' shapes, rows that wrap and lower to fit, only the rows around the choice when too many, the arrows, a click's card, a picture fitted in its room) and a group card's title, the icons (a desktop entry's Icon and StartupWMClass, an SVG drawn at the size asked), X11 programs' scale (the screens' largest, never below 1, `stretch`; a new window's first geometry that Smithay rounded; Xft.dpi), two taps within half a second on a dark screen | `cargo test`, CI `rust` |
| `tests/headless.rs` (8) | outputs (size, scale, name, place), a bar and a window drawn where they belong, size and scale changes reaching programs, `quit`; SIGTERM, SIGINT and SIGHUP ending it with 0 and no socket left (each backend takes them through a signalfd since 2026-11-24; headless and nested left `wayland-N` behind before); ext-workspace: four workspaces in one group on both screens, the active one marked, an activate applied at the commit, a switch in the compositor told, a fifth made by the group's create and removed when left empty, 5 to 7 made by switching to 7 and kept while 7 holds a window, at most nine | `cargo test`, CI `rust` |
| `tests/x11.rs` (3) | X11 programs through a real XWayland (skipped where it is not installed): a window that waits to be told it has the focus (as Wine's do) is made active and given it; the pointer's theme and size are in XWayland's X resources (Xcursor.theme, Xcursor.size, beside Xft.dpi) and set again when `cursor-theme` changes; on a 1920x1200 screen at 125 % the X screen is 1920x1200 with Xft.dpi 120, and 1536x960 with 96 when xwayland.conf says `stretch = true` | `cargo test`, CI `rust` |
| `tests/behaviour.rs` (19) | with the tests' own small clients: the bar and dock reserve their room; windows open centred in the rest (one bigger than it made to fit, less the gap), kept clear of a dock that appears, back from maximize at a normal size, pulled back inside a screen that gets a smaller mode, then scale 2, or is unplugged (on another desktop too); Super+Up/Left/Down; full screen over the bar and dock; workspaces by keys; the keyboard following new windows, clicks and an exclusive panel, typing reaching the window; the keyboard back with the window when a panel that had it goes (unmapped, destroyed, its surface destroyed before its layer, its screen unplugged), and kept by a panel that has it while a window closes; a hidden window (minimized, on another workspace) asked to close gets the frames it closes in; the lock on every output taking every key, a second locker refused, a crashed one leaving the screen locked and a new one taking over; only the media keys' shortcuts working while locked; screen copies (whole, part, scale 2, with damage, while locked, a wrong buffer); grim's picture equal to the screen; screens off and on; idle and resume | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/tools.rs` (6) | yggdrasil-wm, the layer Yggdrasil's tools share, on it: found before a Hyprland named in the environment; windows (with their processes), screens, the free room, the pointer and the desktops read; desktops shown and a window sent to another; a window placed, snapped, maximized and back, minimized and closed; a tiled one floated where it is put; the pointer timeout set and put back; a title with a line break stays one line; the Task Manager's bring-to-front and close, the clipboard's paste into an editor and a terminal (none into the lock screen), Settings' focus by class, the shortcut list, reload, the pointer theme, the keyboard layout, layers; exit ends it | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/keys.rs` (10) | the session's shortcut table, with stand-ins as the compositor's only PATH (each writes down how it was called; nothing real runs): every key starts the command line it starts on Hyprland (`yggdrasil-panelctl notifications`, `loginctl lock-session`, `yggdrasil-shot` ...); Super and Ctrl tapped on their own (the start menu, Find my mouse) and not when another key comes between, and the key still reaching the program; held volume and magnifier keys repeating (600 ms, then 25 a second) and stopping when let go or another key is pressed, mute not repeating; Super+W, Alt+F4 and Super+Q closing (a keep-running program minimized), Super+Down minimizing a maximized window, Super+Shift+M showing the minimized windows and hiding them, one brought back staying; the shortcuts working while a panel has the keyboard (the start menu, a clicked pop-up), on the window active under it; the bindings listed with the session's words, the lists Settings and the panel's card make from them equal to those from `yggdrasil.lua`; Alt+Tab (docs/COMPOSITOR.md, "Window switching"): the cards most recent first, Tab and Shift+Tab round the ends, Alt+Shift+Tab on the oldest, Escape, the arrows and Enter, switching when Alt is let go, a minimized window brought back, a click on a card and outside the panel, snapped windows as one group card brought forward together (snapshot), nothing drawn while open over still windows or once closed, a window's new picture on its card | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/look.rs` (4, and 1 that only measures) | the window look against PNG snapshots (`tests/snapshots`) and numbers worked out from the session's values: the border (the accent at 80 % in front, faint behind, in the dark and the light style, another accent read again), the corners cut round, the shadow's fall-off, a front window's shadow over the one behind; maximized and tiled windows with the gaps, full screen with none of it; a window opening (40 ms in), sliding to maximized (60 ms in, its picture stretched) and closing (50 ms in) on a stopped clock; "reduce motion" | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/outputs.rs` (9) | the keyboard layout from the system's file as the installer writes it (in the test's own etc folder): `keyboard` and the keymap programs get (German, then French when the file changes, the installer's try-out kept until it does), vconsole.conf when there is no X11 file, US when neither, xkb's default for a layout it cannot load; Settings > Displays' monitors.lua in Settings' own form (a mode, a scale, a place left of the first screen, a screen off; read back through yggdrasil-wm's `screens_all` with modes and rates, changed and reloaded as Settings does, back to the command line's when the file goes); wlr-output-management (heads, modes, a custom mode and scale applied, a screen off with its bar closed and its window moved, a stale configuration cancelled, all off refused, a test changing nothing, back on to the right); screens plugged in and out (named, placed, offered to programs; the windows moved off at their place, a maximized one maximized again, snapshot; a screen plugged in while locked all black); a laptop's own screen (a fake eDP-1 beside a fake HDMI-A-1) listed first, at the left and main, the monitor main while the lid is closed and the laptop's screen main again when it opens, Settings' choice first; a screen whose link bounces three times kept with its windows and panels, and one gone for good removed after 2 s; a screen turned a quarter (`transform`) with its bar told the new width, a maximized window refitting upright and programs told the turn, turned back through wlr-output-management, a turn that is not one refused, and the turn kept when Settings' file is read again; a screen's colour matrix through hyprland-ctm-control-v1 (Night Light's hyprsunset), set on commit, a second program blocked, the tint taken off when the program goes, a negative value refused | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/places.rs` (5) | a program's window back where its last one closed (the session's file and lines), not for a second window, a maximized one or one under 120 x 80; places the Hyprland session kept used, kept clear of the bar, skipped when off every screen; a kept place too big for the screen made to fit it less the gap, a dialog of the same process with another class neither given a kept place nor kept, a window bigger than the screen made to fit; windows opened on a screen in `~/.config/yggdrasil/tiling` tiled with the session's gaps (snapshot), the others taking the room of one that closes, new ones floating once the screen no longer tiles; Steam's own maximize request ignored, Super+Up still maximizing it | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/fullscreen.rs` (9) | a full screen window and the windows over it, each check at once on the pixels at a probe point, the surface a click reaches, the surface a key reaches and the window told it is active: a window opened over it, switching by Alt+Tab, clicks and `activate`, minimize and restore, closing, leaving and asking for full screen in front and behind, a menu and a dialog, two screens (each its own bar), the dock growing, the screen getting smaller, a hidden window's resize grips, the lock while windows open and close behind it, a finger on the touchscreen in the same order as the pointer (docs/COMPOSITOR.md, "Full screen and the windows over it") | `cargo test`, CI `rust` |
| `tests/capture.rs` (3) | the window list and window pictures the way the panel takes them (ext-foreign-toplevel-list, ext-image-capture-source, ext-image-copy-capture): windows listed with title and app id, a new title followed, a closed one said closed; a window covered by another copied whole and exactly (both formats, snapshot), the next copy waiting until the window changes, a wrong buffer size failing, a minimized window still copied, a closed one's session stopped; at scale 2 the copy in the screen's pixels, a window that grows telling its new size; no window copied while locked (a screen's copy shows the lock); a screen's copy equal to its screenshot, a screen unplugged stopping its session | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/pointer.rs` (5) | the pointer drawn by the compositor, with themes of the test's own: the theme in appearance.conf at XCURSOR_SIZE (snapshot), the theme's 48 picture on a scale 2 screen, `ygg-theme cursor`'s change through yggdrasil-wm at once, the plain arrow for a theme that is not installed, grim -c with the pointer and grim without; a program's own cursor surface with its hotspot (snapshot), a shape by name (cursor-shape-v1), hidden by the program, the default arrow over nothing, ext-image-copy-capture's paint_cursors; hidden 0.25 s after the last motion while yggdrasil-idle's pointer timeout is set through yggdrasil-wm, shown again by motion, never hidden at 0; moved to the middle of the first screen when its screen is unplugged (snapshot), kept inside a screen that gets smaller, left alone when one is plugged in; Yggdrasil's own theme as ygg-cursors draws it (snapshot; when `YGGDRASIL_TEST_CURSORS` names it) | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/laptop.rs` (2) | the touchscreen: the seat gets touch with the first touch, a finger down on a window lands at its place on the window (and gives it the keyboard), moves and lifts, each change a frame; with the screen off (idle's power off) one touch leaves it off and reaches no program, taps too far apart too, two within half a second turn it on, and touch reaches the window again | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/input.rs` (2) | Settings > Peripherals' input.conf: taken at start (programs told the key repeat; the virtual mouse at half speed, left-handed, scrolling naturally at twice the distance, from every mouse's settings and its own section), the pointer kept on the screens, the devices listed through yggdrasil-wm as Settings lists them, a change applied through yggdrasil-wm's `apply_input` at once, the defaults when the file goes; a game's relative motion (with and without the mouse's acceleration), its pointer lock (the pointer stays, the motion goes on) and confinement (kept inside its window), each let go again (constraints.rs) | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/apps.rs` (9) | Yggdrasil's real programs inside it: the panel (bar, dock, wallpaper drawn; the free room; the terminal stand-in centred and maximized into it; a notification toast that leaves the keyboard alone; the start menu taking the keyboard and giving it back; a smaller screen), the dock's window list (resting on the window's icon opens it, and its card shows the window's picture, red left of green as the window), the panel's desktop buttons ("+" makes and shows desktop 2, desktop 1's button goes back) and window zones (Super+Shift+Z's sheet for the window in front, a click on the left zone puts it there), `yggdrasil-snap` (the four halves of the free room, maximize and back, minimize) and `yggdrasil-close` (a keep-running program minimized, any other asked to close), `yggdrasil-logout` with stand-ins as its only PATH (every window asked; all closed: the session ends; one stays: the screen locks and the session goes on), Settings (egui drawn by Mesa in software: centred at 1060x720, drawn, Super+Up and back, Super+Q ends it, SIGTERM ends it with 0 and its socket gone), the lock screen in its test-auth build (two outputs, one at 1.5, nothing of the session shows, a wrong password keeps it locked, the right one unlocks and the keyboard goes back) | CI `compositor-headless`; on a desktop `tests/apps-podman.sh` |

Every test starts its own compositor in a private folder, has a time limit (the test program ends past it), and every process it
starts dies with it. The programs get an empty environment, the compositor's private runtime folder and home, and a session bus of
the test's own without service activation. The lock is only ever the `--features test-auth` build, in `target/test-auth`; a build
that would check the password with PAM is refused. `tests/apps-podman.sh` builds everything (and draws Yggdrasil's cursor theme
with ygg-cursors into `target/test-cursors`) and runs the test files in a rootless podman container with no devices and no
network; `YGGDRASIL_TEST_SHOTS=FOLDER` keeps the panel's screenshot of the dock's window list there to look at.

Found by these tests and fixed: the lock screen's debug build ended right after locking at 1280x720 (its panel edge sat on a
fractional pixel and tripped tiny-skia's anti-aliasing); the compositor let a second locker take over a running lock; the keyboard
stayed on the lock's surface after unlocking.

## Yggdrasil's tools on it: yggdrasil-wm (2026-11-12)

`packages/wm` is the layer Yggdrasil's own tools share to ask the compositor things and tell it to do them: the windows, the
desktops, the screens and the free room, the pointer, and showing desktops, sending, placing, maximizing, minimizing and closing
windows. It picks the backend at run time: this compositor when its control socket for the program's own Wayland display exists
(`$XDG_RUNTIME_DIR/yggdrasil-compositor-$WAYLAND_DISPLAY.ctl`), else Hyprland's socket. Every exchange has a time limit (2 s;
200 ms for what the zones ask while a window is dragged) and a size limit (4 MB from Hyprland, 1 MB from this compositor), and a
socket is spoken to only when it is a socket owned by this user. On this compositor the panel's window list, activate and minimize
stay on wlr-foreign-toplevel (the standard protocol), and its window previews on ext-image-copy-capture as on Hyprland (item 6);
the rest goes through the desktop commands above.

Moved to it: the panel's desktop buttons (reading, showing, adding and removing desktops), window zones and Snap Assist, the dock's
minimize and restore (Hyprland's hidden "minimized" workspace), the pointer finder, the focused screen, the screens' layout, the
shortcuts card and Hyprland's config-error toast; `yggdrasil-snap`, `yggdrasil-close` and `yggdrasil-logout` (now Rust programs in
that crate, built by the session package); `yggdrasil-idle`'s pointer hiding; the Task Manager's windows, close and bring-to-front;
the clipboard's paste; Settings' Displays, reload, shortcuts list and pop-out focus; the game fixes' screen size; the pointer
theme (`ygg-theme cursor`); the browser warm-up's window check; the portal's window in front; the screenshot tool's windows and
the splash's wait for the desktop; `yggdrasil-xmove` (Hyprland-only, through the layer's bounded raw request); the installer's
keyboard try-out. The Task Manager and xmove keep sending the exact requests they wrote to the socket themselves.

Proof that nothing changes on Hyprland: `packages/wm/tests/hyprland.rs` answers from Hyprland 0.57.1's recorded replies (every key,
its order and the formatting as recorded with read-only `hyprctl -j` queries; titles, classes, addresses and screens made up) on a
stand-in socket that writes down each request: every tool sends the bytes hyprctl sent (`wire.txt`, recorded from hyprctl against a
stand-in socket in a sandbox), in the same order. The old shell scripts and the new snap and close programs were also run side by
side against one stand-in in a sandbox (the scripts with the real hyprctl): every side, with and without tiling screens and a
keep-running list, the same requests and exit codes (the scripts looked at the screens twice, the programs once); so were the old
log-out script and the clipboard's paste line (all windows closed or one left; an editor and a terminal): the same requests, calls
of the system's programs, exit codes and files. The parity harnesses of the game fixes (466 cases) and of ygg-theme (766) give the
Rust side a stand-in Hyprland socket that writes the same call down as the hyprctl stand-in: all the same as the Python scripts.
Proof on this compositor: `tests/tools.rs` and `tests/apps.rs` above.

## What it matches of Yggdrasil's Hyprland session

The session's behaviour is set in `packages/session/hyprland/yggdrasil.lua` (with `keep-clear.lua` and the minimize plugin).

**Matched, and tested headless:**
- Windows float and open centred, kept clear of the bar and the dock (`float, center` plus keep-clear): here they open centred in
  the free room, one bigger than that room made as big as it less the outer gap and centred (keep-clear's `open`, item 9), moved
  out from under a bar or dock that appears, and come back from maximize at three
  quarters of the room when they were nearly as big as it. (Hyprland centres on the whole monitor and keep-clear moves the
  window after; the result differs by at most half the bar's or dock's height.)
- The panel as layer-shell surfaces: bar on Top reserving its height, dock on Top reserving its height and gap, wallpaper on
  Background, menus with exclusive zone 0 under the bar, toasts on Overlay without the keyboard.
- The session's whole shortcut table (2026-11-12, below): the same keys, words and command lines; Super+Up maximize and restore,
  Super+Left/Right halves, Super+Down minimize (the session's `yggdrasil-snap`), Super+Q, Super+W and Alt+F4 close (a
  keep-running program minimized, as `yggdrasil-close`), maximize into the free room less the gap, full screen covering the bar
  and the dock.
- The window look (2026-11-12, below): border, rounded corners, shadow, gaps, the open, close and move animations, "reduce motion".
- Workspaces: Super+Ctrl+1..9, Super+Ctrl+Left/Right, Super+Ctrl+Shift+1..9 sending the window. 1 to 4 are always there; 5 to 9
  are made when first shown or a window is sent there (or a pager asks, ext-workspace's create) and go again when they are
  empty and left, from the last one down, so the numbers of the others stay.
- Alt+Tab (2026-11-19, docs/COMPOSITOR.md "Window switching"): live window cards in wrapping rows, group cards for snapped and
  tiled windows; Hyprland's session only cycles, so this is more than it has.
- Click to focus (`follow_mouse = 2`), a new window takes the keyboard, a panel asking for the keyboard exclusively (the start
  menu) gets it at once and gives it back to the window when it closes (the session's `layer.closed` rule). A panel keeps the
  keyboard only while it is there (alive, mapped, on a screen, asking for it), and the compositor's shortcuts come before a
  panel's keyboard, acting on the window active under it (since 2026-11-19: on the laptop a panel pop-up that went kept the
  keyboard, and Super+Q did nothing).
- The lock (ext-session-lock): every output covered, every key to the lock, no shortcut while locked but the media keys'
  (Hyprland's `locked` binds), a second locker refused, a crashed locker leaves the screen locked and a new one may take over
  (`allow_session_lock_restore`).
- Screenshots and streaming (wlr-screencopy, shared memory), screens off and on (wlr-output-power), idle (ext-idle-notify),
  several outputs side by side at fractional scales.
- The dock's window previews: ext-foreign-toplevel-list, ext-image-capture-source and ext-image-copy-capture (item 6).
- Screen sharing for programs (2026-11-24, [packages/screencast](../screencast/README.md)): xdg-desktop-portal's ScreenCast
  answered by yggdrasil-portal; the share picker asks which screens, and yggdrasil-screencast publishes each as a PipeWire stream
  through ext-image-copy-capture. The own session routes it there by its desktop name (`Yggdrasil-Compositor`); Hyprland's
  session keeps Hyprland's portal. Whole screens, shared memory. Screenshots and picked colours for programs (the portal's
  Screenshot, [packages/portal](../portal/README.md)): grim, `yggdrasil-shot --portal` and `yggdrasil-colorpicker --portal`.
- The pointer drawn with the cursor theme, a program's own pointer, hidden while the screen is dimmed (item 7).
- Yggdrasil's tools through yggdrasil-wm (above): the panel's desktop buttons and window zones, `yggdrasil-snap`, `yggdrasil-close`,
  the idle manager's pointer timeout, `ygg-theme cursor`'s pointer theme.
- The system's keyboard layout, Settings > Displays' `monitors.lua` (modes, scales, places, screens off), screens that come and
  go, remembered window places and the screens set to tile, Steam's maximize request ignored (items 3 and 5, 2026-11-12, below).

**Built but only tried nested (not in the headless tests):** XWayland and games (opaque games, `_NET_WM_STATE` before mapping,
Wine's WM_STATE), Super+T and tiling workspaces, window controls for server-side decoration, move and resize
by the program's own title bar, Super+drag, xdg-activation, idle inhibit, clipboard and primary selection.

**X11 programs at the screen's resolution** (2026-11-22): XWayland's client scale is the largest scale of the screens that are
on, so XWayland is told each screen's size in pixels: on a 1920x1200 screen at 125% a game is offered 1920x1200 and drawn sharp,
not 1536x960 stretched (KDE's "apply scaling themselves", Hyprland's `force_zero_scaling`). Xft.dpi is set to match (120 at 125%),
so GTK, Qt and Chromium-based programs (Steam's window) keep the desktop's size; a program that reads none of it draws smaller.
`stretch = true` in `~/.config/yggdrasil/xwayland.conf` keeps the old way. Tried headless with XWayland on (`--output
1920x1200@1.25`, `YGGDRASIL_COMPOSITOR_XWAYLAND=1`): `xrandr` gave 1536x960 before and 1920x1200 after, `xprop -root
RESOURCE_MANAGER` shows Xft.dpi 120, and a GTK window under X11 drew at its place and size.

**The gap to Hyprland, item by item** (what each item has, and what it still lacks):
1. **What is left of "our tools call hyprctl"** (every tool moved to yggdrasil-wm on 2026-11-12, above). Hyprland's own on purpose:
   hyprsunset's night light (its own socket; this compositor has no gamma control yet, item 11), the Hyprland plugin builder
   (`yggdrasil-hypr-plugin`), the config-error toast's event socket and
   xmove (both do nothing elsewhere), and Settings' own Hyprland file and night light. Settings > Displays lists and changes this
   compositor's screens (it writes `monitors.lua` and reloads through yggdrasil-wm, as on Hyprland; the compositor takes the file
   on reload, item 3). Other programs see its workspaces through ext-workspace (one group on every screen, workspaces 1 to 4 and
   those made past them, each can be activated, and a new one can be asked for; since 2026-11-24) and its windows through ext-foreign-toplevel-list (item 6). Missing: the control
   socket on the real session (today only headless).
2. ~~The shortcut table~~, closed 2026-11-12: the built-in table is the session's (`src/shortcuts.rs`; a unit test reads
   `yggdrasil.lua` and fails on any bind missing on either side), with its words and the same command lines (Super+N, Super+S
   and Ctrl+Alt+S, Super+Shift+Z, Super+/ and the Ctrl taps through `yggdrasil-panelctl`; Print and Super+Shift+S, Super+Shift+C,
   Ctrl+Shift+Esc, the magnifier, the screen reader, `loginctl lock-session`, XF86AudioPause), the window keys as
   `yggdrasil-close` and `yggdrasil-snap` do them (Super+W; the keep-running list now in yggdrasil-conf; Super+Down minimizes),
   Super+Shift+M showing the minimized windows (as Hyprland's special workspace), taps on a modifier (release binds that do not
   take the key: Super for the start menu, Ctrl for Find my mouse), repeat while held for volume, brightness and the magnifier at
   Hyprland's 600 ms and 25 a second (also what programs are told now; was 300 ms and 40). The control socket's `binds` gives
   each one's kind and the mouse's two binds, and yggdrasil-wm reads the release flag. Headless, programs it starts get no system
   bus (Super+L's `loginctl` would otherwise lock the running desktop). The switcher is this compositor's own (Hyprland only
   cycles).
   - **The magnifier** (2026-11-24): Super+Alt+= / - / 0 run `yggdrasil-zoom`, which steps this compositor's `zoom` through
     yggdrasil-wm (as Settings > Accessibility > Zoom does), 1 to 8 in halves. The screen under the pointer is drawn that many
     times larger about the pointer, which stays on the same point: every placement of a frame goes through one camera
     (`look::Camera`), so pictures, title bars, borders, rounded clips and layers stay together, and programs' pictures are
     stretched about their zoomed corner (`look::Clipped`, as an animation stretches a window). Off, the frame is placed
     exactly as before (the pixel snapshots of `tests/look.rs` are unchanged). Not zoomed: the window switcher and the
     profile question's cards, a full screen game drawn opaque on the GPU, and the pointer itself. `tests/look.rs`
     (`the_magnifier_draws_the_screen_larger_about_the_pointer`) and `tests/tools.rs` (through yggdrasil-wm).
3. ~~Input and outputs from the system's settings~~, closed 2026-11-12:
   - **The keyboard layout** (`src/keymap.rs`) from `/etc/X11/xorg.conf.d/00-keyboard.conf`, read as `yggdrasil.lua` reads it
     (`XkbLayout`, `XkbVariant`, `XkbOptions`; US when there is none), and when that file is missing from `/etc/vconsole.conf`'s
     `XKBLAYOUT`/`XKBVARIANT`/`XKBOPTIONS` (the installer writes both alike; Hyprland's config looks only at the first). Taken
     again when either file changes; a layout set from outside (the installer's try-out) stays until then. A layout xkb cannot
     load falls back to xkb's default, with a line in the log.
   - **Settings > Displays** (`src/outputs.rs`): the same `~/.config/yggdrasil/monitors.lua` Hyprland loads, its
     `hl.monitor({ output, mode, position, scale })` and `disabled = true` lines (the last naming a screen wins, `output = ""` is
     for every screen); a screen it does not name keeps its own mode and scale (headless: the command line's) and goes to the
     right of the others. Read at start, on `reload` and the 2-second look when it changed, and for a screen that appears. The
     control socket's `screens all` gives yggdrasil-wm's `screens_all` every screen with its rate and modes (Hyprland's
     `2560x1440@59.95Hz` form) and the ones that are off.
   - **wlr-output-management** version 4 (`src/outputmgmt.rs`): heads with their modes, current mode, place and scale; a
     configuration's modes (custom ones too, headless), scales, places and screens off are applied, tested, refused (every screen
     off; a turn or variable refresh) or cancelled (made for an older state), and every holder is told the change.
   - **Screens that come and go** (headless `add-output`, `remove-output`; a screen switched off is the same): its wl_output goes,
     its panels are told `closed`, its lock surface is dropped (a screen plugged in while locked shows black until the locker
     covers it); windows on it move to the first screen at their place on it (maximized, full screen and snapped ones fitted to it
     again, floating ones kept clear of its bar), and windows on a screen that moves go with it.
   - Along the way: new windows open on the screen the pointer is on (Hyprland's focused monitor), and maximize, full screen,
     Super+Left/Right and tiling work on the window's own screen (they were all on the first screen).
   Left: turning (transform) and variable refresh are not offered; on the
   hardware backend the modes will be the connectors' own (no custom modes).
4. ~~The window look~~, closed 2026-11-12 (`src/look.rs`), with the session's values: the 1-pixel border in the accent colour at
   80 % (Yggdrasil's purple when none is chosen) and a faint one behind (white 8 % dark, black 9 % light), corners rounded at 10,
   the shadow (18, power 3, black 27 %), gaps 8 around maximized and tiled windows and 4 + 4 between tiled ones (Hyprland's
   maximize keeps the gaps: a maximized window is now the free room less 9 all round), none of it in full screen; windows open
   growing from their middle and fading in, close shrinking and fading out (from their last picture on the screen, headless
   only), slide to a new place and size (maximize, snapping, tiling, keep-clear, the tools' `place`), 800 ms on Hyprland's
   default curve, none with "reduce motion" or while dragged. Accent, style and reduce motion are read again with the 2-second
   look at the config files. Drawn in software too: corner and shadow pictures made once per scale and stretched along the
   edges, the window's own picture clipped by damage regions with the curve's pixels at their share. Measured on a release
   build (1920x1080, a bar, a dock, three 800x600 windows; `tests/look.rs idle_cost`), before and after: idle 0.004 % and
   0.005 % of a core (350 and 550 µs in 10 s), 25.6 and 26.3 MB resident (22.4 and 23.2 MB proportional), an 800x600 window
   drawn anew 0.76-0.87 and 0.86-0.98 ms a frame; a window opening costs 90-100 ms of CPU over its 800 ms, closing 40-47 ms,
   and nothing is drawn after. Not done: the workspace slide, the border's colour fading, layers' animations, dimming behind
   the minimized windows on show; nested (GPU) there is no closing animation and games are neither rounded nor stretched.
5. ~~Remembered places and the tiling screens~~, closed 2026-11-12:
   - **Window places** (`src/places.rs`): when a program's last window closes as a plain floating window (not tiled, maximized,
     full screen or minimized) of at least 120 x 80, its place and size go into the session's own
     `~/.local/state/yggdrasil/window-places` in the session's lines (`class<TAB>x<TAB>y<TAB>w<TAB>h`, written next to it and
     renamed over it), and its next window opens there when no other window of it is open (of its class or its process, item 9)
     and the middle of that spot is on a screen, kept clear of the bar and the dock and no bigger than that screen now. A place
     kept on Hyprland is used here and the other way round.
   - **Tiling screens**: a window that opens on a screen named in `~/.config/yggdrasil/tiling` (Settings > Displays) tiles there
     with the session's gaps, as the session's `tile-on-SCREEN` rules; decided when the window opens, as Hyprland applies the
     rule, so a change to the file is for windows opened after it. The per-workspace profiles of `workspaces.conf` stay as well.
   - **Steam's own maximize request** (Wayland or X11, and `_NET_WM_STATE` asked before its window shows) is ignored, as the
     session's `steam-no-self-maximize`; Super+Up still maximizes it.
   Measured for items 3 and 5 together on release builds (the same `tests/look.rs idle_cost` set: 1920x1080, a bar, a dock,
   three 800x600 windows), two runs each, before and after: idle 0.005-0.006 % and 0.006 % of a core (540-611 and 612-613 µs in
   10 s; the 2-second look now also checks four more files' times), 26.2-26.3 and 26.4-26.5 MB resident (22.8 and 22.9-23.0 MB
   proportional), an 800x600 window drawn anew 808-810 and 789-849 µs a frame.
   Measured for items 3 and 5 together, as for item 4 (release build, `tests/look.rs idle_cost`, two runs each, the build before
   and after side by side): idle 530-611 and 580-622 µs of CPU in 10 s (0.005-0.006 % and 0.006 % of a core), 26.3 and
   26.4-26.5 MB resident (22.7-22.8 and 22.9-23.0 MB proportional); an 800x600 window drawn anew 0.77-0.86 and 0.86-1.02 ms a
   frame (within the runs' spread). The 2-second look reads four more file times (the keyboard's two, the tiling file,
   monitors.lua); nothing is read or drawn while nothing changes.
6. ~~The panel's window previews~~, closed 2026-11-12 (`src/capture.rs`, `src/toplevels.rs`). The panel takes its previews through
   the standard protocols on Hyprland and here alike (`packages/panel/src/capture.rs`, unchanged), so the protocols were added
   here, and nothing in the panel or yggdrasil-wm changed:
   - **ext-foreign-toplevel-list** (Smithay's): every window, shown, minimized or on another desktop, with its title and app id,
     followed after every commit and frame, and closed when it closes. Nested too.
   - **ext-image-capture-source**: a window by its list handle, or a screen by its wl_output.
   - **ext-image-copy-capture** (headless): a session tells the size in the screen's pixels (a window's size at its screen's
     scale) and ARGB8888 and XRGB8888 in shared memory; the first copy of a session is made at once, a later one when the window
     (any commit in its surfaces) or the screen (drawn with damage, or the pointer moved for `paint_cursors`) changed; a source
     that changed size fails the copy with `buffer_constraints` and tells the new size; a window that closes or a screen that
     goes stops its sessions. A window is drawn on its own, covered or not, on another desktop or minimized (its last picture),
     without the border and shadow, transparent where it draws nothing; nothing of a window is copied while the session is
     locked (a screen's copy then shows the lock, as wlr-screencopy's). `paint_cursors` paints the pointer into a screen's copy.
   Proof: `tests/capture.rs` with the tests' own client, and the panel itself in `tests/apps.rs` (its card shows the window).
   Measured (release build, the `idle_cost` set): an 800x600 window's picture costs 1.1-1.2 ms of the compositor's CPU (the
   panel asks about once a second per window while its card is open); afterwards it is about 4 MB larger (34.3 from 30.3-30.5 MB
   resident; it stays at about 36 MB after 50 and after 200 pictures: the allocator keeps the freed picture for the next one).
   Left: GPU buffers (dmabuf) with the hardware backend, and the nested window (no read-back from its GPU yet); a session for the
   pointer's own picture (create_pointer_cursor_session) stops at once.
7. ~~The pointer~~, closed 2026-11-12 (`src/pointer.rs`):
   - **The theme** as `ygg-theme cursor` stores it: set on the running compositor (`cursor-theme NAME SIZE` through yggdrasil-wm,
     taken at once), else appearance.conf's `cursor` when that theme is installed (read again when the file changes), else
     Yggdrasil's own; the size from XCURSOR_SIZE (24 when unset) times the screen's scale, the theme's nearest size used. Looked
     up as libXcursor does (~/.local/share/icons, ~/.icons, the data folders, `Inherits`), with the xcursor crate; a plain arrow
     when no theme is installed. Each shape at each size is read once.
   - **What it shows**: the shape a program names (cursor-shape-v1, with its other names and then the default arrow), the
     program's own cursor surface (shared memory, its hotspot, stretched when drawn for another scale; frame callbacks sent), or
     nothing when a program hides it; the default arrow over no program. Animated cursors show their first picture.
   - **Hidden while dimmed**: after `pointer-timeout` seconds without motion (yggdrasil-idle sets 0.25 while it dims, through
     yggdrasil-wm, as Hyprland's `cursor:inactive_timeout`); the next motion shows it.
   - **Screens that go**: a pointer left on no screen goes to the middle of the first screen when its own was unplugged or
     switched off, else to the nearest point of the nearest screen (a screen that got smaller). Not counted as input (idle stays).
   - **Drawn** headless over a screen's picture as a hardware cursor plane would be: `screenshot NAME FILE pointer`, grim -c
     (wlr-screencopy's overlay_cursor) and `paint_cursors` copies show it; every other copy is the screen without it, as before.
   Proof: `tests/pointer.rs` (snapshots of the test's themes, a program's pointer, the pointer after its screen went, and
   Yggdrasil's own arrow as ygg-cursors draws it). Left: nested, the host window still draws the pointer (a program's own
   picture shows as the default arrow there); the hardware backend puts it on the cursor plane and wakes a frame when it hides.
   Measured for items 6 and 7 (release build, `tests/look.rs idle_cost`, the builds before and after side by side, five runs
   in all): idle 547-572 and 560-621 µs of CPU in 10 s (0.005-0.006 % and 0.006 % of a core; one run after at 733 µs on a busy
   machine), 26.6-26.7 and 26.5-26.8 MB resident (23.3-23.5 and 23.3-23.6 MB proportional), an 800x600 window drawn anew
   688-828 and 693-821 µs a frame. Nothing is drawn for the pointer while it rests: it is painted only into what asks for it.
8. ~~Settings > Peripherals' mouse and keyboard settings~~, closed 2026-11-12 for the headless backend (`src/input.rs`): Settings
   writes `~/.config/yggdrasil/input.conf` (its form and parser are yggdrasil-wm's `input.rs`, the file the Hyprland session
   reads); the compositor reads it with that parser at start, on `reload` and with the 2-second look when it changed, and
   yggdrasil-wm's `apply_input` asks for a reload here after Settings saved it (it said "not yet" before). Key repeat goes to
   every program at once (wl_keyboard.repeat_info) and to the compositor's own held shortcuts. The mouse settings (speed,
   acceleration profile, scroll factor, natural scrolling, left-handed), every mouse's and per mouse in `[name]` sections, apply
   to the headless virtual mouse `yggdrasil-headless-mouse` (`pointer-move`, `scroll`, `click`): its speed as libinput's flat
   profile works it out (times 1 + sensitivity), the scroll factor and natural scrolling as Hyprland applies them, left and right
   buttons swapped when left-handed. The control socket's `devices` lists the mice and keyboards, which yggdrasil-wm's
   `input_devices` reads for Settings' "Each mouse". Proof: `tests/input.rs` (the file at start, the client told the repeat,
   half speed, swapped buttons, natural scrolling at twice the distance, the pointer kept on the screens; Settings' change
   applied through yggdrasil-wm, the defaults when the file goes) and a unit test. Measured on the `idle_cost` set: idle 592-654
   µs in 10 s (633 for the build before, run between them on a busy machine), 26.5-26.7 MB resident; one more file time in the
   2-second look. Left: the adaptive profile's curve is libinput's own (headless an adaptive mouse moves as a flat one; on the
   hardware backend libinput is told the speed and profile per device, item 11); Settings still shows its old "not yet" note
   on this compositor and hides "Each mouse" there (`packages/settings/src/peripherals.rs`, `input::NOT_YET`), for Settings to
   drop.
9. ~~No floating window opens bigger than its screen~~, closed 2026-11-13, as the Hyprland session since d058852
   (`src/placement.rs`'s `open`, the same function and cases as `keep-clear.lua`'s `open()`, with `packages/session/tests/
   keep-clear.lua` and `window-open.lua` on the Lua side): a window (dialogs and pop-ups too) that opens bigger than the room
   the bar and the dock leave free is made as big as that room less the outer gap (the look's gaps_out, 8) and centred in it;
   one that fits keeps its size; a remembered place is put back no bigger than its screen is now (the scale may have changed
   since); and for the window places a window of the same process counts as the same program, so a dialog with a class of its
   own (VS Code's "code" beside "com.microsoft.VSCode") is neither given a kept place nor kept. Proof: the unit test with the Lua
   cases (the laptop at 1.5 and 2, portrait, a bare screen, no gap), `tests/places.rs` and `tests/behaviour.rs`. Left: the
   session checks again 150 ms and 1.2 s after a window opens (Electron sets its saved size late); here a window is placed
   once, at its first picture, and a size it sets for itself later is left as it is.
10. **The session:** `yggdrasil-session` starting it instead of Hyprland (the environment to systemd and D-Bus, the autostart
   target, restarts after a crash), and a log kept across a crash. A first step, 2026-11-13: the launcher is Rust now, and
   `packages/session/tests/nested_session.rs` starts it with this compositor, headless, in Hyprland's place (a stand-in
   "Hyprland" on the PATH runs it), with the autostart line of yggdrasil.lua as its `--run` program and stand-ins for systemd and
   D-Bus: the environment arrives with this compositor's display, the session target starts and stops, a compositor killed
   under the session is started again. Left: the launcher choosing this compositor itself, and the autostart without Hyprland's
   Lua.
11. **The hardware backend** (DRM/KMS), developed in a virtual machine, never on the live session. Milestone 1 done 2026-11-15
   (above, `src/udev.rs`): libseat (logind and seatd), udev GPUs and connectors, GBM/EGL with OpenGL ES or pixman on dumb
   buffers, a DrmCompositor per screen paced by the page flip, libinput, virtual terminals, a clean exit; the panel, Settings and
   foot drawn and used in the VM.
12. **The bake-off:** the same games and the CAD model measured against Hyprland (`tools/bakeoff`), and only then a decision.

## NVIDIA (2026-11-23)

| Part | How | Checked |
| --- | --- | --- |
| Waiting for programs' GPUs | A commit with a GPU buffer is applied only once the buffer is drawn (src/sync.rs): through explicit sync, linux-drm-syncobj-v1 (the program's acquire point on a DRM timeline; the release point is signalled when the compositor lets go of the buffer), offered when the render GPU can wait on a timeline through an eventfd; otherwise through the buffer's own fences (implicit sync). NVIDIA's driver needs the explicit kind: without either, frames can be shown half drawn or out of order | nested on my desktop's RTX 2080 Ti (driver 620.73): the compositor offered the protocol on the render node, and vkcube (Vulkan) and eglgears_wayland (EGL) set an acquire and a release point on all 1,054 of their frames in 8 s; glxgears through XWayland 24.1 on all 427 of its frames (72 frames a second, the window's pace); no errors |
| Which GPU draws | The one a screen is connected to: the boot GPU when it has one, else the first GPU with a screen (a desktop with two cards may boot from the one without screens), else the boot GPU; `YGGDRASIL_COMPOSITOR_GPU=card1` (or a path) chooses; the choice and why are printed at the start | unit tests (`the_gpu_with_the_screens_draws_and_the_variable_wins`, the connectors read from sysfs); one GPU on my desktop, so the two-card case is not yet tried on hardware |
| GPU memory | NVIDIA's driver keeps memory a Wayland compositor frees in its own pool (about 1 GB, as other compositors found) unless the application profile `GLVidHeapReuseRatio=0` applies; the package and `session/install-session.sh` install `session/nvidia-application-profile.json` as `/etc/nvidia/nvidia-application-profiles-rc.d/50-yggdrasil-compositor` | not yet measured on hardware |

`YGGDRASIL_COMPOSITOR_EXPLICIT_SYNC=0` leaves explicit sync out, for a driver that gets it wrong (the implicit wait stays).

## Games on the hardware backend (2026-11-23)

| Part | How | Checked |
| --- | --- | --- |
| Variable refresh (VRR) | Each screen learns at setup whether its CRTC can switch VRR without a modeset (the monitor's `vrr_capable`); before each frame VRR follows `YGGDRASIL_COMPOSITOR_VRR`: 2, the default, while a full screen window covers the screen (as the Hyprland session's `vrr = 2`), 1 always, 0 never. A screen that needs a modeset for it keeps a fixed rate, so a game going full screen never blanks it. The switch is logged | unit test of the rule; my desktop's ultrawide reports `vrr_capable` 1 (modetest); not yet seen switching with a game |
| Direct scanout | Each GBM screen has two dmabuf feedbacks: the renderer's, and one with the primary and overlay planes' formats (those the renderer can also read) as a scanout tranche first. After each frame a window the screen showed directly, or tried to and could not for the buffer's format, is sent the scanout feedback (Smithay's `select_dmabuf_feedback`), so a full screen game allocates buffers the screen can show without a copy. The log says when a screen's picture starts and stops going out directly | builds and the headless tests; not yet seen with a game |

Not possible yet: tearing for games that ask for it (wp-tearing-control): Smithay 0.7's DRM compositor has no asynchronous page
flips.
On the hardware backend (`--drm`) the NVIDIA card has not been tried yet: `session/install-session.sh --entry-only` adds the
login entry without changing the default session, for that first try.

## Before it replaces Hyprland

It replaces Hyprland on a machine only after it has been checked there from a separate login entry, with Hyprland's entry kept
as the way back: NVIDIA (modeset, explicit sync, a machine with two GPUs), several monitors and their places, fractional scaling,
VRR, direct scanout and tearing for games, the lock, idle dimming, suspend and resume, screenshots and streaming, every shortcut
of the session's table, and frame times against Hyprland in the bake-off. Of these only a VT switch has been tried on a real
(virtual) screen, in the test machine on 2026-11-15.

## The hardware backend (milestone 1, 2026-11-15)

    yggdrasil-compositor --drm [--run COMMAND ...]        (--backend drm and --backend udev are the same)

It drives the screens itself, started from a text console (a virtual terminal) as the user logged in there (`src/udev.rs`).
It refuses to start inside a desktop (WAYLAND_DISPLAY, DISPLAY or HYPRLAND_INSTANCE_SIGNATURE set; libseat would refuse
anyway, the desktop's session holds the GPU). It is developed in a virtual machine and never started on my running desktop.

| Part | How | Checked |
| --- | --- | --- |
| Seat | libseat through Smithay's session backend: logind (the default) or seatd, the GPU and input devices opened without root | both, in the VM: logind from an autologin on tty1, seatd from ssh |
| GPUs | udev lists them (the boot GPU first; it renders for programs) and tells when one comes or goes | one virtio GPU, with virgl (3D) and without |
| Screens | every connected connector (not a VR headset's "non-desktop") gets a free CRTC and an `Output` named as the kernel names it (DP-1, HDMI-A-1, eDP-1, Virtual-1) with all its modes, its size in millimetres and make and model from its EDID; Settings' monitors.lua sets mode, scale and place (outputs.rs, as headless); a mode changed later (wlr-output-management) is set on the CRTC at the next frame; a screen plugged in or out (a udev "change") is added or removed | Settings > Displays showed "RHT QEMU Monitor (Virtual-1)", 1920x1080 at 75 Hz; `wlr-randr --mode 1280x1024` changed the scanout to 1280x1024 and the panels followed, and back; since 2026-11-15 floating windows are pulled back inside the smaller screen (Settings went from 430,156 to 220,156, all of it on screen; before, 210 pixels of it hung off the right edge); EDID parsing has a unit test; hotplug is written, not yet tried (one virtual screen) |
| Drawing | hardware EGL on the GPU's GBM device: OpenGL ES (GlesRenderer), GBM buffers, programs' GPU buffers (linux-dmabuf with the render node in the default feedback, wl_drm); otherwise (no 3D, EGL only in software, or `YGGDRASIL_COMPOSITOR_RENDERER=pixman`) dumb buffers drawn with pixman; one Smithay `DrmCompositor` per screen, the pointer as a cursor element (on the cursor plane when the GPU has one) | virgl: "draws with OpenGL ES"; no 3D: "no hardware OpenGL ES (EGL is software only): drawing in software"; the same desktop drawn both ways |
| Frames | drawn only after a wake-up (a commit, input, a setting), never again before the page flip's vblank; a frame with no change is not sent and the next waits one refresh; nothing is drawn and no timer runs while nothing changes but the 2-second look at the config files | `YGGDRASIL_COMPOSITOR_DEBUG=1` prints each frame: with only the panel, one empty frame (nothing sent) every 2 s |
| Input | libinput (keyboards, mice, touchpads, a VM's tablet): keys and shortcuts as in the other backends, relative and absolute pointer motion, buttons, the wheel; input.conf's speed, profile, natural scrolling and left-handed buttons set on each device (again when the file changes), its scroll factor applied here | typing into foot through the VM's USB keyboard, the tablet moving the pointer and clicking Settings' pages |
| Virtual terminals | Ctrl+Alt+F1..F12 switch (xkb's XF86Switch_VT_n, also while locked); away: the screens are no longer drawn and libinput is suspended; back: the GPU is taken again and every screen drawn whole | Ctrl+Alt+F2/F3/F4 to the text login and Ctrl+Alt+F1 back, with seatd and with logind, GLES and pixman; logind needs polkit for the switch (the guest had none at first: "Permission denied") |
| Ending | `ctl quit`, SIGTERM, SIGINT, SIGHUP (through a signalfd): the screens and the seat are let go, the sockets removed, the console comes back. The programs it starts get those signals unblocked again (`plain_signals`; until 2026-11-15 they inherited them blocked, and Settings never ended on SIGTERM); XWayland, which Smithay starts, still inherits them blocked and ends with the compositor | after `ctl quit` and after `kill -TERM`, tty1's shell prompt was back on the screen and no socket was left |
| Control socket | the same as headless for the tools (yggdrasil-wm) and `outputs`, `scale`, `power`, `windows`, `focus`, `lock`, `quit`; `devices` lists the real devices; no input is made from it (only the devices give input) | `ctl outputs`, `ctl windows`, `ctl quit` in the VM |

### The laptop (milestone 2, built and first run 2026-11-19)

What a laptop needs, built for my ThinkPad X1 Yoga (Intel Iris Xe, i915; a 14-inch 1920x1200 eDP panel, USB-C/Thunderbolt and
HDMI, a lid, a tablet-mode switch, touchpad, TrackPoint, touchscreen and pen). The rules that need no hardware have unit tests
(`src/laptop.rs`, `src/input.rs`, `src/readback.rs`), the touchscreen a headless test (`tests/laptop.rs`); the rest is tried by
hand on the laptop's text console with `tests/laptop-tty.sh` (below). Two runs of about 20 and 25 minutes on 2026-11-19 (the
release build on VT 3; the logs stay on the laptop) checked the parts as the last column says.

| Part | How | Checked |
| --- | --- | --- |
| Seat and drawing | logind's seat through libseat; OpenGL ES on the Intel GPU (i915) | 2026-11-19: seat0 from logind, `/dev/dri/card1 draws with OpenGL ES`; Alt+Tab's live cards, the panel and the windows drawn at 1.25 |
| The internal screen | a connector of kind eDP, LVDS or DSI is the laptop's own (`laptop::is_internal`); it is given a CRTC before the others, so a dock full of monitors never leaves it without one; it is listed first, laid out at the left (unless placed) and is the main screen unless Settings > Displays chose another (`display::pick`, as the panel's dock), also when it comes back after the lid. A mode monitors.lua asks is matched within half a hertz (its 60 for the panel's 60.003 Hz) | 2026-11-19: eDP-1 1920x1200 at 60.003 Hz on its CRTC at 1.25, listed first and main with a monitor plugged in; main back on it after the lid since the same day (headless: `tests/outputs.rs`) |
| The lid | libinput's lid switch. Closed while another screen is connected and on (not switched off in Settings): the laptop's screen is taken off its CRTC, its panels closed and its windows moved, as when a screen is unplugged; opened: it comes back with what monitors.lua says for it. Closed with no other screen: it stays, nothing is drawn on it and its CRTC is switched off, and logind suspends (its HandleLidSwitch; docked, it ignores the lid, and so does this). `ctl lid closed` and `ctl lid open` do what the switch would, `ctl switches` tells both switches, from the start (open and off until libinput says otherwise) | 2026-11-19: alone, closed took the screen off and opened brought it back; with an HDMI monitor in, closed took eDP-1 off at once (the monitor became main) and opened put it back on its CRTC at once |
| Night Light | hyprland-ctm-control-v1 (ctm.rs, the protocol file in `protocols/`, BSD-3-Clause from hyprland-protocols), which hyprsunset speaks: Settings' Night Light and the notch's tile run it as on Hyprland; a screen's matrix goes into its CRTC's gamma table, each colour scaled by its row (exact for hyprsunset's colour temperatures, nothing more drawn), set again after a terminal had the screen; `ctl ctm` lists them | hyprsunset 0.4 at 4000 K against the headless backend: red 1, green 0.81, blue 0.65, back to none when it ended (2026-11-21); not yet seen on the laptop's screen |
| The tablet-mode switch and turning | recorded (`ctl switches`: `tablet-mode on`) and handed to the tablet watcher as the Hyprland session's binds do (`/usr/lib/yggdrasil/tablet mode on\|off`), which turns the built-in screen with the accelerometer through `transform NAME N` (wl_output's numbers; also monitors.lua's `transform` and wlr-output-management's): the picture, the layout, the panels and maximized windows, and where a touch or the pen lands (touch.rs) turn together, and a turn stays when Settings' file is read again | 2026-11-19: folded back and forth, "tablet mode on" and "off" each time (two of the laptop's devices tell it) |
| Hotplug | udev's "change" of the GPU: the connectors are looked at again; new ones are set up (the new screens before the old are given up, so the last screen is never gone in between), gone ones give their CRTC back (the screen's DrmCompositor goes, which switches its CRTC off; a connector a dock took away with it can still be switched off). New connector objects (a DisplayPort hub or dock: MST) reset the GPU first, since Smithay learns a connector's properties only then: every screen is dark for a moment and drawn whole again. A screen whose connector goes is kept 2 s with its windows, panels and wl_output and, back within it, only gets its CRTC again; one plugged in is set up once its link stayed 0.7 s | 2026-11-19: a 2560x2880 monitor on HDMI-A-1 set up at 1.25 beside eDP-1, the bar on both; unplugged and plugged back, its first frame 2 ms after the kernel saw it. Its link bounced three times in 26 s while the monitor hunted between its inputs; since then such a bounce keeps the screen (headless: `tests/outputs.rs`) |
| The default scale | for a screen monitors.lua gives no scale: its pixels per inch over 135 under 20 inches (a laptop's, seen up close) and 110 above (a monitor on a desk), GNOME's rule, in Settings' steps of 0.25, from 1 to 3, leaving at least 800x480; an unknown size (0, a projector, an EDID that gives its aspect ratio) is 1. The laptop's 1920x1200 panel, 30x19 cm, is 1.25, what I chose in Settings on its Hyprland session (Hyprland's own automatic scale would be 1.5); a 27-inch 4K monitor is 1.5, 24 inches at 1920x1080 and a 34-inch 3440x1440 are 1. The Hyprland session leaves monitors on a cable at 1 | 2026-11-19: eDP-1 at 1.25 |
| Suspend and resume | libseat does not pause the session for a suspend to idle (s2idle, the laptop's only kind), so the backend asks the kernel: a realtime timer far in the future with TFD_TIMER_CANCEL_ON_SET is cancelled when the system resumes (and when the clock is set; the time spent suspended, CLOCK_BOOTTIME less CLOCK_MONOTONIC, tells the two apart). No D-Bus, no logind, nothing woken while nothing happens. On waking: every screen's DrmCompositor reads its CRTC's state again and the next frame is whole, the input devices are set up again, and the connectors are looked at again (a monitor unplugged while it slept). `ctl resumed` does the same by hand | 2026-11-19, twice (`systemctl suspend`, s2idle): "woke after 12 s asleep" and "after 21 s"; eDP-1 drawn again at once at 1920x1200 and 1.25, the touchpad taking input right after with its settings, the windows as they were |
| Screens off (DPMS) | a screen switched off by wlr-output-power (yggdrasil-idle's idle time) or in Settings has its CRTC switched off (the DrmCompositor's `clear`: no picture and no backlight, instead of a black picture); on again, its mode is set again with a whole frame. A switch back from another terminal switches a screen that is off off again | 2026-11-19: off and on for real (no picture, backlight 0), and `ctl power HDMI-A-1 off`/`on` gave a monitor that stayed dark its picture with a fresh modeset (wlr-output-power through the protocol headless: tests/behaviour.rs) |
| Touchpad and TrackPoint | input.conf's touchpad keys (`tap_to_click`, `tap_and_drag`, `drag_lock`, `disable_while_typing`, `click_method = button_areas\|clickfinger`, `middle_button_emulation`, `scroll_method = two_finger\|edge\|on_button_down\|none`, `scroll_button`) in `[touchpad]` (every touchpad), `[trackpoint]` (every TrackPoint) or a device's own section; a touchpad takes natural scrolling and the scroll factor of its own, not every mouse's, as Hyprland's `input:touchpad`. Without them, the Hyprland session's: taps click and drag, nothing while typing, the bottom corners are the buttons, natural scrolling; a TrackPoint (and a touchpad's companion "-mouse", which carries its buttons) scrolls while its middle button is held. Devices are named as Hyprland names them, so a section means the same device on both; `ctl input` lists every device, its kind and what it was given (the form: `packages/wm/src/input.rs`; Settings has no page for them yet, and writing the file keeps them). A touchpad's scroll tells programs an axis stopped only when libinput says so | 2026-11-19: every device found (keyboard, touchpad and its "-mouse", TrackPoint, pen, touch, lid and other switches); taps, natural scrolling, typing blocking the pad and the button areas; the TrackPoint scrolling on its middle button; two fingers and the TrackPoint paging through the panel |
| Touchscreen and pen | the laptop screen's (as the Hyprland session maps them): touch as wl_touch to the program under the finger (which gets the keyboard, as with a click), the seat offering touch once a touchscreen is there; the pen moves the pointer, its tip is the left button and its side buttons the right and middle ones (tablet-v2, pressure and tilt for drawing programs, is not offered) | headless: tests/laptop.rs; 2026-11-19: the pen and touch on the laptop's screen, the panel's pop-ups opened and closed by touch |
| Screen copies | wlr-screencopy and ext-image-copy-capture (grim 1.5 takes the second), drawn again on the GPU with the screen's own elements (the lock screen while locked, black while it is off) into shared memory (read back; GL's RGBA turned into XRGB8888) or straight into the program's GPU buffer (dmabuf, OpenGL ES only: whole screens and windows, without the pointer); single windows for the panel's previews. Made while another terminal shows too, so grim from the desktop's session does not wait. Read back top row first, as Smithay's GLES renderer draws (its mapping's "flipped" is not followed) | 2026-11-19: grim from the Hyprland side on VT 7 got eDP-1's picture, upside down on the GPU path that day; upright since in a unit test with Mesa's software OpenGL ES |

`tests/laptop-tty.sh` runs it on the laptop: on a text console (Ctrl+Alt+F3, logged in as me), with the release build already
made; the panel, Settings and a terminal (foot, else ghostty) on a D-Bus session bus of their own, `YGGDRASIL_COMPOSITOR_DEBUG=1`,
everything printed in `~/.cache/yggdrasil-compositor/laptop-DATE.log`. It prints the Wayland and control sockets and writes them
to `~/.cache/yggdrasil-compositor/laptop-current.env` (`YGG_WAYLAND`, `YGG_CTL`), for the desktop's session:
`WAYLAND_DISPLAY=$YGG_WAYLAND grim shot.png` and `yggdrasil-compositor ctl --socket "$YGG_CTL" outputs`. Settings there writes
the same files the desktop reads.

Not in it yet, in the order of the next milestones: a GPU drawing for another (Smithay's GpuManager/MultiRenderer, for hybrid
laptops); several monitors; VRR; direct scanout of full
screen games tuned per GPU; wp-tearing-control; 10-bit), and meanwhile: presentation-time feedback, VR headsets leased
(drm-lease), the pen as a tablet (tablet-v2).

**Lightness**, measured in the VM (QEMU, 4 vCPUs; CPU of all the compositor's threads from /proc/PID/task/*/schedstat over 60 s
of idling, nothing moving; release build, 1920x1080 at 75 Hz, the panel's bar, notch and dock; XWayland running):

| Renderer | With the panel only | Panel and Settings open | Resident memory |
| --- | --- | --- | --- |
| OpenGL ES (virgl) | 7.2 ms in 60 s = 0.012 % of a core, 34 wake-ups | 4.7 ms = 0.0079 %, 30 wake-ups (before Settings stopped waking to look for changes, 2026-11-15: 185-215 ms = 0.31-0.36 %; docs/APPS_SETTINGS.md) | 62-64 MB (44 MB of it the GPU driver's libraries, file-backed) |
| pixman, chosen (`YGGDRASIL_COMPOSITOR_RENDERER=pixman`) | 4.4 ms = 0.0074 %, 34 wake-ups | 25.5 ms = 0.043 % | 22.6 MB |
| pixman, fallen back to (no 3D) | 4.6 ms = 0.0077 % | | 49.5 MB (Mesa's software EGL was loaded to find out, and stays mapped) |

The 34 wake-ups a minute are the 2-second look at the config files (and its frame with nothing to send). For a small machine
without a GPU driver, `YGGDRASIL_COMPOSITOR_RENDERER=pixman` keeps Mesa out altogether.

**The test machine** (`tests/drm-vm.sh`, `tests/drm-vm.py`): an Arch guest of my own, run as me with plain QEMU (no root; Limbo's
own guests have no display and need root, so it only lends its Arch cloud image as a read-only base), memory capped (4 GB in a
6 GB scope), a virtio GPU with virgl through the host's render node (`boot gl`) or without 3D (`boot 2d`), a USB keyboard and
tablet. `provision` installs what the guest needs (mesa, seatd, libinput, polkit, foot, XWayland, fonts, wlr-randr) and logs `ygg`
in on tty1 by itself; `deploy` copies the release builds in; `start [ARGS]` runs the compositor on tty1 through logind (default:
the panel and Settings); `shot NAME [KEYS]` presses keys and saves the screen (through QEMU's VNC socket, which reads the virgl
picture back); `type`, `click`, `move` give input; `ctl`, `measure`, `stop`. Screenshots stay in the VM's folder, not here.

## Building and running the tests

    cargo test                                  # unit, headless, behaviour and tools tests (the app tests skip)
    packages/compositor/tests/apps-podman.sh    # with the panel, Settings, the lock, snap and close, in a container
    packages/compositor/tests/drm-vm.sh boot    # the hardware backend in its test machine (above): boot, provision, deploy, start
