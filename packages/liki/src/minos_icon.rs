//! liki-minos-icon [R,G,B|- [dark|light|-]]: gives Minos the right icon for the icon pack in use, coloured like the desktop.
//! Run at each login (autostart), by Settings when the accent or style changes, and by the panel when the icon pack changes.
//!
//! - Liki (Yggdrasil's own pack): liki-build draws Minos with the other app icons, so nothing goes into hicolor.
//! - Another pack that has an icon called "minos": nothing to do, the pack's own is used.
//! - Another pack without one: that pack's own file-manager icon (a vector if it has one, else its largest picture), put in the
//!   user's copy of "hicolor", which every pack falls back to; when it has none either, the bare Minos logo, its lines in the
//!   accent colour (a darker shade on the light version).
//!
//! As the shell script it replaced (same arguments, files, calls and exit codes): the accent, style and pack read from
//! ~/.config/yggdrasil/appearance.conf as its sed read them, the packs searched as `find -L` searched them (folders in their
//! own order, links followed, loops skipped). Fixed: "-" for the style reads it as "-" for the accent always did (the script
//! looked for a picture called "-.svg" and stopped); an accent given that is not three numbers is read from the settings (the
//! script did arithmetic with it, which bash evaluates as code). Not 1:1: numbers with a leading zero are decimal (bash read
//! "010" as octal and stopped at "08"); PNGs of equal size are chosen by name in byte order; the third argument (the tile
//! shape, which nothing drew any more) is ignored.

use crate::icon::{cp, update_icon_cache, why, write_new, xdg};
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const SRC: &str = "/usr/share/liki/minos-icon";
const DEFAULT_ACCENT: &str = "124,84,214";
const FILE_MANAGERS_SVG: [&str; 5] = ["system-file-manager", "file-manager", "org.kde.dolphin", "org.gnome.Nautilus", "nautilus"];
const FILE_MANAGERS_PNG: [&str; 3] = ["system-file-manager", "file-manager", "org.kde.dolphin"];

pub fn main(args: &[OsString]) -> i32 {
    let arg = |i: usize| args.get(i).map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
    let data = xdg("XDG_DATA_HOME", ".local/share");
    let conf = xdg("XDG_CONFIG_HOME", ".config").join("yggdrasil/appearance.conf");
    let hi = data.join("icons/hicolor/scalable/apps");
    let hi_png = data.join("icons/hicolor/256x256/apps");
    let hicolor = data.join("icons/hicolor");

    let mut accent = arg(0);
    if accent == "-" || !is_accent(&accent) {
        accent = appearance(&conf, "accent");
        if !is_accent(&accent) {
            accent = DEFAULT_ACCENT.into();
        }
    }
    let mut mode = arg(1);
    if mode.is_empty() || mode == "-" {
        mode = if appearance(&conf, "style") == "light" { "light" } else { "dark" }.into();
    }
    let mut pack = appearance(&conf, "icons");
    if pack.is_empty() || pack.contains('/') {
        pack = "Liki".into();
    }
    let clear_hicolor = || {
        let _ = std::fs::remove_file(hi.join("minos.svg"));
        let _ = std::fs::remove_file(hi_png.join("minos.png"));
    };

    if pack == "Liki" {
        clear_hicolor();
        update_icon_cache(&hicolor);
        return 0;
    }
    let dirs = [PathBuf::from("/usr/share/icons").join(&pack), data.join("icons").join(&pack)];
    if !find(&dirs, &["minos.svg", "minos.png"], true).is_empty() {
        clear_hicolor();
    } else if let Some(svg) = FILE_MANAGERS_SVG.iter().find_map(|n| find(&dirs, &[&format!("{n}.svg")], true).into_iter().next()) {
        clear_hicolor();
        if mkdir(&hi).and_then(|()| cp(&svg, &hi.join("minos.svg"))).is_err() {
            return 1;
        }
    } else if let Some(png) = FILE_MANAGERS_PNG.iter().find_map(|n| largest(find(&dirs, &[&format!("{n}.png")], false))) {
        clear_hicolor();
        if mkdir(&hi_png).and_then(|()| cp(&png, &hi_png.join("minos.png"))).is_err() {
            return 1;
        }
    } else {
        // nothing of the pack's to use: the bare logo
        clear_hicolor();
        if let Err(code) = logo(&mode, &hex(&accent, &mode), &hi.join("minos.svg")) {
            return code;
        }
    }
    update_icon_cache(&hicolor);
    0
}

