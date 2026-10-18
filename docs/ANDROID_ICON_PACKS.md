# Android icon packs in Labyrinth

Settings > Appearance > Icons > "Install an icon pack" accepts an Android icon pack (`.apk`) as well as folders, archives and https links. `liki-icon-import` does the work:

- The `.apk` is opened as a zip file and only **read**; nothing in it is run. Only PNG pictures from `res/drawable*`, `res/mipmap*` and `assets` are used (square, 32 px or more, size-limited), plus the pack's own list of which Android app gets which picture (`appfilter.xml`, plain or Android's compiled form).
- Each picture goes to the installed program it fits: the pack's names (the app's package name, the picture's name) are compared with each program's icon name, name, command and window class. Programs with no match keep their usual icon, because the theme inherits from `hicolor` (and the rest of your icons come from whatever else is selected).
- The result is a normal icon theme in `~/.local/share/icons/<Name>` (the name comes from the file name, minus versions and brackets). Choose it in Settings > Icons like any other.

Not supported: vector drawables (`.xml` icons; many modern packs ship PNG too), adaptive-icon layers, packs with no appfilter, and anything that does not match an installed program by name. A pack made only of vector drawables imports nothing.

**Licences.** Most paid Android icon packs are licensed to the buyer. Labyrinth does not ship any, and the importer does not check licences: use packs you have the right to use (there are many free and open ones). A "patched" or cracked APK is also a risk of its own: the importer never runs it, but do not install such a file on a phone or anywhere else.
