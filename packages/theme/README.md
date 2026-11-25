# Yggdrasil's look for other programs: ygg-theme and ygg-cursors (Rust)

`ygg-theme` writes Yggdrasil's colours for the programs Yggdrasil does not write: GTK's settings and colours, the system
GTK 3 and GTK 4 themes, and, only when a program built on KDE's libraries is installed, KDE's colour scheme files. It also
switches the mouse pointer. Settings, the panel (Fylgja) and the package build run it:

    ygg-theme apply [--dark|--light] [--accent R,G,B]   write the colour scheme and the GTK settings for this user
    ygg-theme scheme dark|light [R,G,B]                  print the colour scheme for programs built on KDE's libraries
    ygg-theme gtk dark|light [R,G,B]                     print the GTK colour definitions
    ygg-theme gtk3-theme|gtk4-theme dark|light           print the system GTK 3 or GTK 4 theme file (what the package installs)
    ygg-theme cursor [NAME]                              print the mouse pointer theme, or switch to NAME

It is in Rust (`src/`), replacing the Python script (kept in `python/ygg-theme` for a rollback until 2026-11-15; git has it), with the same
commands, output and files byte for byte. `gtk4-theme` reads GTK 4's own theme from the installed libgtk-4, opened at run
time (no python-gobject), and replaces its colours as the script did; without libgtk-4 it falls back as the script did
(GTK's own theme underneath, Yggdrasil's surfaces on top).

`cargo test` carries the script's tests over (the colour scheme, the GTK colours, ini files, apply with and without KDE
programs). `parity/compare.py` runs both on the same inputs: every print command in many accents, GTK 4's own theme
retinted in 47 accents dark and light, and apply and cursor on every combination of files that may be there before, with
stand-ins for gsettings, systemctl, hyprctl and pkill (and, for the Rust side, which sets the pointer through yggdrasil-wm, a
stand-in Hyprland socket that writes the same call down); output, calls and files must be the same.

## The default system font

