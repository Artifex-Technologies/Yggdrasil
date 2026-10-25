# Third-party notices

Yggdrasil's own code is under the Mozilla Public License 2.0 ([LICENSE](LICENSE)). What it is built on, or follows, keeps its own
licence, listed here.

- **EdenExplorer** (MIT, mtucciarone): its feature set and UX are the specification for `packages/minos`. No code is copied; if any is, its MIT copyright
  notice is kept here.
- **Omarchy** (MIT, David Heinemeier Hansson): the desktop model; consumed as upstream where possible.
- **Omarchy Lock Screen Explorer** (MIT, (c) 2026 SirJul1337): the Split lock screen design (`packages/lock`) is redrawn natively after it; no QML or Quickshell code is used.
- **omarchy-nightlight** (MIT, (c) 2026 Jeremy Langer): Night Light (the notch and Settings > Displays, `packages/yggdrasil-conf/src/nightlight.rs`) follows its design; the fade (one-minute hyprsunset profiles, even steps in mireds, cut to fit the schedule) is adapted from its `Panel.qml`.
- **Smithay** (MIT): the library `packages/compositor` is built on.
- The Rust crates each program depends on are listed in its `Cargo.lock`; `cargo audit` runs in CI.
- **Material Design Icons** (Apache 2.0, Pictogrammers): `night-light-symbolic` in `packages/liki/share/symbolic` is its `theme-light-dark`, the icon omarchy-nightlight shows.
- **Lucide** (ISC, Lucide Contributors; portions (c) Cole Bemis 2013-2022, Feather, MIT): the line icons in `packages/liki/share/symbolic` (volume, Wi-Fi, Bluetooth, power and the like), taken from `lucide-static` 0.468.0 and renamed to the freedesktop symbolic names.
- **Omarchy plugins rewritten in Rust** (all MIT; their designs and behaviour followed, code written anew unless noted):
  - omarchy-agent-usage (robzolkos): `packages/agent-usage`.
  - omarchy-proton-authenticator ((c) 2026 DevSviat): `packages/authenticator`; its `pa-helper` is built unchanged from a pinned
    commit and installed with its own MIT licence.
  - omarchy-snipboard ((c) 2026 Jordan Kicklighter): `packages/clipboard`.
  - omarchy-timer ((c) 2026 nickmdrummer): `packages/timer`.
  - omarchy-speedup-browser-startup ((c) 2026 agentrby): `packages/browser-warm`.
  - omarchy-health-mode ((c) 2026 musgandapur645-sudo): `packages/health`; its reminder messages and default cadences are used, with
    its licence in `packages/health/data/UPSTREAM-LICENSE`.
  - Bluechip ((c) 2026 Chizzler), bt-battery ((c) 2026 joe93), omarchy-plugin-dualsense ((c) 2026 Atos Lins): `packages/gamepads`
    and `packages/controllers`.
  - omarchy-2048 ((c) 2026 Imran H.), after gabrielecirulli/2048 (MIT, (c) 2014 Gabriele Cirulli): `packages/game-2048`.
  - omarchy-sudoku-plugin ((c) 2026 Morganczech (gruut)): `packages/sudoku`.
- **omarchy-weather-nowcast** (Hizoul, GPL-3.0): only the idea of the rain view in `packages/weather`; no code is taken from it.
- **Simple Icons** (CC0-1.0): the brand logos in `packages/liki/share/brand-icons`.
- **Microsoft PowerToys** (MIT, (c) Microsoft Corporation): `packages/colorpicker` follows its Color Picker (a frozen screen, a loupe, copy formats); no code is taken from it.
