# yggdrasil-clipboard

Clipboard history and snippets in a small Yggdrasil window (Rust, egui, yggdrasil-ui). **Super+V** opens it; pressing it again, or
Esc, closes it. Type to search, Enter puts the pick on the clipboard and closes the window.

A rewrite in Rust of the ideas in [jwkicklighter/omarchy-snipboard](https://github.com/jwkicklighter/omarchy-snipboard) (MIT): a
search line over the list, and snippets (texts kept on purpose). It replaces the Clipboard section that Fylgja (the notch) had.

    yggdrasil-clipboard          the window
    yggdrasil-clipboard daemon   the history keeper (the user service yggdrasil-clipboard.service)

## History

The daemon watches the clipboard with `wl-paste --watch` while Settings > Appearance > "Clipboard history" is on, and keeps the last
50 texts (up to 64 KiB each) and the last 5 PNG pictures (up to 8 MiB each) **in memory only**. Copying the same thing again moves
it to the top. What a password manager marks as secret (wl-clipboard's `CLIPBOARD_STATE=sensitive`, KDE's
`x-kde-passwordManagerHint`) is skipped. Turning the setting off stops the watching and forgets everything at once (the setting is
read again within two seconds of a change); logging out forgets it too.

The window asks the daemon over a socket in the runtime folder, `$XDG_RUNTIME_DIR/yggdrasil-clipboard.sock` (mode 0600), one JSON
line each way: `{"op":"list"}`, `{"op":"copy","id":N}`, `{"op":"delete","id":N}`, `{"op":"clear"}`.

## Snippets

Kept in `~/.local/share/yggdrasil/clipboard/snippets.json` (owner-only), pinned first, then by use. Ctrl+N writes a new one,
Ctrl+E edits the selected one (on a copied text: starts a snippet from it), Ctrl+P pins a snippet or keeps a copied text as a
pinned snippet, Delete removes (with an empty search line; Ctrl+Delete always).

## Keys

Arrows move, Enter copies, Tab switches between History and Snippets, Esc closes. **Shift+Enter** also pastes into the window
underneath, only under Hyprland: the window closes, then Hyprland's own `sendshortcut` presses Ctrl+V there (Ctrl+Shift+V in a
terminal). Nothing types through a fake keyboard device; elsewhere Shift+Enter just copies.
