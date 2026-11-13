# yggdrasil-compositor

Yggdrasil's own Wayland compositor, in Rust on [Smithay](https://github.com/Smithay/smithay). **Nothing switches to it.** My
plan (2026-11-10): Hyprland stays the daily compositor while the rest of Yggdrasil is ported to Rust; only after those ports does
this one get a hardware (DRM/KMS) backend, and it replaces Hyprland only once it passes the checklist at the end of this file on
my own PC. Until then it runs nested in a window or headless, and its headless mode is where Yggdrasil's own programs are
tested on it. The longer history (nested mode, XWayland, what was tried by hand) is in [docs/COMPOSITOR.md](../../docs/COMPOSITOR.md).

## The headless test mode

    yggdrasil-compositor --headless [--outputs N] [--size WxH] [--scale S] [--output WxH[@S]]... [--run COMMAND ...]

No window, no screen, no GPU and no input device: every output is a picture in memory drawn by pixman (software). It refuses
the desktop's own runtime folder and drops WAYLAND_DISPLAY, DISPLAY, HYPRLAND_INSTANCE_SIGNATURE and the desktop's session bus,
so nothing it starts can reach the running desktop. A control socket (`yggdrasil-compositor ctl COMMAND`) drives and reads it:

| Command | What it does |
| --- | --- |
| `outputs`, `resize NAME WxH`, `scale NAME S`, `power NAME on\|off` | the virtual outputs (one switched off by Settings or a program says `disabled`) |
| `add-output WxH[@S]`, `remove-output NAME` | a screen plugged in (the next name, HEADLESS-N, placed as Settings' file says or to the right) and unplugged (its panels closed, its windows moved to the first screen; the last one stays) |
| `screenshot NAME FILE.png [pointer]` | the output's picture; with `pointer`, the pointer painted over it as the screen shows it |
| `pointer-look` | what the pointer looks like: `theme NAME SIZE SHAPE`, `surface WxH X,Y` (a program's own picture and its hotspot), `hidden resting` or `hidden program` |
| `windows` | every window: app id, title, place, size, workspace, states (active, floating/tiled, maximized, fullscreen, left, right, minimized) |
| `layers`, `free` | the layer-shell surfaces per output; the room they leave free for windows |
| `focus`, `workspace`, `lock` | what has the keyboard; the workspace showing; the lock and its surfaces |
| `key super+ctrl+2`, `key-down COMBO`, `key-up COMBO`, `type TEXT`, `pointer X Y`, `click [left\|right\|middle]` | input made inside the compositor (US layout); a key held down repeats its shortcut |
| `pointer-move DX DY`, `scroll DY [DX]`, `input` | the virtual mouse moved (at its speed in input.conf) and its wheel turned (scroll factor, natural scrolling); `click` is its buttons (left-handed: swapped); `input` gives the mouse and keyboard settings in force |
| `clock`, `clock stop`, `clock +MS`, `clock run` | the clock the animations run on: stopped and moved on by hand, a frame in the middle of an animation can be looked at |
| `quit` | end it |
| `clients`, `active`, `workspaces`, `workspace N`, `activate`/`close`/`minimize ID`, `maximize ID on\|off\|toggle`, `send ID N`, `place ID X Y W H`, `cursor`, `screens`, `screens all`, `layers`, `binds`, `reload`, `pointer-timeout [S]`, `cursor-theme [NAME SIZE]`, `keyboard [LAYOUT [VARIANT]]`, `devices`, `modifiers`, `paste ctrl\|ctrl+shift`, `exit` | the desktop commands Yggdrasil's own tools use through yggdrasil-wm (`src/wmctl.rs`; below): nothing outside the compositor; the only key presses are `paste`'s Ctrl+V into the window with the keyboard (as Hyprland's sendshortcut), refused while locked |

Headless it also offers wlr-screencopy (grim, yggdrasil-shot, Sunshine's shared-memory path), ext-image-copy-capture with
ext-image-capture-source (single windows and screens: the panel's window previews) and wlr-output-power (yggdrasil-idle), served
from the pictures in memory, and wlr-output-management (wlr-randr, kanshi: the screens listed with their modes, and changed). The
standard window list, ext-foreign-toplevel-list, is offered nested too.

### The tests

| File | What it checks | Runs |
| --- | --- | --- |
| `src/*` unit tests (47) | options, sizes and scales, the refused runtime folder and session bus, shortcuts (the table held to the session's `yggdrasil.lua`: every bind, its words, release and repeat), tiling with the session's gaps, placement (the same cases as `keep-clear.lua`, its `open()` too), the look (the session's values, Hyprland's curve, opening, moving and closing, the corners' pixels), the keyboard files (the installer's, localectl's, vconsole.conf), Settings' monitors.lua (its lines, the last rule naming a screen winning), the window-places lines, the pointer (appearance.conf's `cursor`, XCURSOR_SIZE, painting it over a picture, the plain arrow), input.conf (a mouse's section over every mouse's, the defaults, buttons, scrolling, speed) | `cargo test`, CI `rust` |
| `tests/headless.rs` (6) | outputs (size, scale, name, place), a bar and a window drawn where they belong, size and scale changes reaching programs, `quit` | `cargo test`, CI `rust` |
| `tests/behaviour.rs` (14) | with the tests' own small clients: the bar and dock reserve their room; windows open centred in the rest (one bigger than it made to fit, less the gap), kept clear of a dock that appears, back from maximize at a normal size; Super+Up/Left/Down; full screen over the bar and dock; workspaces by keys; the keyboard following new windows, clicks and an exclusive panel, typing reaching the window; the lock on every output taking every key, a second locker refused, a crashed one leaving the screen locked and a new one taking over; only the media keys' shortcuts working while locked; screen copies (whole, part, scale 2, with damage, while locked, a wrong buffer); grim's picture equal to the screen; screens off and on; idle and resume | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/tools.rs` (6) | yggdrasil-wm, the layer Yggdrasil's tools share, on it: found before a Hyprland named in the environment; windows (with their processes), screens, the free room, the pointer and the desktops read; desktops shown and a window sent to another; a window placed, snapped, maximized and back, minimized and closed; a tiled one floated where it is put; the pointer timeout set and put back; a title with a line break stays one line; the Task Manager's bring-to-front and close, the clipboard's paste into an editor and a terminal (none into the lock screen), Settings' focus by class, the shortcut list, reload, the pointer theme, the keyboard layout, layers; exit ends it | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/keys.rs` (5) | the session's shortcut table, with stand-ins as the compositor's only PATH (each writes down how it was called; nothing real runs): every key starts the command line it starts on Hyprland (`yggdrasil-panelctl notifications`, `loginctl lock-session`, `yggdrasil-shot` ...); Super and Ctrl tapped on their own (the start menu, Find my mouse) and not when another key comes between, and the key still reaching the program; held volume and magnifier keys repeating (600 ms, then 25 a second) and stopping when let go or another key is pressed, mute not repeating; Super+W, Alt+F4 and Super+Q closing (a keep-running program minimized), Super+Down minimizing a maximized window, Super+Shift+M showing the minimized windows and hiding them, one brought back staying; the bindings listed with the session's words, the lists Settings and the panel's card make from them equal to those from `yggdrasil.lua` | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/look.rs` (4, and 1 that only measures) | the window look against PNG snapshots (`tests/snapshots`) and numbers worked out from the session's values: the border (the accent at 80 % in front, faint behind, in the dark and the light style, another accent read again), the corners cut round, the shadow's fall-off, a front window's shadow over the one behind; maximized and tiled windows with the gaps, full screen with none of it; a window opening (40 ms in), sliding to maximized (60 ms in, its picture stretched) and closing (50 ms in) on a stopped clock; "reduce motion" | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/outputs.rs` (5) | the keyboard layout from the system's file as the installer writes it (in the test's own etc folder): `keyboard` and the keymap programs get (German, then French when the file changes, the installer's try-out kept until it does), vconsole.conf when there is no X11 file, US when neither, xkb's default for a layout it cannot load; Settings > Displays' monitors.lua in Settings' own form (a mode, a scale, a place left of the first screen, a screen off; read back through yggdrasil-wm's `screens_all` with modes and rates, changed and reloaded as Settings does, back to the command line's when the file goes); wlr-output-management (heads, modes, a custom mode and scale applied, a screen off with its bar closed and its window moved, a stale configuration cancelled, all off refused, a test changing nothing, back on to the right); screens plugged in and out (named, placed, offered to programs; the windows moved off at their place, a maximized one maximized again, snapshot; a screen plugged in while locked all black) | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/places.rs` (5) | a program's window back where its last one closed (the session's file and lines), not for a second window, a maximized one or one under 120 x 80; places the Hyprland session kept used, kept clear of the bar, skipped when off every screen; a kept place too big for the screen made to fit it less the gap, a dialog of the same process with another class neither given a kept place nor kept, a window bigger than the screen made to fit; windows opened on a screen in `~/.config/yggdrasil/tiling` tiled with the session's gaps (snapshot), the others taking the room of one that closes, new ones floating once the screen no longer tiles; Steam's own maximize request ignored, Super+Up still maximizing it | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/capture.rs` (3) | the window list and window pictures the way the panel takes them (ext-foreign-toplevel-list, ext-image-capture-source, ext-image-copy-capture): windows listed with title and app id, a new title followed, a closed one said closed; a window covered by another copied whole and exactly (both formats, snapshot), the next copy waiting until the window changes, a wrong buffer size failing, a minimized window still copied, a closed one's session stopped; at scale 2 the copy in the screen's pixels, a window that grows telling its new size; no window copied while locked (a screen's copy shows the lock); a screen's copy equal to its screenshot, a screen unplugged stopping its session | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/pointer.rs` (5) | the pointer drawn by the compositor, with themes of the test's own: the theme in appearance.conf at XCURSOR_SIZE (snapshot), the theme's 48 picture on a scale 2 screen, `ygg-theme cursor`'s change through yggdrasil-wm at once, the plain arrow for a theme that is not installed, grim -c with the pointer and grim without; a program's own cursor surface with its hotspot (snapshot), a shape by name (cursor-shape-v1), hidden by the program, the default arrow over nothing, ext-image-copy-capture's paint_cursors; hidden 0.25 s after the last motion while yggdrasil-idle's pointer timeout is set through yggdrasil-wm, shown again by motion, never hidden at 0; moved to the middle of the first screen when its screen is unplugged (snapshot), kept inside a screen that gets smaller, left alone when one is plugged in; Yggdrasil's own theme as ygg-cursors draws it (snapshot; when `YGGDRASIL_TEST_CURSORS` names it) | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/input.rs` (1) | Settings > Peripherals' input.conf: taken at start (programs told the key repeat; the virtual mouse at half speed, left-handed, scrolling naturally at twice the distance, from every mouse's settings and its own section), the pointer kept on the screens, the devices listed through yggdrasil-wm as Settings lists them, a change applied through yggdrasil-wm's `apply_input` at once, the defaults when the file goes | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/apps.rs` (9) | Yggdrasil's real programs inside it: the panel (bar, dock, wallpaper drawn; the free room; the terminal stand-in centred and maximized into it; a notification toast that leaves the keyboard alone; the start menu taking the keyboard and giving it back; a smaller screen), the dock's window list (resting on the window's icon opens it, and its card shows the window's picture, red left of green as the window), the panel's desktop buttons ("+" makes and shows desktop 2, desktop 1's button goes back) and window zones (Super+Shift+Z's sheet for the window in front, a click on the left zone puts it there), `yggdrasil-snap` (the four halves of the free room, maximize and back, minimize) and `yggdrasil-close` (a keep-running program minimized, any other asked to close), `yggdrasil-logout` with stand-ins as its only PATH (every window asked; all closed: the session ends; one stays: the screen locks and the session goes on), Settings (egui drawn by Mesa in software: centred at 1060x720, drawn, Super+Up and back, Super+Q ends it), the lock screen in its test-auth build (two outputs, one at 1.5, nothing of the session shows, a wrong password keeps it locked, the right one unlocks and the keyboard goes back) | CI `compositor-headless`; on a desktop `tests/apps-podman.sh` |

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
- Four workspaces: Super+Ctrl+1..4, Super+Ctrl+Left/Right, Super+Ctrl+Shift+1..4 sending the window.
- Click to focus (`follow_mouse = 2`), a new window takes the keyboard, a panel asking for the keyboard exclusively (the start
  menu) gets it at once and gives it back to the window when it closes (the session's `layer.closed` rule).
- The lock (ext-session-lock): every output covered, every key to the lock, no shortcut while locked but the media keys'
  (Hyprland's `locked` binds), a second locker refused, a crashed locker leaves the screen locked and a new one may take over
  (`allow_session_lock_restore`).
- Screenshots and streaming (wlr-screencopy, shared memory), screens off and on (wlr-output-power), idle (ext-idle-notify),
  several outputs side by side at fractional scales.
- The dock's window previews: ext-foreign-toplevel-list, ext-image-capture-source and ext-image-copy-capture (item 6).
- The pointer drawn with the cursor theme, a program's own pointer, hidden while the screen is dimmed (item 7).
- Yggdrasil's tools through yggdrasil-wm (above): the panel's desktop buttons and window zones, `yggdrasil-snap`, `yggdrasil-close`,
  the idle manager's pointer timeout, `ygg-theme cursor`'s pointer theme.
- The system's keyboard layout, Settings > Displays' `monitors.lua` (modes, scales, places, screens off), screens that come and
  go, remembered window places and the screens set to tile, Steam's maximize request ignored (items 3 and 5, 2026-11-12, below).

**Built but only tried nested (not in the headless tests):** XWayland and games (opaque games, `_NET_WM_STATE` before mapping,
Wine's WM_STATE), Alt+Tab switcher, Super+T and tiling workspaces, window controls for server-side decoration, move and resize
by the program's own title bar, Super+drag, xdg-activation, idle inhibit, clipboard and primary selection.

**Not yet (the gap), in the order I would close it:**
1. **What is left of "our tools call hyprctl"** (every tool moved to yggdrasil-wm on 2026-11-12, above). Hyprland's own on purpose:
   hyprsunset's night light (its own socket; this compositor has no gamma control yet, item 11), the Hyprland plugin builder
   (`yggdrasil-hypr-plugin`), the magnifier (`yggdrasil-zoom`, Hyprland's zoom: its keys are bound here, it zooms nothing yet),
   the config-error toast's event socket and
   xmove (both do nothing elsewhere), and Settings' own Hyprland file and night light. Settings > Displays lists and changes this
   compositor's screens (it writes `monitors.lua` and reloads through yggdrasil-wm, as on Hyprland; the compositor takes the file
   on reload, item 3). Still to come: the control socket on the real session (today only headless), and ext-workspace so other
   programs see this compositor's desktops the standard way (its windows they see through ext-foreign-toplevel-list since item 6).
2. ~~The shortcut table~~, closed 2026-11-12: the built-in table is the session's (`src/shortcuts.rs`; a unit test reads
   `yggdrasil.lua` and fails on any bind missing on either side), with its words and the same command lines (Super+N, Super+S
   and Ctrl+Alt+S, Super+Shift+Z, Super+/ and the Ctrl taps through `yggdrasil-panelctl`; Print and Super+Shift+S, Super+Shift+C,
   Ctrl+Shift+Esc, the magnifier, the screen reader, `loginctl lock-session`, XF86AudioPause), the window keys as
   `yggdrasil-close` and `yggdrasil-snap` do them (Super+W; the keep-running list now in yggdrasil-conf; Super+Down minimizes),
   Super+Shift+M showing the minimized windows (as Hyprland's special workspace), taps on a modifier (release binds that do not
   take the key: Super for the start menu, Ctrl for Find my mouse), repeat while held for volume, brightness and the magnifier at
   Hyprland's 600 ms and 25 a second (also what programs are told now; was 300 ms and 40). The control socket's `binds` gives
   each one's kind and the mouse's two binds, and yggdrasil-wm reads the release flag. Headless, programs it starts get no system
   bus (Super+L's `loginctl` would otherwise lock the running desktop). Left: the magnifier's zoom itself (item 1), and the
   switcher is this compositor's own (Hyprland only cycles).
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
11. **The hardware backend** (DRM/KMS), developed in a Limbo virtual machine, never on the live session: libseat, udev and hotplug,
   GBM/EGL and Smithay's DrmCompositor with page-flip pacing, libinput, then the checklist below.
12. **The bake-off:** the same games and the CAD model measured against Hyprland (`tools/bakeoff`), and only then a decision.

## Checklist before it may replace Hyprland on my PC

Each item is checked on my own machine (NVIDIA, more than one screen), from a separate login entry, with Hyprland's entry kept
and a way back:
- [ ] NVIDIA: nvidia-drm with modeset, GBM, the desktop and a game drawn without corruption; a machine with two GPUs picks the right one
- [ ] Explicit sync (linux-drm-syncobj-v1): no flicker and no frames out of order in games and in Vivaldi
- [ ] Several monitors: places and modes as Settings > Displays says, unplugging and plugging back, the dock following the main screen
- [ ] Scaling: fractional scales sharp for Wayland programs, a sensible size for X11 programs
- [ ] VRR: on for full screen games only (the session's `vrr = 2`), no flicker on the desktop
- [ ] Direct scanout of full screen games (`direct_scanout = 2`) and tearing allowed for games that ask (wp-tearing-control)
- [ ] The lock, idle dimming, screens off and on, suspend and resume, a VT switch and back
- [ ] Screenshots (yggdrasil-shot, the colour picker) and Sunshine streaming to the iPad
- [ ] Every shortcut of the session's table, the media keys on the lock screen (with the real yggdrasil-mediakey)
- [ ] Frame times, 1% lows and input feel at least as good as Hyprland's in the bake-off games; idle CPU and memory measured

## Building and running the tests

    cargo test                                  # unit, headless, behaviour and tools tests (the app tests skip)
    packages/compositor/tests/apps-podman.sh    # with the panel, Settings, the lock, snap and close, in a container
