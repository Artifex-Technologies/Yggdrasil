//! Archives through 7-Zip: browse ZIP / 7z / RAR / TAR / ISO and more like folders, extract,
//! and create. One external tool (`7zz` or `7z`) handles every format, instead of a Rust crate
//! per format; 7-Zip reads RAR but cannot create it.
//!
//! An archive is addressed by a plain path: `/home/me/photos.zip/2024/july` means the folder
//! `2024/july` inside the file `/home/me/photos.zip`. [`split`] tells the two parts apart, so the
//! rest of the app (history, breadcrumbs, tabs) needs no special case.
//!
//! Everything runs 7-Zip with an argument list (never through a shell), puts `--` before names so
//! a file called `-x` cannot become an option, and extracts only into folders we choose.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::listing::Entry;

/// Extensions treated as archives. Office files (docx, xlsx) are zip inside but are documents to
/// their users, so they are not listed.
pub const EXTENSIONS: &[&str] = &[
    "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "tzst", "lzma", "iso", "cab", "arj", "lzh", "wim",
    "cpio", "rpm", "deb", "jar", "apk", "z", "cbz", "cbr",
];

pub fn is_archive_name(p: &Path) -> bool {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

/// A real file with an archive extension (so a folder named `x.zip` is still a folder).
pub fn is_archive_file(p: &Path) -> bool {
    is_archive_name(p) && p.is_file()
}

/// `(archive file, folder inside it)` when `path` is inside an archive; `inner` is "" for the top.
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    for a in path.ancestors() {
        if is_archive_file(a) {
            let rel = path.strip_prefix(a).ok()?;
            let inner = rel.components().filter_map(|c| match c { Component::Normal(s) => Some(s.to_string_lossy().into_owned()), _ => None }).collect::<Vec<_>>().join("/");
            return Some((a.to_path_buf(), inner));
        }
    }
    None
}

/// The 7-Zip executable, if one is installed.
pub fn find_7z() -> Option<&'static PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let names: &[&str] = if cfg!(windows) { &["7zz.exe", "7z.exe", "7za.exe"] } else { &["7zz", "7z", "7za", "7zr"] };
            let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
            if cfg!(windows) {
                dirs.push(PathBuf::from(r"C:\Program Files\7-Zip"));
                dirs.push(PathBuf::from(r"C:\Program Files (x86)\7-Zip"));
            }
            dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
        })
        .as_ref()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArcEntry {
    /// Slash-separated path inside the archive.
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub encrypted: bool,
    pub modified: Option<SystemTime>,
}

fn civil_to_epoch(y: i64, m: i64, d: i64, hh: i64, mm: i64, ss: i64) -> i64 {
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60 + ss
}

/// "2026-10-10 13:21:59.7960023" -> time (treated as UTC; 7-Zip does not say).
fn parse_time(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let (date, time) = s.split_once(' ')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, mo, da) = (d.next()??, d.next()??, d.next()??);
    let t = time.split('.').next()?;
    let mut tp = t.split(':').map(|x| x.parse::<i64>().ok());
    let (h, mi, se) = (tp.next()??, tp.next()??, tp.next()??);
    let e = civil_to_epoch(y, mo, da, h, mi, se);
    (e >= 0).then(|| UNIX_EPOCH + Duration::from_secs(e as u64))
}

