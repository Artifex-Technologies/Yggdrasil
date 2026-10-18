# Yggdrasil's look for other programs: `yggdrasil-theme`

Everything Yggdrasil writes itself (the panel, Settings, Minos, the Marketplace, the lock and login screens, the polkit box, the compositor) draws its own look from one palette
(`packages/labyrinth-ui/src/theme.rs`). `yggdrasil-theme` (`packages/theme`) gives the programs it does **not** write, Qt, KDE and GTK programs, and the mouse pointer, the same look.
It replaces the WhiteSur KDE, GTK and cursor themes (about 29 MB, GPL-3, macOS-style), and needs no other theme package.

| Part | How |
| --- | --- |
| **Colours for Qt and KDE programs** | a KDE colour scheme (`YggdrasilDark`, `YggdrasilLight`) made from the palette and the accent colour, with Qt's own **Fusion** style. No Kvantum, Breeze or other style. |
| **GTK programs** | GTK's own **Adwaita** with Yggdrasil's colours: a small GTK 3 theme (`Yggdrasil`) and a stylesheet in `~/.config/gtk-3.0` and `gtk-4.0` (libadwaita reads only that one) holding the colours in the accent. |
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
- GTK 4 programs that do not use libadwaita get GTK's own Adwaita with the colours; their shapes are not restyled.
- The login screen (SDDM, Breeze) is replaced by `labyrinth-greeter` later, with the same cursors.
