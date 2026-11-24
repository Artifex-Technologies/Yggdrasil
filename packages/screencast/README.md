# yggdrasil-screencast

Screen sharing on Yggdrasil's own compositor. It copies one screen through the standard Wayland capture protocols
(ext-image-capture-source and ext-image-copy-capture, which the compositor already serves for the panel's window previews) and
publishes the frames as a PipeWire video stream: a `Video/Source` node that a viewer (a browser, OBS, a video call) links to.

    yggdrasil-screencast --output NAME [--cursor]

- `--output NAME`: the screen, by the name the compositor gives it (`HEADLESS-1`, `DP-1`, `eDP-1`).
- `--cursor`: the pointer painted into the frames.

Once the stream is there it prints `node N` (its PipeWire node) on a line of its own, then runs until the screen goes, the
compositor ends, or it gets SIGTERM or SIGINT.

## How it works

- Two threads. The capture thread talks to the compositor (blocking Wayland calls); PipeWire's loop runs the stream. The newest
  frame passes from one to the other; a frame not sent yet is replaced by a newer one, so a slow viewer never builds a queue.
- Frames come as the screen changes. The compositor answers a capture only once something new is drawn, so nothing is copied
  or sent while the screen stands still. The stream drives itself (a `DRIVER` node) and starts a cycle for each new frame.
- The format is the screen's own: BGRx, or BGRA where the compositor offers only ARGB8888 (wl_shm's XRGB8888 and ARGB8888 in
  memory order), at the screen's size, up to 60 frames a second. Buffers are shared memory (memfd) or plain memory, one frame
  each.
- A viewer that connects gets the newest frame at once, not only after the screen next changes.

## Test

`cargo test` runs `tests/stream.rs` on a PipeWire of its own: a minimal configuration in a temporary folder (no devices, nothing
of the desktop's audio), the headless compositor at 320x240, yggdrasil-screencast on its screen, and a viewer in the test that
finds the two ports through PipeWire's registry, links them and reads the first frame. It checks the frame is one whole screen of
32-bit pixels and that its first pixel is the compositor's background colour. Every process it starts dies with the test (a
parent-death signal), even when the test is killed. It needs `pipewire` and the compositor's debug build (or
`YGGDRASIL_COMPOSITOR`); without them it is skipped, except in CI (`YGGDRASIL_TEST_SCREENCAST_REQUIRED`), where that fails.

## Known limitations

- Shared memory only: each frame is copied twice (the compositor's buffer, then PipeWire's), not passed on the GPU (dmabuf).
- Whole screens only, not single windows.
- The size is the screen's when it starts; a screen that changes size ends the stream.
