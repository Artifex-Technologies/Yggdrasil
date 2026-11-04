# Huginn

Yggdrasil's video app: videos without an account, laid out like Agora. Named after Odin's raven that flies out over the world each day
and brings back what it saw.

- **Home:** the newest videos from your subscriptions (no account: they are kept on this computer).
- **Search, channels, saved videos, history.**
- **Play** in yggdrasil-player (VLC underneath; the picture and the sound come as two streams above 720p), **Listen** to the sound
  only, **chapters** play from their start, **downloads** of a video or its sound into `~/Videos/Huginn`.
- **Bring your subscriptions** from NewPipe (its export), FreeTube or OpenTubeX (`profiles.db`) or Google Takeout
  (`subscriptions.csv`); export them for NewPipe again.
- `huginn LINK` opens a YouTube video or channel link; `huginn --search WORDS` searches.

How it works: `huginn-core` (no UI, so an iOS app can use it later) has the library and one small extractor interface; on the desktop
the extractor is [yt-dlp](https://github.com/yt-dlp/yt-dlp), which keeps up with YouTube's changes. Pictures come only from YouTube's
picture hosts. Your subscriptions, history and saved videos are in `~/.local/share/yggdrasil/huginn`; settings in
`~/.config/yggdrasil/huginn.conf`.

Written from the features of NewPipe (GPL-3.0) and OpenTubeX (AGPL-3.0), not from their code.

Note: YouTube's terms do not allow third-party apps or downloading; like NewPipe, FreeTube and yt-dlp, Huginn is offered for people to
decide for themselves.