`data/58-yggdrasil-default-font.conf`, installed in `/usr/share/fontconfig/conf.avail` with a link in `conf.default` (Arch's
way: fontconfig's pacman hook links it into `/etc/fonts/conf.d`), prefers Atkinson Hyperlegible for `sans-serif`, `sans` and
`system-ui` and Atkinson Hyperlegible Mono for `monospace`, on a system where no font was chosen. A chosen font
(`56-yggdrasil-font.conf`) wins; the package depends on the two fonts. Its test is in yggdrasil-conf (`fonts.rs`,
`the_default_file_names_the_defaults`), next to the list it must match. Details: docs/APPS_SETTINGS.md, Appearance > Fonts.

## ygg-cursors

`ygg-cursors OUTDIR [--preview]` draws the cursor theme (the package runs it when it is built): 22 shapes, 96 names, five
sizes, an animated busy ring, written as X cursor files. It is Rust now (the crate in `cursors/`), replacing the Python script (kept in
`python/ygg-cursors` for a rollback until 2026-11-15; git has it), and it draws **every pixel as the script did with Pillow**: I translated the parts of
Pillow 13.0 the script used (`cursors/src/NOTICE-Pillow` has Pillow's licence and what was translated):

- the drawing at 8x (`draw.rs`): ImageDraw's polygon fill, lines and wide lines with curved joints, filled ellipses and
  ellipse outlines, pie slices, arcs and rounded rectangles, from libImaging/Draw.c and ImageDraw.py, with C's float and
  double arithmetic kept as it was;
- the shrinking (`resample.rs`): Lanczos in 22-bit fixed point on premultiplied colour, and the sheet's alpha compositing;
- the help cursor's "?" (`text.rs`): the font fc-match names for sans-serif:bold, opened with FreeType, laid out by libraqm
  when it is there (as Pillow does), rendered and blended as Pillow's font_render and its fill with a mask do; without that
  font, Pillow's own default font (the Aileron subset Pillow carries, CC0), as Pillow fell back to it. libfreetype and
  libraqm are opened at run time, only for the help cursor.

The X cursor files are therefore byte for byte the script's; only `preview.png` is a different file (another PNG encoder)
with the same pixels.

Not 1:1: a font whose "?" comes as a bitmap other than 8-bit grey (Pillow converted mono, 2- and 4-bit ones, and stopped
with an error on colour ones) draws no "?" here; fc-match does not give such fonts for sans-serif:bold.

Tests: `cargo test` runs it on a scratch folder (the names programs ask for, five sizes, the 60 frames of the busy cursor,
80 ms each) and checks every cursor file and the preview's pixels against hashes of what the Python script wrote with
Pillow, without fc-match on the PATH so the "?" is in Pillow's default font, which needs nothing from the system but
FreeType (the help cursor and the preview are FreeType's own rendering, so they are compared when FreeType is 2.15.0, the
version the hashes were made with), and checks the shrink against Pillow's own loops, tap by tap, on made-up pictures.
`parity/cursors.py` runs both on the same folders: on 2026-11-11 and again on 2026-11-25 (with four threads and with one),
22 cases of 22 the same (the system's font and 9 others: TrueType, CFF, a variable font, one without a "?", a colour
bitmap font FreeType cannot size; a path FreeType cannot open, a bare file name and a name without extension found in the
font folders, nothing, no fc-match at all; the usage, a theme there before, a folder where a link goes, an output that is
a file): every X cursor file byte for byte, the preview's pixels exactly, the output and the exit codes. No tolerance is
needed anywhere.

    cargo build --release && python3 parity/cursors.py [-v]

## Why Rust: measured

Measured on 2026-11-11 (the ygg-cursors rows on 2026-11-25) with `tools/bench-port.py` against the Python scripts from git,
in scratch folders (stand-ins for gsettings, systemctl, hyprctl and pkill; no display), on my desktop (16 threads). Method
and every case: [docs/PORT_NUMBERS.md](../../docs/PORT_NUMBERS.md).

| Metric | Python | Rust | Change |
| --- | --- | --- | --- |
| `ygg-theme scheme`, `gtk`, `gtk3-theme`, wall time | 27.7–27.8 ms | 0.56–0.58 ms | −98 %, 27.1 ms faster |
| `ygg-theme gtk4-theme dark` (GTK 4's own theme, retinted), wall time | 116 ms | 32.9 ms | −72 %, 82.7 ms faster |
| `ygg-theme gtk4-theme dark`, peak memory | 57.6 MB | 32.3 MB | −44 %, 25.3 MB less |
| `ygg-theme apply`, wall time | 34.7 ms | 3.73 ms | −89 %, 31.0 ms faster |
| `ygg-theme apply`, CPU time | 32.8 ms | 3.62 ms | −89 %, 29.1 ms less |
| `ygg-theme`, peak memory (the other commands) | 15.9 MB | 3.3–4.0 MB | −75 to −79 % |
| `ygg-cursors OUTDIR --preview` (the whole theme), wall time | 1028 ms | 40.3 ms | −96 %, 987 ms faster |
| `ygg-cursors OUTDIR --preview`, CPU time | 1026 ms | 115 ms | −89 %, 910 ms less |
| `ygg-cursors`, peak memory | 31.6 MB | 16.1 MB | −49 %, 15.5 MB less |
| on disk | 44 KB of scripts (+ Python, 75 MB, python-gobject and python-pillow) | 1.9 MB (two binaries) | +1.9 MB; the package needs no Python |

`apply` waits for each of the programs it tells (gsettings, systemctl, hyprctl, pkill) and goes on the moment each one
ends. Until 2026-11-11 it looked every 5 ms instead, and took 21.7 ms; the `apply` numbers above are from a run that day
after the change, the rest from the first run. GTK 4's library is most of `gtk4-theme`'s time and memory in both.

Until 2026-11-25 `ygg-cursors` was slower than Python with Pillow, whose drawing and resampling are C: 1604 ms against
1057 ms, wall and CPU alike, at 15.1 MB. It now draws the same bytes in a 25th of Python's time:

- The shrink from 8x adds up runs of equal pixels and stretches of equal rows instead of every pixel under every weight.
  The shapes are drawn without smoothing, so a row is a few long runs, mostly transparent (left out: they add nothing), and
  many rows repeat. A run's share of an output pixel is one multiplication by a sum of weights, and a stretch is added
  once with the sum of its rows' weights: the same integer sums in another order, so the same result. The weights are
  made once a size, and pixels are premultiplied per run, not as a copy of the canvas.
- The busy ring's hole is a disc drawn in transparent ink. The script pasted transparent through a mask the size of the
  canvas, a blend of every pixel in every busy frame, with the same result (a test checks it for every pixel value).
- Drawn spans are written as whole pixels, each thread clears and reuses one canvas, and fc-match is asked once (on a
  thread of its own, while the shapes are drawn).
- `ygg-cursors` is its own crate (`cursors/`) so it can be built at opt-level 3 while `ygg-theme` stays built for size
  (`Cargo.toml`: one workspace, both crates built, tested and checked from this folder).
- Up to four threads draw the frames: as many as the CPUs it may use, fewer when the build asks for fewer jobs (make's
  `-jN` in `MAKEFLAGS`, as makepkg passes it, or `CARGO_BUILD_JOBS`). The main thread writes each file in the script's order
  as soon as its frames are drawn, and draws the help cursor itself (FreeType is for one thread). With one job it draws on
  the main thread alone: 108 ms, 10.4 MB.

Each thread needs its own 8x canvas (2.4 MB at 96 px), so four threads peak at 16.1 MB, 1 MB more than the one-thread
version before; six would take 34 ms at 21 MB.

More accurate:

- ygg-theme: parity 132 print cases, GTK 4's own theme retinted in 47 accents dark and light (94), and `apply` and `cursor`
  on 540 combinations of files that may be there before: all the same, byte for byte.
- ygg-cursors: parity 22 of 22 cases, every X cursor file byte for byte and the preview's pixels exactly, with no
  tolerance, with four threads and with one; `cargo test` checks every cursor against hashes of Pillow's output.
- No python-gobject (GTK 4's theme is read from libgtk-4 itself) and no Python or Pillow to build the package.
