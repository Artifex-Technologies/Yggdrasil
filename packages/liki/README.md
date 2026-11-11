# Liki's tools (Rust)

Liki is Yggdrasil's icon pack ([docs/LIKI.md](../../docs/LIKI.md)). Three of its tools are Rust now, replacing the Python
scripts at the same paths with the same interface: arguments (as Python's argparse took them, help and errors included),
output, exit codes and files.

| Tool | Installed as | For |
| --- | --- | --- |
| `liki-build` | `/usr/bin/liki-build` | the pack: Yggdrasil's own glyphs and the installed apps' logos (Simple Icons) as SVG, in a shape and a style; the package runs it with `--natives-only`, Settings > Appearance > Icons runs it ("Make", and `--rebuild` when the accent changes) |
| `liki-icns` | `/usr/bin/liki-icns` | the biggest PNG in a macOS `.icns` (`liki-icon` uses it) |
| `liki-icon-import` | `/usr/bin/liki-icon-import` | an Android icon pack (`.apk`) as a Linux icon theme; Settings > Appearance > Icons > "Install an icon pack" runs it |

`liki-icon` and `liki-minos-icon` stay shell scripts in `bin/`.

The pack is only ever read: `liki-icon-import` reads the `.apk` as a zip archive the way Python's zipfile read it (the end
record, ZIP64, data before the archive, cp437 and UTF-8 names, the Unicode path field, the local headers' checks,
overlapping members, CRCs), its compiled `appfilter.xml` as the script's own parser did, and a plain `appfilter.xml` through
libexpat, the same parser Python's ElementTree used (opened at run time; the package depends on `expat`). A member is never
read past its stated size.

## Bugs fixed (in the Rust tools and in the Python scripts kept for a rollback)

- `liki-build`: a pack name with `[` or `*` in it (`--name "Liki[12]"`) was read by `glob` as a pattern: the stale SVGs of
  *other* packs (`Liki1`, `Liki2`) were deleted and the pack's own were not. The folders are now listed as they are.
- `liki-build`: a desktop file with a `/` in its `Icon=` that matched a logo wrote the logo outside the pack
  (`Icon=../../x/firefox`) or stopped the whole build with a traceback (`Icon=sub/firefox`). Such names are left out.
- `liki-build`: without `gtk-update-icon-cache` (an optional dependency) the build stopped with a traceback after writing the
  pack, so the choice was not remembered; it now goes on without the cache.
- `liki-build --rebuild` with a damaged `~/.config/labyrinth/brand-pack.json` (a list, a number, an unknown shape, a name such
  as `../x`) stopped with a traceback after the pack's icons had been deleted. Values that are not a shape, a style or a
  usable pack name are left as given; a file that is not an object counts as no earlier build.
