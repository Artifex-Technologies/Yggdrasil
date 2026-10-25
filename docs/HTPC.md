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

`yggdrasil-player` (packages/player; shown as "VLC": VLC in Yggdrasil's look, the default for videos) is the desktop player the
HTPC will build on: libVLC 3 plays (all VLC-specific code in `src/vlc.rs`), the window around the picture is Yggdrasil's own,
and the screen does not dim or lock while a video plays. On the desktop it simply plays from the start.

Continue / start over belongs to the HTPC only (owner, 2026-10-24). A first version of it (a per-video place store that skips
the first minute and the credits) was in the desktop player briefly and is in its git history (`packages/player/src/resume.rs`,
removed in the commit after 3de2553) to be reused for the HTPC library.

## Still to decide

- The library has its own folder with a fixed layout (decided 2026-10-24): movies, and shows with a folder per season and the
  episodes in it, roughly `Movies/<Title (Year)>/…` and `Shows/<Show>/Season NN/<episode files>` (exact names to settle when the
  HTPC work starts). Still open: whether that folder may span several drives or a network share, and how posters and titles are
  found (local files only, or an online film database, which would be the first online lookup and needs a privacy decision).
- The HTPC as a separate session on the login screen ("Yggdrasil TV") or as a full-screen mode of the normal desktop.
- Remote: controller only, or also a TV remote over HDMI-CEC or an IR receiver.
