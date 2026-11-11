//! liki-icon-import: an Android icon pack (an .apk, a zip archive) as a Linux icon theme. The pack is only read, nothing in
//! it is run: its list of which Android app gets which picture (appfilter.xml, plain or compiled) and its square PNG
//! pictures. Each picture goes to the installed program it fits, by comparing the pack's names (the app's package and
//! picture name) with the installed programs' icon names, names, commands, window classes and desktop file names.

pub mod axml;
pub mod xml;
pub mod zip;

use crate::argparse::{Arg, Kind, Parser};
use crate::py::{self, join, norm};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use zip::{Bad, Zip};

/// unpacked bytes of pictures read from one pack
const MAX_TOTAL: u64 = 700 * 1024 * 1024;
/// one picture
const MAX_PNG: u64 = 6 * 1024 * 1024;
/// appfilter.xml (the largest packs' lists are a few MB; a bigger one is not read, as in the script since this fix)
pub const MAX_APPFILTER: u64 = 32 * 1024 * 1024;
const GENERIC: [&str; 18] = [
    "com", "org", "net", "io", "android", "google", "apps", "app", "the", "activity", "main",
    "launcher", "mobile", "free", "pro", "lite", "beta", "dev",
];
const GENERIC_MORE: [&str; 2] = ["desktop", "stable"];

fn generic(s: &str) -> bool {
    let l = s.to_lowercase();
    GENERIC.contains(&l.as_str()) || GENERIC_MORE.contains(&l.as_str())
}

/// [(component, drawable)] from assets/appfilter.xml (plain) or res/xml/appfilter.xml or res/raw/appfilter.xml (compiled
/// or plain): the first of them that parses and lists at least one picture.
pub fn parse_appfilter(zf: &Zip) -> Result<Vec<(String, String)>, Bad> {
    let mut names: HashMap<String, String> = HashMap::new();
    for e in &zf.entries {
        names.insert(e.filename.to_lowercase(), e.filename.clone());
    }
    let mut items = vec![];
    for path in [
        "assets/appfilter.xml",
        "res/xml/appfilter.xml",
        "res/raw/appfilter.xml",
    ] {
        let Some(real) = names.get(path) else {
            continue;
        };
        let entry = zf.get(real).unwrap();
        if entry.file_size > MAX_APPFILTER {
            continue;
        }
        let data = zf.read_member(entry)?;
        let elems = if data.starts_with(b"\x03\x00") {
            axml::parse(&data)
        } else {
            xml::parse(&data)
        };
        let Ok(elems) = elems else {
            continue;
        };
        for (tag, a) in elems {
            let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            if tag == "item" {
                if let Some(d) = get("drawable").filter(|d| !d.is_empty()) {
                    items.push((get("component").unwrap_or_default(), d));
                }
            }
        }
        if !items.is_empty() {
            break;
        }
    }
    Ok(items)
}

/// Python's re.IGNORECASE match of one character against a lower-case ASCII letter (Unicode simple lower case, and
/// the dotless i and long s that sre treats as i and s).
fn ci(c: char, p: char) -> bool {
    c.to_ascii_lowercase() == p
        || (p == 'i' && (c == 'ı' || c == 'İ'))
        || (p == 's' && c == 'ſ')
        || (p == 'k' && c == '\u{212a}')
}

fn ci_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let mut it = s.char_indices();
    for p in prefix.chars() {
        let (_, c) = it.next()?;
        if !(c == p || (p.is_ascii_alphabetic() && ci(c, p))) {
            return None;
        }
    }
    Some(it.next().map(|(i, _)| &s[i..]).unwrap_or(""))
}

