//! yggdrasil-game-fix: per-game fixes for Steam games (Settings > Games > Game fixes). The Python script's behaviour
//! (doc/game-fix.txt is its help): the fixes are data (game-fixes.json), a game's settings files are edited line by line
//! (kv "Key = value" or ini), everything else in them kept byte for byte, a backup or a "made here" mark first, undo puts the
//! files back. Nothing here needs administrator rights.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use pycompat::json::{self, py_str, Map, Value};

use crate::py::{self, Crash, R};

pub const DOC: &str = include_str!("doc/game-fix.txt");
const BACKUP: &str = ".yggdrasil-backup";
const CREATED: &str = ".yggdrasil-created";

fn catalogue_path() -> String {
    py::env_or("YGGDRASIL_GAME_FIXES", "/usr/share/yggdrasil/game-fixes.json")
}

fn isdir(p: &str) -> bool {
    Path::new(p).is_dir()
}

fn isfile(p: &str) -> bool {
    Path::new(p).is_file()
}

fn exists(p: &str) -> bool {
    Path::new(p).exists()
}

/// A text read with errors="replace" (each bad byte sequence becomes U+FFFD), universal newlines.
fn read_replace(path: &str) -> Option<String> {
    fs::read(path).ok().map(|b| py::universal(&String::from_utf8_lossy(&b)))
}

// --- Steam -------------------------------------------------------------------------------------------------------------

pub fn steam_roots(home: &str) -> Vec<String> {
    [py::join(home, ".local/share/Steam"), py::join(home, ".steam/steam")].into_iter().filter(|r| isdir(&py::join(r, "steamapps"))).collect()
}

fn re(pattern: &str) -> regex::Regex {
    regex::Regex::new(&pattern.replace(r"\s", pycompat::PY_SPACE_CLASS)).unwrap()
}

pub fn libraries(root: &str) -> Vec<String> {
    let mut libs = vec![root.to_string()];
    if let Some(text) = read_replace(&py::join(root, "steamapps/libraryfolders.vdf")) {
        for m in re(r#""path"\s+"([^"]+)""#).captures_iter(&text) {
            let p = m[1].replace("\\\\", "\\");
            if !libs.contains(&p) {
                libs.push(p);
            }
        }
    }
    libs
}

/// A game Steam has installed: its steamapps folder, its install folder's name, its name.
#[derive(Debug, Clone)]
pub struct Game {
    pub steamapps: String,
    pub installdir: String,
    pub name: String,
}

