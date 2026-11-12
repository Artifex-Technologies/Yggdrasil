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
| `outputs`, `resize NAME WxH`, `scale NAME S`, `power NAME on\|off` | the virtual outputs |
| `screenshot NAME FILE.png` | the output's picture |
| `windows` | every window: app id, title, place, size, workspace, states (active, floating/tiled, maximized, fullscreen, left, right, minimized) |
| `layers`, `free` | the layer-shell surfaces per output; the room they leave free for windows |
| `focus`, `workspace`, `lock` | what has the keyboard; the workspace showing; the lock and its surfaces |
| `key super+ctrl+2`, `type TEXT`, `pointer X Y`, `click [left\|right\|middle]` | input made inside the compositor (US layout) |
| `quit` | end it |

Headless it also offers wlr-screencopy (grim, yggdrasil-shot, Sunshine's shared-memory path) and wlr-output-power (yggdrasil-idle),
served from the pictures in memory.

### The tests

| File | What it checks | Runs |
| --- | --- | --- |
| `src/*` unit tests (28) | options, sizes and scales, the refused runtime folder and session bus, shortcuts, tiling, placement (the same cases as `keep-clear.lua`) | `cargo test`, CI `rust` |
| `tests/headless.rs` (6) | outputs (size, scale, name, place), a bar and a window drawn where they belong, size and scale changes reaching programs, `quit` | `cargo test`, CI `rust` |
| `tests/behaviour.rs` (14) | with the tests' own small clients: the bar and dock reserve their room; windows open centred in the rest, kept clear of a dock that appears, back from maximize at a normal size; Super+Up/Left/Down; full screen over the bar and dock; workspaces by keys; the keyboard following new windows, clicks and an exclusive panel, typing reaching the window; the lock on every output taking every key, a second locker refused, a crashed one leaving the screen locked and a new one taking over; only the media keys' shortcuts working while locked; screen copies (whole, part, scale 2, with damage, while locked, a wrong buffer); grim's picture equal to the screen; screens off and on; idle and resume | `cargo test`, CI `rust` and `compositor-headless` |
| `tests/apps.rs` (3) | Yggdrasil's real programs inside it: the panel (bar, dock, wallpaper drawn; the free room; the terminal stand-in centred and maximized into it; a notification toast that leaves the keyboard alone; the start menu taking the keyboard and giving it back; a smaller screen), Settings (egui drawn by Mesa in software: centred at 1060x720, drawn, Super+Up and back, Super+Q ends it), the lock screen in its test-auth build (two outputs, one at 1.5, nothing of the session shows, a wrong password keeps it locked, the right one unlocks and the keyboard goes back) | CI `compositor-headless`; on a desktop `tests/apps-podman.sh` |

Every test starts its own compositor in a private folder, has a time limit (the test program ends past it), and every process it
starts dies with it. The programs get an empty environment, the compositor's private runtime folder and home, and a session bus of
the test's own without service activation. The lock is only ever the `--features test-auth` build, in `target/test-auth`; a build
that would check the password with PAM is refused. `tests/apps-podman.sh` builds everything and runs the two test files in a
rootless podman container with no devices and no network.

Found by these tests and fixed: the lock screen's debug build ended right after locking at 1280x720 (its panel edge sat on a
fractional pixel and tripped tiny-skia's anti-aliasing); the compositor let a second locker take over a running lock; the keyboard
stayed on the lock's surface after unlocking.

## What it matches of Yggdrasil's Hyprland session

The session's behaviour is set in `packages/session/hyprland/yggdrasil.lua` (with `keep-clear.lua` and the minimize plugin).

**Matched, and tested headless:**
- Windows float and open centred, kept clear of the bar and the dock (`float, center` plus keep-clear): here they open centred in
  the free room, are shortened to it, moved out from under a bar or dock that appears, and come back from maximize at three
  quarters of the room when they were nearly as big as it. (Hyprland centres on the whole monitor and keep-clear moves the
  window after; the result differs by at most half the bar's or dock's height.)
- The panel as layer-shell surfaces: bar on Top reserving its height, dock on Top reserving its height and gap, wallpaper on
  Background, menus with exclusive zone 0 under the bar, toasts on Overlay without the keyboard.
- Super+Up maximize and restore, Super+Left/Right halves, Super+Down restore then minimize (the session's `yggdrasil-snap`),
  Super+Q close, maximize into the free room, full screen covering the bar and the dock.
- Four workspaces: Super+Ctrl+1..4, Super+Ctrl+Left/Right, Super+Ctrl+Shift+1..4 sending the window.
- Click to focus (`follow_mouse = 2`), a new window takes the keyboard, a panel asking for the keyboard exclusively (the start
  menu) gets it at once and gives it back to the window when it closes (the session's `layer.closed` rule).
- The lock (ext-session-lock): every output covered, every key to the lock, no shortcut while locked but the media keys'
  (Hyprland's `locked` binds), a second locker refused, a crashed locker leaves the screen locked and a new one may take over
  (`allow_session_lock_restore`).
- Screenshots and streaming (wlr-screencopy, shared memory), screens off and on (wlr-output-power), idle (ext-idle-notify),
  several outputs side by side at fractional scales.

**Built but only tried nested (not in the headless tests):** XWayland and games (opaque games, `_NET_WM_STATE` before mapping,
Wine's WM_STATE), Alt+Tab switcher, Super+T and tiling workspaces, window controls for server-side decoration, move and resize
by the program's own title bar, Super+drag, xdg-activation, idle inhibit, clipboard and primary selection, the Super tap for the
start menu.

**Not yet (the gap), in the order I would close it:**
1. **Our own tools still call `hyprctl`:** the panel's workspace buttons, window zones and window list, `yggdrasil-snap`,
   `yggdrasil-close`, `yggdrasil-idle`'s pointer hiding. They need ext-workspace and a control socket on the real session
   (the headless one is the start), so each tool talks to whichever compositor runs.
2. **The shortcut table:** Super+N, Super+W, Super+Shift+M (minimized
   windows), Super+Shift+Z (zones), Print and Super+Shift+S (yggdrasil-shot), Super+Shift+C, Super+/, Ctrl+Shift+Esc, the
   magnifier (Super+Alt+=/-/0) and the screen reader (Super+Alt+S), the Ctrl double-tap (release binds that do not consume the
   key), key repeat for the volume keys.
3. **Input and outputs from the system's settings:** the keyboard layout from `/etc/X11/xorg.conf.d/00-keyboard.conf` (today
   the default US layout), Settings > Displays (`monitors.lua` modes, places and scales; wlr-output-management), outputs that
   come and go.
4. **The window look:** the 1-pixel border in the accent colour, rounded corners (10), shadows, the gaps (4 and 8), animations
   with "reduce motion".
5. **Remembered places** (the session's `window-places`) and the per-screen tiling file (`~/.config/yggdrasil/tiling`; the
   compositor has per-workspace profiles in `workspaces.conf` instead), Steam's maximize request ignored.
6. **The panel's window previews:** ext-foreign-toplevel-list and ext-image-copy-capture; screencopy into GPU buffers (dmabuf).
7. **The pointer:** drawn by the compositor with the cursor theme, hidden while the screen is dimmed.
8. **The session:** `yggdrasil-session` starting it instead of Hyprland (the environment to systemd and D-Bus, the autostart
   target, restarts after a crash), and a log kept across a crash.
9. **The hardware backend** (DRM/KMS), developed in a Limbo virtual machine, never on the live session: libseat, udev and hotplug,
   GBM/EGL and Smithay's DrmCompositor with page-flip pacing, libinput, then the checklist below.
10. **The bake-off:** the same games and the CAD model measured against Hyprland (`tools/bakeoff`), and only then a decision.

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

    cargo test                                  # unit, headless and behaviour tests (the app tests skip)
    packages/compositor/tests/apps-podman.sh    # with the panel, Settings and the lock, in a container
