# What KDE still does on this machine, and what replaces it

Taken from the running session (2026-10-16, Plasma 6.8.0): the processes, the loaded `kded6` modules, the installed KDE packages and their config
files. Companion to `DESKTOP_ROADMAP.md` (which fixes the order). Effort: **S** = a day or less, **M** = a few days, **L** = weeks, **XL** = months.
"Keep" means: no plan to replace soon.

## What is running

| KDE piece | What it does for us today | Replacement | Effort | Notes |
| --- | --- | --- | --- | --- |
| `kwin_wayland` | compositor, window management, the protocols our panel uses (`plasma-window-management`, plasma-shell for the dock, layer-shell), global-shortcut routing, screen recording | a Rust compositor (Smithay) | XL | Last. Until then, move the panel from KWin-only protocols to standard ones (`ext-foreign-toplevel-list`, `wlr-foreign-toplevel`, layer-shell for the dock) so it runs on any compositor and KWin can be swapped without touching it. |
| `plasmashell` | with our panels removed it still draws the **wallpaper** (Fylgja's wallpaper section calls `plasma-apply-wallpaperimage`), hosts widgets, KRunner, the old tray | a wallpaper layer-shell surface in the panel | S | **Built 2026-10-16, off by default:** `own_wallpaper = yes` in `~/.config/labyrinth/panel.conf` and the picture's path in `~/.config/labyrinth/wallpaper`; Fylgja and Settings then set that file instead of calling Plasma. Tested on the ultrawide (a layer-shell background surface, drawn once per screen size). Then plasmashell can stop, once nothing else needs it. |
| `powerdevil` (`org_kde_powerdevil`) | idle: dim, display off, lock request, sleep; brightness; power profiles; battery. Suspended the machine against its own config on 2026-10-15; blanks the screen and something locks it | **labyrinth-idle** (idle through `ext-idle-notify-v1`, logind inhibitors, an honest log) | M | Roadmap item 1. A prototype that only *observes* is in `packages/idle`. |
| `kscreenlocker_greet` | lock screen | a Rust locker on `ext-session-lock-v1` + PAM | M | Roadmap item 4 (see the section at the end of DESKTOP_ROADMAP). Broken right now until you log out (stale KWin). **Prototype written 2026-10-16** (`packages/lock`, `labyrinth-lock`: the typing/lockout logic is unit-tested, PAM goes through libpam directly and fails closed, a test-only `test-auth` feature types keys by itself). **But the running KWin 6.7.5 does not offer `ext_session_lock_manager_v1` at all** (checked with WAYLAND_DEBUG: only `ext_idle_notifier_v1`, layer-shell, output management ... are listed), so it could not be run against the real session. To do after the log out: check again on KWin 6.8 (`labyrinth-lock` says "the compositor does not offer ext-session-lock-v1" if still missing). If 6.8 also lacks it, the locker has to wait for our own compositor, or hook into KWin's own lock path. Nothing was locked on this machine. |
| `polkit-kde-authentication-agent-1` | the password box for `pkexec` (Settings uses it for network, power limits, clear data) | our own polkit agent (D-Bus `AuthenticationAgent`, PAM through polkit's helper) | M | **Prototype built 2026-10-16** (`packages/polkit`, `labyrinth-polkit`): registers for the login session, shows a password box (message, action id, user), talks to polkit's helper over its socket (`/run/polkit/agent-helper.socket`; the line protocol is unit-tested against a stand-in helper: right password, wrong password, cancel) and wipes the password after use. Checked live with `pkexec`: the box appears with the right text and focus, Escape cancels cleanly. **Not yet checked with a real password end to end**, and the look is plain. Only one agent can serve a session: KDE's was stopped for the test and started again after. `--process PID` registers for one process but `pkexec` does not use that, so tests need the session registration. |
| `kded6` | a host for ~25 small modules (below) | none: replace what we use, drop the rest | - | |
| `kded6`: `statusnotifierwatcher` | the system-tray registry (apps with tray icons: Steam, Spotify...) | our own StatusNotifierWatcher + a tray in the bar | M | Roadmap item 6. **Host built 2026-10-16** (`packages/panel/src/tray.rs`): the panel registers with the existing watcher, shows Active items' icons in the bar (icon name, the item's own icon folder, or its pixels), left click = `Activate`, right click = `ContextMenu`; tested with a fake item (`tests/tools/fake_sni.py`: icon drawn, both clicks arrive). Items whose status is `Passive` (Spotify and Solaar right now) are hidden, as the spec says; an overflow popup for them is not built. Items that only offer a dbusmenu (no `ContextMenu` of their own) need our own menu drawing: not built. The watcher itself (kded's) is still KDE's. |
| `kded6`: `appmenu` + `gmenudbusmenuproxy` | global menu (menus in the top bar) | none planned; maybe later | L | Not used by our bar. |
| `kded6`: `bluedevil` | Bluetooth pairing dialogs | a pairing agent (BlueZ `Agent1`) with a small window | M | Fylgja already toggles Bluetooth. |
| `kded6`: `networkmanagement` (+ plasma-nm secrets agent) | asks for Wi-Fi passwords | an NM secret agent | M | Settings has Wi-Fi already. |
| `kded6`: `audioshortcutsservice`, `mprisservice` | media keys | keybindings in our shortcut daemon | S | |
| `kded6`: `devicenotifications`, `freespacenotifier`, `oom_notifier`, `trashdirnotify`, `remotenotifier`, `desktopnotifier` | notifications for disks, space, memory, trash | send them through our notification service from a small user daemon (udisks2 signals) | S | `devicenotifications` (a USB stick was plugged in) is the useful one. |
| `kded6`: `ktimezoned`, `kded_accounts`, `kameleon`, `smbwatcher`, `wpad_detector`, `donationmessage`, `lookandfeelautoswitcher`, `plasma_accentcolor_service`, `screenreader`, `baloosearchmodule`, `browserintegration*`, `gtkconfig` | assorted conveniences | drop; `gtkconfig` (keeps GTK apps' theme in step) and `plasma_accentcolor_service` (accent colour) are the ones Settings relies on, replace those two with a few lines in Settings | S | |
| `kglobalacceld` (in `kded6`/`plasma_session_shortcuts`) | global shortcuts; the bare Super key comes from here (our workaround listens to its signal) | our own shortcut daemon, or the compositor's bindings once we own it | M | Wayland has no standard "global shortcut" protocol; it is the compositor's job. |
| `kscreen` + `kscreen-doctor` | display layout, scale, refresh; Settings > Displays and the panel read it | Settings talks the compositor's output-management protocol (`kde_output_management_v2` today, `wlr-output-management` or ours later) | M | The 6.8 JSON change (no `enabled` key) already bit us once; the selfcheck watches it. |
| `xdg-desktop-portal` + `-kde` (+ `-gtk`) | the open/save dialogs, screenshot, screencast, settings, inhibit that every app asks for | our own portal backend: **Minos as the file chooser** first | L | Roadmap item 3, the heart of the privacy model (per-app permissions). Start with `FileChooser`. |
| `kwalletd6` + `kwallet-pam` | secret storage (browser and app passwords) | do NOT write our own: **gnome-keyring** (decided with the owner 2026-10-16; the owner uses Proton Pass as the password manager), unlocked at login by PAM | S | A secrets store is the worst thing to get wrong. `gnome-keyring` is installed and the `pam_gnome_keyring.so` lines are already in `/etc/pam.d/sddm*` (next to kwallet's). KDE's `ksecretd` currently serves `org.freedesktop.secrets` and holds 5 items: the browsers' "Safe Storage" keys (Chrome, Chromium), and the GitHub CLI token (twice). They must be copied first: **`labyrinth-keyring-migrate`** (`--check` lists them without secrets). **Done 2026-10-16** (the owner allowed me to type their password into the tool): 4 items copied, KDE's service off, `gh` works; the daemon started by the tool died with its terminal and was restarted detached (`setsid`). `passwd` does not update the keyring until `/etc/pam.d/passwd` gets `-password optional pam_gnome_keyring.so use_authtok` (an edit I may not make; the line is in `profile/airootfs/etc/pam.d/passwd`). Log out and in once. |
| `ksmserver` / `startplasma-wayland` | starts the session, autostart, logout | a small session script + a systemd user target | M | After the compositor question is settled. |
| `sddm` (+ `sddm-helper`) | login screen | `greetd` with a Labyrinth greeter (greetd is a plain protocol; the greeter is our own Rust program, same look as the locker) | M | Roadmap item 5. |
| `baloo_file` | file indexing (CPU and disk, and a privacy question: it reads everything) | nothing: Minos has its own search | S | **Quick win, not done:** `balooctl6 disable`. Reversible. |
| `kactivitymanagerd`, `kaccess` | activities, accessibility bell | drop (accessibility needs a plan before `kaccess`/`screenreader` go) | S | |

## Installed KDE packages that matter (explicit)

`plasma-desktop`, `plasma-workspace`, `kwin`, `kscreen`, `kscreenlocker`, `sddm`, `xdg-desktop-portal-kde`, `kwallet-pam`, `plasma-nm`, `plasma-pa`,
`plasma-firewall`, `plasma-systemmonitor`, `plasma-browser-integration`, `kdeplasma-addons`, `kde-gtk-config`, `breeze`, `breeze-gtk`, `whitesur-kde-theme`
(AUR), `kwin-scripts-krohnkite` (tiling), `spectacle` (screenshots), `kdenlive`. About 78 packages carry a KDE or Qt-framework name; the
frameworks stay as long as any Qt application (Kdenlive, Wireshark, Vivaldi's Qt integration, Ardour) is installed.

## What our own programs depend on in KDE (these are the real coupling)

- Panel: `plasma-window-management` (task list), `org_kde_plasma_shell` (the dock as a panel window), `kglobalaccel` signal (Super key), KWin scripting
  (tiling, labyrinth-screen-keys), `kscreen-doctor -j` (layout), `plasma-apply-wallpaperimage` and `plasma-apply-colorscheme` (look), `kwriteconfig6` /
  `kreadconfig6` (power and lock settings), `qdbus6`/`busctl` to `org.kde.KWin`.
- Settings: `kscreen-doctor`, PowerDevil and kscreenlocker config files, `plasma-apply-colorscheme`, Kvantum.
- Minos: none of KDE's libraries (egui), but the portal and drag-and-drop paths go through KWin.
Each of these is a small adapter; the plan is to hide each behind one function so a replacement changes one place.

## Recommended order of work (what I would do with a free day, and why)

1. **labyrinth-idle** (observe first, then act): fixes the idle/lock/blank mystery with data, and it is the first roadmap item. *Started.*
2. **Wallpaper surface** (S): makes plasmashell optional; unblocks stopping it.
3. **Polkit agent + locker** (M each, share a UI): the two security-sensitive pieces, tested together in the VM.
4. **Tray host (StatusNotifierWatcher)** (M): the bar's biggest missing feature for everyday apps.
5. **Standard-protocol panel** (M): swap the KWin-only protocols for the standard ones; this is what makes the compositor replaceable.
6. **Portal backend: FileChooser** (L): Minos as every app's file dialog.
7. **greetd greeter** (M), then **shortcut daemon** (M), **output management** (M).
8. **Compositor** (XL), only when everything above is solid.

Quick wins that need no code (decide when you are back): `balooctl6 disable`; drop the unused `kded6` modules; use `gnome-keyring` or KeePassXC instead of `kwalletd6`.

## labyrinth-idle: first findings (2026-10-16)

`packages/idle` is running in observe-only mode (autostart on the owner's account). It logs to `~/.local/state/labyrinth/idle.log` when the user has been
idle for 30 s, 1, 2, 3, 4, 5, 7, 10, 15, 30 and 60 minutes, with whether the screen is locked and which programs hold logind sleep/idle inhibitors. First line
seen: `idle 30s  locked=false inhibitors=[NetworkManager(sleep), UPower(sleep), code(sleep), compositor(sleep)]`: VS Code and KWin hold *sleep* inhibitors,
nothing holds an *idle* inhibitor (so nothing stops the screen from dimming, blanking or locking). Together with `labyrinth-lockwatch`'s log this should name
what blanks and locks the screen; read both after the next time it happens. To make it act (lock, display off, suspend) write `~/.config/labyrinth/idle.conf`
(`mode = act`, then `lock = 600` etc.) and turn the matching PowerDevil/kscreenlocker settings off first, or both will act.
