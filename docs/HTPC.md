# Yggdrasil HTPC (plan)

A living-room mode of Yggdrasil: run from the couch with a game controller or a TV remote, in the spirit of the Xbox dashboard
(and SteamOS's Big Picture, with an Xbox-like look). Planned 2026-10-24; nothing below is built yet except the player's core.

## What it should do

- **Home screen, Xbox-style.** Rows of large tiles scrolled with a controller or remote: continue watching, movies, games,
  apps. Everything reachable with a D-pad, A/B (or OK/Back) and the shoulder buttons; no small text, no mouse needed.
- **Games through Steam.** Installed Steam games as tiles (from Steam's library files), started with `steam://rungameid/<id>`.
  "Get more games" opens Steam's Big Picture on the store to install. Nothing of Steam's is rebuilt.
- **Discord, as on Xbox and PS5.** Who is online and what they play, joining a voice channel, with the call kept going while
  a game or movie runs. Through Discord's own client or its official interfaces only: nothing that breaks Discord's terms.
- **Your movies, like a streaming service.** The owner's own film collection on the system: a library with posters, and for
  each film "Continue from 1:02:13" / "Start over", remembered per film. Playback is Yggdrasil's player (below).
- **Streaming apps through Waydroid.** Android streaming apps (Netflix, Disney+ and others) running in Waydroid, shown as
  tiles like any other app. Note to check: some streaming apps limit picture quality or refuse to run without Google's device
  certification; which ones work in Waydroid has to be tested app by app.

## Built so far: the player

`yggdrasil-player` (packages/player) is the desktop player the HTPC will reuse:

- VLC plays (libVLC 3, the stable series; all VLC-specific code in `src/vlc.rs`, so a move to libVLC 4 later touches one file),
  and the window around the picture is Yggdrasil's own.
- **Continue where you left off:** `src/resume.rs` keeps each video's place (~/.local/state/yggdrasil/player.json), skips the
  first minute and the credits, and forgets a film watched to the end. Opening a film with a saved place asks "Continue from
  … / Start over"; the start screen lists what you were watching. The HTPC library uses the same store.
- While a video plays, the screen does not dim or lock (a logind idle lock).

## Still to decide

- Where the films live (one folder, several drives, a network share) and how posters and titles are found (local files
  only, or an online film database, which would be the first online lookup and needs a privacy decision).
- The HTPC as a separate session on the login screen ("Yggdrasil TV") or as a full-screen mode of the normal desktop.
- Remote: controller only, or also a TV remote over HDMI-CEC or an IR receiver.
