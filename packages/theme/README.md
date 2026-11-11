# Yggdrasil's look for other programs: ygg-theme and ygg-cursors (Rust)

`ygg-theme` writes Yggdrasil's colours for the programs Yggdrasil does not write: GTK's settings and colours, the system
GTK 3 and GTK 4 themes, and, only when a program built on KDE's libraries is installed, KDE's colour scheme files. It also
switches the mouse pointer. Settings, the panel (Fylgja) and the package build run it:

    ygg-theme apply [--dark|--light] [--accent R,G,B]   write the colour scheme and the GTK settings for this user
    ygg-theme scheme dark|light [R,G,B]                  print the colour scheme for programs built on KDE's libraries
    ygg-theme gtk dark|light [R,G,B]                     print the GTK colour definitions
    ygg-theme gtk3-theme|gtk4-theme dark|light           print the system GTK 3 or GTK 4 theme file (what the package installs)
    ygg-theme cursor [NAME]                              print the mouse pointer theme, or switch to NAME

It is in Rust (`src/`), replacing the Python script that is kept in `python/ygg-theme` for a rollback, with the same
commands, output and files byte for byte. `gtk4-theme` reads GTK 4's own theme from the installed libgtk-4, opened at run
time (no python-gobject), and replaces its colours as the script did; without libgtk-4 it falls back as the script did
(GTK's own theme underneath, Yggdrasil's surfaces on top).

`cargo test` carries the script's tests over (the colour scheme, the GTK colours, ini files, apply with and without KDE
programs). `parity/compare.py` runs both on the same inputs: every print command in many accents, GTK 4's own theme
retinted in 47 accents dark and light, and apply and cursor on every combination of files that may be there before, with
stand-ins for gsettings, systemctl, hyprctl and pkill; output, calls and files must be the same.

## ygg-cursors

`ygg-cursors OUTDIR [--preview]` draws the cursor theme (the package runs it when it is built): 22 shapes, 96 names, five
sizes, an animated busy ring, written as X cursor files. It is Rust now (`src/cursors`), replacing the Python script kept in
`python/ygg-cursors` for a rollback, and it draws **every pixel as the script did with Pillow**: I translated the parts of
Pillow 13.0 the script used (`src/cursors/NOTICE-Pillow` has Pillow's licence and what was translated):

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
version the hashes were made with). `parity/cursors.py` runs both on the same folders: on 2026-11-11, 22 cases of 22 the same (the system's font and
9 others: TrueType, CFF, a variable font, one without a "?", a colour bitmap font FreeType cannot size; a path FreeType
cannot open, a bare file name and a name without extension found in the font folders, nothing, no fc-match at all; the
usage, a theme there before, a folder where a link goes, an output that is a file): every X cursor file byte for byte, the
preview's pixels exactly, the output and the exit codes. No tolerance is needed anywhere.

    cargo build --release && python3 parity/cursors.py [-v]
