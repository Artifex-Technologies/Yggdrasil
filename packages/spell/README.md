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

- **Correct as you type** (`yggdrasil-autocorrect`, off unless switched on: Settings > Keyboard > Spelling and languages >
  Correct as I type, which enables the user service). An input method (Wayland's `zwp_input_method_v2`) that never takes the
  keyboard: apps that take part in text input (GTK, Qt, Firefox, Chromium; Electron apps with `--enable-wayland-ime`) share
  the text around the cursor with it.
  - When a word has just been finished (a space or punctuation typed after it), it is replaced only when the fix is all but
    certain (`src/autocorrect.rs`):
    - the word is wrong in every switched-on language and in no word list;
    - it is 3 or more letters, all lowercase or capitalised;
    - the dictionary's first suggestion is one edit away and keeps the first letter;
    - and that suggestion is either two neighbouring letters swapped (teh, adn, thier, wrold) or the only suggestion one edit
      away. "helo" is left alone, because "hole" comes first and "hello" is one of several.
  - Backspace straight after a correction puts the typed word back, and that word is not corrected again until the input
    method starts again.
  - Never in passwords, PINs, numbers, addresses, links, names, terminals, or text an app marks hidden or sensitive; only
    right after one character was typed (not after a paste or a cursor jump).
  - The dictionary work runs on a thread of its own while the word is still being typed, so the answer is ready when the
    word ends; a word not looked at yet is left alone. Tried in a GTK text box at 35 and 80 ms a key: every slip above fixed,
    "helo" kept, undo worked. About 17 MB of memory, no CPU while idle.

Nothing here uses the network (the services run with no network at all) or writes outside your own folders, except your offer to
the shared list.

```
yggdrasil-spell check WORD...     yggdrasil-spell text "TEXT"     yggdrasil-spell languages
yggdrasil-spell words             yggdrasil-spell add WORD        yggdrasil-spell remove WORD
yggdrasil-spell sync              yggdrasil-spell watch
yggdrasil-spell global shared | pending | merge | approve WORD... | deny WORD...    (merge, approve, deny: root)
```

Still open:

- Electron apps take part in autocorrect only when started with `--enable-wayland-ime`. Since 2026-11-07 a switch under "Correct
  as I type" (and `yggdrasil-spell electron on|off|status`) writes it into `~/.config/electron-flags.conf` (apps on Arch's
  electron, Obsidian among them) and `code-flags.conf` (VS Code), marked as Yggdrasil's; off takes out only that line. Off by
  default: with some Electron versions the flag has caused doubled or lost keys. Not yet tried with VS Code and Obsidian here.
- Word packs by size (SCOWL's small, medium and large English lists) are not built; the packs now are the cspell-dicts topic
  lists (tech terms, companies, gaming, names).
