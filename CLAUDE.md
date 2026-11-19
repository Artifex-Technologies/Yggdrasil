# Yggdrasil: notes for AI sessions

## What this is

Yggdrasil is a desktop environment written in Rust: the desktop of Labyrinth OS, built to be reused by other systems (LykanOS),
so nothing here may depend on one operating system (what an OS adds comes through integration files, docs/INTEGRATIONS.md).
Every program is its own Cargo project under `packages/` (panel, compositor, settings, session, lock and login screens, Minos,
Agora, installer, and the small apps and helpers); the README's table lists them. It runs today as a session on Hyprland
(`packages/session`); its own compositor (`packages/compositor`, Smithay) is in progress.

## Build and test

Every package builds into the one shared `target/` at the top (`.cargo/config.toml`). Always cap builds and tests:

    cd packages/<name>
    systemd-run --user --scope -q -p MemoryMax=8G -p MemorySwapMax=0 env CARGO_BUILD_JOBS=4 nice -n 19 cargo test --quiet
    systemd-run --user --scope -q -p MemoryMax=8G -p MemorySwapMax=0 env CARGO_BUILD_JOBS=4 nice -n 19 cargo build --release

Run one test by name (`cargo test --quiet --lib NAME`) rather than a whole crate when that is all a change touches. The
compositor is the heaviest crate; build it only when it changed.

CI (`.github/workflows/checks.yml`, an Arch container, every push): for every crate `cargo fmt --check`, `cargo clippy
--all-targets -- -D warnings`, `cargo test` and `cargo audit` (the ignored advisories are listed with their reason in the
workflow); the panel's leak stress test on headless sway and on the own compositor; Yggdrasil's programs on the headless
compositor; the login screen in cage on three screens; the session's Lua and the package recipes parsed. After a push, watch
the run for the full commit: `gh run list --commit <sha>`.

Package recipes: `packages/pkgbuilds/<name>/PKGBUILD` (`makepkg -f` there). `VERSION` is the system version shared with
Labyrinth OS; packages are `VERSION-<build number>`.

## Rules

- Privacy: no personal details in commits, code or docs: no names of people, account names, hostnames, IPs, MACs, SSIDs,
  serials or emails. Write docs in the first person as the owner ("I decided"), and "Sarina", not "her". Scan the staged diff
  before every push.
- Push as you go; stage files by explicit path, `git pull --rebase` before pushing.
- Never touch the running desktop, its displays or input devices from a session or a test: the compositor is tried headless,
  nested, or in a virtual machine, never on the live session; tests use stand-ins and fixtures, never the real system.
- Yggdrasil must stay light and quick on any machine, down to a Raspberry Pi (aarch64): nothing drawn while nothing changes, no
  busy polling, measure before and after (docs/COMPOSITOR.md, "Lightness"; docs/PORT_NUMBERS.md).
- Everything of ours is Rust. No KDE: nothing of it is started, called or linked.
- Public docs describe what is built, how to use and test it, and the proof; a short "Known limitations" is allowed, plans
  and to-do lists are not.

## Contracts

- What Yggdrasil provides (integration files, the appearance and settings files, the compositor's headless control socket)
  and what it uses from Heimdall and Sarina: [docs/CONTRACTS.md](docs/CONTRACTS.md).
- Heimdall's and Sarina's own contracts: their repositories' docs/CONTRACTS.md.

## Open work

docs/plans/BACKLOG.md and docs/plans/LOG.md (private; ask the owner).

## Know-how

- **Generated docs.** The desktop part of `docs/SHORTCUTS.md` is generated from `packages/session/hyprland/yggdrasil.lua`;
  the test `the_shortcuts_doc_is_current` (packages/yggdrasil-conf, src/shortcuts.rs) fails when it is stale.
  `YGGDRASIL_WRITE_DOCS=1 cargo test --lib the_shortcuts_doc_is_current` in packages/yggdrasil-conf rewrites it. Hand-written
  text lives only after `## In the apps`. The screenshots in the README and docs/ come from `scripts/screenshots.sh` (on the
  headless compositor, about 10 minutes).
- **The shared look.** `packages/yggdrasil-ui` holds the egui apps' palette, widgets, fonts (`fonts.rs`, the system font from
  fontconfig), accessibility (`theme::apply_accessibility`, called every frame), drop-downs and the privacy prefs; change the
  look there, not per app. `packages/yggdrasil-conf` reads and writes the desktop's settings files.
- **The compositor.** `packages/compositor/README.md` has the headless mode (`--headless`, `ctl` commands), its tests, the gap
  to Hyprland item by item, and the hardware backend (`--drm`, tried in a virtual machine: `tests/drm-vm.sh`).
  `YGGDRASIL_COMPOSITOR_DEBUG=1` prints input and move requests.
- **Tools talk to the compositor through `packages/wm`** (yggdrasil-wm: Hyprland's socket or the own compositor's control
  socket, whichever runs); do not call `hyprctl` from a new tool.
- **Settings fixtures.** A debug build of Settings reads made-up systems from `packages/settings/tests/fixtures`
  (`system.json`, `laptop.json`, `wifi.json`, `peripherals.json`).
- **Bar add-ons** are `bar-item` integration files (docs/INTEGRATIONS.md), not code in the panel.