/// re.match(r"^(res/(drawable|mipmap)[^/]*/|assets/)[^/]+\.png$", name, re.I)
pub fn picture_path(name: &str) -> bool {
    let rest = if let Some(r) = ci_prefix(name, "assets/") {
        r
    } else if let Some(r) = ci_prefix(name, "res/") {
        let Some(r) = ci_prefix(r, "drawable").or_else(|| ci_prefix(r, "mipmap")) else {
            return false;
        };
        match r.find('/') {
            Some(i) => &r[i + 1..],
            None => return false,
        }
    } else {
        return false;
    };
    if rest.contains('/') {
        return false;
    }
    let chars: Vec<char> = rest.chars().collect();
    chars.len() >= 5
        && chars[chars.len() - 4] == '.'
        && chars[chars.len() - 3..]
            .iter()
            .zip("png".chars())
            .all(|(&c, p)| ci(c, p))
}

/// (width, height) of a PNG's header
fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if !b.starts_with(b"\x89PNG\r\n\x1a\n") || b.len() < 24 {
        return None;
    }
    Some((
        u32::from_be_bytes(b[16..20].try_into().unwrap()),
        u32::from_be_bytes(b[20..24].try_into().unwrap()),
    ))
}

/// name -> (width, bytes): the biggest square picture of each name (the first of equal ones), at least 32 pixels.
pub fn collect_pngs(zf: &Zip) -> Result<BTreeMap<String, (u32, Vec<u8>)>, Bad> {
    let mut best: BTreeMap<String, (u32, Vec<u8>)> = BTreeMap::new();
    let mut total: u64 = 0;
    for info in &zf.entries {
        let n = &info.filename;
        if info.is_dir() || !n.to_lowercase().ends_with(".png") || info.file_size > MAX_PNG {
            continue;
        }
        if !picture_path(n) {
            continue;
        }
        total += info.file_size;
        if total > MAX_TOTAL {
            break;
        }
        let b = zf.read_member(info)?;
        let Some((w, h)) = png_size(&b) else {
            continue;
        };
        if w != h || w < 32 {
            continue;
        }
        let base = py::basename(n);
        let name: String = base
            .chars()
            .take(base.chars().count().saturating_sub(4))
            .collect();
        if best.get(&name).is_none_or(|(bw, _)| w > *bw) {
            best.insert(name, (w, b));
        }
    }
    Ok(best)
}

/// The first line of the desktop file's first section that starts with KEY=, its value stripped.
fn field(sect: &str, key: &str) -> String {
    for line in sect.split('\n') {
        if let Some(v) = line.strip_prefix(key).and_then(|r| r.strip_prefix('=')) {
            return pycompat::strip(v).to_string();
        }
    }
    String::new()
}

