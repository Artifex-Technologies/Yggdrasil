# Yggdrasil's look for other programs: ygg-theme (Rust) and ygg-cursors (Python)

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

## ygg-cursors stays Python

`bin/ygg-cursors` draws the cursor theme with Pillow when the package is built (a polygon, ellipse, arc and line rasteriser
at 8x, a "?" through FreeType, then Lanczos down to five sizes). A Rust version would only be 1:1 by re-implementing
Pillow's drawing and resampling code and its text layout; anything else changes the cursors' pixels. It runs once per
package build and never on a running desktop, so it stays Python for now; the package only needs Python and Pillow to
build, not to run.
