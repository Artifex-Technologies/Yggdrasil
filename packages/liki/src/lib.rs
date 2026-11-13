//! Liki's tools in Rust, replacing the Python scripts at the same paths (kept in `python/` for a rollback) with the same
//! arguments, output, exit codes and files:
//!
//!   build.rs     liki-build: the icon pack of app logos (Simple Icons, CC0) and Liki's own glyphs, as SVG text
//!   icns.rs      liki-icns: the biggest PNG inside a macOS .icns file
//!   import/      liki-icon-import: an Android icon pack (.apk) as a Linux icon theme; the pack is only read
//!                (zip.rs: the archive as Python's zipfile read it; axml.rs: Android's compiled XML; xml.rs: plain XML
//!                through libexpat, the parser Python's ElementTree used)
//!   icon.rs      liki-icon: an app's icon replaced by a picture (.svg, .png, .icns), a folder of them imported, or reset
//!   minos_icon.rs liki-minos-icon: Minos's icon for the icon pack in use, in the accent colour
//!   argparse.rs  the command lines as Python's argparse parsed and printed them (help, usage, errors, exit code 2)
//!   py.rs        the Python behaviours the scripts' results depend on (text files with universal newlines, float
//!                formatting, folder listings in the folder's own order, ~ and $XDG_* as os.path did)
//!   pyjson.rs    json.loads as Python accepts it, for the remembered build (brand-pack.json)
//!
//! Where a script stopped with a traceback, the tool stops too, with exit code 1 and one line on standard error.

pub mod argparse;
pub mod build;
pub mod icns;
pub mod icon;
pub mod import;
pub mod minos_icon;
pub mod py;
pub mod pyjson;

/// Stop with exit code 1 and one line on standard error (where the script ended in a traceback).
pub fn crash(prog: &str, msg: impl std::fmt::Display) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    eprintln!("{prog}: {msg}");
    std::process::exit(1)
}