/// Every installed game by its appid.
pub fn installed(home: &str) -> R<BTreeMap<String, Game>> {
    let mut out = BTreeMap::new();
    let mut seen: Vec<std::path::PathBuf> = vec![];
    for root in steam_roots(home) {
        for lib in libraries(&root) {
            let sa = py::join(&lib, "steamapps");
            let real = realpath(&sa);
            if seen.contains(&real) || !isdir(&sa) {
                continue;
            }
            seen.push(real);
            let rd = fs::read_dir(&sa).map_err(|e| Crash(py::oserror(&e, Some(&sa))))?;
            for e in rd {
                let e = e?;
                let Some(n) = e.file_name().to_str().map(str::to_string) else { continue };
                if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) {
                    continue;
                }
                let Some(text) = read_replace(&py::join(&sa, &n)) else { continue };
                let field = |k: &str| -> String {
                    re(&format!(r#""{}"\s+"([^"]*)""#, regex::escape(k))).captures(&text).map(|c| c[1].to_string()).unwrap_or_default()
                };
                let appid = field("appid");
                if !appid.is_empty() {
                    out.insert(appid, Game { steamapps: sa.clone(), installdir: field("installdir"), name: field("name") });
                }
            }
        }
    }
    Ok(out)
}

/// os.path.realpath (a path that does not exist is made absolute and cleaned as far as it goes).
fn realpath(p: &str) -> std::path::PathBuf {
    match fs::canonicalize(p) {
        Ok(r) => r,
        Err(_) => {
            // the longest part that exists, resolved, and the rest as it is
            let path = Path::new(p);
            let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
            let mut base = abs.clone();
            let mut rest = vec![];
            while !base.exists() {
                match (base.file_name().map(|f| f.to_os_string()), base.parent().map(Path::to_path_buf)) {
                    (Some(f), Some(parent)) => {
                        rest.push(f);
                        base = parent;
                    }
                    _ => break,
                }
            }
            let mut out = fs::canonicalize(&base).unwrap_or(base);
            for f in rest.into_iter().rev() {
                out.push(f);
            }
            std::path::PathBuf::from(pycompat::normpath(&out.to_string_lossy()))
        }
    }
}

/// The places a fix's paths can name.
pub fn places(steamapps: &str, installdir: &str, appid: &str) -> Vec<(String, Value)> {
    let prefix = py::join(&py::join(&py::join(&py::join(steamapps, "compatdata"), appid), "pfx"), "drive_c");
    let user = py::join(&py::join(&prefix, "users"), "steamuser");
    vec![
        ("game".into(), py::join(&py::join(steamapps, "common"), installdir).into()),
        ("prefix".into(), prefix.clone().into()),
        ("userprofile".into(), user.clone().into()),
        ("localappdata".into(), py::join(&py::join(&user, "AppData"), "Local").into()),
        ("documents".into(), py::join(&user, "Documents").into()),
    ]
}

// --- the screen --------------------------------------------------------------------------------------------------------

fn run_timeout(argv: &[&str], secs: u64) -> Option<String> {
    use std::io::Read;
    let mut child = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let mut so = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut b = vec![];
        let _ = so.read_to_end(&mut b);
        b
    });
    let mut se = child.stderr.take()?;
    std::thread::spawn(move || {
        let mut b = vec![];
        let _ = se.read_to_end(&mut b);
    });
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < std::time::Duration::from_secs(secs) => std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = reader.join().ok()?;
    // text=True: strict UTF-8 (the script's except caught a decoding error too)
    String::from_utf8(out).ok().map(|t| py::universal(&t))
}

/// int(x) of a JSON value (None where int() raised).
fn int_of(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(*b as i128),
        Value::Number(n) => n.as_i64().map(|i| i as i128).or_else(|| n.as_u64().map(|u| u as i128)).or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i128)),
        Value::String(s) => pycompat::int(s),
        _ => None,
    }
}

/// (width, height) of the largest monitor: YGGDRASIL_SCREEN=WxH, else hyprctl, else xrandr.
pub fn screen() -> Option<(i128, i128)> {
    let forced = py::env_or("YGGDRASIL_SCREEN", "");
    let d = r"(\d+)";
    if let Some(c) = regex::Regex::new(&format!("^{d}x{d}$")).unwrap().captures(&forced) {
        if !forced.ends_with('\n') {
            return Some((pycompat::int(&c[1])?, pycompat::int(&c[2])?));
        }
    }
    let from_hyprctl = || -> Option<(i128, i128)> {
        let text = run_timeout(&["hyprctl", "-j", "monitors"], 5)?;
        let mons = json::loads(&text).ok()?;
        let mons = mons.as_array()?;
        let area = |m: &Value| -> Option<f64> {
            let w = m.get("width")?;
            let h = m.get("height")?;
            Some(num(w)? * num(h)?)
        };
        let mut best: Option<(&Value, f64)> = None;
        for m in mons {
            // max(): the first of the largest; a monitor without a size stopped it (the script's except caught that)
            let a = area(m)?;
            if best.map(|(_, b)| a > b).unwrap_or(true) {
                best = Some((m, a));
            }
        }
        let (b, _) = best?;
        Some((int_of(&b["width"])?, int_of(&b["height"])?))
    };
    if let Some(s) = from_hyprctl() {
        return Some(s);
    }
    let out = run_timeout(&["xrandr", "--current"], 5)?;
    let mut best: Option<(i128, i128)> = None;
    for c in regex::Regex::new(r" connected (?:primary )?(\d+)x(\d+)\+").unwrap().captures_iter(&out) {
        let s = (pycompat::int(&c[1])?, pycompat::int(&c[2])?);
        if best.map(|b| s.0 * s.1 > b.0 * b.1).unwrap_or(true) {
            best = Some(s);
        }
    }
    best
}

/// A number for the comparison max() made (width * height); text and other values made it fail.
fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(*b as i32 as f64),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

// --- files -------------------------------------------------------------------------------------------------------------