/// [(icon name, keys)] for the installed programs: the names an Android pack might call them (in the order first met).
pub fn installed_apps() -> Vec<(String, BTreeSet<String>)> {
    let dirs = [
        PathBuf::from("/usr/share/applications"),
        py::expanduser("/.local/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
        py::expanduser("/.local/share/flatpak/exports/share/applications"),
    ];
    let mut apps: Vec<(String, BTreeSet<String>)> = vec![];
    for d in dirs {
        for f in py::listing(&d, ".desktop") {
            let Ok(Ok(text)) = py::read_text(&f, true) else {
                continue;
            };
            let sect = text.split("\n[").next().unwrap();
            let (icon, name, ex, wm) = (
                field(sect, "Icon"),
                field(sect, "Name"),
                field(sect, "Exec"),
                field(sect, "StartupWMClass"),
            );
            if icon.is_empty()
                || icon.starts_with('/')
                || (field(sect, "NoDisplay").to_lowercase() == "true" && name.is_empty())
            {
                continue;
            }
            let file_name = f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let stem: String = file_name
                .chars()
                .take(file_name.chars().count().saturating_sub(8))
                .collect();
            let mut keys: BTreeSet<String> = BTreeSet::new();
            keys.insert(norm(&icon));
            keys.insert(norm(&name));
            keys.insert(if ex.is_empty() {
                String::new()
            } else {
                norm(py::basename(pycompat::split(&ex)[0]))
            });
            keys.insert(norm(&wm));
            keys.insert(norm(&stem));
            // reverse-DNS icon names: the last part is the program's own name
            keys.insert(norm(icon.rsplit('.').next().unwrap()));
            keys.remove("");
            match apps.iter_mut().find(|(i, _)| *i == icon) {
                Some((_, k)) => k.extend(keys),
                None => apps.push((icon, keys)),
            }
        }
    }
    apps
}

/// re.search(r"ComponentInfo\{([^/}]+)/?([^}]*)\}", component): (package, class)
fn component_info(c: &str) -> Option<(&str, &str)> {
    let mut from = 0;
    while let Some(k) = c[from..].find("ComponentInfo{") {
        let start = from + k + 14;
        let rest = &c[start..];
        let g1_end = rest.find(['/', '}']).unwrap_or(rest.len());
        if g1_end > 0 && g1_end < rest.len() {
            let after = &rest[g1_end..];
            if after.starts_with('}') {
                return Some((&rest[..g1_end], ""));
            }
            // after a "/", the class runs to the next "}"
            if let Some(close) = after[1..].find('}') {
                return Some((&rest[..g1_end], &after[1..1 + close]));
            }
        }
        from = from + k + 1;
    }
    None
}

pub fn android_keys(component: &str, drawable: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    keys.insert(norm(drawable));
    if let Some((pkg, cls)) = component_info(component) {
        for seg in pkg.split('.') {
            if !norm(seg).is_empty() && !generic(seg) {
                keys.insert(norm(seg));
            }
        }
        keys.insert(norm(pkg.rsplit('.').next().unwrap()));
        let last = cls.rsplit('.').next().unwrap();
        if !norm(last).is_empty() && !generic(last) {
            keys.insert(norm(last));
        }
    }
    keys.remove("");
    keys
}

/// {linux icon name: picture name}. A program gets a picture when one of its own names is the picture's name; failing that,
/// when it is the pack's package or class name for exactly one picture. A name several pictures claim is never guessed.
/// Names are tried with the icon's own first, then the longest, then in alphabetical order (the script's order among
/// names of equal length changed from run to run, see python/liki-icon-import).
pub fn match_(
    items: &[(String, String)],
    pngs: &BTreeMap<String, (u32, Vec<u8>)>,
    apps: &[(String, BTreeSet<String>)],
) -> Vec<(String, String)> {
    let mut by_name: HashMap<String, String> = HashMap::new();
    let mut by_pkg: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (comp, drawable) in items {
        if !pngs.contains_key(drawable) {
            continue;
        }
        by_name
            .entry(norm(drawable))
            .or_insert_with(|| drawable.clone());
        let own = norm(drawable);
        for k in android_keys(comp, drawable) {
            if k != own {
                by_pkg.entry(k).or_default().insert(drawable.clone());
            }
        }
    }
    let mut chosen: Vec<(String, String)> = vec![];
    for (icon, keys) in apps {
        let own = norm(icon);
        let mut order: Vec<&String> = keys.iter().collect();
        order.sort_by(|a, b| {
            (**a != own, std::cmp::Reverse(a.chars().count()), *a).cmp(&(
                **b != own,
                std::cmp::Reverse(b.chars().count()),
                *b,
            ))
        });
        let pick = order
            .iter()
            .find(|k| k.chars().count() >= 3 && by_name.contains_key(**k))
            .map(|k| by_name[*k].clone())
            .or_else(|| {
                order
                    .iter()
                    .find(|k| {
                        k.chars().count() >= 4 && by_pkg.get(**k).is_some_and(|s| s.len() == 1)
                    })
                    .map(|k| by_pkg[*k].iter().next().unwrap().clone())
            });
        if let Some(p) = pick {
            chosen.push((icon.clone(), p));
        }
    }
    chosen
}

/// re.match(r"^[A-Za-z0-9][A-Za-z0-9._+-]{0,120}$", n)
pub fn safe_icon_name(n: &str) -> bool {
    let n = n.strip_suffix('\n').unwrap_or(n);
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 121
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(c))
}