/// Three numbers separated by commas, as the script checked the accent it read.
pub fn is_accent(s: &str) -> bool {
    let p: Vec<&str> = s.split(',').collect();
    p.len() == 3 && p.iter().all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The accent as #rrggbb; on the light version a darker shade (each part * 62 / 100), so the lines read on its grey.
pub fn hex(accent: &str, mode: &str) -> String {
    let n: Vec<u64> = accent.split(',').map(|p| p.parse::<u64>().unwrap_or(u64::MAX)).collect();
    let f = |x: u64| if mode == "light" { x.saturating_mul(62) / 100 } else { x };
    format!("#{:02x}{:02x}{:02x}", f(n[0]), f(n[1]), f(n[2]))
}

/// KEY from the desktop's appearance file ("key = value" lines): the rest of the first line that starts with KEY and "=" (blanks
/// around them allowed), as `sed -n 's/^[[:space:]]*KEY[[:space:]]*=[[:space:]]*//p' | head -n1` gave it.
pub fn appearance(conf: &Path, key: &str) -> String {
    let Ok(text) = std::fs::read(conf) else { return String::new() };
    let space = |b: &u8| matches!(b, b' ' | b'\t' | b'\x0b' | b'\x0c' | b'\r');
    for line in text.split(|&b| b == b'\n') {
        let l = &line[line.iter().take_while(|b| space(b)).count()..];
        let Some(l) = l.strip_prefix(key.as_bytes()) else { continue };
        let l = &l[l.iter().take_while(|b| space(b)).count()..];
        let Some(l) = l.strip_prefix(b"=") else { continue };
        let l = &l[l.iter().take_while(|b| space(b)).count()..];
        return String::from_utf8_lossy(l).into_owned();
    }
    String::new()
}

/// `find -L DIRS -name N1 -o -name N2 ...`: every entry so named under the folders (the folders themselves included), depth
/// first in each folder's own order, links followed, a folder met again inside itself skipped. FIRST: stop at the first.
pub fn find(dirs: &[PathBuf], names: &[&str], first: bool) -> Vec<PathBuf> {
    let mut out = vec![];
    for d in dirs {
        walk(d, names, first, &mut Vec::new(), &mut out);
        if first && !out.is_empty() {
            break;
        }
    }
    out
}

fn walk(p: &Path, names: &[&str], first: bool, above: &mut Vec<(u64, u64)>, out: &mut Vec<PathBuf>) {
    // a link that leads nowhere is still an entry (find -L tests it as itself); a path that is not there is not
    if std::fs::symlink_metadata(p).is_err() {
        return;
    }
    if p.file_name().is_some_and(|n| names.iter().any(|w| n == *w)) {
        out.push(p.to_path_buf());
        if first {
            return;
        }
    }
    let Ok(m) = std::fs::metadata(p) else { return };
    if !m.is_dir() || above.contains(&(m.dev(), m.ino())) {
        return;
    }
    let Ok(rd) = std::fs::read_dir(p) else { return };
    above.push((m.dev(), m.ino()));
    for e in rd.flatten() {
        walk(&e.path(), names, first, above, out);
        if first && !out.is_empty() {
            break;
        }
    }
    above.pop();
}

/// The biggest file (by its size, links followed); of equal ones the last by name, as `sort -rn | head -n1` chose.
fn largest(found: Vec<PathBuf>) -> Option<PathBuf> {
    found
        .into_iter()
        .map(|p| (std::fs::metadata(&p).or_else(|_| std::fs::symlink_metadata(&p)).map(|m| m.len()).unwrap_or(0), p))
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_os_str().as_encoded_bytes().cmp(b.1.as_os_str().as_encoded_bytes())))
        .map(|(_, p)| p)
}

fn mkdir(d: &Path) -> Result<(), ()> {
    std::fs::create_dir_all(d).map_err(|e| eprintln!("mkdir: cannot create directory '{}': {}", d.display(), why(&e)))
}

/// The bare logo (dark or light) with its lines (@LINE@, the first on each line of the file) in the accent, written whole and
/// renamed into place. Err(exit code) as the script's tools stopped it.
fn logo(mode: &str, hex: &str, to: &Path) -> Result<(), i32> {
    let from = Path::new(SRC).join(format!("{mode}.svg"));
    let text = std::fs::read(&from).map_err(|e| {
        eprintln!("sed: can't read {}: {}", from.display(), why(&e));
        2
    })?;
    let mut out = Vec::with_capacity(text.len());
    for (i, line) in text.split(|&b| b == b'\n').enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        match line.windows(6).position(|w| w == b"@LINE@") {
            Some(at) => {
                out.extend_from_slice(&line[..at]);
                out.extend_from_slice(hex.as_bytes());
                out.extend_from_slice(&line[at + 6..]);
            }
            None => out.extend_from_slice(line),
        }
    }
    if let Some(parent) = to.parent() {
        mkdir(parent).map_err(|()| 1)?;
    }
    let tmp = PathBuf::from(format!("{}.tmp", to.display()));
    write_new(&tmp, &out, 0o666).map_err(|e| {
        eprintln!("liki-minos-icon: {}: {}", tmp.display(), why(&e));
        1
    })?;
    std::fs::rename(&tmp, to).map_err(|e| {
        eprintln!("mv: cannot move '{}' to '{}': {}", tmp.display(), to.display(), why(&e));
        1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents() {
        assert!(is_accent("124,84,214"));
        assert!(!is_accent("124,84"));
        assert!(!is_accent("1,2,3,4"));
        assert!(!is_accent("x[$(touch /tmp/p)],1,1"), "bash would have run this in its arithmetic");
        assert!(!is_accent(" 1,2,3"));
        assert_eq!(hex("124,84,214", "dark"), "#7c54d6");
        assert_eq!(hex("124,84,214", "light"), "#4c3484");
        assert_eq!(hex("255,255,255", "light"), "#9e9e9e");
    }

    #[test]
    fn appearance_lines() {
        let d = std::env::temp_dir().join(format!("liki-minos-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("appearance.conf");
        std::fs::write(&f, b"accent_color = 1,1,1\n  accent \t= 10,20,30 \nstyle=light\nicons = Papirus\naccent = 9,9,9\n").unwrap();
        assert_eq!(appearance(&f, "accent"), "10,20,30 ");
        assert_eq!(appearance(&f, "style"), "light");
        assert_eq!(appearance(&f, "icons"), "Papirus");
        assert_eq!(appearance(&f, "font"), "");
        let _ = std::fs::remove_dir_all(&d);
    }
}
