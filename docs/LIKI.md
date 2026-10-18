# Liki: Yggdrasil's icon pack

**Liki** is the main icon theme of Yggdrasil, shared by Labyrinth and Chymaera (the pack used to be called "Labyrinth Brands"). Source: `packages/liki`; package: `liki-icon-theme`.
It stands on `hicolor` only. **No KDE (Breeze), macOS (MacTahoe) or GNOME (Adwaita) icons are used or required.**

## What it holds
- **The system's own icons**: folders (and the Documents, Downloads, Music, Pictures, Videos, Desktop folders), the home folder, drives, the trash (empty and full), file types.
- **Yggdrasil's apps**: Settings, Minos, the Marketplace, Calculator, plus Sarina, Heimdall, Limbo and Daedalus CAD.
- **The default app icon** (`application-default-icon`, also named `application-x-executable`, `applications-other` and `unknown`): a plain four-square mark. **An app that no pack has an icon for gets this one**, in the dock, the start menu,
  notifications, the Marketplace and Settings > Apps, instead of a coloured letter.
- **App logos** (about 90, a selection from [Simple Icons](https://github.com/simple-icons/simple-icons), CC0; the logos stay their owners' trademarks), made per user by `liki-build` for the apps installed on that computer.

## How the desktop looks icons up
The pack chosen in KDE (`[Icons] Theme` in `kdeglobals`), then `hicolor` (where programs put their own icons), then Liki, then the default app icon. With Liki chosen it is just Liki and hicolor.
The panel, Minos, the Marketplace and Settings all use that order and notice a changed pack within a few seconds.

## The tools (`/usr/bin`; the old `labyrinth-*` names still work)
- `liki-build [--shape glyph|rounded|circle|hexagon|squircle] [--style brand|dark|purple] [--rebuild]`: builds the per-user pack in `~/.local/share/icons/Liki` from the installed apps (and the accent colour). `--natives-only` writes only the pack's own icons: that is what the package ships in `/usr/share/icons/Liki`.
  Settings > Appearance > Icons > Liki runs it ("Make"), and again when the accent changes.
- `liki-minos-icon`: with another pack chosen, gives Minos that pack's own file-manager icon (in the user's `hicolor`); with Liki chosen there is nothing to do, `liki-build` draws Minos.
- `liki-icon APP FILE`: use a picture for one app (also `--import FOLDER`, `--reset APP`); `liki-icns` reads macOS `.icns` files.
- `liki-icon-import PACK`: turns an Android icon pack into a Linux icon theme ([ANDROID_ICON_PACKS.md](ANDROID_ICON_PACKS.md)).

## Choosing and installing another pack
Settings > Appearance > Icons lists every pack on the system and in `~/.local/share/icons`; "Use" applies it to the whole desktop and "Install an icon pack" takes a folder, an archive or a link.
Another pack is used first; Liki and the default app icon fill in what it lacks.

## Chymaera
Chymaera uses the same package; nothing in Liki is Labyrinth-specific.