/// The theme's name from the pack's file name: no extension, no (...) or [...], no version numbers, only letters, digits,
/// spaces and dashes.
pub fn default_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let base = py::basename(trimmed);
    // os.path.splitext: the last dot that is not one of the leading ones
    let stem = match base.rfind('.') {
        Some(i) if base[..i].chars().any(|c| c != '.') => &base[..i],
        _ => base,
    };
    // re.sub(r"\(.*?\)|\[.*?\]", " ", stem): a bracket to its first closing one on the same line
    let mut s = String::new();
    let chars: Vec<char> = stem.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let close = match chars[i] {
            '(' => Some(')'),
            '[' => Some(']'),
            _ => None,
        };
        if let Some(close) = close {
            if let Some(j) = chars[i + 1..]
                .iter()
                .position(|&c| c == close || c == '\n')
                .map(|j| i + 1 + j)
                .filter(|&j| chars[j] == close)
            {
                s.push(' ');
                i = j + 1;
                continue;
            }
        }
        s.push(chars[i]);
        i += 1;
    }
    // re.sub(r"\bv?\d+(\.\d+)+\b", " ", s): versions such as v1.8.1
    let s = strip_versions(&s);
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let words = pycompat::split(&s);
    if words.is_empty() {
        "Imported icons".into()
    } else {
        words.join(" ")
    }
}