/// re.sub(r"\{(\w+)\}", ...): each {name} whose name is in `values` replaced by str() of its value.
pub fn fill(text: &str, values: &[(String, Value)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let name_len: usize = after.chars().take_while(|c| pycompat::isword(*c)).map(char::len_utf8).sum();
        if name_len > 0 && after[name_len..].starts_with('}') {
            let name = &after[..name_len];
            match values.iter().find(|(k, _)| k == name) {
                Some((_, v)) => out.push_str(&py_str(v)),
                None => {
                    out.push('{');
                    out.push_str(name);
                    out.push('}');
                }
            }
            rest = &after[name_len + 1..];
        } else {
            out.push('{');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Bytes that are not UTF-8 are kept through an edit as the script kept them (errors="surrogateescape"): each becomes a
/// character of a private-use range while the file is edited and the same byte again when it is written.
const ESCAPE_BASE: u32 = 0x10FD00;

fn decode_escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                break;
            }
            Err(e) => {
                let (good, bad) = rest.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(good).unwrap());
                let n = e.error_len().unwrap_or(bad.len());
                for &b in &bad[..n] {
                    out.push(char::from_u32(ESCAPE_BASE + b as u32).unwrap());
                }
                rest = &bad[n..];
            }
        }
    }
    out
}

fn encode_escape(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        let cp = c as u32;
        if (ESCAPE_BASE + 0x80..=ESCAPE_BASE + 0xFF).contains(&cp) {
            out.push((cp - ESCAPE_BASE) as u8);
        } else {
            let mut b = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
        }
    }
    out
}

/// The file's lines (without their ends) and whether it used \r\n.
pub fn read_lines(path: &str) -> R<(Vec<String>, bool)> {
    let bytes = fs::read(path).map_err(|e| Crash(py::oserror(&e, Some(path))))?;
    let text = decode_escape(&bytes);
    let crlf = text.contains("\r\n");
    Ok((text.replace("\r\n", "\n").split('\n').map(str::to_string).collect(), crlf))
}

pub fn write_lines(path: &str, lines: &[String], crlf: bool) -> R<()> {
    let mut text = lines.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if crlf {
        text = text.replace('\n', "\r\n");
    }
    fs::write(path, encode_escape(&text)).map_err(|e| Crash(py::oserror(&e, Some(path))))
}

/// line.partition("="): (before, whether there was one, after)
fn partition(l: &str) -> (&str, bool, &str) {
    match l.split_once('=') {
        Some((k, v)) => (k, true, v),
        None => (l, false, ""),
    }
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

pub fn kv_get(lines: &[String], key: &str) -> Option<String> {
    for l in lines {
        let (k, sep, v) = partition(l);
        if sep && pycompat::strip(k) == key {
            return Some(pycompat::strip(v).to_string());
        }
    }
    None
}

pub fn kv_set(mut lines: Vec<String>, key: &str, value: &str) -> Vec<String> {
    for l in lines.iter_mut() {
        let (k, sep, _) = partition(l);
        if sep && pycompat::strip(k) == key {
            *l = format!("{key} = {value}");
            return lines;
        }
    }
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.push(format!("{key} = {value}"));
    lines
}

/// (index of the [section] line, index after its last line). Err: the section is None and a header had to be compared with
/// it (the script stopped there).
pub fn ini_find(lines: &[String], section: Option<&str>) -> R<Option<(usize, usize)>> {
    let mut start = None;
    for (i, l) in lines.iter().enumerate() {
        let s = pycompat::strip(l);
        if s.starts_with('[') && s.ends_with(']') {
            if let Some(st) = start {
                return Ok(Some((st, i)));
            }
            let inner: String = {
                let cs: Vec<char> = s.chars().collect();
                if cs.len() >= 2 { cs[1..cs.len() - 1].iter().collect() } else { String::new() }
            };
            let Some(section) = section else { return Err(Crash("AttributeError: 'NoneType' object has no attribute 'lower'".into())) };
            if lower(pycompat::strip(&inner)) == lower(section) {
                start = Some(i);
            }
        }
    }
    Ok(start.map(|s| (s, lines.len())))
}

pub fn ini_get(lines: &[String], section: Option<&str>, key: &str) -> R<Option<String>> {
    let Some((start, end)) = ini_find(lines, section)? else { return Ok(None) };
    for l in &lines[start + 1..end] {
        let (k, sep, v) = partition(l);
        if sep && lower(pycompat::strip(k)) == lower(key) {
            return Ok(Some(pycompat::strip(v).to_string()));
        }
    }
    Ok(None)
}

pub fn ini_set(mut lines: Vec<String>, section: Option<&str>, key: &str, value: &str) -> R<Vec<String>> {
    let Some((start, end)) = ini_find(&lines, section)? else {
        while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        let sec = section.map(str::to_string).unwrap_or_else(|| "None".into());
        lines.extend([String::new(), format!("[{sec}]"), format!("{key}={value}")]);
        return Ok(lines);
    };
    for i in start + 1..end {
        let (k, sep, _) = partition(&lines[i]);
        if sep && lower(pycompat::strip(k)) == lower(key) {
            lines[i] = format!("{key}={value}");
            return Ok(lines);
        }
    }
    // after the section's last non-empty line
    let mut at = end;
    while at > start + 1 && pycompat::strip(&lines[at - 1]).is_empty() {
        at -= 1;
    }
    lines.insert(at, format!("{key}={value}"));
    Ok(lines)
}

fn type_error(what: &str) -> Crash {
    Crash(format!("TypeError/KeyError: {what}"))
}

fn get<'a>(v: &'a Value, key: &str) -> R<&'a Value> {
    v.get(key).ok_or_else(|| Crash(format!("KeyError: {}", pycompat::repr(key))))
}

