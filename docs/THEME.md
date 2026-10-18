# Yggdrasil's look for other programs: `yggdrasil-theme`

Everything Yggdrasil writes itself (the panel, Settings, Minos, the Marketplace, the lock and login screens, the polkit box, the compositor) draws its own look from one palette
(`packages/labyrinth-ui/src/theme.rs`). `yggdrasil-theme` (`packages/theme`) gives the programs it does **not** write, Qt, KDE and GTK programs, and the mouse pointer, the same look.
It replaces the WhiteSur KDE, GTK and cursor themes (about 29 MB, GPL-3, macOS-style), and needs no other theme package.

| Part | How |
| --- | --- |
| **Colours for Qt and KDE programs** | a KDE colour scheme (`YggdrasilDark`, `YggdrasilLight`) made from the palette and the accent colour, with Qt's own **Fusion** style. No Kvantum, Breeze or other style. |
| **GTK programs** | the theme `Yggdrasil`: for GTK 3, GTK's own Adwaita with Yggdrasil's colours; for GTK 4, GTK's own theme with its fixed greys and blues replaced by Yggdrasil's greys and the accent (done when the package is built, from the GTK that is installed; plain CSS afterwards). Libadwaita programs (Ghostty and most GNOME programs) read the colours in `~/.config/gtk-4.0/gtk.css`, which `ygg-theme` writes in the accent. |
| **The mouse pointer** | the `Yggdrasil` cursor theme, drawn by `ygg-cursors` (22 shapes, 96 names, 5 sizes, an animated busy ring): white shapes with a dark purple outline so they read on any background. |

## `ygg-theme`
- `ygg-theme apply [--dark|--light] [--accent R,G,B]`: writes the colour scheme into `~/.local/share/color-schemes` and its colours into `~/.config/kdeglobals` (where KDE and Qt programs read them, and where Yggdrasil keeps the accent and the scheme name that tell its own programs dark from light),
  the GTK settings and stylesheets, the pointer setting and `~/.config/environment.d/50-yggdrasil-cursor.conf`; then tells the running session (`plasma-apply-colorscheme`, `plasma-apply-cursortheme`, `gsettings`) where those exist. Anything you added to your own GTK stylesheet below its marker line is kept.
- Settings > Appearance (dark or light, the accent colour) and the notch's dark-mode switch call it, so a change shows in every program.
- `ygg-theme scheme|gtk|gtk3-theme ...` print the files (what the package installs).
- `ygg-cursors OUTDIR [--preview]` draws the cursor theme (the package runs it at build time; Pillow is only needed for that).

## Why this and not a bigger theme
Lightweight (a few text files and one small cursor set, nothing running), stable (it uses Qt's and GTK's own styles, which every release keeps working), secure and private (nothing is downloaded, no third-party theme code or scripts are installed or run, no network), and one palette for everything.

## Not done yet
- Qt programs read the colours through KDE's platform theme (`plasma-integration`) until Yggdrasil has its own session; then a small replacement (or `qt6ct`) reads the scheme.
- A light variant exists but has had much less use than the dark one; check it before relying on it.
- GTK 4 programs that do not use libadwaita get GTK's own shapes in Yggdrasil's colours; the accent in the system theme file is the default purple (a changed accent reaches them only through `~/.config/gtk-4.0/gtk.css`, which GTK 4 loads after the theme, so it does, but some small parts of GTK's theme keep their built-in values).
- In one program (pavucontrol) the volume slider's knob is not visible; a plain GTK 4 slider is fine.
- The login screen (SDDM, Breeze) is replaced by `labyrinth-greeter` later, with the same cursors.
