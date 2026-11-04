# Huginn

Yggdrasil's video app: videos without an account, laid out like Agora. Named after Odin's raven that flies out over the world each day
and brings back what it saw.

- **Home:** the newest videos from your subscriptions (no account: they are kept on this computer).
- **Search, channels, saved videos, history.**
- **Play** inside the window, with the controls over the picture (Space, Left/Right 10 s, M, F full screen, Esc). Go elsewhere in the
  app and it keeps playing small in the corner, as on YouTube's phone apps; **pop out** moves it to yggdrasil-player's own window at
  the same second. **Listen** plays the sound only; **chapters** jump; a video watched before goes on where it stopped.
- **Downloads** of a video or its sound into `~/Videos/Huginn`.
- A channel's name on a video card opens the channel.
- **Bring your subscriptions** from NewPipe (its export), FreeTube or OpenTubeX (`profiles.db`) or Google Takeout
  (`subscriptions.csv`); export them for NewPipe again.
- `huginn LINK` opens a YouTube video or channel link (`--play` starts it, `--mini` starts it in the corner, `--mute` without sound);
  `huginn --search WORDS` searches.

How it works: `huginn-core` (no UI, so an iOS app can use it later) has the library and one small extractor interface; on the desktop
the extractor is [yt-dlp](https://github.com/yt-dlp/yt-dlp), which keeps up with YouTube's changes. Pictures come only from YouTube's
picture hosts. Playing: YouTube refuses stream requests that do not come from the kind of client the stream was issued to, so
yt-dlp downloads the streams into `~/.cache/yggdrasil-player/streams` and the player (libVLC, the same engine as yggdrasil-player)
plays from that file while it fills, through a relay on 127.0.0.1. A jump to a part not downloaded yet waits for it; the files are
deleted when playing stops. Your subscriptions, history and saved videos are in `~/.local/share/yggdrasil/huginn`; settings in
`~/.config/yggdrasil/huginn.conf`.

Written from the features of NewPipe (GPL-3.0) and OpenTubeX (AGPL-3.0), not from their code.

Note: YouTube's terms do not allow third-party apps or downloading; like NewPipe, FreeTube and yt-dlp, Huginn is offered for people to
decide for themselves.
