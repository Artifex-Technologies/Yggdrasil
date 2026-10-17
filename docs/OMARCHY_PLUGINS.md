# Using the Omarchy plugin marketplace from Labyrinth

Findings from 2026-10-17 (read from the marketplace's public repository `omacom/omarchy-plugin-marketplace`; nothing was installed or run).

## What the marketplace is

- plugins.omarchy.org / omarchyplugins.com is a **catalog**: a static site plus `registry.json` (about 11 MB, 5,603 listings on this date). Each entry points at a public GitHub repository and a commit that was checked when it was listed.
- A plugin is a folder with `manifest.json` (id, version, kinds, entry points) and **QML files** for **Omarchy's "Quattro" shell**, which is built on **Quickshell** and runs inside Omarchy's one long-lived shell process. Kinds: `bar-widget`, `panel`, `overlay` (full screen), `menu`, `service` (headless), `bar` (whole bar). The QML imports Omarchy's own modules (`qs.Ui`, `qs.Commons`) and talks to the shell over its IPC (`omarchy-shell lock explore`). Install is `omarchy plugin add <git url> --enable`, which **clones the repository's moving HEAD**.
- The catalog's own notice: plugins **run unsandboxed with your user's permissions**; the automated check is a static pattern scan, not an audit. Capabilities the scan flagged across the catalog: installer 1,293, privilege (sudo) 1,137, package-manager 1,109, service-management 933, remote-build 792, bundled binary 56, **sudoers modification 54**.
- Mix: Widgets 1,540, Productivity 1,131, System 899, Hardware 539, Desktop 504, Developer tools 429, Appearance 425; 214 are tagged `games` (Snake, Minesweeper, Tetris, Solitaire, Blackjack ...); 3,866 are tagged `quickshell`, 1,169 `hyprland`.
- The Lock Screen Explorer (`io.github.sirjul1337.lock-explorer`) is a `service` + `overlay` plugin of about 60 QML designs. It **replaces Omarchy's own lock service** and uses the compositor's session-lock protocol plus Omarchy's theme, `ttfx`, MPRIS, wttr.in, FIDO2 / fingerprint / face helpers. Its scan result: privilege, package-manager, service-management, installer.

## What that means for Labyrinth

The plugins are not programs we can just run: they are QML for a shell we do not have. Labyrinth's panel is Rust drawn with tiny-skia, with no QML engine. Three separate problems:

1. **Runtime.** Something has to host Quattro QML: Quickshell plus Omarchy's shell tree (`qs.Ui`, `qs.Commons`, theme and IPC services). Quickshell runs on KWin through layer-shell, so panels, overlays and floating widgets are feasible; bar widgets have no Omarchy bar to sit in, so they would be shown as floating windows from a launcher. Some plugins use Hyprland-only calls (workspaces, window rules) and would not work.
2. **Lock screens.** They need the compositor's `ext-session-lock-v1`, which the KWin on this machine does not offer (see KDE_INVENTORY.md, the locker row). Until we own the compositor, or KWin offers it, lock-screen plugins cannot replace our locker. Our own locker could still gain *designs* of its own.
3. **Trust.** Running 5,600 unreviewed, unsandboxed community plugins is the opposite of Labyrinth's model, and 54 of them edit sudoers. The host would have to run plugins the way `labyrinth-rea` runs Ghidra: a bubblewrap sandbox, no home directory, network through the per-app rules, privileged and installer capabilities refused, and each plugin **pinned to the commit that was reviewed** (not a moving HEAD).

## Proposed stages (each useful on its own)

0. **A store page** in Settings (next to Optional software): browse and search the catalog from `registry.json` (fetched on request, cached), filter by category and tag, show the verification state and the flagged capabilities in plain words, and hide or block the risky ones. No Omarchy code runs yet.
1. **Quattro host, sandboxed:** a vendored Omarchy shell tree plus Quickshell in a bubblewrap sandbox (`labyrinth-plugin-host`), loading one plugin per sandbox, pinned to a commit; start with games and panels (214 games). Per-plugin permissions shown in Settings > Apps like any app.
2. **Plugin lock designs for our locker**, as a separate piece of work (a Labyrinth design format and a few designs ported by hand), or the real plugins once a compositor offers session-lock.

CachyOS ships no app store of its own that I know of (it has a "Hello" welcome app with tweaks and a package list); Manjaro's is Pamac, a package-manager front end, which is a different thing from a plugin market. Labyrinth already has `labyrinth-apps` (optional software); stage 0 would be the plugin-sized version of it.