fn as_str<'a>(v: &'a Value, what: &str) -> R<&'a str> {
    v.as_str().ok_or_else(|| type_error(what))
}

/// [(section or None, key, value)] the file should hold.
pub fn wanted(spec: &Value, values: &[(String, Value)]) -> R<Vec<(Option<String>, String, String)>> {
    let mut out = vec![];
    let set = match spec.get("set") {
        None => return Ok(out),
        Some(Value::Object(m)) => m,
        Some(_) => return Err(type_error("'set' has no items()")),
    };
    for (k, v) in set {
        if let Value::Object(inner) = v {
            for (kk, vv) in inner {
                out.push((Some(k.clone()), kk.clone(), fill(&py_str(vv), values)));
            }
        } else {
            out.push((None, k.clone(), fill(&py_str(v), values)));
        }
    }
    Ok(out)
}

fn is_ini(spec: &Value) -> R<bool> {
    Ok(get(spec, "format")? == "ini")
}

/// 'applied', 'not applied' or 'missing' (no file yet).
pub fn file_state(path: &str, spec: &Value, values: &[(String, Value)]) -> R<&'static str> {
    if !isfile(path) {
        return Ok("missing");
    }
    let (lines, _) = read_lines(path)?;
    for (section, key, value) in wanted(spec, values)? {
        let now = if is_ini(spec)? { ini_get(&lines, section.as_deref(), &key)? } else { kv_get(&lines, &key) };
        match now {
            Some(n) if lower(pycompat::strip(&n)) == lower(&value) => {}
            _ => return Ok("not applied"),
        }
    }
    Ok("applied")
}

// --- fixes -------------------------------------------------------------------------------------------------------------

/// The catalogue's "games". Err(message) is what main prints (the script caught OSError, ValueError and KeyError there).
pub fn load_catalogue(path: &str) -> Result<R<Vec<Value>>, String> {
    let bytes = fs::read(path).map_err(|e| py::oserror(&e, Some(path)))?;
    let text = String::from_utf8(bytes).map_err(|e| format!("'utf-8' codec can't decode: {e}"))?;
    let v = json::loads(&text)?;
    match v {
        Value::Object(m) => match m.get("games") {
            Some(Value::Array(a)) => Ok(Ok(a.clone())),
            // iterating something else: the script stopped later, with a TypeError
            Some(_) => Ok(Err(type_error("games is not a list"))),
            None => Err("'games'".into()),
        },
        _ => Ok(Err(type_error("the catalogue is not an object"))),
    }
}

fn files_of(fix: &Value) -> R<&Vec<Value>> {
    get(fix, "files")?.as_array().ok_or_else(|| type_error("files is not a list"))
}

fn appid_of(fix: &Value) -> R<&Value> {
    get(fix, "appid")
}