/// re.sub(r"\bv?\d+(\.\d+)+\b", " ", s) with Python's \w (str.isalnum() or "_") and \d (any script's decimal digits)
fn strip_versions(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let word = |k: usize| k < c.len() && pycompat::isword(c[k]);
    let digit = |k: usize| k < c.len() && pycompat::decimal(c[k]).is_some();
    let boundary = |k: usize| (k > 0 && pycompat::isword(c[k - 1])) != word(k);
    // the end of a match starting at `k` (digits then one or more ".digits", the longest run that ends at a boundary)
    let match_at = |k: usize| -> Option<usize> {
        if !digit(k) {
            return None;
        }
        let mut j = k;
        while digit(j) {
            j += 1;
        }
        let mut ends = vec![];
        while j < c.len() && c[j] == '.' && digit(j + 1) {
            j += 1;
            while digit(j) {
                j += 1;
            }
            ends.push(j);
        }
        ends.into_iter().rev().find(|&e| boundary(e))
    };
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if boundary(i) {
            let m = if c[i] == 'v' {
                match_at(i + 1).or_else(|| match_at(i))
            } else {
                match_at(i)
            };
            if let Some(e) = m {
                out.push(' ');
                i = e;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// Turn the pack into a theme at OUT_ROOT/NAME; (theme folder, programs matched, programs looked at), or the message the
/// script stopped with (exit code 1), or Bad for a file that cannot be read as a zip.
pub fn build(
    pack: &Path,
    name: &str,
    out_root: &Path,
) -> Result<Result<(PathBuf, usize, usize), String>, Bad> {
    let file = std::fs::File::open(pack)
        .unwrap_or_else(|e| crate::crash("liki-icon-import", format!("{}: {e}", pack.display())));
    let zf = Zip::open(file)?;
    let items = parse_appfilter(&zf)?;
    let pngs = collect_pngs(&zf)?;
    drop(zf);
    if pngs.is_empty() {
        return Ok(Err(
            "No PNG pictures found in that file (is it an Android icon pack?).".into(),
        ));
    }
    if items.is_empty() {
        return Ok(Err(
            "That file has no appfilter.xml, so there is no list of which app gets which picture."
                .into(),
        ));
    }
    let apps = installed_apps();
    let chosen = match_(&items, &pngs, &apps);
    let with_png = items.iter().filter(|(_, d)| pngs.contains_key(d)).count();
    println!(
        "The pack lists {} app(s), {} of them with a usable PNG picture ({} PNG pictures in all).",
        items.len(),
        with_png,
        pngs.len()
    );
    println!(
        "{} installed program(s) looked at, {} matched.",
        apps.len(),
        chosen.len()
    );
    let theme = join(out_root, name);
    if theme.is_dir() && join(&theme, "index.theme").is_file() {
        // a fresh import replaces the old one (shutil.rmtree, which refuses a link)
        if std::fs::symlink_metadata(&theme)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            crate::crash("liki-icon-import", "Cannot call rmtree on a symbolic link");
        }
        if let Err(e) = std::fs::remove_dir_all(&theme) {
            crate::crash("liki-icon-import", format!("{}: {e}", theme.display()));
        }
    }
    let mut sizes: BTreeSet<u32> = BTreeSet::new();
    for (icon, draw) in &chosen {
        if !safe_icon_name(icon) {
            continue;
        }
        let (w, b) = &pngs[draw];
        let d = join(&join(&theme, format!("{w}x{w}")), "apps");
        if let Err(e) = std::fs::create_dir_all(&d) {
            crate::crash("liki-icon-import", format!("{}: {e}", d.display()));
        }
        let f = join(&d, format!("{icon}.png"));
        if let Err(e) = std::fs::write(&f, b) {
            crate::crash("liki-icon-import", format!("{}: {e}", f.display()));
        }
        sizes.insert(*w);
    }
    if chosen.is_empty() {
        let pack_names: BTreeSet<&String> = items.iter().map(|(_, d)| d).collect();
        let ex_pack: Vec<&str> = pack_names.iter().take(8).map(|s| s.as_str()).collect();
        let mut here: Vec<&String> = apps.iter().map(|(i, _)| i).collect();
        here.sort();
        let ex_here: Vec<&str> = here.iter().take(8).map(|s| s.as_str()).collect();
        return Ok(Err(format!(
            "None of the installed programs matched a picture in that pack.\n  The pack's picture names look like: {}\n  Installed icon names look like: {}",
            ex_pack.join(", "),
            ex_here.join(", ")
        )));
    }
    let dirs: Vec<String> = sizes.iter().map(|s| format!("{s}x{s}/apps")).collect();
    let parts: String = sizes
        .iter()
        .map(|s| format!("\n[{s}x{s}/apps]\nSize={s}\nType=Threshold\nContext=Applications\n"))
        .collect();
    let index = join(&theme, "index.theme");
    let text = format!(
        "[Icon Theme]\nName={name}\nComment=Imported from an Android icon pack ({} of {} installed programs matched)\nInherits=hicolor\nDirectories={}\n{parts}",
        chosen.len(),
        apps.len(),
        dirs.join(",")
    );
    if let Err(e) = std::fs::write(&index, text) {
        crate::crash("liki-icon-import", format!("{}: {e}", index.display()));
    }
    Ok(Ok((theme, chosen.len(), apps.len())))
}

pub fn parser() -> Parser {
    Parser::new(
        Parser::prog_from_argv0(),
        "Turn an Android icon pack into a Linux icon theme.",
        vec![
            Arg {
                flags: &[],
                dest: "pack",
                kind: Kind::Value {
                    choices: None,
                    default: None,
                },
                help: None,
            },
            Arg {
                flags: &["--name"],
                dest: "name",
                kind: Kind::Value {
                    choices: None,
                    default: None,
                },
                help: None,
            },
            Arg {
                flags: &["--out"],
                dest: "out",
                kind: Kind::Value {
                    choices: None,
                    default: None,
                },
                help: None,
            },
        ],
    )
}

/// liki-icon-import; the exit code.
pub fn main(argv: &[String]) -> i32 {
    let a = match parser().parse(argv) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let pack = a.str("pack").unwrap().to_string();
    if !Path::new(&pack).is_file() {
        eprintln!("error: no such file: {pack}");
        return 2;
    }
    let name = a
        .str("name")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_name(&pack));
    let out = a
        .str("out")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            join(
                &py::env_or("XDG_DATA_HOME", || py::expanduser("/.local/share")),
                "icons",
            )
        });
    match build(Path::new(&pack), &name, &out) {
        Err(_) => {
            eprintln!("error: that is not a zip/apk file");
            1
        }
        Ok(Err(msg)) => {
            use std::io::Write;
            let _ = std::io::stdout().flush();
            eprintln!("{msg}");
            1
        }
        Ok(Ok((theme, n, total))) => {
            println!(
                "{name}: {n} of {total} installed programs matched, written to {}",
                theme.display()
            );
            println!("Choose it in Settings > Appearance > Icons (use the 'Use' button); programs without a match keep their usual icon.");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_name_drops_versions_and_tags() {
        assert_eq!(
            default_name("/x/Camo Dark v1.8.1 (Patched).apk"),
            "Camo Dark"
        );
        assert_eq!(default_name("/x/Pack_2.0[mod].zip"), "Pack 2 0");
        assert_eq!(default_name("/x/.apk"), "apk");
        assert_eq!(default_name("/x/(only).apk"), "Imported icons");
        assert_eq!(default_name("Icons 1.2.3a.apk"), "Icons 3a");
        assert_eq!(default_name("dir/"), "dir");
    }

    #[test]
    fn pictures_are_found_where_android_keeps_them() {
        assert!(picture_path("res/drawable-nodpi/firefox.png"));
        assert!(picture_path("RES/MipMap-xxhdpi/x.PNG"));
        assert!(picture_path("assets/a.png"));
        assert!(picture_path("res/mıpmap/x.png"));
        assert!(picture_path("reſ/drawable/x.png"));
        assert!(!picture_path("res/drawable/sub/x.png"));
        assert!(!picture_path("res/raw/x.png"));
        assert!(!picture_path("assets/.png"));
        assert!(!picture_path("assets/x.jpg"));
    }

    #[test]
    fn matching_uses_package_and_picture_names() {
        let items = vec![
            (
                "ComponentInfo{org.mozilla.firefox/x.Y}".to_string(),
                "ic_browser".to_string(),
            ),
            (
                "ComponentInfo{com.spotify.music/z.A}".to_string(),
                "spotify".to_string(),
            ),
        ];
        let pngs: BTreeMap<String, (u32, Vec<u8>)> = [
            ("ic_browser".to_string(), (192, vec![])),
            ("spotify".to_string(), (192, vec![])),
        ]
        .into_iter()
        .collect();
        let set = |v: &[&str]| {
            v.iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<String>>()
        };
        let apps = vec![
            (
                "firefox".to_string(),
                set(&["firefox", "firefoxwebbrowser"]),
            ),
            (
                "spotify-client".to_string(),
                set(&["spotifyclient", "spotify"]),
            ),
            ("unrelated".to_string(), set(&["unrelated"])),
        ];
        assert_eq!(
            match_(&items, &pngs, &apps),
            vec![
                ("firefox".into(), "ic_browser".into()),
                ("spotify-client".into(), "spotify".into())
            ]
        );
    }

    #[test]
    fn generic_names_are_not_guessed() {
        let items = vec![
            (
                "ComponentInfo{com.android.settings/x.A}".to_string(),
                "ic_phone".to_string(),
            ),
            (
                "ComponentInfo{com.samsung.settings/x.A}".to_string(),
                "ic_other".to_string(),
            ),
        ];
        let pngs: BTreeMap<String, (u32, Vec<u8>)> = [
            ("ic_phone".to_string(), (192, vec![])),
            ("ic_other".to_string(), (192, vec![])),
        ]
        .into_iter()
        .collect();
        let apps = vec![(
            "preferences-system".to_string(),
            ["settings".to_string()].into_iter().collect(),
        )];
        assert!(match_(&items, &pngs, &apps).is_empty());
    }

    #[test]
    fn components_are_read_as_the_pattern_reads_them() {
        assert_eq!(
            component_info("ComponentInfo{a.b/c.D}"),
            Some(("a.b", "c.D"))
        );
        assert_eq!(component_info("ComponentInfo{a.b}"), Some(("a.b", "")));
        assert_eq!(
            component_info("x ComponentInfo{/x} ComponentInfo{p/q/r}"),
            Some(("p", "q/r"))
        );
        assert_eq!(component_info("ComponentInfo{a/b"), None);
        assert!(!safe_icon_name("../evil"));
        assert!(safe_icon_name("org.gnome.Nautilus"));
    }
}
