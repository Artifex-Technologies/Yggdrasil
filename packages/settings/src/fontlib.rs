//! Settings > Appearance > Fonts, the font library: the fonts installed on this computer (fontconfig's list), adding your own
//! (to ~/.local/share/fonts, no password) and removing those you added.
//!
//! Every file added goes through Heimdall's file guard first, whatever it came from: `heimdall-fileguard scan --json` for a
//! font file, `heimdall-fileguard extract --json --only ttf,otf,ttc,woff2` for a .zip (the guard unpacks it, with its own
//! limits; Settings never unpacks an archive). A refused file is not installed and the reason is shown; a warning is shown and
//! the font can still be installed by choice. A guard without `scan` and `extract` (an older Heimdall) or no guard at all:
//! nothing is accepted. Each file is also read here as a font before it is installed: TrueType, OpenType and collections with
//! skrifa (Rust, memory-safe: the same reader egui draws with), WOFF2 by its header and fontconfig's own reading in a process of
//! its own (fc-scan); 50 MB at most.
//!
//! The order, so nothing is checked in one place and installed from another: the file is copied into a private folder
//! (~/.cache/yggdrasil/font-import, 0700), the guard and the font check read that copy, and that copy is what is moved into the
//! fonts folder. Then `fc-cache` for that folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The kinds of font file taken.
pub const KINDS: [&str; 4] = ["ttf", "otf", "ttc", "woff2"];
/// The largest font file taken.
pub const MAX_BYTES: u64 = 50 << 20;
/// The file guard (Heimdall's).
pub const GUARD: &str = "heimdall-fileguard";

/// Your own fonts' folder: `$XDG_DATA_HOME/fonts`, else `~/.local/share/fonts` (fontconfig reads both).
pub fn user_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"))
        .join("fonts")
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

// ------------------------------------------------------------------------------------------------ the installed fonts

/// A family as fontconfig lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub name: String,
    /// Its styles (Regular, Bold, ...; a variable font's named weights each count).
    pub faces: usize,
    pub files: Vec<PathBuf>,
    /// Every file is in your own fonts folder: you added it, and can remove it here.
    pub yours: bool,
    /// The file to draw its sample with (the regular face when there is one), and its index in a collection.
    pub sample: Option<(PathBuf, u32)>,
}

/// `fc-list --format '%{family[0]}\t%{style[0]}\t%{file}\t%{index}\n'`, grouped by family and sorted by name.
pub fn parse_list(text: &str, yours: &Path) -> Vec<Family> {
    // per family: its styles, its files, and the sample file (with its index, and whether it is the regular face)
    type Seen = (Vec<String>, Vec<PathBuf>, Option<(PathBuf, u32, bool)>);
    let mut map: std::collections::BTreeMap<String, Seen> = Default::default();
    for line in text.lines() {
        let mut it = line.split('\t');
        let (Some(name), Some(style), Some(file), Some(index)) = (it.next(), it.next(), it.next(), it.next()) else { continue };
        let name = name.trim();
        if name.is_empty() || file.is_empty() {
            continue;
        }
        let e = map.entry(name.to_string()).or_default();
        if !e.0.iter().any(|s| s == style) {
            e.0.push(style.to_string());
        }
        let file = PathBuf::from(file);
        if !e.1.contains(&file) {
            e.1.push(file.clone());
        }
        // a variable font's named instances have high indices; the face itself is the low 16 bits
        let idx = index.trim().parse::<u32>().unwrap_or(0) & 0xffff;
        let regular = matches!(style, "Regular" | "Book" | "Normal" | "Roman");
        if e.2.as_ref().is_none_or(|(_, _, r)| regular && !*r) {
            e.2 = Some((file, idx, regular));
        }
    }
    let mut out: Vec<Family> = map
        .into_iter()
        .map(|(name, (styles, files, sample))| Family {
            yours: !files.is_empty() && files.iter().all(|f| f.starts_with(yours)),
            name,
            faces: styles.len(),
            files,
            sample: sample.map(|(f, i, _)| (f, i)),
        })
        .collect();
    out.sort_by_key(|f| f.name.to_lowercase());
    out
}