fn context(fix: &Value, games: &BTreeMap<String, Game>) -> R<Vec<(String, Value)>> {
    let appid = as_str(appid_of(fix)?, "appid")?;
    let g = games.get(appid).ok_or_else(|| Crash(format!("KeyError: {}", pycompat::repr(appid))))?;
    let mut values = places(&g.steamapps, &g.installdir, appid);
    if let Some((w, h)) = screen() {
        values.push(("width".into(), Value::from(w as i64)));
        values.push(("height".into(), Value::from(h as i64)));
    }
    Ok(values)
}

fn value<'a>(values: &'a [(String, Value)], k: &str) -> Option<&'a Value> {
    values.iter().find(|(n, _)| n == k).map(|(_, v)| v)
}

/// 'applied', 'not applied', 'start once' (Proton has not made the game's folders yet) or 'no screen'.
pub fn status(fix: &Value, games: &BTreeMap<String, Game>) -> R<(&'static str, Vec<(String, Value)>)> {
    let values = context(fix, games)?;
    if value(&values, "width").is_none() {
        return Ok(("no screen", values));
    }
    let mut in_prefix = false;
    for s in files_of(fix)? {
        let path = as_str(get(s, "path")?, "path")?;
        if ["prefix", "userprofile", "localappdata", "documents"].iter().any(|p| path.contains(&format!("{{{p}}}"))) {
            in_prefix = true;
            break;
        }
    }
    if in_prefix && !isdir(&py_str(value(&values, "prefix").unwrap())) {
        return Ok(("start once", values));
    }
    let mut all = true;
    for spec in files_of(fix)? {
        let path = fill(as_str(get(spec, "path")?, "path")?, &values);
        if file_state(&path, spec, &values)? != "applied" {
            all = false;
        }
    }
    Ok((if all { "applied" } else { "not applied" }, values))
}

/// What apply() and undo() could end with: a message (the script's SystemExit) or a crash.
pub enum Stop {
    Exit(String),
    Crash(Crash),
}

impl From<Crash> for Stop {
    fn from(c: Crash) -> Stop {
        Stop::Crash(c)
    }
}

pub fn apply(fix: &Value, games: &BTreeMap<String, Game>) -> Result<(), Stop> {
    let (state, values) = status(fix, games)?;
    match state {
        "no screen" => return Err(Stop::Exit("the screen size could not be read".into())),
        "start once" => return Err(Stop::Exit("start the game once first (Proton makes its folders then)".into())),
        _ => {}
    }
    for spec in files_of(fix)? {
        let path = fill(as_str(get(spec, "path")?, "path")?, &values);
        let (mut lines, crlf);
        if isfile(&path) {
            if !exists(&format!("{path}{BACKUP}")) && !exists(&format!("{path}{CREATED}")) {
                crate::bootscreen::copy2(&path, &format!("{path}{BACKUP}"))?;
            }
            (lines, crlf) = read_lines(&path)?;
        } else {
            let d = py::dirname(&path);
            fs::create_dir_all(&d).map_err(|e| Crash(py::oserror(&e, Some(&d))))?;
            let mut nl_crlf = spec.get("newline").map(|v| v == "crlf").unwrap_or(false);
            let template = spec.get("template").filter(|t| py_truthy(t));
            let from_template = match template {
                Some(t) => {
                    let tp = fill(as_str(t, "template")?, &values);
                    if isfile(&tp) { Some(tp) } else { None }
                }
                None => None,
            };
            match from_template {
                Some(tp) => (lines, nl_crlf) = read_lines(&tp)?,
                None => {
                    let tl = match spec.get("template_lines") {
                        None => vec![],
                        Some(Value::Array(a)) => a.iter().map(|l| as_str(l, "template line").map(|s| fill(s, &values))).collect::<R<Vec<_>>>()?,
                        Some(_) => return Err(type_error("template_lines").into()),
                    };
                    lines = tl;
                }
            }
            crlf = nl_crlf;
            let mark = format!("{path}{CREATED}");
            fs::write(&mark, "").map_err(|e| Crash(py::oserror(&e, Some(&mark))))?;
        }
        let ini = is_ini(spec)?;
        for (section, key, value) in wanted(spec, &values)? {
            lines = if ini { ini_set(lines, section.as_deref(), &key, &value)? } else { kv_set(lines, &key, &value) };
        }
        write_lines(&path, &lines, crlf)?;
    }
    Ok(())
}

fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn undo(fix: &Value, games: &BTreeMap<String, Game>) -> Result<(), Stop> {
    let values = context(fix, games)?;
    for spec in files_of(fix)? {
        let path = fill(as_str(get(spec, "path")?, "path")?, &values);
        let backup = format!("{path}{BACKUP}");
        let created = format!("{path}{CREATED}");
        if exists(&backup) {
            fs::rename(&backup, &path).map_err(|e| Crash(py::oserror(&e, Some(&backup))))?;
        } else if exists(&created) {
            for p in [&path, &created] {
                if exists(p) {
                    fs::remove_file(p).map_err(|e| Crash(py::oserror(&e, Some(p))))?;
                }
            }
        }
    }
    Ok(())
}

fn dispatch(argv: &[String]) -> R<i32> {
    let home = py::env_or("HOME", "");
    if argv.is_empty() || argv[0] == "-h" || argv[0] == "--help" {
        println!("{}", pycompat::strip(DOC));
        return Ok(if argv.is_empty() { 2 } else { 0 });
    }
    let cat = catalogue_path();
    let fixes = match load_catalogue(&cat) {
        Ok(Ok(f)) => f,
        Ok(Err(crash)) => return Err(crash),
        Err(msg) => {
            eprintln!("yggdrasil-game-fix: cannot read {cat}: {msg}");
            return Ok(1);
        }
    };
    let games = installed(&home)?;
    let mut mine = vec![];
    for f in &fixes {
        if let Value::String(id) = appid_of(f)? {
            if games.contains_key(id) {
                mine.push(f);
            }
        }
    }
    if argv[0] == "list" {
        let mut rows = vec![];
        for f in &mine {
            let (state, values) = status(f, &games)?;
            let mut r = Map::new();
            r.insert("appid".into(), appid_of(f)?.clone());
            r.insert("name".into(), get(f, "name")?.clone());
            r.insert("title".into(), get(f, "title")?.clone());
            r.insert("about".into(), f.get("about").cloned().unwrap_or_else(|| "".into()));
            r.insert("tip".into(), f.get("tip").cloned().unwrap_or_else(|| "".into()));
            r.insert("launch".into(), f.get("launch").cloned().unwrap_or_else(|| "".into()));
            r.insert("state".into(), state.into());
            let screen = match (value(&values, "width"), value(&values, "height")) {
                (Some(w), Some(h)) => format!("{}x{}", py_str(w), py_str(h)),
                _ => String::new(),
            };
            r.insert("screen".into(), screen.into());
            rows.push(Value::Object(r));
        }
        if argv.iter().any(|a| a == "--json") {
            println!("{}", json::dumps_indent(&Value::from(rows), 1));
        } else {
            for r in &rows {
                println!("{}  {}: {}  [{}]", py_str(&r["appid"]), py_str(&r["name"]), py_str(&r["title"]), py_str(&r["state"]));
            }
        }
        return Ok(0);
    }
    if (argv[0] == "apply" || argv[0] == "undo") && argv.len() > 1 {
        let Some(fix) = mine.iter().find(|f| f.get("appid").map(|a| a == argv[1].as_str()).unwrap_or(false)) else {
            eprintln!("yggdrasil-game-fix: no fix for an installed game {}", argv[1]);
            return Ok(2);
        };
        let r = if argv[0] == "apply" { apply(fix, &games) } else { undo(fix, &games) };
        match r {
            Ok(()) => {}
            Err(Stop::Exit(m)) => {
                eprintln!("yggdrasil-game-fix: {m}");
                return Ok(1);
            }
            Err(Stop::Crash(c)) => return Err(c),
        }
        println!("{}", status(fix, &games)?.0);
        return Ok(0);
    }
    eprintln!("{}", pycompat::strip(DOC));
    Ok(2)
}

/// The helper: `argv` without the program's name (as sys.argv[1:]).
pub fn main(argv: &[String]) -> i32 {
    match dispatch(argv) {
        Ok(c) => c,
        Err(c) => py::crashed("yggdrasil-game-fix", &c),
    }
}

#[cfg(test)]
mod tests;
