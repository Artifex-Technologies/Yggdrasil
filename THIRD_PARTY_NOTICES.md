# Third-party notices

- **EdenExplorer** (MIT, mtucciarone): its feature set and UX are the specification for `packages/minos`. No code is copied; if any is, its MIT copyright
  notice is kept here.
- **Omarchy** (MIT, David Heinemeier Hansson): the desktop model; consumed as upstream where possible.
- **Omarchy Lock Screen Explorer** (MIT, (c) 2026 SirJul1337): the Split lock screen design (`packages/lock`) is redrawn natively after it; no QML or Quickshell code is used.
- **omarchy-nightlight** (MIT, (c) 2026 Jeremy Langer): Night Light (the notch and Settings > Displays, `packages/yggdrasil-conf/src/nightlight.rs`) follows its design; the fade (one-minute hyprsunset profiles, even steps in mireds, cut to fit the schedule) is adapted from its `Panel.qml`.
- **Smithay** (MIT): the library `packages/compositor` is built on.
- The Rust crates each program depends on are listed in its `Cargo.lock`; `cargo audit` runs in CI.
- **Material Design Icons** (Apache 2.0, Pictogrammers): `night-light-symbolic` in `packages/liki/share/symbolic` is its `theme-light-dark`, the icon omarchy-nightlight shows.
- **Lucide** (ISC, Lucide Contributors; portions (c) Cole Bemis 2013-2022, Feather, MIT): the line icons in `packages/liki/share/symbolic` (volume, Wi-Fi, Bluetooth, power and the like), taken from `lucide-static` 0.468.0 and renamed to the freedesktop symbolic names.
