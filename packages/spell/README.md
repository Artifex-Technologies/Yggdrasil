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
- **Shared by the computer:** `/var/lib/yggdrasil/spell/global.txt`. A word gets there when **two accounts offer it, or an
  administrator approves it** (`src/global.rs`):
  - Each account offers the words it added itself (spell.conf `share = yes`, the default; Settings > Keyboard > Spelling and
    languages > Offer my words) to `/var/lib/yggdrasil/spell/offered/<uid>.txt`. That folder is like /tmp: anyone may create
    their own file, nobody else can list or read it, and a file counts only for the account that owns it.
  - Only plain words are offered: letters, with an apostrophe or hyphen inside, 2 to 40 of them. Never digits, links, addresses,
    or a long run with no vowel (a password typed into the wrong box).
  - The root unit `yggdrasil-spell-global.path` runs `yggdrasil-spell global merge` when an offer changes. It writes
    `global.txt` (for everyone) and `pending.txt` (words one account offered, without saying which; readable by the `wheel`
    group only).
  - Administrators choose in Settings (Share / Don't share), through `pkexec /usr/lib/yggdrasil/spell-global approve|deny WORD`.
    A word denied never reaches the shared list.
  - Shared words reach every app's list like a word pack that is on, and never join an account's own list.

Nothing here uses the network (the services run with no network at all) or writes outside your own folders, except your offer to
the shared list.

```
yggdrasil-spell check WORD...     yggdrasil-spell text "TEXT"     yggdrasil-spell languages
yggdrasil-spell words             yggdrasil-spell add WORD        yggdrasil-spell remove WORD
yggdrasil-spell sync              yggdrasil-spell watch
yggdrasil-spell global shared | pending | merge | approve WORD... | deny WORD...    (merge, approve, deny: root)
```
