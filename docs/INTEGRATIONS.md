# Integrations: how an operating system plugs into Yggdrasil

Yggdrasil never names an operating system or its programs. What an OS adds (an assistant, a security monitor, a package backend, a
cloud drive, its branding) it describes in **integration files**; Yggdrasil reads them and shows the matching features. With no file,
the feature is simply not there: Yggdrasil works on its own.

## Where

- `/usr/share/yggdrasil/integrations/*.conf`: shipped by the operating system's packages.
- `/etc/yggdrasil/integrations/*.conf`: the administrator's additions or overrides (same name wins).

One file per integration, plain `key = value` lines (`#` comments), the same style as Yggdrasil's other settings. Unknown keys are
ignored, so an older Yggdrasil ignores newer keys. Every command is run directly (no shell), with arguments split on spaces; a value
that is a path must be absolute.

## Kinds

Each file starts with `kind = ...`.

### `assistant` (a voice or chat assistant)

```
kind = assistant
name = Sarina                      # shown in the panel, Fylgja and Settings
ask = /usr/local/bin/labyrinth-sarina-ask        # hold-to-talk: the recording's path is added as the last argument; prints the answer
speak = /usr/local/bin/labyrinth-sarina-speak    # reads text aloud (text on standard input); `--stop` stops it
warm = /usr/local/bin/labyrinth-sarina-warm      # optional: wake the model while you are still talking
runtime = labyrinth-sarina         # folder in $XDG_RUNTIME_DIR where it reports speaking.pid and conversations (conv-ID.session)
shortcut = Super+S                 # the hold-to-talk key Yggdrasil registers (Ctrl+Alt+S is added as a second way in)
```

Yggdrasil owns the key, the recording, Fylgja's listening and speaking animation and the answer card; the assistant owns the rest.
Its settings are declared with the controls below and appear on Settings' Assistant page.

### `packages` (how Agora installs software)

```
kind = packages
install = /usr/bin/labyrinth-apps install     # + package or component names
remove = /usr/bin/labyrinth-apps remove
update = /usr/bin/labyrinth-apps update
components = /usr/share/labyrinth/components.json   # the OS's own optional software, shown in Agora
catalog_edition = labyrinth        # which entries of a shared catalog apply
```

Without one, Agora shows Flatpak and leaves system packages alone.

### `security` (a security monitor)

```
kind = security
name = Heimdall
alerts = http://127.0.0.1:8930/v1/alerts   # read-only, loopback only
scan = /usr/bin/heimdalld scan-files       # Settings > Security > Scan a folder
quarantine = /usr/local/bin/labyrinth-quarantine
```

### `cloud` (a cloud drive in Minos)

```
kind = cloud
name = Proton Drive
mount = /home/%u/ProtonDrive               # %u is the user
service = /usr/share/labyrinth/proton-drive.service   # user unit Minos installs and starts
sign_in = /usr/bin/minos-cloud sign-in proton
```

### `branding`

```
kind = branding
accent = #7c54d6                   # the default accent until the user picks one
wallpaper = /usr/share/wallpapers/labyrinth/labyrinth-purple.png
```

The operating system's name and logo come from `/etc/os-release` (`NAME`, `LOGO`), not from here.

### `settings-page` (an OS page in Settings)

```
kind = settings-page
title = Restore points
icon = document-revert
open = /usr/local/bin/labyrinth-restore --ui
```

A row in Settings' list that opens the OS's own window; Yggdrasil does not draw the OS's pages.

## Settings controls (any kind)

Any integration can add controls to its Settings page (Assistant for `assistant`, Security for `security`, otherwise a page named by
`name`). Each is one line; the value is `label | description | command`. The command is run with one more argument:

- `switch.ID = label | description | /path/cmd`: `status` prints `on` or `off`; `on` and `off` change it.
- `choice.ID = label | description | /path/cmd`: `list` prints one option per line; `get` prints the current one; `set OPTION` sets it.
- `action.ID = label | description | /path/cmd`: run as is when the button is pressed (output, if any, is shown).

Controls appear in the order of their IDs. A command that fails or is missing hides its control; Settings never guesses.

## What stays out of integrations

Anything every system needs is Yggdrasil's own: media keys, screen keys, user accounts, power limits, printers, the app launcher.
Those helpers live in Yggdrasil under `yggdrasil-*` names.
