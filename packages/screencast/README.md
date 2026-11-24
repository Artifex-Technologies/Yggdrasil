# yggdrasil-screencast

Screen sharing on Yggdrasil's own compositor. A program (a browser, OBS, a video call) asks to share the screen through
xdg-desktop-portal; three parts of Yggdrasil answer:

1. **yggdrasil-portal** (`packages/portal`, `src/screencast.rs`) answers `org.freedesktop.impl.portal.ScreenCast`: it keeps
   the program's sessions, asks the person, starts one worker per chosen screen and hands back the streams' PipeWire nodes
   (the portal frontend then gives the program those nodes and nothing else of PipeWire).
2. **yggdrasil-share-picker** (`packages/share-picker`) asks the person which screen (or screens, when the program asked for
   several). Nothing is shared without it: Cancel, Escape or closing it shares nothing.
3. **yggdrasil-screencast** (this package), the worker, copies one screen through the standard Wayland capture protocols
   (ext-image-capture-source and ext-image-copy-capture, which the compositor already serves for the panel's window previews)
   and publishes the frames as a PipeWire video stream: a `Video/Source` node.

The session on the own compositor routes screen sharing here: its desktop names are `Yggdrasil-Compositor:Yggdrasil`, and
xdg-desktop-portal reads `yggdrasil-compositor-portals.conf` (installed with the portal) for that session. The session on
Hyprland keeps Hyprland's portal for screen sharing (`yggdrasil-portals.conf`).

## The worker

    yggdrasil-screencast --output NAME [--cursor]

- `--output NAME`: the screen, by the name the compositor gives it (`HEADLESS-1`, `DP-1`, `eDP-1`).
- `--cursor`: the pointer painted into the frames (the program asked for the cursor "embedded").

Once the stream is there it prints `node N` (its PipeWire node) on a line of its own, then runs until the screen goes, the
compositor ends, or it gets SIGTERM or SIGINT.

- Two threads. The capture thread talks to the compositor (blocking Wayland calls); PipeWire's loop runs the stream. The newest
  frame passes from one to the other; a frame not sent yet is replaced by a newer one, so a slow viewer never builds a queue.
- Frames come as the screen changes. The compositor answers a capture only once something new is drawn, so nothing is copied
  or sent while the screen stands still. The stream drives itself (a `DRIVER` node) and starts a cycle for each new frame.
- The format is the screen's own: BGRx, or BGRA where the compositor offers only ARGB8888 (wl_shm's XRGB8888 and ARGB8888 in
  memory order), at the screen's size, up to 60 frames a second. Buffers are shared memory (memfd) or plain memory, one frame
  each.
- A viewer that connects gets the newest frame at once, not only after the screen next changes.

## The portal's side

- `CreateSession`, `SelectSources` (one screen or several; the cursor hidden or painted in) and `Start`, as the frontend calls
  them. `Start` runs the picker (`--app APP_ID`, `--multiple` when the program asked for several), then a worker per chosen
  screen, and answers each stream's node with its place and size in the compositor's space (`position`, `size`; logical
  pixels) and an id of the session's own (the screens' connector names are not handed out).
- Offered: whole screens (`AvailableSourceTypes` 1), the cursor hidden or embedded (`AvailableCursorModes` 3).
- A worker ends when the program closes the session (`Session.Close`), when its screen goes or the compositor ends (once the
  last of a session's workers has ended, the session says `Closed`), and with the portal: each worker is tied by a
  parent-death signal to a thread of the portal's that lives exactly as long as the worker, and is signalled through a pidfd
  (never a process that took its number later). A screen is never left shared by a worker nobody owns.
- `Request.Close` while the picker is open closes the picker: nothing is shared.

## Tests

`cargo test` here runs, besides the unit tests:

- `tests/stream.rs`: the worker on a PipeWire of its own (a minimal configuration in a temporary folder: no devices, nothing of
  the desktop's audio) and the headless compositor at 320x240, with a viewer in the test that finds the two ports through
  PipeWire's registry, links them and reads the first frame: one whole screen of 32-bit pixels, its first pixel the
  compositor's background colour.
- `tests/portal.rs`: the portal's build on a D-Bus of the test's own, with the same PipeWire and a headless compositor with two
  screens, a stand-in picker that answers for the person, and the real worker. Four sessions: one shared and closed by the
  program (the picker told who asks and that one screen is wanted; the worker told the chosen screen and the pointer; the
  stream's node, place and size answered; the worker gone after Close); one whose screen is unplugged (the worker ends, the
  session says Closed); one the person cancels (no worker); and one whose portal is killed (its worker dies with it).

Every process they start dies with the test (a parent-death signal), even when the test is killed. They need `pipewire`,
`dbus-daemon`, the compositor's debug build (or `YGGDRASIL_COMPOSITOR`) and the portal's (or `YGGDRASIL_PORTAL`); without them
they are skipped, except in CI (`YGGDRASIL_TEST_SCREENCAST_REQUIRED`), where that fails.

## Known limitations

- Shared memory only: each frame is copied twice (the compositor's buffer, then PipeWire's), not passed on the GPU (dmabuf).
- Whole screens only, not single windows.
- The size is the screen's when it starts; a screen that changes size ends the stream.
- Not yet tried by hand with a browser or OBS in the own compositor's session; the tests drive the portal as the frontend does.
- The picker's choice is not remembered between sessions (no restore tokens).
