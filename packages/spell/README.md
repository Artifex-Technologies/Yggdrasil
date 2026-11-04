# yggdrasil-spell

Yggdrasil's spelling library and command. It checks words against the Hunspell dictionaries installed on the system (one package per
language: Settings > Keyboard > Spelling and languages adds and removes them), in every language you have switched on at once, and
keeps **one word list** that all your apps share.

- **Engine:** [spellbook](https://github.com/helix-editor/spellbook) (pure Rust, Hunspell-compatible, MPL-2.0). Old dictionaries
  that are not UTF-8 are converted with glibc's `iconv` when they load.
- **Your languages:** `~/.config/yggdrasil/spell.conf` (`languages = en_US, de_DE`). A word is right when any of them knows it.
- **Your words:** `~/.local/share/yggdrasil/spell/words.txt` (each word with the apps it came from) and `removed.txt` (words you
  took out, so they stay out everywhere).
- **The apps' lists, kept in step** by `yggdrasil-spell watch` (the user service `yggdrasil-spell.service`):
  GTK apps through enchant (`~/.config/enchant/<language>.dic`), Firefox (`persdict.dat` in each profile), and every Chromium
  browser and Electron app (`Custom Dictionary.txt` in each profile, with the checksum line Chromium expects). Apps are found by
  what is on disk, so a newly installed Electron app joins by itself. A browser reads its list when it starts.
- **Shared by the computer:** `/var/lib/yggdrasil/spell/global.txt` (read; how words reach it is decided separately).

Nothing here uses the network (the service runs with no network at all) or writes outside your own folders.

```
yggdrasil-spell check WORD...     yggdrasil-spell text "TEXT"     yggdrasil-spell languages
yggdrasil-spell words             yggdrasil-spell add WORD        yggdrasil-spell remove WORD
yggdrasil-spell sync              yggdrasil-spell watch
```
