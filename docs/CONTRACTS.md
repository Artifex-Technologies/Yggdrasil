# Contracts

What other projects may rely on from Yggdrasil, and what Yggdrasil relies on from them. One section per contract. Changing
one of these means changing its users in the same push, or keeping the old shape working.

## Provided by Yggdrasil

### Integration files (`/usr/share/yggdrasil/integrations/*.conf`, `/etc/yggdrasil/integrations/*.conf`)

How an operating system plugs into the desktop: `key = value` lines, one file per integration, kinds `assistant`, `packages`,
`security`, `bar-item`, `apps`, `branding`, `installer`, `settings-page`. Unknown keys are ignored, so an older Yggdrasil
accepts newer files. The full shape: [INTEGRATIONS.md](INTEGRATIONS.md).
Used by: Labyrinth OS (Sarina's `assistant` file, its installer, branding, package backend), LykanOS (its installer, Sarina),
Heimdall (`security`), Limbo (`apps`), the bar add-ons in this repository (`bar-item`).

### The assistant's update verdict (`update_verdict` in an `assistant` integration file)

A file whose first line is `approved` or `held`, then one `name old -> new` line per update tried; Settings > General >
Software Update reads it each time it shows the updates ([INTEGRATIONS.md](INTEGRATIONS.md), "assistant").
Used by: Sarina (the writer).

### The appearance file (`~/.config/yggdrasil/appearance.conf`)

Dark or light and the accent colour, written by Settings > Appearance (`yggdrasil-conf`), re-read by programs at most once a
second.
Used by: the Argus app (`argus-tools/crates/argus-app/src/theme.rs`), every Yggdrasil app through `yggdrasil-ui`.

### The desktop's other settings files (`~/.config/yggdrasil/*.conf`, `/etc/yggdrasil/*`)

Read through the `yggdrasil-conf` crate: `idle.conf` (the idle manager), `/etc/yggdrasil/greeter.conf` (the login screen's
design, session and wallpaper, [LOGIN_SCREEN.md](LOGIN_SCREEN.md)), and the power services' `/etc/yggdrasil/tuning.json`,
`case-fans.json`, `gpu-fan.json` ([packages/power-helpers](../packages/power-helpers/README.md)).
Used by: Labyrinth OS (its ISO profile ships defaults), LykanOS (`greeter.conf`, `idle.conf`).

### The compositor's headless mode and control socket (`yggdrasil-compositor --headless`)

Virtual outputs, a virtual keyboard and mouse, window lists and screenshots driven over a control socket, for tests only
(never the real devices): [packages/compositor/README.md](../packages/compositor/README.md), "The headless test mode".
Used by: the Argus app's `argus-tools/crates/argus-app/smoke-yggdrasil.sh` (the app against its fake Flipper, in CI).

## Used by Yggdrasil (provided elsewhere)

| Contract | Provider | Used here by |
| --- | --- | --- |
| `heimdall-fileguard scan --json` and `extract --json` | Heimdall | Settings > Appearance > Fonts (every added font file) |
| `~/.config/heimdall/privacy.json` (`downloads_guard`, `remove_hidden_info`, `lookalike_warnings`, ...) | Heimdall | Settings > Security > Privacy writes it (`yggdrasil_ui::privacy_prefs`); Minos, Settings and Agora read it |
| `heimdall-multilink status --json` | Heimdall | Settings > Wi-Fi & Network > Several connections at once |
| `sarina fleet-status --json` | Sarina | the Agents bar add-on (`packages/agent-usage`) |