- `liki-icns` opened (emptied) the output before reading the input: a file that was not an `.icns` left an empty picture,
  and the same path for both destroyed the input. It now reads and checks first. (`liki-icon --import` also installed that
  empty file as the app's icon; it now stops on a broken `.icns`.)
- `liki-icon-import`: a compiled `appfilter.xml` whose string pool pointed past its end stopped the import with a traceback
  (IndexError); it is now skipped like any other unreadable `appfilter.xml`.
- `liki-icon-import`: a broken deflate stream, a pack cut short, an encrypted or unknown kind of member ended in a traceback;
  it is now "error: that is not a zip/apk file" (exit code 1), as for any file that is not a zip.
- `liki-icon-import`: `zf.read()` inflated a member fully (up to 2 GB in one step) before cutting it to its stated size, and
  `appfilter.xml` had no size limit: a small crafted `.apk` could take gigabytes of memory. Members are read up to their
  stated size, and an `appfilter.xml` over 32 MB is not read.
- `liki-icon-import`: among an app's names of equal length the order followed Python's string hashing, which changes from
  run to run, so the same pack could give an app a different picture each time. Names of equal length are now tried in
  alphabetical order.

## What is not 1:1

- Where a script stopped with a traceback, the tool stops too, with exit code 1 and one line on standard error.
- zipfile's warnings (two members starting at the same place, an empty Unicode path field) are not printed.
- A pack member compressed with bzip2, LZMA or Zstandard (zipfile could read those; Android never uses them) makes the pack
  unreadable here. An `appfilter.xml` declared in an 8-bit encoding libexpat does not know itself (windows-1252, koi8-r ...),
  which ElementTree read through Python's codecs, does not parse here (UTF-8, UTF-16, ISO-8859-1 and US-ASCII do).
- A deflated member whose stream runs on past its stated size is read up to that size; whether a broken tail after that is
  noticed can depend on the inflater (zlib then, miniz_oxide now). Either way it is the same error message.
- A remembered build nested more than 500 levels deep counts as no earlier build (Python stopped with a RecursionError near a
  thousand).

## Tests and parity

`cargo test`: the tools as programs on scratch folders (the package's natives-only build, a user build with logos, the
accent, remembering and `--rebuild`, the glob bug, argparse's errors and help, `.icns` files, a pack becoming a theme, junk),
the readers of outside input fed broken data (20 000 changed `.icns` files, 3 000 changed archives, every one of whose
members is checked never to be read past its stated size, 20 000 changed compiled XML documents, 5 000 changed plain ones),
and the Python behaviours they depend on.

`parity/compare.py` runs each tool and its script on the same made-up worlds (HOME a scratch folder, a stand-in
`gtk-update-icon-cache` on the PATH) and compares exit codes, output, the stand-in's calls and every file afterwards. On
2026-11-11, with `--fuzz 200`: liki-build 120 of 120 cases the same (every shape and style, names, 21 remembered files, 10
accents, 7 damaged brand-icon folders, XDG folders set to nothing, no cache tool, 30 command lines and the help and errors
at 7 widths and in colour), liki-icns 210 of 210 (200 of them fuzzed), liki-icon-import 40 of 40 made packs, 17 of 17
crafted archives (ZIP64, data before the archive, duplicate and NUL names, the Unicode path field, encryption, an unknown
method, overlapping members, a member longer than it says) and 800 of 800 fuzzed ones (compiled and plain `appfilter.xml`,
whole archives, the archives' own fields).

    cargo build --release && python3 parity/compare.py [--only build|icns|import] [--fuzz N] [--seed N] [-v]

## The Python scripts

They stay in `python/` with the same fixes, for a rollback, until these have run a while; `tests/test_liki_icon_import.py`
still tests that one.

## Why Rust: measured

Measured on 2026-11-11 with `tools/bench-port.py` against the Python scripts from git (before these fixes), on the parity
script's made-up worlds, on my desktop (16 threads). Method and every case: [docs/PORT_NUMBERS.md](../../docs/PORT_NUMBERS.md).

| Metric | Python | Rust | Change |
| --- | --- | --- | --- |
| `liki-build --natives-only` (the package's pack), wall time | 46.7 ms | 2.75 ms | −94 %, 44.0 ms faster |
| `liki-build`, a user's pack with app logos, wall time | 51.4 ms | 7.10 ms | −86 %, 44.3 ms faster |
| `liki-icns`, wall time | 9.27 ms | 0.44 ms | −95 %, 8.8 ms faster |
| `liki-icon-import`, a small pack, wall time | 48.5 ms | 1.58 ms | −97 %, 46.9 ms faster |
| `liki-icon-import`, a pack of 3,000 icons, wall time | 127 ms | 30.3 ms | −76 %, 97.0 ms faster |
| peak memory | 10.1–22.5 MB | 2.3–5.7 MB | −74 to −83 % |
| on disk | 35 KB of scripts (+ Python, 75 MB) | 1.3 MB (three binaries) | +1.3 MB; Liki no longer needs Python |

More accurate: the nine bugs listed above under "Bugs fixed" (found while porting, fixed in both), and parity of liki-build
120 of 120 cases, liki-icns 210 of 210, liki-icon-import 40 of 40 made packs, 17 of 17 crafted archives and 800 of 800
fuzzed ones.