/// The installed fonts, now.
pub fn installed() -> Vec<Family> {
    let out = Command::new("fc-list")
        .args(["--format", "%{family[0]}\t%{style[0]}\t%{file}\t%{index}\n"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    parse_list(&out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default(), &user_dir())
}

/// The families whose name has `query` in it (any case); all for an empty query.
pub fn search<'a>(list: &'a [Family], query: &str) -> Vec<&'a Family> {
    let q = query.trim().to_lowercase();
    list.iter().filter(|f| q.is_empty() || f.name.to_lowercase().contains(&q)).collect()
}

// ------------------------------------------------------------------------------------------------ the file guard

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Refused,
}

/// What the guard said about one file.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub level: Level,
    pub reasons: Vec<String>,
    /// ClamAV's part: "clean", "infected:SIGNATURE", "unavailable" or "not-fully-scanned".
    pub clamav: String,
}

impl Verdict {
    fn from_json(v: &serde_json::Value) -> Option<Verdict> {
        let level = match v["verdict"].as_str()? {
            "ok" => Level::Ok,
            "warn" => Level::Warn,
            "refused" => Level::Refused,
            _ => return None,
        };
        let reasons =
            v["reasons"].as_array().map(|a| a.iter().filter_map(|r| r.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let clamav = v["clamav"].as_str().unwrap_or("").to_string();
        // an infection is a refusal whatever the verdict field says
        let level = if clamav.starts_with("infected") { Level::Refused } else { level };
        Some(Verdict { level, reasons, clamav })
    }

    /// The reasons in a line, ClamAV's word when it could not look.
    pub fn words(&self) -> String {
        let mut w = self.reasons.clone();
        match self.clamav.as_str() {
            "unavailable" => w.push("the virus scanner (ClamAV) is not available".into()),
            "not-fully-scanned" => w.push("the virus scanner could not read all of it".into()),
            c if c.starts_with("infected:") => w.push(format!("ClamAV found {}", &c["infected:".len()..])),
            _ => {}
        }
        w.join("; ")
    }
}

/// Why nothing can be added.
#[derive(Clone, Debug, PartialEq)]
pub enum GuardMissing {
    /// No heimdall-fileguard at all.
    Absent,
    /// One without `scan` and `extract`.
    TooOld,
}

/// Whether the guard can check new files: its help names both `scan` and `extract`.
pub fn guard_ready(prog: &str) -> Result<(), GuardMissing> {
    let out = Command::new(prog).arg("--help").stdin(Stdio::null()).output().map_err(|_| GuardMissing::Absent)?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let has = |cmd: &str| text.lines().any(|l| l.split_whitespace().take(2).collect::<Vec<_>>() == ["heimdall-fileguard", cmd]);
    if has("scan") && has("extract") {
        Ok(())
    } else {
        Err(GuardMissing::TooOld)
    }
}

/// `scan --json`'s answer: a verdict per path, in the order asked; Err when it is not that.
pub fn parse_scan(text: &str, asked: &[PathBuf]) -> Result<Vec<Verdict>, String> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| "the file guard's answer is not JSON".to_string())?;
    let files = v["files"].as_array().ok_or("the file guard named no files")?;
    asked
        .iter()
        .map(|p| {
            files
                .iter()
                .find(|f| f["path"].as_str() == Some(&*p.to_string_lossy()))
                .and_then(Verdict::from_json)
                .ok_or_else(|| format!("the file guard said nothing about {}", p.display()))
        })
        .collect()
}

