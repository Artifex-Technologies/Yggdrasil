# Integrations: how an operating system plugs into Yggdrasil

Yggdrasil never names an operating system or its programs. What an OS adds (an assistant, a security monitor, a package backend,
its branding) it describes in **integration files**; Yggdrasil reads them and shows the matching features. With no file,
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
name = Labyrinth                   # whose catalog it is: "Reviewed by Labyrinth"
catalog = /usr/share/labyrinth/agora/catalog.json     # the OS's reviewed catalog (format: docs/MARKETPLACE.md)
components = /usr/share/labyrinth/components.json     # the OS's own optional software (id and packages of each)
component = /usr/local/bin/labyrinth-apps             # run with `install ID` or `remove ID`; AGORA_SUDO names a password-box sudo
own_builds = /usr/share/labyrinth/pkgbuilds           # one folder per package the OS builds itself: never updated from the AUR
edition = labyrinth                # optional: which entries of a shared catalog apply (default: ID in /etc/os-release)
```

Without one, Agora lists only the sources the user adds (each marked as not reviewed) and Flathub.

### `security` (a security monitor)

```
kind = security
name = Heimdall
api = http://127.0.0.1:8930                # GET /healthz answers "ok"; GET /v1/alerts?n=N lists alerts. Loopback addresses only
scan = /usr/local/bin/labyrinth-security-setting scan   # + a folder; prints "INFECTED path: Signature" lines and a "scanned ..." line
quarantine = /usr/local/bin/labyrinth-quarantine        # `list --json`, `restore ID`, `delete ID`
distress = /usr/bin/labyrinth-distress     # optional: the distress password's root helper (run with pkexec), Settings and the lock screen
distress_set = /etc/labyrinth/distress.json   # when this exists, a wrong password at the lock screen is shown to the helper
contained = /etc/heimdall/contained        # optional: when this file exists the assistant is contained (JSON "reason", "level")
release = /usr/bin/heimdalld contain release   # run with pkexec: put back the approved files and lift the lock
held_release = /usr/bin/heimdalld immunity release   # optional: its own quarantine (GET /v1/quarantine on the api); run with pkexec + ID
held_confirm = /usr/bin/heimdalld immunity confirm   # with held_release: "It's malware" for a held file (pkexec + ID)
switch.a_auto_quarantine = Move certain malware to quarantine automatically | ... | /usr/local/bin/labyrinth-security-setting auto-quarantine
```

An alert is `{"time": RFC 3339, "detector_id", "severity": "info" | "warning" | "critical", "summary", "event": {"Fields": {...}}}`
(`process`, `remote_addr`, `remote_port` and `app` are used when present). The panel turns them into notifications under `name`
(Settings > Notifications chooses how many); Settings > Security gets a page named `name` with the state, alerts, new connections,
the quarantine, the scan, the declared controls and, while the assistant is contained, a "Restore and release" button, and what the monitor holds in its own quarantine with Release and It's malware. Each part appears only when its key is there.

### `bar-item` (a bar add-on)

```
kind = bar-item
name = Agent usage
command = /usr/bin/yggdrasil-agent-usage --bar   # prints one JSON object (below)
interval = 300                                    # seconds between runs, at least 60; it also runs when its card is opened
```

The panel runs the command on a thread of its own (30-second limit, 64 KiB of output) and shows a pill in the top bar; a click opens
its card. The command prints `{"text": "...", "state": "ok" | "warn" | "alert" | "off", "lines": [{"title", "value", "percent"
(0-100, draws a meter), "note", "alert"}], "help": "..."}`. "off" or no text hides the pill; warn is amber, alert red. Text is
drawn as given (cut short, control characters removed): an add-on can show things, never run anything through the panel. Add-ons
are installed as packages (Agora lists them under Plugins), so removing the package removes the pill.

### `apps` (an app launcher with per-app rules)

```
kind = apps
launch = /usr/local/bin/labyrinth-launch           # + a desktop id; exits 0 when it started the app (dock, start menu, Settings)
unit_prefix = lapp_                                # its systemd units: PREFIXid.slice per app, PREFIXid_N.scope per run
settings = %h/.config/labyrinth/apps               # ID.json per app, read by the launcher: {"sandbox", "files": "all" | "folders",
                                                   #   "folders", "see_others"}; written by Settings > Apps
usage = %h/.local/share/labyrinth/app-usage.json   # {ID: {"launches", "last", "seconds"}}: "sort by usage"
network = /usr/local/bin/labyrinth-appnet          # run with pkexec: `set ID allow|block|ethernet|wifi|ask`
network_rules = /etc/labyrinth/app-network.json    # {"rules": {UID: {ID: mode}}}, readable by the user
network_ask = /run/labyrinth/appnet-ask.sock       # {"op":"list"} and {"op":"forget","app","ip"}, one JSON line each way
```

Without one, apps start as their desktop entries say, and Settings > Apps leaves out the network and sandbox sections. `%h` is the
home folder.

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
Proton Drive in Minos is Minos's own feature too (an rclone mount; its user unit ships with Minos and runs only once you connect).
Those helpers live in Yggdrasil under `yggdrasil-*` names.