/// Parse `7z l -slt -ba` output: blocks of `Key = Value` lines separated by blank lines.
pub fn parse_listing(text: &str) -> Vec<ArcEntry> {
    let mut out = Vec::new();
    let mut cur: BTreeMap<&str, &str> = BTreeMap::new();
    let flush = |cur: &mut BTreeMap<&str, &str>, out: &mut Vec<ArcEntry>| {
        if let Some(p) = cur.get("Path") {
            let path = p.replace('\\', "/").trim_matches('/').to_string();
            if !path.is_empty() {
                let is_dir = cur.get("Folder") == Some(&"+") || cur.get("Attributes").is_some_and(|a| a.starts_with('D'));
                out.push(ArcEntry {
                    path,
                    size: cur.get("Size").and_then(|s| s.parse().ok()).unwrap_or(0),
                    is_dir,
                    encrypted: cur.get("Encrypted") == Some(&"+"),
                    modified: cur.get("Modified").and_then(|m| parse_time(m)),
                });
            }
        }
        cur.clear();
    };
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            flush(&mut cur, &mut out);
        } else if let Some((k, v)) = line.split_once(" = ") {
            cur.insert(k.trim(), v.trim());
        } else if let Some(k) = line.strip_suffix(" =") {
            cur.insert(k.trim(), "");
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// The immediate children of `inner` (a folder inside the archive) as listing entries. Folders that
/// exist only implicitly (a file at `a/b/c.txt` with no `a` entry) are shown too.
pub fn children(archive: &Path, entries: &[ArcEntry], inner: &str) -> Vec<Entry> {
    let prefix = if inner.is_empty() { String::new() } else { format!("{}/", inner.trim_matches('/')) };
    let mut seen: BTreeMap<String, Entry> = BTreeMap::new();
    for e in entries {
        let Some(rest) = e.path.strip_prefix(&prefix) else { continue };
        if rest.is_empty() {
            continue;
        }
        let (name, nested) = match rest.split_once('/') {
            Some((first, _)) => (first.to_string(), true),
            None => (rest.to_string(), false),
        };
        let is_dir = nested || e.is_dir;
        let virt = archive.join(format!("{prefix}{name}"));
        let slot = seen.entry(name.clone()).or_insert_with(|| Entry {
            hidden: name.starts_with('.'),
            name,
            path: virt,
            is_dir,
            size: 0,
            modified: None,
            usage: None,
        });
        if !nested && !e.is_dir {
            slot.size = e.size;
            slot.modified = e.modified;
        } else if !nested {
            slot.modified = e.modified;
        }
        slot.is_dir |= is_dir;
    }
    seen.into_values().collect()
}

/// The distinct first path components of every entry: what extracting would put in the target folder.
pub fn top_level_names(entries: &[ArcEntry]) -> std::collections::BTreeSet<String> {
    entries.iter().filter_map(|e| e.path.split('/').next()).filter(|s| !s.is_empty()).map(String::from).collect()
}

fn run(args: Vec<std::ffi::OsString>, cwd: Option<&Path>) -> Result<String, String> {
    let exe = find_7z().ok_or("7-Zip is not installed (Arch: pacman -S 7zip)")?;
    let mut c = Command::new(exe);
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let out = c.output().map_err(|e| format!("cannot run 7-Zip: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        return Ok(text);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let all = format!("{text}\n{err}");
    if all.contains("Wrong password") || all.contains("Enter password") {
        return Err("This archive needs a password (not supported yet)".into());
    }
    let msg = all.lines().find(|l| l.contains("ERROR") || l.contains("Error")).unwrap_or("7-Zip failed").trim().to_string();
    Err(msg)
}

fn os(s: &str) -> std::ffi::OsString {
    s.into()
}

/// `list`, remembering the last archive so moving around inside one does not re-run 7-Zip every
/// click. Keyed by path, modification time and size, so a changed archive is re-read.
pub fn list_cached(archive: &Path) -> Result<Vec<ArcEntry>, String> {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(PathBuf, Option<SystemTime>, u64, Vec<ArcEntry>)>> = Mutex::new(None);
    let meta = std::fs::metadata(archive).ok();
    let key = (archive.to_path_buf(), meta.as_ref().and_then(|m| m.modified().ok()), meta.map(|m| m.len()).unwrap_or(0));
    if let Ok(g) = CACHE.lock() {
        if let Some((p, m, l, v)) = g.as_ref() {
            if (p, m, l) == (&key.0, &key.1, &key.2) {
                return Ok(v.clone());
            }
        }
    }
    let v = list(archive)?;
    if let Ok(mut g) = CACHE.lock() {
        *g = Some((key.0, key.1, key.2, v.clone()));
    }
    Ok(v)
}

pub fn list(archive: &Path) -> Result<Vec<ArcEntry>, String> {
    let text = run(vec![os("l"), os("-slt"), os("-ba"), os("-sccUTF-8"), os("--"), archive.into()], None)?;
    Ok(parse_listing(&text))
}

/// A name inside an archive is safe to extract by itself if it is relative and has no `..`.
pub fn safe_inner(inner: &str) -> bool {
    !inner.is_empty() && !inner.starts_with('/') && !inner.starts_with('\\') && !inner.split(['/', '\\']).any(|c| c == ".." || c.contains(':'))
}

fn out_dir(dest: &Path) -> std::ffi::OsString {
    let mut s = std::ffi::OsString::from("-o");
    s.push(dest.as_os_str());
    s
}

/// Extract everything into `dest` (created if needed). Existing files are renamed, not overwritten.
pub fn extract_all(archive: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    run(vec![os("x"), os("-y"), os("-aou"), out_dir(dest), os("--"), archive.into()], None).map(|_| ())
}

/// Extract one entry (a file or folder) into `dest`, keeping its folder structure below it.
pub fn extract_entry(archive: &Path, inner: &str, dest: &Path) -> Result<(), String> {
    if !safe_inner(inner) {
        return Err(format!("refusing unsafe name inside archive: {inner}"));
    }
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    run(vec![os("x"), os("-y"), os("-aou"), out_dir(dest), os("--"), archive.into(), inner.into()], None).map(|_| ())
}

/// Extract one FILE flat (no folders) into `dest`, for opening it. Returns where it landed.
pub fn extract_file_flat(archive: &Path, inner: &str, dest: &Path) -> Result<PathBuf, String> {
    if !safe_inner(inner) {
        return Err(format!("refusing unsafe name inside archive: {inner}"));
    }
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    run(vec![os("e"), os("-y"), os("-aou"), out_dir(dest), os("--"), archive.into(), inner.into()], None)?;
    let name = Path::new(inner).file_name().ok_or("no file name")?;
    Ok(dest.join(name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    SevenZ,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::SevenZ => "7z",
        }
    }
}

/// Compress `sources` (which must share one parent folder) into `dest_archive`. Names inside the
/// archive are relative to that parent, so no absolute paths leak into it.
pub fn create(dest_archive: &Path, sources: &[PathBuf], format: Format) -> Result<(), String> {
    let first = sources.first().ok_or("nothing to compress")?;
    let parent = first.parent().ok_or("cannot compress a drive root")?;
    let mut args = vec![os("a"), os(&format!("-t{}", format.ext())), os("-y"), os("--"), dest_archive.into()];
    for s in sources {
        if s.parent() != Some(parent) {
            return Err("selected items are in different folders".into());
        }
        args.push(s.file_name().ok_or("bad name")?.into());
    }
    run(args, Some(parent)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZIP: &str = include_str!("../tests/fixtures/7z_list_zip.txt");
    const SEVENZ: &str = include_str!("../tests/fixtures/7z_list_7z.txt");

    fn names(v: &[Entry]) -> Vec<(String, bool)> {
        v.iter().map(|e| (e.name.clone(), e.is_dir)).collect()
    }

    #[test]
    fn extension_detection() {
        assert!(is_archive_name(Path::new("a.ZIP")));
        assert!(is_archive_name(Path::new("a.tar.gz")));
        assert!(is_archive_name(Path::new("a.rar")));
        assert!(!is_archive_name(Path::new("a.docx")));
        assert!(!is_archive_name(Path::new("zip")));
    }

    #[test]
    fn parses_real_zip_listing() {
        let e = parse_listing(ZIP);
        let paths: Vec<_> = e.iter().map(|x| x.path.as_str()).collect();
        assert!(paths.contains(&"docs/deep/big.txt"), "{paths:?}");
        assert!(paths.contains(&"readme.txt"));
        let big = e.iter().find(|x| x.path == "docs/deep/big.txt").unwrap();
        assert_eq!((big.size, big.is_dir), (5000, false));
        assert!(e.iter().find(|x| x.path == "docs").unwrap().is_dir);
        assert!(big.modified.is_some());
        assert!(e.iter().any(|x| x.path == "Música/canción.txt"), "unicode names survive");
    }

    #[test]
    fn parses_real_7z_listing_the_same_way() {
        let z = parse_listing(ZIP);
        let s = parse_listing(SEVENZ);
        let key = |v: &[ArcEntry]| {
            let mut k: Vec<_> = v.iter().map(|x| (x.path.clone(), x.size, x.is_dir)).collect();
            k.sort();
            k
        };
        assert_eq!(key(&z), key(&s));
    }

    #[test]
    fn children_of_the_top_and_of_a_subfolder() {
        let e = parse_listing(ZIP);
        let a = Path::new("/x/test.zip");
        let top = children(a, &e, "");
        assert!(names(&top).contains(&("readme.txt".into(), false)));
        assert!(names(&top).contains(&("docs".into(), true)));
        assert_eq!(top.iter().find(|x| x.name == "docs").unwrap().path, a.join("docs"));
        let docs = children(a, &e, "docs");
        assert_eq!(names(&docs).iter().filter(|(n, _)| n == "deep").count(), 1);
        let deep = children(a, &e, "docs/deep");
        assert_eq!(deep.len(), 1);
        assert_eq!(deep[0].size, 5000);
        assert!(children(a, &e, "nope").is_empty());
    }

    #[test]
    fn top_level_names_decide_whether_to_wrap_in_a_folder() {
        let many = parse_listing(ZIP);
        assert_eq!(top_level_names(&many).len(), 4); // docs, empty dir, Música, readme.txt
        let one = vec![
            ArcEntry { path: "trip/a.txt".into(), size: 1, is_dir: false, encrypted: false, modified: None },
            ArcEntry { path: "trip/sub/b.txt".into(), size: 1, is_dir: false, encrypted: false, modified: None },
        ];
        assert_eq!(top_level_names(&one).into_iter().collect::<Vec<_>>(), vec!["trip".to_string()]);
    }

    #[test]
    fn implicit_folders_are_shown() {
        let e = vec![ArcEntry { path: "a/b/c.txt".into(), size: 3, is_dir: false, encrypted: false, modified: None }];
        let top = children(Path::new("/z.zip"), &e, "");
        assert_eq!(names(&top), vec![("a".to_string(), true)]);
    }

    #[test]
    fn split_finds_the_archive_inside_a_path() {
        let d = std::env::temp_dir().join(format!("arc-split-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("real.zip.d")).unwrap();
        std::fs::write(d.join("a.zip"), "x").unwrap();
        assert_eq!(split(&d.join("a.zip")), Some((d.join("a.zip"), String::new())));
        assert_eq!(split(&d.join("a.zip").join("sub").join("deeper")), Some((d.join("a.zip"), "sub/deeper".into())));
        assert_eq!(split(&d.join("real.zip.d")), None);
        std::fs::create_dir_all(d.join("folder.zip")).unwrap(); // a FOLDER named like an archive
        assert_eq!(split(&d.join("folder.zip").join("x")), None);
    }

    #[test]
    fn unsafe_names_are_refused() {
        assert!(safe_inner("docs/a.txt"));
        assert!(!safe_inner("../evil"));
        assert!(!safe_inner("a/../../evil"));
        assert!(!safe_inner("/etc/passwd"));
        assert!(!safe_inner("C:\\x"));
        assert!(!safe_inner(""));
        assert!(extract_entry(Path::new("/x.zip"), "../evil", Path::new("/tmp")).is_err());
    }

    #[test]
    fn time_parsing() {
        let t = parse_time("2026-10-10 13:21:59.7960023").unwrap();
        assert_eq!(t.duration_since(UNIX_EPOCH).unwrap().as_secs(), 1_791_638_519);
        assert!(parse_time("garbage").is_none());
        assert!(parse_time("").is_none());
    }

    /// Real 7-Zip round trip. Skipped (with a message) when 7-Zip is not installed.
    #[test]
    fn real_7zip_round_trip() {
        if find_7z().is_none() {
            eprintln!("SKIPPED: 7-Zip not installed");
            return;
        }
        let d = std::env::temp_dir().join(format!("arc-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src/sub dir")).unwrap();
        std::fs::write(d.join("src/a.txt"), "alpha").unwrap();
        std::fs::write(d.join("src/sub dir/b.txt"), "bravo").unwrap();
        let sources = vec![d.join("src")];
        for (fmt, name) in [(Format::Zip, "t.zip"), (Format::SevenZ, "t.7z")] {
            let arc = d.join(name);
            create(&arc, &sources, fmt).unwrap();
            let l = list(&arc).unwrap();
            assert!(l.iter().any(|e| e.path == "src/a.txt"), "{name}: {l:?}");
            assert!(l.iter().any(|e| e.path == "src/sub dir/b.txt"));
            // no absolute paths inside the archive
            assert!(l.iter().all(|e| !e.path.starts_with('/') && !e.path.contains(':')));
            let one = extract_file_flat(&arc, "src/sub dir/b.txt", &d.join(format!("flat-{name}"))).unwrap();
            assert_eq!(std::fs::read_to_string(one).unwrap(), "bravo");
            extract_all(&arc, &d.join(format!("all-{name}"))).unwrap();
            assert_eq!(std::fs::read_to_string(d.join(format!("all-{name}/src/a.txt"))).unwrap(), "alpha");
        }
        // a file whose name looks like an option must not be treated as one
        std::fs::write(d.join("-x"), "dash").unwrap();
        create(&d.join("dash.zip"), &[d.join("-x")], Format::Zip).unwrap();
        assert!(list(&d.join("dash.zip")).unwrap().iter().any(|e| e.path == "-x"));
        assert!(list(&d.join("missing.zip")).is_err());
    }
}
