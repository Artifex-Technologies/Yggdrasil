# Desktop roadmap: replacing KDE, one piece at a time

Decided 2026-10-15 (owner): **keep CachyOS as the base**, and slowly replace most of KDE with Labyrinth's
own desktop, written in Rust (and Go where it fits). Reasons: KDE feels dated and slow, and not familiar
enough; GNOME has the same problem in other ways. The owner likes much of what both did over the years and
wants something that feels familiar at once (a Windows-style start menu and keyboard use, a macOS-style top
bar and dock) while giving the user control of their privacy. The long-term aim is a privacy-first desktop
OS in the spirit of GrapheneOS, that can still run CAD and games (see the plan below).

Replacing the base distribution was considered and rejected: its kernel, optimised packages and gaming setup
are real work the project would otherwise have to do and keep patched, and none of it is where the goals are
won. What the project builds sits above that layer.

## Already ours

Top bar and dock, start menu (Folders theme, keyboard use: Tab, arrows, Super), Fylgja (the notch: controls,
clipboard, stats, wallpaper, power, now playing), Settings, Minos (file explorer, with drag and drop to other
windows and a copy window with merge, progress and Replace / Skip / Keep both), calculator, weather, tile
borders, the dock's strut handling. Plus Sarina, Heimdall and Limbo as optional components.

## Still KDE, in the order I would replace them

Each step must work alongside KDE until it has earned the place, and each can be done and shipped alone.

1. **Power and idle manager** (replaces PowerDevil). PowerDevil suspended the machine at 09:46 on
   2026-10-15 with "never sleep" written in its config; a system-level block (`/etc/systemd/sleep.conf.d/10-never-sleep.conf`)
   is the stopgap. Own it: read the idle state through Wayland, honour logind inhibitors, one clear setting
   per behaviour, an honest log of why it slept.
2. **Notification service** (`org.freedesktop.Notifications`), with the notch or a right-hand pane (see STATUS.md). **Built 2026-10-16** (see NOTIFICATIONS.md): service, grouped list, pop-ups, Fylgja section, right pane, Heimdall feed; to check on a fresh install.
3. **Portals backend** (replaces xdg-desktop-portal-kde): **Minos as the file chooser** (the open/save dialog
   every app shows), screenshot and screencast. This is also where per-app permissions (files, camera,
   microphone, location, screen) are asked and remembered, so it is the heart of the privacy model.
4. **Screen locker and polkit agent.**
5. **Login screen** (replaces SDDM).
6. **System tray host** (StatusNotifierItem) so apps with tray icons keep working.
7. **Global shortcuts, input and display settings** (replace kglobalacceld and kscreen, which today do the
   Super key and the screen layout; the Super key already needed a workaround for KWin 6.7).
8. **Compositor** (replaces KWin), last. A Rust compositor on a library such as Smithay. It needs input,
   several monitors, fractional scaling, drag and drop, screen sharing, games and the NVIDIA driver to be solid
   first. Only start it when everything above is stable.

## Familiar first

A new desktop that feels foreign loses people. Rules: keyboard shortcuts people already know (Super opens the
start menu with search active; Win/Super+E is the file explorer; Alt+Tab; Ctrl+C/V everywhere); mouse
behaviour the same as Windows and macOS (a click on a selected name renames it; double-click opens);
everything reachable by keyboard.

## GrapheneOS for the desktop (long-term)

Pillars: verified boot with the user's own Secure Boot keys and the TPM; a read-only root with signed atomic
updates; every app sandboxed with explicit permissions (Limbo, Flatpak, portals); hardened defaults (a
hardened allocator, kernel hardening, default-deny firewall, no telemetry); Heimdall and Sarina watching for
misbehaviour. Games and CAD are named exceptions with their own permission profile (direct GPU access), kept
out of the default state. The NVIDIA proprietary driver weakens the hardening; accepted for gaming.
Start with the sandbox and permission layer (steps 3 above, per-app network rules in Settings), then Secure
Boot with the user's keys as an installer option; the read-only root comes later.

## Daedalus CAD

A separate project (`Daedalus_CAD`, SketchUp-style modeller in Rust: half-edge mesh, solid inspector and
repair, sheet-metal flat patterns, wgpu viewer). First milestone: build and test it on Linux. Windows CAD
programs the user may still need run through Limbo, with GPU passthrough (not yet tested).

## Screen locker, and keeping a replacement desktop current and safe (decided 2026-10-16)

**Replace kscreenlocker: yes, later, on the standard protocol.** Wayland has `ext-session-lock-v1`: the compositor hands the whole screen to one
client until that client says "unlocked". A Labyrinth locker would be a small Rust program that speaks that protocol, draws our own
look, and checks the password through PAM. It does not depend on KDE internals, so a KDE update cannot break it the way the 6.8 upgrade broke
kscreenlocker's greeter (that failure was an upgrade of one KDE piece under a running older KWin, not a protocol change).
- A locker is security-critical: a bug is either a lock-out or a bypass. So: keep kscreenlocker as the fallback and test ours in the VM first
  (lock, wrong password, right password, a crash of the locker must leave the screen LOCKED, switching to a text console must not bypass it,
  suspend and resume, multiple monitors), then switch the default. Never ship it untested.
- Order: (1) our own power / idle manager (it decides when to dim, lock, sleep: this also explains and fixes the unexplained locks),
  (2) the locker, (3) the next KDE pieces on the roadmap (portals, tray). Each piece replaces a KDE one only when it has passed its tests.

**Staying current and security-compliant while replacing KDE piece by piece:**
- Our programs talk to the system through stable, documented interfaces (Wayland protocols, freedesktop D-Bus APIs, PAM, logind, PipeWire),
  not KDE internals; where we must read a KDE tool (kscreen-doctor, powerdevil) we parse it defensively and test against its newest output
  (the 6.8 "enabled" key is the example, now covered by a test).
- `labyrinth-selfcheck` after every upgrade; the pacman hook reminds to log out after Plasma upgrades.
- Rust dependencies: run `cargo audit` and `cargo deny` in the build; update crates on a schedule; the lock files are committed.
- System updates stay the distribution's job (CachyOS/Arch `pacman -Syu`, weekly), with Heimdall's integrity checks now package-aware (an update is info, an unexplained change is critical).
- A short security checklist per release (secrets scan, dependency audit, VM boot and install test, selfcheck clean) before an ISO is published.
- Replacing KDE is a long road; until a piece is replaced and proven, the KDE one keeps receiving the distribution's security fixes.
