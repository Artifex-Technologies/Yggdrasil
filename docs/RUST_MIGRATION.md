# Rust migration: what was ported and what it measured

The full record is [PORT_NUMBERS.md](PORT_NUMBERS.md): the method, every case and each port since the table below.

Each program I ported from Python, against the Python it replaced, on the same inputs (2026-11-11, my desktop, 16 threads).
How it was measured and every case: [PORT_NUMBERS.md](PORT_NUMBERS.md). Each package's README has its own table,
the bugs the port fixed and where the Rust one is worse.

| Program | Time, Python → Rust | Memory, Python → Rust | More |
| --- | --- | --- | --- |
| tune, casefan, gpufan, powerlimit | `casefan get` 39.9 → 2.16 ms (−95 %) | the four loops (no card): 35.4 → 0.9 MB PSS (−98 %), 650 → 170 wake-ups a minute | [power-helpers](../packages/power-helpers/README.md) |
| account, bootscreen, game-fix, gpu-run | `account list` 23.7 → 0.50 ms (−98 %); `gpu-run list` 32.1 → 9.14 ms (−72 %) | 13.5–15.0 → 2.5–4.6 MB; a boot picture 130 → 70 MB (−46 %; it was 189 MB, +46 %, until 2026-11-25) | [settings-helpers](../packages/settings-helpers/README.md) |
| labyrinth-appusage | start 18.5 → 1.1 ms CPU (−94 %) | 7.0 → 0.6 MB PSS (−92 %) | [appusage](../packages/appusage/README.md) |
| the crash watch | start 49.7 → 0.61 ms CPU (−99 %) | 22.9 → 0.8 MB PSS (−97 %) | [crashwatch](../packages/crashwatch/README.md) |
| pkg-root | a refused command 20.0 → 0.44 ms (−98 %) | 12.8 → 2.4 MB (−81 %) | [agora-helpers](../packages/agora-helpers/README.md) |
| ygg-theme | `gtk4-theme` 116 → 32.9 ms (−72 %) | 57.6 → 32.3 MB (−44 %) | [theme](../packages/theme/README.md) |
| ygg-cursors | the whole theme 1028 → 40.3 ms (−96 %), 1026 → 115 ms CPU (−89 %; it was 1604 ms, +52 %, until 2026-11-25) | 31.6 → 16.1 MB (−49 %) | [theme](../packages/theme/README.md) |
| liki-build, liki-icns, liki-icon-import | a pack of 3,000 icons 127 → 30.3 ms (−76 %) | 22.5 → 5.7 MB (−74 %) | [liki](../packages/liki/README.md) |
| yggdrasil-install-system | `--check` 35.6 → 0.72 ms (−98 %); a whole install (stand-ins) 106 → 62.4 ms CPU | 18.0 → 3.6 MB (−80 %) | [install-system](../packages/install-system/README.md) |
| the lights hook (shell; now inside yggdrasil-idle) | lights-off 100 → 2.82 ms (−97 %), lights-on 20.5 → 0.91 ms (−96 %) | 175 processes per lights-off → 0 | [idle](../packages/idle/README.md) |
| the session's, panel's, Liki's, Agora's and spell check's last 11 shell scripts | `liki-icon --import` 32.4 → 2.43 ms (−93 %); the plugin loader at login 20.3 → 3.01 ms (−85 %); a key press (panelctl) 4.19 → 3.61 ms | the plugin loader 11.1 → 3.8 MB (no Python) | [PORT_NUMBERS](PORT_NUMBERS.md#the-desktops-last-shell-scripts-2026-11-12) |
| the session's launcher, `yggdrasil-session` (bash) | to Hyprland 8.11 → 3.13 ms (−61 %); 44 processes a minute while the session runs → 0 | 4 processes, 9.5 MB RSS → 1 process, 2.7 MB | [PORT_NUMBERS](PORT_NUMBERS.md#the-sessions-launcher-yggdrasil-session-2026-11-13) |
| gpuunlock (root; the graphics card unlock) and its grub.d fragment | `status --json` 55.3 → 1.69 ms (−97 %), 52.5 → 0.49 ms with no NVIDIA card; the fragment while armed 6.41 → 1.49 ms, with nothing armed 0.89 → 1.44 ms (**+0.55 ms**) | 23.6 → 3.9 MB (−83 %) | [settings-helpers](../packages/settings-helpers/README.md), [PORT_NUMBERS](PORT_NUMBERS.md#the-graphics-card-unlock-gpuunlock-2026-11-13) |

On disk the binaries are bigger than the scripts (0.3 to 1.3 MB each against a few KB), but the scripts needed Python
(75 MB), and Agora, the theme package, Liki, the session, the installer and Settings no longer do.
