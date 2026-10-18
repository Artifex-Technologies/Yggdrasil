# Third-party notices

- **EdenExplorer** (MIT, mtucciarone): its feature set and UX are the specification for `packages/minos`. No code is copied; if any is, its MIT copyright
  notice is kept here.
- **Omarchy** (MIT, David Heinemeier Hansson): the desktop model; consumed as upstream where possible.
- **Omarchy Lock Screen Explorer** (MIT, (c) 2026 SirJul1337): the Split lock screen design (`packages/lock`) is redrawn natively after it; no QML or Quickshell code is used.
- **Smithay** (MIT): the library `packages/compositor` is built on.
- The Rust crates each program depends on are listed in its `Cargo.lock`; `cargo audit` runs in CI.
