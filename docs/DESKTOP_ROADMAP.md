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
2. **Notification service** (`org.freedesktop.Notifications`), with the notch or a right-hand pane (see STATUS.md).
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
