# yggdrasil-authenticator

Proton Authenticator's 2FA codes in a small Yggdrasil window (Rust, egui, yggdrasil-ui): type a few letters, press Enter, and the
code is on the clipboard, marked sensitive and cleared after 20 seconds.

Adapted from [devSviat/omarchy-proton-authenticator](https://github.com/devSviat/omarchy-proton-authenticator) (MIT). Its helper,
`pa-helper` (Rust), does everything that touches the vault and is used unchanged, built from a pinned, reviewed commit: it reads
the official app's local vault read-only, takes the key from the keyring (Secret Service), never uses the network, holds secrets
in locked memory, and answers one JSON line per command. This window only shows what it answers and asks it to copy.

Needs the official Proton Authenticator app (AUR `proton-authenticator-bin`), signed in and synced once.

    yggdrasil-authenticator          your codes
    yggdrasil-authenticator --demo   the helper's demo accounts (public RFC test seeds)

In the top bar: `authenticator.conf` (installed to `/usr/share/yggdrasil/integrations/`) puts a key there. Its card is drawn by the
panel (docs/INTEGRATIONS.md, "Cards you can use"): a search box, up to eight matching accounts with their codes, and a click copies
one. The panel runs `yggdrasil-authenticator --bar` only while the card is open (and again at each code rollover), and
`yggdrasil-authenticator press ID [TEXT]` for its controls; what is typed stays in the runtime folder (memory, gone at logout).

    yggdrasil-authenticator --bar --demo           the card with the demo accounts
    yggdrasil-authenticator press --demo search git