/// Checks files with the guard (exit 0: none refused, 1: some refused, both with the JSON; anything else: an error).
pub fn scan(prog: &str, files: &[PathBuf]) -> Result<Vec<Verdict>, String> {
    if files.is_empty() {
        return Ok(vec![]);
    }
    let out = Command::new(prog)
        .args(["scan", "--json"])
        .args(files)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("the file guard did not start: {e}"))?;
    match out.status.code() {
        Some(0 | 1) => parse_scan(&String::from_utf8_lossy(&out.stdout), files),
        _ => Err(format!("the file guard failed: {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}

/// What the guard unpacked from an archive.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Unpacked {
    /// The name in the archive, where it was written, and the guard's verdict.
    pub files: Vec<(String, PathBuf, Verdict)>,
    /// Entries left out, and why.
    pub skipped: Vec<(String, String)>,
    /// The whole archive refused, and why.
    pub refused: Option<String>,
}

/// `extract --json`'s answer. Every file it wrote must be inside `outdir`.
pub fn parse_extract(text: &str, outdir: &Path) -> Result<Unpacked, String> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| "the file guard's answer is not JSON".to_string())?;
    let mut u = Unpacked { refused: v["refused"].as_str().map(str::to_string), ..Default::default() };
    for f in v["files"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let name = f["name"].as_str().unwrap_or("").to_string();
        let path = PathBuf::from(f["path"].as_str().ok_or("a file without a path")?);
        if !path.starts_with(outdir) || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(format!("the file guard wrote {} outside its folder", path.display()));
        }
        let verdict = Verdict::from_json(f).ok_or("a file without a verdict")?;
        u.files.push((name, path, verdict));
    }
    for s in v["skipped"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        u.skipped.push((s["name"].as_str().unwrap_or("").to_string(), s["reason"].as_str().unwrap_or("").to_string()));
    }
    Ok(u)
}

/// Unpacks the font files of an archive with the guard into `outdir` (which must not exist yet).
pub fn extract(prog: &str, archive: &Path, outdir: &Path) -> Result<Unpacked, String> {
    let out = Command::new(prog)
        .args(["extract", "--json", "--only", &KINDS.join(",")])
        .arg(archive)
        .arg(outdir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("the file guard did not start: {e}"))?;
    match out.status.code() {
        Some(0 | 1) => parse_extract(&String::from_utf8_lossy(&out.stdout), outdir),
        _ => Err(format!("the file guard failed: {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}

// ------------------------------------------------------------------------------------------------ is it a font?

/// The family name of a TrueType or OpenType font, or the first of a collection, read with skrifa; Err when it is not a font
/// with a family name, a character map and outlines.
pub fn check_font(bytes: &[u8]) -> Result<String, String> {
    use skrifa::raw::FileRef;
    use skrifa::{string::StringId, MetadataProvider};
    if bytes.len() as u64 > MAX_BYTES {
        return Err("larger than 50 MB".into());
    }
    let file = FileRef::new(bytes).map_err(|_| "not a font file".to_string())?;
    let mut family = None;
    for font in file.fonts() {
        let font = font.map_err(|_| "a damaged font".to_string())?;
        let name = [StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME]
            .into_iter()
            .find_map(|id| font.localized_strings(id).english_or_first().map(|s| s.to_string()).filter(|s| !s.trim().is_empty()))
            .ok_or("a font without a name")?;
        let charmap = font.charmap();
        let outlines = font.outline_glyphs();
        let drawable = charmap.mappings().take(64).any(|(_, gid)| outlines.get(gid).is_some());
        if !drawable {
            return Err("a font with no letters".into());
        }
        family.get_or_insert(name);
    }
    family.ok_or_else(|| "a collection with no fonts".into())
}

/// A WOFF2 file's header: the signature, its own length, a known flavour and a sane number of tables.
pub fn check_woff2_header(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err("larger than 50 MB".into());
    }
    if bytes.len() < 48 || &bytes[0..4] != b"wOF2" {
        return Err("not a WOFF2 file".into());
    }
    let u32_at = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let flavor = u32_at(4);
    if ![0x0001_0000, u32::from_be_bytes(*b"OTTO"), u32::from_be_bytes(*b"true"), u32::from_be_bytes(*b"ttcf")].contains(&flavor) {
        return Err("a WOFF2 file of an unknown kind".into());
    }
    if u32_at(8) as usize != bytes.len() {
        return Err("a damaged WOFF2 file (its length is wrong)".into());
    }
    let tables = u16::from_be_bytes([bytes[12], bytes[13]]);
    if tables == 0 || tables > 512 {
        return Err("a damaged WOFF2 file".into());
    }
    Ok(())
}

/// What fontconfig makes of a file (FreeType, in a process of its own): its first family name.
fn fc_scan_family(path: &Path) -> Option<String> {
    let out = Command::new("fc-scan").args(["--format", "%{family[0]}\n"]).arg(path).stdin(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

/// The font's family, or why it is not taken.
pub fn validate(path: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a plain file".into());
    }
    if meta.len() > MAX_BYTES {
        return Err("larger than 50 MB".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    match ext_of(path).as_str() {
        "ttf" | "otf" | "ttc" => check_font(&bytes),
        "woff2" => {
            check_woff2_header(&bytes)?;
            fc_scan_family(path).ok_or_else(|| "fontconfig cannot read it as a font".into())
        }
        _ => Err("not a font file (.ttf, .otf, .ttc, .woff2 or a .zip of them)".into()),
    }
}

// ------------------------------------------------------------------------------------------------ adding

/// One file on its way in.
#[derive(Clone, Debug, PartialEq)]
pub struct Incoming {
    /// What it is called (the file's name, or its name in the archive).
    pub name: String,
    /// The private copy that was checked, and that is installed.
    pub staged: Option<PathBuf>,
    pub family: Option<String>,
    pub verdict: Option<Verdict>,
    /// Why it cannot be installed (refused, not a font, too large, ...).
    pub problem: Option<String>,
}

impl Incoming {
    fn refused(name: String, why: String) -> Incoming {
        Incoming { name, staged: None, family: None, verdict: None, problem: Some(why) }
    }

    /// Installed without asking: checked as a font, and the guard had nothing to say.
    pub fn clean(&self) -> bool {
        self.problem.is_none() && self.verdict.as_ref().is_some_and(|v| v.level == Level::Ok)
    }

    /// Installed only when chosen: the guard warned.
    pub fn warned(&self) -> bool {
        self.problem.is_none() && self.verdict.as_ref().is_some_and(|v| v.level == Level::Warn)
    }
}

fn shown_name(p: &Path) -> String {
    yggdrasil_ui::lookalike::visible(&p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

/// Removes what earlier imports left in the staging folder (files warned about and never installed, a crash halfway).
pub fn clean_staging() {
    let _ = std::fs::remove_dir_all(yggdrasil_conf::dirs::cache().join("font-import"));
}

/// A private folder for one import: ~/.cache/yggdrasil/font-import/<new>, 0700.
pub fn staging_dir() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let base = yggdrasil_conf::dirs::cache().join("font-import");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&base)?;
    for n in 0..1000u32 {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let d = base.join(format!("{}-{t}-{n}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no staging folder"))
}

/// A name not yet taken in `stage`: `n.ext`, else `n-1.ext`, ...
fn fresh(stage: &Path, n: usize, ext: &str) -> PathBuf {
    let dot = if ext.is_empty() { String::new() } else { format!(".{ext}") };
    (0..)
        .map(|k| stage.join(if k == 0 { format!("{n}{dot}") } else { format!("{n}-{k}{dot}") }))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .unwrap_or_else(|| stage.join(format!("{n}{dot}")))
}

/// Copies a dropped or chosen file into `stage` (at most MAX_BYTES read, whatever it says its size is).
fn copy_in(src: &Path, stage: &Path, n: usize) -> Result<PathBuf, String> {
    use std::io::{Read, Write};
    let f = std::fs::File::open(src).map_err(|e| e.to_string())?;
    if !f.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a plain file".into());
    }
    let mut bytes = vec![];
    f.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("larger than 50 MB".into());
    }
    let to = fresh(stage, n, &ext_of(src));
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&to).map_err(|e| e.to_string())?;
    out.write_all(&bytes).map_err(|e| e.to_string())?;
    Ok(to)
}

/// Brings `sources` (files and .zip archives) into a staging folder and checks each: the guard, then the font check. Nothing is
/// installed here. Without a guard that can check, nothing is taken at all.
pub fn bring_in(prog: &str, sources: &[PathBuf], stage: &Path) -> Result<Vec<Incoming>, String> {
    guard_ready(prog).map_err(|m| match m {
        GuardMissing::Absent => "Adding fonts needs Heimdall's file guard (heimdall-fileguard), which checks every file first.".to_string(),
        GuardMissing::TooOld => {
            "Adding fonts needs a newer Heimdall privacy package: its file guard cannot check fonts yet. Nothing was added.".to_string()
        }
    })?;
    let mut out = vec![];
    let mut plain: Vec<(String, PathBuf)> = vec![];
    for (n, src) in sources.iter().enumerate() {
        let name = shown_name(src);
        let ext = ext_of(src);
        if ext == "zip" {
            let dir = fresh(stage, n, "zip-files");
            match extract(prog, src, &dir) {
                Err(e) => out.push(Incoming::refused(name, e)),
                Ok(u) => {
                    if let Some(why) = u.refused {
                        out.push(Incoming::refused(name, why));
                        continue;
                    }
                    for (inner, path, verdict) in u.files {
                        let shown = format!("{name}: {}", yggdrasil_ui::lookalike::visible(&inner));
                        out.push(checked(shown, path, verdict));
                    }
                    for (inner, why) in u.skipped {
                        out.push(Incoming::refused(format!("{name}: {}", yggdrasil_ui::lookalike::visible(&inner)), why));
                    }
                }
            }
        } else if KINDS.contains(&ext.as_str()) {
            match copy_in(src, stage, n) {
                Ok(p) => plain.push((name, p)),
                Err(e) => out.push(Incoming::refused(name, e)),
            }
        } else {
            out.push(Incoming::refused(name, "not a font file (.ttf, .otf, .ttc, .woff2 or a .zip of them)".into()));
        }
    }
    let paths: Vec<PathBuf> = plain.iter().map(|(_, p)| p.clone()).collect();
    match scan(prog, &paths) {
        Ok(verdicts) => {
            for ((name, p), v) in plain.into_iter().zip(verdicts) {
                out.push(checked(name, p, v));
            }
        }
        Err(e) => out.extend(plain.into_iter().map(|(name, _)| Incoming::refused(name, e.clone()))),
    }
    Ok(out)
}

fn checked(name: String, path: PathBuf, verdict: Verdict) -> Incoming {
    if verdict.level == Level::Refused {
        let why = verdict.words();
        return Incoming {
            name,
            staged: None,
            family: None,
            verdict: Some(verdict),
            problem: Some(if why.is_empty() { "refused".into() } else { why }),
        };
    }
    match validate(&path) {
        Ok(family) => Incoming { name, staged: Some(path), family: Some(family), verdict: Some(verdict), problem: None },
        Err(e) => Incoming { name, staged: None, family: None, verdict: Some(verdict), problem: Some(e) },
    }
}

/// A file name for the fonts folder: the original's letters, digits and . _ - (others become _), its kind kept.
pub fn safe_name(name: &str, ext: &str) -> String {
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut s: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    s = s.trim_start_matches('.').chars().take(100).collect();
    if s.is_empty() {
        s = "font".into();
    }
    format!("{s}.{ext}")
}

/// Moves a checked file into `dir` (made when missing) under a safe name; a file of that name with the same bytes is already
/// installed, another gets a number. Returns where it went.
pub fn install(inc: &Incoming, dir: &Path) -> Result<PathBuf, String> {
    let staged = inc.staged.as_ref().filter(|_| inc.problem.is_none()).ok_or("not checked")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let ext = ext_of(staged);
    let base = inc.name.rsplit(": ").next().unwrap_or(&inc.name);
    let bytes = std::fs::read(staged).map_err(|e| e.to_string())?;
    for k in 0..100 {
        let name = safe_name(base, &ext);
        let name = if k == 0 { name } else { name.replacen(&format!(".{ext}"), &format!("-{k}.{ext}"), 1) };
        let to = dir.join(&name);
        match std::fs::symlink_metadata(&to) {
            Ok(m) if m.is_file() && std::fs::read(&to).is_ok_and(|b| b == bytes) => return Ok(to),
            Ok(_) => continue,
            Err(_) => {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o644)).map_err(|e| e.to_string())?;
                // the same file system (both under the home folder) renames; another one copies
                if std::fs::rename(staged, &to).is_err() {
                    let tmp = dir.join(format!(".{name}.part"));
                    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
                    std::fs::rename(&tmp, &to).map_err(|e| e.to_string())?;
                }
                return Ok(to);
            }
        }
    }
    Err("too many fonts of that name".into())
}

/// Removes a family you added: only files inside your fonts folder (none through a link), then fontconfig is told.
pub fn remove(f: &Family, dir: &Path) -> Result<(), String> {
    if !f.yours {
        return Err("only fonts you added can be removed here".into());
    }
    let dir = std::fs::canonicalize(dir).map_err(|e| e.to_string())?;
    for file in &f.files {
        let real = std::fs::canonicalize(file).map_err(|e| e.to_string())?;
        if !real.starts_with(&dir) || std::fs::symlink_metadata(file).map(|m| !m.is_file()).unwrap_or(true) {
            return Err(format!("{} is not one of your font files", file.display()));
        }
    }
    for file in &f.files {
        std::fs::remove_file(file).map_err(|e| e.to_string())?;
    }
    refresh_cache(&dir);
    Ok(())
}

/// fontconfig's cache for your fonts folder, so programs see the change.
pub fn refresh_cache(dir: &Path) {
    let _ = Command::new("fc-cache").arg("-f").arg(dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

// ------------------------------------------------------------------------------------------------ a stale font cache

/// The families `fc-list : family` lists (a font's names on one line, comma-separated; an escaped comma is part of a name).
pub fn parse_families(text: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for line in text.lines() {
        let mut name = String::new();
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => name.extend(chars.next()),
                ',' => {
                    out.insert(std::mem::take(&mut name).trim().to_string());
                }
                c => name.push(c),
            }
        }
        out.insert(name.trim().to_string());
    }
    out.remove("");
    out
}

/// The families fontconfig lists now (None: fc-list did not answer).
pub fn listed(fc_list: &str) -> Option<std::collections::HashSet<String>> {
    let o = Command::new(fc_list).args([":", "family"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    o.status.success().then(|| parse_families(&String::from_utf8_lossy(&o.stdout)))
}

/// The fonts to choose from whose file is where its package puts it, but which fontconfig does not list.
pub fn unlisted(listed: &std::collections::HashSet<String>, exists: impl Fn(&Path) -> bool) -> Vec<&'static yggdrasil_conf::fonts::Choice> {
    yggdrasil_conf::fonts::SANS
        .iter()
        .chain(yggdrasil_conf::fonts::MONO)
        .filter(|c| !listed.contains(c.family) && exists(Path::new(c.file)))
        .collect()
}

/// What `heal` found.
#[derive(Debug, PartialEq)]
pub enum Healed {
    /// Every font whose file is here is listed (or fontconfig is not there to ask).
    Fine,
    /// The cache was made again; these are still not listed.
    Refreshed(Vec<&'static yggdrasil_conf::fonts::Choice>),
}

/// A stale cache: fontconfig keeps a folder's cache while the folder's modification time, in whole seconds, is the same, so
/// files added in the same second as the cache was made stay unlisted (two pacman transactions in one second did it:
/// APPS_SETTINGS.md). When a font's files are on disk but fontconfig does not list it, your own cache is made again
/// (`fc-cache -f`, no password: it goes to ~/.cache/fontconfig, which fontconfig prefers when it is newer) and the list read
/// again. Run once per run of Settings, on a thread of its own (about two seconds on a desktop with Noto's 600 files).
pub fn heal(fc_list: &str, fc_cache: &str, exists: impl Fn(&Path) -> bool) -> Healed {
    let Some(before) = listed(fc_list) else { return Healed::Fine };
    if unlisted(&before, &exists).is_empty() {
        return Healed::Fine;
    }
    let _ = Command::new(fc_cache).arg("-f").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let after = listed(fc_list).unwrap_or_default();
    Healed::Refreshed(unlisted(&after, &exists))
}

// ------------------------------------------------------------------------------------------------ the sample line

/// The sample drawn in a font: a coverage mask (one byte per pixel) `width` x `height`, or None when the font cannot be read or
/// has none of the letters.
pub fn render_sample(bytes: &[u8], index: u32, text: &str, px: f32) -> Option<(usize, usize, Vec<u8>)> {
    use skrifa::instance::{LocationRef, Size};
    use skrifa::outline::{DrawSettings, OutlinePen};
    use skrifa::MetadataProvider;
    struct Pen(tiny_skia::PathBuilder, f32, f32);
    impl OutlinePen for Pen {
        fn move_to(&mut self, x: f32, y: f32) {
            self.0.move_to(self.1 + x, self.2 - y);
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.0.line_to(self.1 + x, self.2 - y);
        }
        fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
            self.0.quad_to(self.1 + cx, self.2 - cy, self.1 + x, self.2 - y);
        }
        fn curve_to(&mut self, a: f32, b: f32, c: f32, d: f32, x: f32, y: f32) {
            self.0.cubic_to(self.1 + a, self.2 - b, self.1 + c, self.2 - d, self.1 + x, self.2 - y);
        }
        fn close(&mut self) {
            self.0.close();
        }
    }
    let font = skrifa::FontRef::from_index(bytes, index).ok()?;
    let location = font.axes().location([("wght", 400.0)]);
    let loc = LocationRef::from(&location);
    let size = Size::new(px);
    let metrics = font.metrics(size, loc);
    let glyphs = font.glyph_metrics(size, loc);
    let (charmap, outlines) = (font.charmap(), font.outline_glyphs());
    let ascent = metrics.ascent.ceil().max(1.0);
    let height = (ascent - metrics.descent.floor()).max(1.0) as usize + 2;
    let ids: Vec<_> = text.chars().filter_map(|c| charmap.map(c)).collect();
    if ids.is_empty() {
        return None;
    }
    let width = (ids.iter().map(|g| glyphs.advance_width(*g).unwrap_or(0.0)).sum::<f32>().ceil() as usize + 2).min(4096);
    let mut pix = tiny_skia::Pixmap::new(width as u32, height as u32)?;
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    let mut x = 1.0;
    for g in ids {
        if let Some(o) = outlines.get(g) {
            let mut pen = Pen(tiny_skia::PathBuilder::new(), x, ascent + 1.0);
            if o.draw(DrawSettings::unhinted(size, loc), &mut pen).is_ok() {
                if let Some(path) = pen.0.finish() {
                    pix.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
                }
            }
        }
        x += glyphs.advance_width(g).unwrap_or(0.0);
        if x > width as f32 {
            break;
        }
    }
    // no ink: the font has none of these letters drawn (an emoji or CJK font maps the space, little else)
    let mask: Vec<u8> = pix.pixels().iter().map(|p| p.alpha()).collect();
    mask.iter().any(|&a| a > 0).then_some((width, height, mask))
}

#[cfg(test)]
mod tests;
