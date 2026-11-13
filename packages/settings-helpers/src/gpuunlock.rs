//! gpuunlock: the tensor-rate unlock for a tested GeForce card (Settings > Power > Graphics card, Developer options), run as
//! root through pkexec and by two services. The Python script's behaviour (doc/gpuunlock.txt is its help, unchanged): the
//! card read from sysfs and the driver's /proc files, the support table matched exactly, what blocks arming, the one-try boot
//! entry, the attempt markers across starts, the module's own result, the benchmark judged by rule 8, the heat watch and the
//! power hold. The plan and its safety rules: Labyrinth OS docs/GPU_UNLOCK.md.
//!
//! New here: `gpuunlock grub-entry`, which /etc/grub.d/41_gpu_unlock calls: it prints the one-try entry while an attempt is
//! armed, after the checks the shell fragment made with grep (one entry of ours, one kernel line ending in the option for one
//! PCI address, nothing that opens another entry), and prints nothing otherwise. It always ends with exit code 0.
//!
//! As root it runs no shell: every program by argv, with no standard input. Every file it writes in its state folder is
//! written whole (a new file renamed over the old one) and never through a link. What it reads from the outside is checked
//! as the script checked it; the few places where it is stricter are said in the README.

use std::cell::{Cell, RefCell};
use std::ffi::CString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pycompat::json::{self, Value};

type Map = json::Map<String, Value>;

use crate::py::{self, Crash, R};

pub const DOC: &str = include_str!("doc/gpuunlock.txt");

pub const STATE_DIR: &str = "/var/lib/yggdrasil/gpu-unlock";
pub const TABLE: &str = "/usr/share/yggdrasil/gpu-unlock/supported.json";
pub const GRUB_CFG: &str = "/boot/grub/grub.cfg";
pub const GRUB_DEFAULT: &str = "/etc/default/grub";
pub const ENTRY_ID: &str = "yggdrasil-gpu-unlock";
pub const ENTRY_TITLE: &str = "Labyrinth Linux, graphics card unlock (one try)";
pub const PARAM: &str = "turing_unlock.targets=";
const MODULE_FIRMWARE: &str = "/usr/lib/firmware/turing-unlock/610.43.03";
const MODULE_DROPIN: &str = "/etc/mkinitcpio.conf.d/zz-turing-unlock.conf";
const METHODS: &[&str] = &["turing-unlock-module"];
pub const SECURE_BOOT_VAR: &str = "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";
/// Ollama's list of loaded models (host:port and path)
const OLLAMA: &str = "127.0.0.1:11434";
const OLLAMA_PATH: &str = "/api/ps";
const UNITS: &[&str] = &["yggdrasil-gpu-unlock-boot.service", "yggdrasil-gpu-unlock-verify.service"];
const SERIES: &[&str] = &["open", "proprietary"];
const BOARD_MAKERS: &[(&str, &str)] = &[
    ("1043", "ASUS"),
    ("1458", "Gigabyte"),
    ("1462", "MSI"),
    ("10de", "NVIDIA"),
    ("3842", "EVGA"),
    ("19da", "Zotac"),
    ("196e", "PNY"),
    ("1569", "Palit"),
    ("10b0", "Gainward"),
    ("1b4c", "KFA2/Galax"),
    ("1acc", "Point of View"),
    ("7377", "Colorful"),
    ("1da2", "Sapphire"),
];
/// The benchmark (gpuunlock-bench.cu, installed next to this program): compiled with the CUDA toolkit's nvcc on first use,
/// kept per source.
const BENCH_SRC: &str = "/usr/lib/yggdrasil/gpuunlock-bench.cu";
const NVCC: &str = "/opt/cuda/bin/nvcc";
/// Rule 8: "Unlocked" needs correct results and the FP32-accumulate tensor rate at least this many times the locked baseline;
/// the FP16-accumulate rate (never halved) must stay within CONTROL_DRIFT of the baseline's, or the card was busy or held back.
const MIN_RATIO: f64 = 1.5;
const CONTROL_DRIFT: f64 = 0.15;
const HOT_C: i128 = 85;
const HOT_CAP_W: i64 = 200;
/// % of the card's processors a program may use on average and still count as idle (a desktop redrawing)
const BUSY_SM: i128 = 10;
/// The most read from Ollama's port (anyone may listen there when Ollama does not)
const HTTP_MAX: usize = 1 << 20;

// ---------------------------------------------------------------- errors

/// An OSError where the script caught one or quoted it: the error number and the file it named (e.filename).
#[derive(Debug, Clone, PartialEq)]
pub struct OsErr {
    pub errno: i32,
    pub filename: Option<String>,
}

impl OsErr {
    fn new(e: &io::Error, filename: Option<&str>) -> OsErr {
        OsErr { errno: e.raw_os_error().unwrap_or(libc::EIO), filename: filename.map(String::from) }
    }

    pub fn strerror(&self) -> String {
        py::strerror(self.errno)
    }

    /// Where the script let it through: a traceback (exit code 1).
    fn crash(&self) -> Crash {
        let name = self.filename.as_deref().map(|f| format!(": {}", pycompat::repr(f))).unwrap_or_default();
        Crash(format!("OSError: [Errno {}] {}{name}", self.errno, self.strerror()))
    }
}

impl From<OsErr> for Crash {
    fn from(e: OsErr) -> Crash {
        e.crash()
    }
}

/// How the outside programs are run: (exit code, standard output and error together, stripped), 127 when it is missing, 124
/// when it took longer than the seconds given; Err where starting it failed otherwise (the script raised an OSError there).
pub type Runner = Box<dyn Fn(&[String], u64) -> Result<(i32, String), OsErr>>;
/// The benchmark's JSON for a card (Ok(Err(why)): it gave none), for tests that put a result in its place.
pub type BenchHook = Box<dyn Fn(&str, bool) -> R<Result<Value, String>>>;
/// One heat sample (degrees, slowing itself down), for tests.
pub type SampleHook = Box<dyn Fn() -> R<(Option<i128>, bool)>>;

// ---------------------------------------------------------------- Python's ways with values

/// bool(v) of a value json.load made
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

fn opt_truthy(v: Option<&Value>) -> bool {
    v.map(truthy).unwrap_or(false)
}

/// type(v).__name__
fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// v.get(...) on a value that is not a dict: the script stopped there
fn no_get(v: &Value) -> Crash {
    Crash(format!("AttributeError: '{}' object has no attribute 'get'", type_name(v)))
}

/// d.get(key, {}) followed by .get: the inner dict, an empty one when the key is missing, a Crash for anything else
fn sub_dict(d: &Map, key: &str) -> R<Map> {
    match d.get(key) {
        None => Ok(Map::new()),
        Some(Value::Object(m)) => Ok(m.clone()),
        Some(other) => Err(no_get(other)),
    }
}

/// str(v) where the script formatted a value in a message (None where it was missing)
fn show(v: Option<&Value>) -> String {
    v.map(json::py_str).unwrap_or_else(|| "None".into())
}

/// float(text): whitespace around it, a sign, digits of any script with single underscores between them, a point, an
/// exponent, or inf / infinity / nan in any case. None: a ValueError.
pub fn py_float(text: &str) -> Option<f64> {
    let s = pycompat::strip(text);
    if s.is_empty() {
        return None;
    }
    let mut norm = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            let digit = |j: Option<&char>| j.map(|c| c.is_ascii_digit() || pycompat::decimal(*c).is_some()).unwrap_or(false);
            if !(i > 0 && digit(chars.get(i - 1)) && digit(chars.get(i + 1))) {
                return None;
            }
            continue;
        }
        match pycompat::decimal(c) {
            Some(d) => norm.push(char::from_digit(d, 10)?),
            None if c.is_ascii() => norm.push(c),
            None => return None,
        }
    }
    let lower = norm.to_ascii_lowercase();
    let body = lower.trim_start_matches(['+', '-']);
    if lower.len() - body.len() > 1 {
        return None;
    }
    match body {
        "inf" | "infinity" | "nan" => {}
        _ => {
            // Rust's parser takes the same decimal forms ("1.", ".5", "1e5"), and nothing but those once the words are aside
            if !body.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | '+' | '-')) || body.starts_with('e') {
                return None;
            }
        }
    }
    lower.parse::<f64>().ok()
}

/// float(v) of a value json.load made; None where it raised (TypeError or ValueError)
fn float_of(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => py_float(s),
        _ => None,
    }
}

/// format(x, ".Nf")
pub fn fmt_f(x: f64, prec: usize) -> String {
    if x.is_nan() {
        "nan".into()
    } else if x.is_infinite() {
        (if x > 0.0 { "inf" } else { "-inf" }).into()
    } else {
        format!("{x:.prec$}")
    }
}

/// f"{v:.Nf}" of a value json.load made (the script stopped on text and on lists)
fn fmt_value_f(v: &Value, prec: usize) -> R<String> {
    match v {
        Value::Number(n) => Ok(fmt_f(n.as_f64().unwrap_or(f64::NAN), prec)),
        Value::Bool(b) => Ok(fmt_f(if *b { 1.0 } else { 0.0 }, prec)),
        Value::String(_) => Err(Crash("ValueError: Unknown format code 'f' for object of type 'str'".into())),
        other => Err(Crash(format!("TypeError: unsupported format string passed to {}.__format__", type_name(other)))),
    }
}

/// str(x) of a float
fn float_str(x: f64) -> String {
    if x.is_nan() {
        "nan".into()
    } else if x.is_infinite() {
        (if x > 0.0 { "inf" } else { "-inf" }).into()
    } else {
        json::float_repr(x)
    }
}

fn float_value(x: f64) -> Value {
    serde_json::Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
}

/// The keys of every object sorted, as json.dump(..., sort_keys=True) writes them
fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), sorted(&m[k]))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// str.lstrip()
fn lstrip(s: &str) -> &str {
    s.trim_start_matches(pycompat::isspace)
}

/// str.split(None, maxsplit): at most `max` splits on runs of whitespace; the rest is kept whole (its leading whitespace
/// skipped, its trailing kept)
fn split_max(s: &str, max: usize) -> Vec<&str> {
    let mut out = vec![];
    let mut rest = lstrip(s);
    while !rest.is_empty() {
        if out.len() == max {
            out.push(rest);
            break;
        }
        match rest.char_indices().find(|(_, c)| pycompat::isspace(*c)) {
            None => {
                out.push(rest);
                break;
            }
            Some((i, _)) => {
                out.push(&rest[..i]);
                rest = lstrip(&rest[i..]);
            }
        }
    }
    out
}

/// One exact identity: `^[0-9a-f]{4}:[0-9a-f]{4}$` over the whole text (a final newline is not part of it)
fn is_id(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 9 && b[4] == b':' && b.iter().enumerate().all(|(i, c)| i == 4 || matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// `^[0-9A-F]{2}(\.[0-9A-F]{2}){4}$` over the whole text
fn is_vbios(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 14 && b.iter().enumerate().all(|(i, c)| if i % 3 == 2 { *c == b'.' } else { matches!(c, b'0'..=b'9' | b'A'..=b'F') })
}

/// A full PCI address, `^[0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-7]$` over the whole text
pub fn is_bdf(s: &str) -> bool {
    let b = s.as_bytes();
    let hex = |c: &u8| matches!(c, b'0'..=b'9' | b'a'..=b'f');
    b.len() == 12
        && b[..4].iter().all(hex)
        && b[4] == b':'
        && b[5..7].iter().all(hex)
        && b[7] == b':'
        && b[8..10].iter().all(hex)
        && b[10] == b'.'
        && (b'0'..=b'7').contains(&b[11])
}

// ---------------------------------------------------------------- files

/// open(path, encoding="utf-8", errors="replace").read(): text in universal-newline mode, bad bytes replaced; None on an
/// OSError
fn read_lossy(path: &str) -> Option<String> {
    fs::read(path).ok().map(|b| py::universal(&String::from_utf8_lossy(&b)))
}

/// The file's text, or None when it cannot be read or is not UTF-8 (a boot menu that is not, is not copied or compared)
fn read_text(path: &str) -> Option<String> {
    let b = fs::read(path).ok()?;
    String::from_utf8(b).ok().map(|t| py::universal(&t))
}

/// The file's JSON object, or None (missing, unreadable, not UTF-8, not JSON, not an object)
fn read_json(path: &str) -> Option<Map> {
    match json::loads(&read_text(path)?) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

/// os.makedirs(path, exist_ok=True), with the folder that failed as the error's file
fn makedirs(path: &str) -> Result<(), OsErr> {
    let (mut head, mut tail) = split_path(path);
    if tail.is_empty() {
        (head, tail) = split_path(&head);
    }
    if !head.is_empty() && !tail.is_empty() && !Path::new(&head).exists() {
        match makedirs(&head) {
            Err(e) if e.errno == libc::EEXIST => {}
            r => r?,
        }
    }
    match fs::DirBuilder::new().mode(0o777).create(path) {
        Ok(()) => Ok(()),
        Err(e) => {
            if Path::new(path).is_dir() {
                Ok(())
            } else {
                Err(OsErr::new(&e, Some(path)))
            }
        }
    }
}

/// os.path.split
fn split_path(p: &str) -> (String, String) {
    let i = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let (head, tail) = (&p[..i], &p[i..]);
    let trimmed = head.trim_end_matches('/');
    (if trimmed.is_empty() { head.to_string() } else { trimmed.to_string() }, tail.to_string())
}

/// Removes a file; one that is not there is fine.
fn remove(path: &str) -> Result<(), OsErr> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(OsErr::new(&e, Some(path))),
    }
}

/// Written whole or not at all: a new file beside it, renamed over it. Never through a link: a link left at the temporary
/// name is removed, not followed, and the rename replaces a link at the name itself.
pub fn write_file(path: &str, text: &str, mode: u32) -> Result<(), OsErr> {
    makedirs(&py::dirname(path))?;
    let tmp = format!("{path}.tmp");
    remove(&tmp)?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&tmp)
        .map_err(|e| OsErr::new(&e, Some(&tmp)))?;
    f.write_all(text.as_bytes()).map_err(|e| OsErr::new(&e, None))?;
    f.set_permissions(fs::Permissions::from_mode(mode)).map_err(|e| OsErr::new(&e, None))?;
    drop(f);
    fs::rename(&tmp, path).map_err(|e| OsErr::new(&e, Some(&tmp)))
}

fn write_json(path: &str, d: &Map, mode: u32) -> Result<(), OsErr> {
    write_file(path, &json::dumps_indent(&sorted(&Value::Object(d.clone())), 1), mode)
}

fn is_file(path: &str) -> bool {
    Path::new(path).is_file()
}

/// time.strftime("%Y-%m-%dT%H:%M:%S%z") in local time
pub fn now() -> String {
    extern "C" {
        fn tzset();
    }
    // SAFETY: tzset reads TZ; localtime_r and strftime write only into the buffers given
    unsafe {
        tzset();
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        let mut buf = [0u8; 64];
        let n = libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), c"%Y-%m-%dT%H:%M:%S%z".as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

fn wall() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

// ---------------------------------------------------------------- outside programs

/// subprocess.run(argv, capture_output=True, text=True, timeout=...) as the script used it: the program by argv (searched on
/// the PATH, no shell) with no standard input; its output and errors together, stripped.
pub fn run_program(argv: &[String], timeout: u64) -> Result<(i32, String), OsErr> {
    let mut child = match Command::new(&argv[0]).args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((127, format!("{}: not installed", argv[0]))),
        Err(e) => return Err(OsErr::new(&e, Some(&argv[0]))),
    };
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let (tx, rx) = mpsc::channel::<(u8, Vec<u8>)>();
    let so = child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>);
    let se = child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>);
    for (which, pipe) in [(0u8, so), (1u8, se)] {
        let tx = tx.clone();
        if let Some(mut p) = pipe {
            std::thread::spawn(move || {
                let mut b = vec![];
                let _ = p.read_to_end(&mut b);
                let _ = tx.send((which, b));
            });
        }
    }
    drop(tx);
    let timed_out = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
        Ok((124, format!("{}: timed out", argv[0])))
    };
    // the output is read until both pipes close and the program ends, all within the time (as communicate() did)
    let (mut out, mut err) = (Vec::new(), Vec::new());
    for _ in 0..2 {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((0, b)) => out = b,
            Ok((_, b)) => err = b,
            Err(mpsc::RecvTimeoutError::Timeout) => return timed_out(&mut child),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = match pycompat::process::wait_timeout(&mut child, deadline.saturating_duration_since(Instant::now())) {
        Ok(Some(s)) => s,
        Ok(None) => return timed_out(&mut child),
        Err(e) => return Err(OsErr::new(&e, None)),
    };
    use std::os::unix::process::ExitStatusExt;
    let code = status.code().unwrap_or_else(|| -(status.signal().unwrap_or(1)));
    let text = py::universal(&String::from_utf8_lossy(&out)) + &py::universal(&String::from_utf8_lossy(&err));
    Ok((code, pycompat::strip(&text).to_string()))
}

/// shutil.which(name): the first file on the PATH that may be run (not a folder)
fn which(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH").map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "/bin:/usr/bin".into());
    if path.is_empty() {
        return None;
    }
    let mut seen = vec![];
    for dir in path.split(':') {
        if seen.contains(&dir) {
            continue;
        }
        seen.push(dir);
        let p = py::join(dir, name);
        let Ok(c) = CString::new(p.as_bytes()) else { continue };
        // SAFETY: access() reads a NUL-terminated path
        if Path::new(&p).exists() && unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0 && !Path::new(&p).is_dir() {
            return Some(p);
        }
    }
    None
}

/// The home folder as os.path.expanduser("~") finds it: $HOME, else the password file's
fn home() -> String {
    if let Some(h) = std::env::var_os("HOME") {
        return h.to_string_lossy().into_owned();
    }
    // SAFETY: getpwuid returns a pointer into static storage or null; it is read at once
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned()
    }
}

/// GET of Ollama's list of loaded models: the body of a 2xx answer, or None (nothing listening, an error, an answer that is
/// not HTTP, more than HTTP_MAX). No proxy and no redirect is followed: whatever listens on the port gets one plain request.
fn http_get(addr: &str, path: &str) -> Option<Vec<u8>> {
    use std::net::{SocketAddr, TcpStream};
    let addr: SocketAddr = addr.parse().ok()?;
    let t = Duration::from_secs(2);
    let mut s = TcpStream::connect_timeout(&addr, t).ok()?;
    s.set_read_timeout(Some(t)).ok()?;
    s.set_write_timeout(Some(t)).ok()?;
    s.write_all(format!("GET {path} HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: {addr}\r\nUser-Agent: yggdrasil-gpuunlock\r\nConnection: close\r\n\r\n").as_bytes()).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let fill = |s: &mut TcpStream, buf: &mut Vec<u8>, chunk: &mut [u8]| -> Option<usize> {
        let n = s.read(chunk).ok()?;
        buf.extend_from_slice(&chunk[..n]);
        (buf.len() <= HTTP_MAX + 65536).then_some(n)
    };
    // the head: the status line and the headers, up to an empty line
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n").map(|i| i + 4).or_else(|| find(&buf, b"\n\n").map(|i| i + 2)) {
            break i;
        }
        if buf.len() > 65536 || fill(&mut s, &mut buf, &mut chunk)? == 0 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let status = lines.next()?;
    let parts = split_max(status, 2);
    if parts.len() < 2 || !parts[0].starts_with("HTTP/") || parts[1].len() != 3 || !parts[1].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let code: u16 = parts[1].parse().ok()?;
    if !(200..300).contains(&code) {
        return None;
    }
    let (mut chunked, mut length) = (false, None);
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "transfer-encoding" && v.eq_ignore_ascii_case("chunked") {
                chunked = true;
            } else if k == "content-length" {
                length = v.parse::<usize>().ok();
            }
        }
    }
    if code == 204 {
        return Some(vec![]);
    }
    let mut body: Vec<u8> = buf[head_end..].to_vec();
    if chunked {
        let mut out = Vec::new();
        loop {
            let line_end = loop {
                if let Some(i) = find(&body, b"\n") {
                    break i;
                }
                let before = body.len();
                if fill(&mut s, &mut body, &mut chunk)? == 0 || body.len() == before {
                    return None;
                }
            };
            let size_text = String::from_utf8_lossy(&body[..line_end]);
            let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
            body.drain(..line_end + 1);
            if size == 0 {
                return Some(out);
            }
            if out.len() + size > HTTP_MAX {
                return None;
            }
            while body.len() < size + 1 {
                if fill(&mut s, &mut body, &mut chunk)? == 0 {
                    return None;
                }
            }
            out.extend_from_slice(&body[..size]);
            body.drain(..size);
            // the line end after the chunk
            if body.first() == Some(&b'\r') {
                body.drain(..1);
            }
            while body.is_empty() {
                if fill(&mut s, &mut body, &mut chunk)? == 0 {
                    return None;
                }
            }
            if body.first() == Some(&b'\n') {
                body.drain(..1);
            } else {
                return None;
            }
        }
    }
    match length {
        Some(n) => {
            if n > HTTP_MAX {
                return None;
            }
            while body.len() < n {
                if fill(&mut s, &mut body, &mut chunk)? == 0 {
                    return None;
                }
            }
            body.truncate(n);
            Some(body)
        }
        None => {
            while fill(&mut s, &mut body, &mut chunk)? != 0 {}
            (body.len() <= HTTP_MAX).then_some(body)
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------- the card

/// The first NVIDIA display card, as Settings shows it and the support table matches it.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub name: String,
    pub device: String,
    pub subsystem: String,
    pub vbios: Option<String>,
    pub bdf: String,
    pub bar0: Option<String>,
    pub driver: Option<String>,
    pub driver_series: Option<String>,
}

fn opt(v: &Option<String>) -> Value {
    v.clone().map(Value::String).unwrap_or(Value::Null)
}

impl Card {
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("name".into(), Value::String(self.name.clone()));
        m.insert("device".into(), Value::String(self.device.clone()));
        m.insert("subsystem".into(), Value::String(self.subsystem.clone()));
        m.insert("vbios".into(), opt(&self.vbios));
        m.insert("bdf".into(), Value::String(self.bdf.clone()));
        m.insert("bar0".into(), opt(&self.bar0));
        m.insert("driver".into(), opt(&self.driver));
        m.insert("driver_series".into(), opt(&self.driver_series));
        Value::Object(m)
    }
}

fn maker(subsystem: &str) -> &'static str {
    let v = subsystem.split(':').next().unwrap_or("");
    BOARD_MAKERS.iter().find(|(k, _)| *k == v).map(|(_, n)| *n).unwrap_or("an unknown maker")
}

/// The driver's version in its first /proc line: `\s(\d+\.\d+(?:\.\d+)?)\s` (Python's \s and \d, any script)
fn driver_version(first: &str) -> Option<String> {
    let chars: Vec<char> = first.chars().collect();
    let digit = |c: char| pycompat::decimal(c).is_some();
    let digits = |mut j: usize| {
        let s = j;
        while j < chars.len() && digit(chars[j]) {
            j += 1;
        }
        (j > s).then_some(j)
    };
    for i in 0..chars.len() {
        if !pycompat::isspace(chars[i]) {
            continue;
        }
        let Some(a) = digits(i + 1) else { continue };
        if chars.get(a) != Some(&'.') {
            continue;
        }
        let Some(mut b) = digits(a + 1) else { continue };
        if chars.get(b) == Some(&'.') {
            if let Some(c) = digits(b + 1) {
                if chars.get(c).map(|c| pycompat::isspace(*c)).unwrap_or(false) {
                    return Some(chars[i + 1..c].iter().collect());
                }
            }
        }
        if chars.get(b).map(|c| pycompat::isspace(*c)).unwrap_or(false) {
            b = b.min(chars.len());
            return Some(chars[i + 1..b].iter().collect());
        }
    }
    None
}

/// "0x%08x" % value
fn hex8(v: i128) -> String {
    if v < 0 {
        format!("0x-{:07x}", v.unsigned_abs())
    } else {
        format!("0x{v:08x}")
    }
}

// ---------------------------------------------------------------- the helper

/// What the helper reads and runs. `Gpu::system()` is the real one; the tests make one over a scratch folder.
pub struct Gpu {
    /// A folder standing in for / when reading /sys, /proc, /etc and the firmware ("" on the system)
    pub root: String,
    pub state_dir: String,
    pub table: String,
    pub grub_cfg: String,
    /// host:port of Ollama's API
    pub ollama: String,
    /// The folder this program is in (the benchmark's source may be next to it)
    pub exe_dir: String,
    pub run: Runner,
    pub is_root: Cell<bool>,
    pub bench_hook: Option<BenchHook>,
    pub sample_hook: Option<SampleHook>,
    /// The heat watch's minutes and seconds between samples
    pub watch: Cell<(f64, f64)>,
    /// Some: what is printed is kept here (tests); None: it goes to standard output and error
    pub captured: Option<RefCell<(String, String)>>,
}

impl Gpu {
    pub fn system() -> Gpu {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned())).unwrap_or_default();
        Gpu {
            root: String::new(),
            state_dir: STATE_DIR.into(),
            table: TABLE.into(),
            grub_cfg: GRUB_CFG.into(),
            ollama: OLLAMA.into(),
            exe_dir,
            run: Box::new(run_program),
            // SAFETY: geteuid cannot fail
            is_root: Cell::new(unsafe { libc::geteuid() } == 0),
            bench_hook: None,
            sample_hook: None,
            watch: Cell::new((15.0, 10.0)),
            captured: None,
        }
    }

    fn say(&self, line: &str) {
        match &self.captured {
            Some(c) => {
                let mut c = c.borrow_mut();
                c.0.push_str(line);
                c.0.push('\n');
            }
            None => {
                let _ = writeln!(io::stdout().lock(), "{line}");
            }
        }
    }

    fn warn(&self, line: &str) {
        match &self.captured {
            Some(c) => {
                let mut c = c.borrow_mut();
                c.1.push_str(line);
                c.1.push('\n');
            }
            None => {
                let _ = writeln!(io::stderr().lock(), "{line}");
            }
        }
    }

    fn at(&self, path: &str) -> String {
        format!("{}{path}", self.root)
    }

    fn read(&self, path: &str) -> Option<String> {
        read_lossy(&self.at(path))
    }

    pub fn sp(&self, name: &str) -> String {
        py::join(&self.state_dir, name)
    }

    fn run(&self, argv: &[&str], timeout: u64) -> Result<(i32, String), OsErr> {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        (self.run)(&argv, timeout)
    }

    pub fn boot_id(&self) -> String {
        pycompat::strip(&self.read("/proc/sys/kernel/random/boot_id").unwrap_or_default()).to_string()
    }

    /// One line per event in the state folder's log, and the journal (where Heimdall can read it).
    fn log(&self, event: &str) -> R<()> {
        let line = format!("{} boot={} {event}\n", now(), py::take_chars(&self.boot_id(), 8));
        if makedirs(&self.state_dir).is_ok() {
            if let Ok(mut f) = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .mode(0o644)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(self.sp("log"))
            {
                let _ = f.write_all(line.as_bytes());
            }
        }
        self.run(&["logger", "-t", "yggdrasil-gpu-unlock", "-p", "user.notice", "--", event], 5)?;
        Ok(())
    }

    fn hexid(&self, path: &str) -> String {
        let v = pycompat::strip(&self.read(path).unwrap_or_default()).to_lowercase();
        v.strip_prefix("0x").map(String::from).unwrap_or(v)
    }

    /// The first NVIDIA display card, read from sysfs and the driver's /proc files (no program is run), or None.
    pub fn nvidia_card(&self) -> Option<Card> {
        let base = "/sys/bus/pci/devices";
        let mut names: Vec<String> = fs::read_dir(self.at(base)).ok()?.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
        names.sort();
        for bdf in names {
            let d = format!("{base}/{bdf}");
            if self.hexid(&format!("{d}/vendor")) != "10de" || !self.hexid(&format!("{d}/class")).starts_with("03") {
                continue;
            }
            let res = self.read(&format!("{d}/resource")).unwrap_or_default();
            let bar0 = pycompat::split(&res).first().and_then(|w| pycompat::int16(w)).map(hex8);
            let info = self.read(&format!("/proc/driver/nvidia/gpus/{bdf}/information")).unwrap_or_default();
            let field = |k: &str| {
                pycompat::splitlines(&info)
                    .into_iter()
                    .find(|l| l.starts_with(&format!("{k}:")))
                    .map(|l| pycompat::strip(l.split_once(':').map(|x| x.1).unwrap_or("")).to_string())
            };
            let vbios = field("Video BIOS").filter(|v| !v.is_empty());
            let ver = self.read("/proc/driver/nvidia/version").unwrap_or_default();
            let first = pycompat::splitlines(&ver).first().map(|s| s.to_string()).unwrap_or_default();
            return Some(Card {
                name: field("Model").filter(|v| !v.is_empty()).unwrap_or_else(|| "NVIDIA graphics card".into()),
                device: format!("10de:{}", self.hexid(&format!("{d}/device"))),
                subsystem: format!("{}:{}", self.hexid(&format!("{d}/subsystem_vendor")), self.hexid(&format!("{d}/subsystem_device"))),
                vbios: vbios.map(|v| v.to_uppercase()),
                bdf: bdf.clone(),
                bar0,
                driver: driver_version(&first),
                driver_series: if first.is_empty() {
                    None
                } else if first.contains("Open Kernel Module") {
                    Some("open".into())
                } else {
                    Some("proprietary".into())
                },
            });
        }
        None
    }

    /// True when the firmware says Secure Boot is on (the variable's fifth byte is 1).
    pub fn secure_boot(&self) -> bool {
        fs::read(self.at(SECURE_BOOT_VAR)).map(|b| b.len() >= 5 && b[4] == 1).unwrap_or(false)
    }

    // ------------------------------------------------------------ the support table

    /// The table's entries. Err when any entry is not an exact identity: no wildcards, no patterns, no missing field, and
    /// every entry links the evidence that it ran on that exact board.
    pub fn load_table(&self) -> Result<Vec<Map>, String> {
        let bytes = fs::read(&self.table).map_err(|e| format!("cannot read it ({})", OsErr::new(&e, None).strerror()))?;
        let d =
            String::from_utf8(bytes).ok().map(|t| py::universal(&t)).and_then(|t| json::loads(&t).ok()).ok_or("it is not valid JSON")?;
        let entries = match d.as_object().and_then(|d| d.get("entries")) {
            Some(Value::Array(a)) => a.clone(),
            _ => return Err("it has no list of entries".into()),
        };
        let mut out = vec![];
        for (i, e) in entries.iter().enumerate() {
            let i = i + 1;
            let Value::Object(e) = e else { return Err(format!("entry {i} is not an object")) };
            let text = |k: &str| e.get(k).and_then(|v| v.as_str());
            for k in ["device", "subsystem"] {
                if !text(k).map(is_id).unwrap_or(false) {
                    return Err(format!("entry {i}: {k} must be one exact id like 10de:1e07"));
                }
            }
            if !text("vbios").map(is_vbios).unwrap_or(false) {
                return Err(format!("entry {i}: vbios must be one exact version like 90.02.17.00.47"));
            }
            if !text("driver_series").map(|s| SERIES.contains(&s)).unwrap_or(false) {
                return Err(format!("entry {i}: driver_series must be open or proprietary"));
            }
            if !text("method").map(|s| METHODS.contains(&s)).unwrap_or(false) {
                return Err(format!("entry {i}: method must be one of {}", METHODS.join(", ")));
            }
            if !text("evidence").map(|s| s.starts_with("https://")).unwrap_or(false) {
                return Err(format!("entry {i}: needs an evidence link"));
            }
            out.push(e.clone());
        }
        Ok(out)
    }

    // ------------------------------------------------------------ what may stop an attempt

    /// Why turing_unlock cannot take part in the next start, or None: it must be installed for this kernel, with its
    /// firmware, and the start-up image must load it before the graphics driver.
    fn module_problem(&self) -> R<Option<String>> {
        let (code, out) = self.run(&["modinfo", "-F", "filename", "turing_unlock"], 10)?;
        if code != 0 || pycompat::strip(&out).is_empty() {
            return Ok(Some("The turing_unlock module is not installed (Phase 2 installs it)".into()));
        }
        if !Path::new(&self.at(MODULE_FIRMWARE)).is_dir() {
            return Ok(Some("The turing_unlock firmware is not installed".into()));
        }
        if !is_file(&self.at(MODULE_DROPIN)) {
            return Ok(Some("The start-up image is not set to load turing_unlock before the graphics driver".into()));
        }
        Ok(None)
    }

    /// Why the boot menu would not let the one-try entry be picked, or None.
    pub fn menu_problem(&self) -> Option<String> {
        let text = self.read(GRUB_DEFAULT).unwrap_or_default();
        // ^KEY=['"]?([^'"\n]*) on each line, the first that has it
        let value = |key: &str| {
            text.split('\n').find_map(|line| {
                let rest = line.strip_prefix(key)?.strip_prefix('=')?;
                let rest = rest.strip_prefix(['\'', '"']).unwrap_or(rest);
                let end = rest.find(['\'', '"']).unwrap_or(rest.len());
                Some(pycompat::strip(&rest[..end]).to_string())
            })
        };
        let (timeout, style) = (value("GRUB_TIMEOUT").unwrap_or_default(), value("GRUB_TIMEOUT_STYLE"));
        let zero = !timeout.is_empty() && timeout.chars().all(|c| pycompat::decimal(c).is_some()) && pycompat::int(&timeout) == Some(0);
        if style.as_deref() == Some("hidden") || zero {
            return Some("The boot menu is hidden, so the one-try entry could not be picked at the restart".into());
        }
        None
    }

    /// Why there is no locked baseline to judge an attempt by, or None (rule 8: the ratio is against the card's own locked
    /// rate).
    fn baseline_problem(&self) -> R<Option<String>> {
        let b = match read_json(&self.sp("baseline.json")) {
            Some(b) if !b.is_empty() => b,
            _ => {
                return Ok(Some(
                    "No locked baseline yet: run gpuunlock bench --baseline on the idle card at its default watts first".into(),
                ))
            }
        };
        if opt_truthy(b.get("quick")) || !opt_truthy(sub_dict(&b, "check")?.get("ok")) || !opt_truthy(b.get("mma_f16_acc_f32")) {
            return Ok(Some("The locked baseline is not a full, correct run: run gpuunlock bench --baseline again".into()));
        }
        let card = self.nvidia_card();
        let same = |k: &str, mine: Option<&String>| match (b.get(k), mine) {
            (None | Some(Value::Null), None) => true,
            (Some(Value::String(s)), Some(m)) => s == m,
            _ => false,
        };
        if !same("bdf", card.as_ref().map(|c| &c.bdf)) || !same("vbios", card.as_ref().and_then(|c| c.vbios.as_ref())) {
            return Ok(Some("The locked baseline was measured on another card or firmware: run gpuunlock bench --baseline again".into()));
        }
        Ok(None)
    }

    /// Why arming cannot start on a supported card right now, or None.
    fn arm_blocker(&self) -> R<Option<String>> {
        if let Some(p) = self.module_problem()? {
            return Ok(Some(p));
        }
        if let Some(p) = self.menu_problem() {
            return Ok(Some(p));
        }
        self.baseline_problem()
    }

    fn hold_on(&self) -> bool {
        Path::new(&self.sp("power-hold")).exists()
    }

    /// What the Settings tile shows: the state, why, the card and its table entry. Computed fresh every time.
    pub fn evaluate(&self) -> Eval {
        let e = |state: &str, reason: Value, card: Option<Card>, entry: Option<Map>| Eval { state: state.into(), reason, card, entry };
        let s = |t: &str| Value::String(t.into());
        let Some(card) = self.nvidia_card() else { return e("no-nvidia", s("No NVIDIA graphics card"), None, None) };
        if card.vbios.is_none() || card.driver_series.is_none() {
            return e("unsupported", s("The NVIDIA driver is not loaded, so the card's firmware version cannot be read"), Some(card), None);
        }
        let entries = match self.load_table() {
            Ok(t) => t,
            Err(why) => {
                return e("unsupported", s(&format!("The support table is not valid ({why}); nothing is offered")), Some(card), None)
            }
        };
        let Some(entry) = matching(&card, &entries) else {
            let why = unsupported_reason(&card, &entries);
            return e("unsupported", s(&why), Some(card), None);
        };
        let st = read_json(&self.sp("state.json")).unwrap_or_default();
        let state = st.get("state").and_then(|v| v.as_str());
        if state == Some("failed") {
            let why = st.get("why").filter(|v| truthy(v)).cloned().unwrap_or_else(|| s("The last attempt failed"));
            return e("failed", why, Some(card), Some(entry));
        }
        if Path::new(&self.sp("armed")).exists() {
            return e("armed", s(&format!("Pick \"{ENTRY_TITLE}\" in the boot menu at the next restart")), Some(card), Some(entry));
        }
        if Path::new(&self.sp("trying")).exists() {
            return e("armed", s("This start is the attempt; it is checked once the desktop is up"), Some(card), Some(entry));
        }
        if self.secure_boot() {
            return e("secure-boot", s("Needs Secure Boot off: the unlock module is not signed"), Some(card), Some(entry));
        }
        if let Some(live @ ("unlocked" | "throttled")) = state {
            if st.get("boot_id").and_then(|v| v.as_str()) == Some(self.boot_id().as_str()) {
                let why = st.get("why").filter(|v| truthy(v)).cloned().unwrap_or_else(|| s(""));
                return e(live, why, Some(card), Some(entry));
            }
        }
        e("supported", s("This card matches a tested one"), Some(card), Some(entry))
    }

    pub fn status(&self) -> R<Map> {
        let ev = self.evaluate();
        let st = read_json(&self.sp("state.json")).unwrap_or_default();
        let live = matches!(ev.state.as_str(), "unlocked" | "throttled");
        let blocker = if ev.state == "supported" { self.arm_blocker()? } else { None };
        let mut m = Map::new();
        m.insert("gpu".into(), ev.card.as_ref().map(Card::to_value).unwrap_or(Value::Null));
        m.insert("secure_boot".into(), Value::Bool(self.secure_boot()));
        m.insert("state".into(), Value::String(ev.state.clone()));
        m.insert("reason".into(), ev.reason.clone());
        m.insert("arm_blocker".into(), blocker.map(Value::String).unwrap_or(Value::Null));
        m.insert("last_attempt".into(), st.get("last_attempt").cloned().unwrap_or(Value::Null));
        m.insert("since".into(), if live { st.get("since").cloned().unwrap_or(Value::Null) } else { Value::Null });
        m.insert("ratio".into(), if live { st.get("ratio").cloned().unwrap_or(Value::Null) } else { Value::Null });
        m.insert("power_hold".into(), Value::Bool(self.hold_on()));
        m.insert("log".into(), Value::String(self.sp("log")));
        Ok(m)
    }

    // ------------------------------------------------------------ GRUB

    fn grub_mkconfig(&self) -> Result<(i32, String), OsErr> {
        let cfg = self.grub_cfg.clone();
        self.run(&["grub-mkconfig", "-o", &cfg], 300)
    }

    /// Takes the one-try entry out of the menu, if it is there.
    fn remove_entry(&self) -> Result<(), StepErr> {
        if !Path::new(&self.sp("entry.cfg")).exists() {
            return Ok(());
        }
        remove(&self.sp("entry.cfg"))?;
        let (code, out) = self.grub_mkconfig()?;
        if code != 0 {
            self.log(&format!("grub-mkconfig after removing the entry failed: {}", py::last_chars(&out, 200)))?;
        }
        Ok(())
    }

    /// The card named by turing_unlock.targets= on this start's kernel command line, or None.
    pub fn unlock_requested(&self) -> Option<String> {
        unlock_requested(&self.read("/proc/cmdline").unwrap_or_default())
    }

    // ------------------------------------------------------------ the card in use

    /// What is using the card for compute (a language model, a benchmark, a game), or None. Programs that only hold the
    /// card (the desktop, Sunshine, a browser, an editor) do not count.
    pub fn card_busy(&self) -> R<Option<String>> {
        let (code, out) = self.run(&["nvidia-smi", "pmon", "-c", "3", "-s", "u"], 30)?;
        if code == 0 && !pycompat::strip(&out).is_empty() {
            let busy = pmon_busy(&out)?;
            if !busy.is_empty() {
                return Ok(Some(format!("programs are computing on the card: {}", busy.join(", "))));
            }
        } else {
            let (code, out) = self.run(&["nvidia-smi", "--query-compute-apps=pid,process_name", "--format=csv,noheader"], 15)?;
            if code == 0 && !pycompat::strip(&out).is_empty() {
                let names: Vec<&str> = pycompat::splitlines(&out)
                    .into_iter()
                    .filter(|l| !pycompat::strip(l).is_empty())
                    .map(|l| pycompat::strip(l.rsplit(',').next().unwrap_or("")))
                    .collect();
                return Ok(Some(format!("programs are computing on the card: {}", names.join(", "))));
            }
        }
        self.ollama_models()
    }

    fn ollama_models(&self) -> R<Option<String>> {
        let Some(body) = http_get(&self.ollama, OLLAMA_PATH) else { return Ok(None) };
        let body = body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&body);
        let Ok(text) = std::str::from_utf8(body) else { return Ok(None) };
        let Ok(v) = json::loads(text) else { return Ok(None) };
        let Value::Object(d) = v else { return Err(no_get(&v)) };
        let models = d.get("models").cloned().unwrap_or(Value::Null);
        if !truthy(&models) {
            return Ok(None);
        }
        let Value::Array(models) = models else {
            return Err(Crash(format!("TypeError: the models list is a {}", type_name(&models))));
        };
        let mut names = vec![];
        for m in &models {
            let Value::Object(m) = m else { return Err(no_get(m)) };
            match m.get("name") {
                None => names.push("?".to_string()),
                Some(Value::String(s)) => names.push(s.clone()),
                Some(other) => {
                    return Err(Crash(format!(
                        "TypeError: sequence item {}: expected str instance, {} found",
                        names.len(),
                        type_name(other)
                    )))
                }
            }
        }
        Ok(Some(format!("a language model is loaded: {}", names.join(", "))))
    }

    // ------------------------------------------------------------ commands

    fn need_root(&self, what: &str) -> bool {
        if !self.is_root.get() {
            self.warn(&format!("gpuunlock: {what} needs root (Settings runs it with pkexec)"));
            return false;
        }
        true
    }

    fn refuse(&self, msg: &str, code: i32) -> i32 {
        self.warn(&format!("gpuunlock: {msg}"));
        code
    }

    pub fn cmd_status(&self, as_json: bool) -> R<i32> {
        let s = self.status()?;
        if as_json {
            self.say(&json::dumps(&Value::Object(s)));
        } else {
            match s.get("gpu") {
                Some(Value::Object(g)) => {
                    let f = |k: &str| show(g.get(k));
                    self.say(&format!(
                        "{} ({}, {} {}, VBIOS {}, {})",
                        f("name"),
                        f("device"),
                        maker(&f("subsystem")),
                        f("subsystem"),
                        f("vbios"),
                        f("bdf")
                    ));
                }
                _ => self.say("No NVIDIA card"),
            }
            self.say(&format!("State: {}. {}", show(s.get("state")), show(s.get("reason"))));
            if opt_truthy(s.get("arm_blocker")) {
                self.say(&format!("Cannot arm: {}", show(s.get("arm_blocker"))));
            }
            if opt_truthy(s.get("power_hold")) {
                self.say("Power held at the card's default watts in every mode");
            }
        }
        Ok(0)
    }

    pub fn cmd_arm(&self) -> R<i32> {
        if !self.need_root("arm") {
            return Ok(1);
        }
        let ev = self.evaluate();
        if ev.state == "secure-boot" {
            return Ok(self.refuse(&json::py_str(&ev.reason), 2));
        }
        if ev.state != "supported" {
            return Ok(self.refuse(&format!("not armed: the state is {} ({})", ev.state, json::py_str(&ev.reason)), 2));
        }
        if let Some(blocker) = self.arm_blocker()? {
            return Ok(self.refuse(&format!("not armed: {blocker}"), 2));
        }
        if let Some(busy) = self.card_busy()? {
            return Ok(self.refuse(
                &format!("not armed: {busy}. Stop Sarina first (systemctl --user stop labyrinth-voiced; sudo systemctl stop ollama)"),
                2,
            ));
        }
        let card = ev.card.expect("a supported card");
        let Some(entry_text) = one_try_entry(read_text(&self.grub_cfg).as_deref(), &card.bdf) else {
            return Ok(self.refuse("not armed: the boot menu's first entry is not one plain Linux entry to copy", 2));
        };
        let (code, snap) = self.run(
            &[
                "snapper",
                "-c",
                "root",
                "create",
                "--type",
                "single",
                "--cleanup-algorithm",
                "",
                "--print-number",
                "--description",
                "before GPU unlock",
            ],
            120,
        )?;
        if code != 0 {
            return Ok(self.refuse(&format!("not armed: the snapshot failed ({})", py::last_chars(&snap, 200)), 1));
        }
        let snap = pycompat::strip(&snap).to_string();

        let put_back = |why: &str| -> Result<i32, StepErr> {
            remove(&self.sp("armed"))?;
            remove(&self.sp("power-hold"))?;
            self.remove_entry()?;
            self.log(&format!("arm stopped: {why}; put back"))?;
            Ok(self.refuse(&format!("not armed: {why}; everything was put back"), 1))
        };
        let attempt = || -> Result<i32, StepErr> {
            let before = read_text(&self.grub_cfg);
            write_file(&self.sp("entry.cfg"), &entry_text, 0o644)?;
            let (code, out) = self.grub_mkconfig()?;
            let after = read_text(&self.grub_cfg);
            if code != 0 || first_entry_id(after.as_deref()) != first_entry_id(before.as_deref()) || !has_entry(after.as_deref(), ENTRY_ID)
            {
                let detail =
                    if code != 0 { py::last_chars(&out, 200) } else { "its first entry changed or the new entry is missing".into() };
                return put_back(&format!("the boot menu did not come out as expected ({detail})"));
            }
            let mut hold = Map::new();
            hold.insert("watts".into(), Value::Null);
            hold.insert("since".into(), Value::String(now()));
            hold.insert("why".into(), Value::String("graphics-card unlock armed".into()));
            write_json(&self.sp("power-hold"), &hold, 0o644)?;
            for unit in UNITS {
                let (code, out) = self.run(&["systemctl", "enable", unit], 30)?;
                if code != 0 {
                    return put_back(&format!("{unit} could not be enabled ({})", py::last_chars(&out, 200)));
                }
            }
            let mut armed = Map::new();
            armed.insert("time".into(), Value::String(now()));
            armed.insert("entry".into(), Value::String(ENTRY_ID.into()));
            armed.insert("boot_id".into(), Value::String(self.boot_id()));
            armed.insert("snapshot".into(), Value::String(snap.clone()));
            armed.insert("bdf".into(), Value::String(card.bdf.clone()));
            write_json(&self.sp("armed"), &armed, 0o600)?;
            Ok(-1)
        };
        match attempt() {
            Ok(-1) => {}
            Ok(code) => return Ok(code),
            Err(StepErr::Crash(c)) => return Err(c),
            Err(StepErr::Os(e)) => {
                let why = format!("{}: {}", e.filename.clone().unwrap_or_else(|| "a file".into()), e.strerror());
                return put_back(&why).map_err(Crash::from);
            }
        }
        self.log(&format!("armed: the one-try entry is in the boot menu (snapshot {snap}, {})", card.bdf))?;
        self.say(&format!(
            "Armed. At the next restart pick \"{ENTRY_TITLE}\" in the boot menu; any other choice starts normally. Snapshot {snap} was taken first."
        ));
        Ok(0)
    }

    pub fn cmd_disarm(&self) -> R<i32> {
        if !self.need_root("disarm") {
            return Ok(1);
        }
        if !Path::new(&self.sp("armed")).exists() {
            return Ok(self.refuse("nothing is armed", 0));
        }
        remove(&self.sp("armed"))?;
        self.remove_entry()?;
        remove(&self.sp("power-hold"))?;
        self.log("disarmed by the owner")?;
        self.say("Cancelled: the next restart is a normal one.");
        Ok(0)
    }

    pub fn cmd_reenable(&self) -> R<i32> {
        if !self.need_root("reenable") {
            return Ok(1);
        }
        let mut st = read_json(&self.sp("state.json")).unwrap_or_default();
        if st.get("state").and_then(|v| v.as_str()) != Some("failed") {
            return Ok(self.refuse("the unlock is not switched off by a failure", 0));
        }
        st.insert("state".into(), Value::String("idle".into()));
        st.remove("why");
        write_json(&self.sp("state.json"), &st, 0o644)?;
        self.log("re-enabled by the owner after a failed attempt")?;
        Ok(0)
    }

    pub fn cmd_release(&self) -> R<i32> {
        if !self.need_root("release") {
            return Ok(1);
        }
        if Path::new(&self.sp("armed")).exists() || Path::new(&self.sp("trying")).exists() {
            return Ok(self.refuse("an attempt is pending: the power hold stays until it is checked", 2));
        }
        if !self.hold_on() {
            return Ok(self.refuse("no power hold is set", 0));
        }
        remove(&self.sp("power-hold"))?;
        self.log("power hold ended by the owner: each mode's own limit applies again")?;
        Ok(0)
    }

    fn fail(&self, why: &str, last: Value) -> R<()> {
        let mut st = read_json(&self.sp("state.json")).unwrap_or_default();
        st.insert("state".into(), Value::String("failed".into()));
        st.insert("why".into(), Value::String(why.into()));
        st.insert("last_attempt".into(), last);
        write_json(&self.sp("state.json"), &st, 0o644)?;
        self.log(&format!("attempt failed: {why}"))
    }

    /// Early at every start while an attempt is pending: a `trying` left by an earlier start means that start never reached
    /// the desktop; the module's option on this start's command line means this start is the attempt.
    pub fn cmd_boot(&self) -> R<i32> {
        if !self.need_root("boot") {
            return Ok(1);
        }
        let me = self.boot_id();
        let mut t = read_json(&self.sp("trying"));
        if let Some(tr) = &t {
            if tr.get("boot_id").and_then(|v| v.as_str()) != Some(me.as_str()) {
                remove(&self.sp("trying"))?;
                let mut last = Map::new();
                last.insert("time".into(), tr.get("time").cloned().unwrap_or(Value::Null));
                last.insert("result".into(), Value::String("failed".into()));
                last.insert("why".into(), Value::String("did not reach the desktop".into()));
                let when = tr.get("time").map(json::py_str).unwrap_or_else(|| "?".into());
                self.fail(&format!("The last attempt ({when}) did not reach the desktop. The unlock is off."), Value::Object(last))?;
                t = None;
            }
        }
        if self.unlock_requested().is_some() && t.is_none() {
            let a = read_json(&self.sp("armed"));
            remove(&self.sp("armed"))?;
            let mut tr = Map::new();
            tr.insert("time".into(), Value::String(now()));
            tr.insert("boot_id".into(), Value::String(me.clone()));
            tr.insert("armed".into(), a.as_ref().and_then(|a| a.get("time").cloned()).unwrap_or(Value::Null));
            write_json(&self.sp("trying"), &tr, 0o644)?;
            let armed = a.map(|a| !a.is_empty()).unwrap_or(false);
            self.log(if armed {
                "this start is the attempt"
            } else {
                "the unlock option is on the command line without arming in Settings; followed as an attempt"
            })?;
        }
        Ok(0)
    }

    /// (ok, message): what turing_unlock said about the card at this start, from the kernel log.
    pub fn module_result(&self) -> R<(bool, String)> {
        let (_, kernel_log) = self.run(&["journalctl", "-k", "-b", "-o", "cat", "--no-pager"], 60)?;
        Ok(module_result(&kernel_log))
    }

    /// The benchmark's source: the installed one, or the one next to this program (a checkout).
    fn bench_source(&self) -> Option<String> {
        [self.at(BENCH_SRC), py::join(&self.exe_dir, "gpuunlock-bench.cu")].into_iter().find(|p| is_file(p))
    }

    /// The compiled benchmark, built with nvcc the first time (one build per source text); Err: why there is none.
    fn bench_binary(&self) -> R<Result<String, String>> {
        let Some(src) = self.bench_source() else { return Ok(Err("the benchmark's source is missing (gpuunlock-bench.cu)".into())) };
        let Some(nvcc) = which("nvcc").or_else(|| is_file(NVCC).then(|| NVCC.to_string())) else {
            return Ok(Err("the CUDA toolkit is not installed (pacman -S cuda): the benchmark is compiled with its nvcc".into()));
        };
        let source = fs::read(&src).map_err(|e| OsErr::new(&e, Some(&src)))?;
        let digest = ring::digest::digest(&ring::digest::SHA256, &source);
        let digest: String = digest.as_ref().iter().take(8).map(|b| format!("{b:02x}")).collect();
        let cache = if self.is_root.get() {
            self.at("/var/cache/yggdrasil/gpuunlock-bench")
        } else {
            let base = match std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
                Some(v) => v.to_string_lossy().into_owned(),
                None => {
                    let h = home();
                    let h = h.trim_end_matches('/');
                    format!("{h}/.cache")
                }
            };
            py::join(&py::join(&base, "yggdrasil"), "gpuunlock-bench")
        };
        let out = py::join(&cache, &format!("gpuunlock-bench-{digest}"));
        if !is_file(&out) {
            if let Err(e) = makedirs(&cache) {
                return Ok(Err(format!("no folder for the benchmark ({})", e.strerror())));
            }
            let part = format!("{out}.part");
            let (code, msg) = self.run(&[&nvcc, "-O3", "-arch=sm_75", "-std=c++17", "-o", &part, &src, "-lcublas"], 600)?;
            if code != 0 || !is_file(&part) {
                remove(&part)?;
                let tail = py::last_chars(&msg, 200);
                return Ok(Err(format!(
                    "the benchmark did not compile ({})",
                    if tail.is_empty() { "no program came out".into() } else { tail }
                )));
            }
            fs::rename(&part, &out).map_err(|e| OsErr::new(&e, Some(&part)))?;
        }
        Ok(Ok(out))
    }

    /// The benchmark's JSON for the card at `bdf`; Err: why there is none.
    pub fn run_bench(&self, bdf: &str, quick: bool) -> R<Result<Value, String>> {
        if let Some(h) = &self.bench_hook {
            return h(bdf, quick);
        }
        let binary = match self.bench_binary()? {
            Ok(b) => b,
            Err(why) => return Ok(Err(why)),
        };
        let mut argv = vec![binary.as_str(), "--pci", bdf];
        if quick {
            argv.push("--quick");
        }
        let (code, out) = self.run(&argv, 900)?;
        let res = pycompat::splitlines(pycompat::strip(&out)).last().and_then(|l| json::loads(l).ok());
        let Some(res) = res else {
            return Ok(Err(format!("the benchmark gave no result (exit {code}: {})", py::last_chars(&out, 160))));
        };
        // "error" in res, as Python's `in` reads it for each kind of value
        let has_error = match &res {
            Value::Object(m) => m.contains_key("error"),
            Value::Array(a) => a.iter().any(|x| x.as_str() == Some("error")),
            Value::String(s) => s.contains("error"),
            other => return Err(Crash(format!("TypeError: argument of type '{}' is not iterable", type_name(other)))),
        };
        if has_error {
            let Value::Object(m) = &res else { return Err(Crash(format!("TypeError: {} indices must be integers", type_name(&res)))) };
            return Ok(Err(format!("the benchmark stopped: {}", py::take_chars(&show(m.get("error")), 160))));
        }
        Ok(Ok(res))
    }

    /// (ok, ratio, why) for the card as it is now: the module's own result, then the correctness check against the
    /// processor and the tensor benchmark against the locked baseline (rule 8).
    pub fn check_unlock(&self) -> R<(bool, Option<f64>, String)> {
        let (ok, why) = self.module_result()?;
        if !ok {
            return Ok((false, None, why));
        }
        let Some(card) = self.nvidia_card() else {
            return Ok((false, None, "The module reports the unlock, but the card is not visible to the driver".into()));
        };
        let res = match self.run_bench(&card.bdf, false)? {
            Ok(r) => r,
            Err(why) => return Ok((false, None, format!("The module reports the unlock, but it could not be checked: {why}"))),
        };
        let Value::Object(res) = res else { return Err(no_get(&res)) };
        judge(read_json(&self.sp("baseline.json")).as_ref(), &res)
    }

    fn thermal_sample(&self) -> R<(Option<i128>, bool)> {
        if let Some(h) = &self.sample_hook {
            return h();
        }
        let (code, out) = self
            .run(&["nvidia-smi", "--query-gpu=temperature.gpu,clocks_throttle_reasons.hw_slowdown", "--format=csv,noheader,nounits"], 15)?;
        if code != 0 || !out.contains(',') {
            return Ok((None, false));
        }
        let first = pycompat::splitlines(&out).first().copied().unwrap_or("");
        let Some((t, slow)) = first.split_once(',') else {
            return Err(Crash("ValueError: not enough values to unpack (expected 2, got 1)".into()));
        };
        let (t, slow) = (pycompat::strip(t), pycompat::strip(slow));
        let t = if pycompat::isdigit(t) {
            Some(
                pycompat::int(t)
                    .ok_or_else(|| Crash(format!("ValueError: invalid literal for int() with base 10: {}", pycompat::repr(t))))?,
            )
        } else {
            None
        };
        Ok((t, slow.to_lowercase() == "active"))
    }

    /// The first minutes after an unlock: above HOT_C, or the card slowing itself down, holds the rest of it at HOT_CAP_W.
    pub fn thermal_watch(&self) -> R<bool> {
        let (minutes, every) = self.watch.get();
        let end = wall() + minutes * 60.0;
        while wall() < end {
            let (t, slow) = self.thermal_sample()?;
            if thermal_verdict(&[(t, slow)]) {
                let ts = t.map(|t| t.to_string()).unwrap_or_else(|| "None".into());
                let mut hold = Map::new();
                hold.insert("watts".into(), Value::from(HOT_CAP_W));
                hold.insert("since".into(), Value::String(now()));
                hold.insert(
                    "why".into(),
                    Value::String(format!("the card ran hot after the unlock ({ts} C{})", if slow { ", slowing itself down" } else { "" })),
                );
                write_json(&self.sp("power-hold"), &hold, 0o644)?;
                let mut st = read_json(&self.sp("state.json")).unwrap_or_default();
                st.insert("state".into(), Value::String("throttled".into()));
                st.insert("why".into(), Value::String(format!("Unlocked, but the card ran hot ({ts} C): held at {HOT_CAP_W} W")));
                write_json(&self.sp("state.json"), &st, 0o644)?;
                self.log(&format!("heat watch: {ts} C, slowdown {}; power held at {HOT_CAP_W} W", if slow { "True" } else { "False" }))?;
                return Ok(true);
            }
            std::thread::sleep(Duration::from_secs_f64(every.max(0.0)));
        }
        Ok(false)
    }

    /// Once the desktop is up after an attempt: confirm it or switch the unlock off; then take the entry out of the menu.
    pub fn cmd_verify(&self) -> R<i32> {
        if !self.need_root("verify") {
            return Ok(1);
        }
        if Path::new(&self.sp("armed")).exists() {
            return Ok(0); // a normal start while armed: the entry stays until it is picked or cancelled
        }
        let t = read_json(&self.sp("trying"));
        if let Some(t) = t.filter(|t| t.get("boot_id").and_then(|v| v.as_str()) == Some(self.boot_id().as_str())) {
            let (ok, ratio, why) = self.check_unlock()?;
            let time = t.get("time").cloned().unwrap_or(Value::Null);
            if ok {
                let ratio = ratio.unwrap_or(f64::NAN);
                let mut st = read_json(&self.sp("state.json")).unwrap_or_default();
                let mut last = Map::new();
                last.insert("time".into(), time);
                last.insert("result".into(), Value::String("unlocked".into()));
                last.insert("ratio".into(), float_value(ratio));
                st.insert("state".into(), Value::String("unlocked".into()));
                st.insert("boot_id".into(), Value::String(self.boot_id()));
                st.insert("since".into(), Value::String(now()));
                st.insert("ratio".into(), float_value(ratio));
                st.insert("why".into(), Value::String(format!("Tensor rate {}x of locked (checked)", fmt_f(ratio, 1))));
                st.insert("last_attempt".into(), Value::Object(last));
                write_json(&self.sp("state.json"), &st, 0o644)?;
                self.log(&format!("unlocked: ratio {}, results correct", fmt_f(ratio, 2)))?;
            } else {
                let mut last = Map::new();
                last.insert("time".into(), time);
                last.insert("result".into(), Value::String("failed".into()));
                last.insert("why".into(), Value::String(why.clone()));
                self.fail(&why, Value::Object(last))?;
            }
            remove(&self.sp("trying"))?;
            self.remove_entry()?;
            if ok {
                self.thermal_watch()?;
            }
        } else if Path::new(&self.sp("entry.cfg")).exists() {
            self.remove_entry()?;
        }
        Ok(0)
    }

    /// (limit, default) in watts from nvidia-smi, or None; only numbers (not "nan" or "inf") count.
    fn power_limits(&self) -> R<Option<(f64, f64)>> {
        let (_, out) = self.run(&["nvidia-smi", "--query-gpu=power.limit,power.default_limit", "--format=csv,noheader,nounits"], 15)?;
        let Some(first) = pycompat::splitlines(&out).first().copied() else { return Ok(None) };
        let vals: Option<Vec<f64>> = first.split(',').map(py_float).collect();
        Ok(match vals.as_deref() {
            Some([a, b]) if a.is_finite() && b.is_finite() => Some((*a, *b)),
            _ => None,
        })
    }

    pub fn cmd_bench(&self, args: &[String]) -> R<i32> {
        let quick = args.iter().any(|a| a == "--quick");
        let baseline = args.iter().any(|a| a == "--baseline");
        if args.iter().any(|a| a != "--quick" && a != "--baseline") || quick && baseline {
            return Ok(self.refuse("usage: gpuunlock bench [--quick | --baseline]", 2));
        }
        let card = match self.nvidia_card() {
            Some(c) if c.driver.as_deref().map(|d| !d.is_empty()).unwrap_or(false) => c,
            _ => return Ok(self.refuse("no NVIDIA card with its driver loaded", 2)),
        };
        if baseline {
            if !self.need_root("bench --baseline") {
                return Ok(1);
            }
            if self.module_result()?.0 {
                return Ok(self.refuse("the card is unlocked at this start; the baseline is measured locked", 2));
            }
            let limits = self.power_limits()?;
            if !matches!(limits, Some((l, d)) if (l - d).abs() <= 1.0) {
                let (l, d) = limits.map(|(l, d)| (float_str(l), float_str(d))).unwrap_or(("None".into(), "None".into()));
                return Ok(self.refuse(
                    &format!("the card is at {l} W; the baseline is measured at its default {d} W (Settings > Power, or the power hold)"),
                    2,
                ));
            }
        }
        if !quick {
            if let Some(busy) = self.card_busy()? {
                return Ok(self.refuse(&format!("the card must be idle for a measurement: {busy}"), 2));
            }
        }
        let res = match self.run_bench(&card.bdf, quick)? {
            Ok(r) => r,
            Err(why) => return Ok(self.refuse(&why, 1)),
        };
        self.say(&json::dumps(&res));
        let Value::Object(mut res) = res else { return Err(no_get(&res)) };
        if baseline {
            if !opt_truthy(sub_dict(&res, "check")?.get("ok")) {
                return Ok(self.refuse("wrong results on the locked card: no baseline kept", 1));
            }
            res.insert("time".into(), Value::String(now()));
            res.insert("bdf".into(), Value::String(card.bdf.clone()));
            res.insert("vbios".into(), opt(&card.vbios));
            res.insert("driver".into(), opt(&card.driver));
            let watts = self.power_limits()?.map(|(l, _)| float_value(l)).unwrap_or(Value::Null);
            res.insert("watts".into(), watts);
            write_json(&self.sp("baseline.json"), &res, 0o644)?;
            let key = |k: &str| res.get(k).map(json::py_str).ok_or_else(|| Crash(format!("KeyError: {}", pycompat::repr(k))));
            let line = format!(
                "baseline: FP32-accumulate {} TFLOPS, control {}, cuBLAS 8192 {}",
                key("mma_f16_acc_f32")?,
                key("mma_f16_acc_f16")?,
                show(gemm_at(&res)?.as_ref())
            );
            self.log(&line)?;
        }
        Ok(if opt_truthy(sub_dict(&res, "check")?.get("ok")) { 0 } else { 1 })
    }

    /// The one-try entry for /etc/grub.d/41_gpu_unlock: the file `arm` wrote, printed whole when it has the shape of one
    /// entry of ours, else nothing. Never fails (grub-mkconfig stops at a fragment that does).
    pub fn grub_entry(&self) -> i32 {
        if let Some(text) = checked_entry(&self.sp("entry.cfg")) {
            match &self.captured {
                Some(c) => c.borrow_mut().0.push_str(&text),
                None => {
                    let _ = io::stdout().lock().write_all(text.as_bytes());
                }
            }
            self.warn("Adding the graphics card unlock entry (one try, picked by hand)");
        }
        0
    }

    pub fn main(&self, argv: &[String]) -> i32 {
        let r = (|| -> R<i32> {
            if argv.len() < 2 || argv[1] == "-h" || argv[1] == "--help" {
                self.say(DOC);
                return Ok(if argv.len() >= 2 { 0 } else { 2 });
            }
            let rest = &argv[2..];
            match argv[1].as_str() {
                "status" => self.cmd_status(rest.iter().any(|a| a == "--json")),
                "bench" => self.cmd_bench(rest),
                "grub-entry" if rest.is_empty() => Ok(self.grub_entry()),
                "arm" if rest.is_empty() => self.cmd_arm(),
                "disarm" if rest.is_empty() => self.cmd_disarm(),
                "reenable" if rest.is_empty() => self.cmd_reenable(),
                "release" if rest.is_empty() => self.cmd_release(),
                "boot" if rest.is_empty() => self.cmd_boot(),
                "verify" if rest.is_empty() => self.cmd_verify(),
                _ => {
                    self.warn("gpuunlock: unknown command (see --help)");
                    Ok(2)
                }
            }
        })();
        match r {
            Ok(c) => c,
            Err(c) => {
                self.warn(&format!("gpuunlock: unexpected error (the Python script stopped here with a traceback): {}", c.0));
                1
            }
        }
    }
}

/// An error in a step that may meet an OSError (arming catches those and puts everything back) or a Crash (it stops).
pub enum StepErr {
    Os(OsErr),
    Crash(Crash),
}

impl From<OsErr> for StepErr {
    fn from(e: OsErr) -> StepErr {
        StepErr::Os(e)
    }
}

impl From<Crash> for StepErr {
    fn from(c: Crash) -> StepErr {
        StepErr::Crash(c)
    }
}

impl From<StepErr> for Crash {
    fn from(e: StepErr) -> Crash {
        match e {
            StepErr::Os(o) => o.crash(),
            StepErr::Crash(c) => c,
        }
    }
}

/// What `evaluate` found.
pub struct Eval {
    pub state: String,
    pub reason: Value,
    pub card: Option<Card>,
    pub entry: Option<Map>,
}

/// The entry for exactly this card, firmware and driver series (only entries still marked experimental are offered).
fn matching(card: &Card, entries: &[Map]) -> Option<Map> {
    entries
        .iter()
        .find(|e| {
            let s = |k: &str| e.get(k).and_then(|v| v.as_str());
            s("device") == Some(&card.device)
                && s("subsystem") == Some(&card.subsystem)
                && s("vbios") == card.vbios.as_deref()
                && s("driver_series") == card.driver_series.as_deref()
                && s("status") == Some("experimental")
        })
        .cloned()
}

fn unsupported_reason(card: &Card, entries: &[Map]) -> String {
    let s = |e: &Map, k: &str| e.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let vbios = card.vbios.clone().unwrap_or_else(|| "None".into());
    let series = card.driver_series.clone().unwrap_or_else(|| "None".into());
    let mine = format!("{} {} with VBIOS {vbios}", maker(&card.subsystem), card.subsystem);
    let same: Vec<&Map> = entries
        .iter()
        .filter(|e| s(e, "device") == card.device && s(e, "subsystem") == card.subsystem && Some(s(e, "vbios")) == card.vbios)
        .collect();
    if !same.is_empty() && same.iter().all(|e| Some(s(e, "driver_series")) != card.driver_series) {
        return format!("The tested entry for {mine} is for another NVIDIA driver series (this one: {series})");
    }
    if let Some(first) = same.first() {
        let status = first.get("status").map(json::py_str).unwrap_or_else(|| "not experimental".into());
        return format!("The entry for {mine} is marked {status}, so the unlock is not offered");
    }
    let tested: Vec<String> = entries
        .iter()
        .filter(|e| s(e, "device") == card.device)
        .map(|e| format!("{} {}, VBIOS {}", maker(&s(e, "subsystem")), s(e, "subsystem"), s(e, "vbios")))
        .collect();
    if tested.is_empty() {
        format!("No tested entry for {mine} (no board of this model has been tested)")
    } else {
        format!("No tested entry for {mine} (tested: {})", tested.join("; "))
    }
}

/// The id of the menu's first entry (what starts by default), or None.
pub fn first_entry_id(cfg: Option<&str>) -> Option<String> {
    for line in pycompat::splitlines(cfg.unwrap_or("")) {
        if line.starts_with("menuentry ") || line.starts_with("submenu ") {
            // \$menuentry_id_option '([^']+)'
            let lit = "$menuentry_id_option '";
            let mut from = 0;
            while let Some(i) = line[from..].find(lit) {
                let start = from + i + lit.len();
                if let Some(end) = line[start..].find('\'') {
                    if end > 0 {
                        return Some(line[start..start + end].to_string());
                    }
                }
                from += i + 1;
            }
            return Some(line.to_string());
        }
    }
    None
}

pub fn has_entry(cfg: Option<&str>, ident: &str) -> bool {
    let cfg = cfg.unwrap_or("");
    cfg.contains(&format!("--id {ident}")) || cfg.contains(&format!("$menuentry_id_option '{ident}'"))
}

/// The menu's first entry (the normal start) as a one-try copy: its own title and id, and the module's option for this card
/// added to the kernel line. None when the first entry is not one plain Linux entry, or already has the option.
pub fn one_try_entry(cfg: Option<&str>, bdf: &str) -> Option<String> {
    if !is_bdf(bdf) {
        return None;
    }
    let lines = pycompat::splitlines(cfg.unwrap_or(""));
    let start = lines.iter().position(|l| l.starts_with("menuentry "))?;
    let end = (start + 1..lines.len()).find(|&i| lines[i] == "}")?;
    let mut body: Vec<String> = lines[start + 1..end].iter().map(|s| s.to_string()).collect();
    let kernel: Vec<usize> = body.iter().enumerate().filter(|(_, l)| lstrip(l).starts_with("linux")).map(|(i, _)| i).collect();
    if kernel.len() != 1
        || body.join("\n").contains(PARAM)
        || body.iter().any(|l| ["menuentry", "submenu", "}"].iter().any(|p| lstrip(l).starts_with(p)))
    {
        return None;
    }
    body[kernel[0]] = format!("{} {PARAM}{bdf}", pycompat::rstrip(&body[kernel[0]]));
    let mut out = vec![format!("menuentry '{ENTRY_TITLE}' --class gnu-linux --class os --id {ENTRY_ID} {{")];
    out.extend(body);
    out.push("}".into());
    Some(out.join("\n") + "\n")
}

/// The card named by turing_unlock.targets= on a kernel command line: `(?:^|\s)turing_unlock\.targets=(\S+)`
pub fn unlock_requested(cmdline: &str) -> Option<String> {
    let lit = "turing_unlock.targets=";
    let mut from = 0;
    while let Some(i) = cmdline[from..].find(lit) {
        let at = from + i;
        let before_ok = at == 0 || cmdline[..at].chars().next_back().map(pycompat::isspace).unwrap_or(false);
        if before_ok {
            let rest = &cmdline[at + lit.len()..];
            let end = rest.find(pycompat::isspace).unwrap_or(rest.len());
            if end > 0 {
                return Some(rest[..end].to_string());
            }
        }
        from = at + 1;
        while !cmdline.is_char_boundary(from) {
            from += 1;
        }
    }
    None
}

/// The programs `nvidia-smi pmon -s u` shows using the card's processors (BUSY_SM % or more on average over its samples),
/// as "name (N %)". A program that only holds the card (the compositor, a browser, Sunshine waiting for a stream) shows "-".
pub fn pmon_busy(out: &str) -> R<Vec<String>> {
    let mut cols: Option<Vec<&str>> = None;
    let mut used: Vec<(String, String, Vec<i128>)> = vec![];
    for line in pycompat::splitlines(out) {
        if line.starts_with('#') {
            if cols.is_none() {
                cols = Some(pycompat::split(line.trim_start_matches('#')));
            }
            continue;
        }
        let Some(cols) = cols.as_ref() else { continue };
        if cols.is_empty() || !cols.contains(&"sm") || !cols.contains(&"pid") || pycompat::strip(line).is_empty() {
            continue;
        }
        let f = split_max(line, cols.len() - 1);
        if f.len() < cols.len() {
            continue;
        }
        let idx = |k: &str| cols.iter().position(|c| *c == k).unwrap_or(0);
        let sm = f[idx("sm")];
        let pid = f[idx("pid")].to_string();
        let last = pycompat::strip(f[f.len() - 1]);
        let name = if last.is_empty() { pid.clone() } else { pycompat::split(last)[0].to_string() };
        let n = if pycompat::isdigit(sm) {
            pycompat::int(sm).ok_or_else(|| Crash(format!("ValueError: invalid literal for int() with base 10: {}", pycompat::repr(sm))))?
        } else {
            0
        };
        match used.iter_mut().find(|(p, _, _)| *p == pid) {
            Some(u) => u.2.push(n),
            None => used.push((pid, name, vec![n])),
        }
    }
    Ok(used
        .into_iter()
        .filter(|(_, _, v)| !v.is_empty() && v.iter().sum::<i128>() >= BUSY_SM * v.len() as i128)
        .map(|(_, n, v)| format!("{n} ({} %)", v.iter().sum::<i128>().div_euclid(v.len() as i128)))
        .collect())
}

/// (ok, message): what turing_unlock said about the card, from a kernel log.
pub fn module_result(kernel_log: &str) -> (bool, String) {
    let lines: Vec<&str> =
        pycompat::splitlines(kernel_log).into_iter().filter(|l| l.contains("turing_unlock")).map(pycompat::strip).collect();
    if lines.iter().any(|l| l.contains("FEAT UNLOCKED")) {
        return (true, "The module reports the card unlocked".into());
    }
    if let Some(bad) =
        lines.iter().find(|l| ["unlock failed", "failure ", "no supported card", "did not stop"].iter().any(|w| l.contains(w)))
    {
        return (false, format!("The module reported: {}", py::take_chars(bad, 160)));
    }
    match lines.last() {
        None => (false, "The module did not run at this start (it must be in the start-up image)".into()),
        Some(l) => (false, format!("The module gave no result ({})", py::take_chars(l, 120))),
    }
}

/// The cuBLAS rate at size n=8192 in a benchmark result (the first entry of that size), or None.
pub fn gemm_at(res: &Map) -> R<Option<Value>> {
    let list = res.get("gemm_f16_acc_f32").cloned().unwrap_or(Value::Null);
    if !truthy(&list) {
        return Ok(None);
    }
    let items: Vec<Value> = match list {
        Value::Array(a) => a,
        Value::Object(m) => m.keys().map(|k| Value::String(k.clone())).collect(),
        Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        other => return Err(Crash(format!("TypeError: '{}' object is not iterable", type_name(&other)))),
    };
    for g in &items {
        let Value::Object(g) = g else { return Err(no_get(g)) };
        let is_n = match g.get("n") {
            Some(Value::Number(n)) => n.as_f64() == Some(8192.0),
            _ => false,
        };
        if is_n {
            return Ok(Some(g.get("tflops").cloned().unwrap_or(Value::Null)).filter(|v| !v.is_null()));
        }
    }
    Ok(None)
}

/// (ok, ratio, why) by rule 8: the outputs match the processor's, the card computed as it did when the baseline was taken
/// (the FP16-accumulate control within CONTROL_DRIFT), and the FP32-accumulate tensor rate is at least MIN_RATIO times the
/// baseline's. Rates that are not finite numbers never pass.
pub fn judge(baseline: Option<&Map>, now: &Map) -> R<(bool, Option<f64>, String)> {
    let chk = match now.get("check") {
        Some(v) if truthy(v) => match v {
            Value::Object(m) => m.clone(),
            other => return Err(no_get(other)),
        },
        _ => Map::new(),
    };
    if !opt_truthy(chk.get("ok")) {
        let g = |k: &str| chk.get(k).map(json::py_str).unwrap_or_else(|| "?".into());
        return Ok((false, None, format!("Wrong results: {} of {} outputs differ from the processor's", g("wrong"), g("compared"))));
    }
    let Some(baseline) = baseline.filter(|b| !b.is_empty()) else {
        return Ok((false, None, "No locked baseline to compare with (gpuunlock bench --baseline)".into()));
    };
    if opt_truthy(now.get("quick")) || opt_truthy(baseline.get("quick")) {
        return Ok((false, None, "A quick run measures nothing; the full benchmark is needed".into()));
    }
    let vals = (
        float_of(baseline.get("mma_f16_acc_f32")),
        float_of(now.get("mma_f16_acc_f32")),
        float_of(baseline.get("mma_f16_acc_f16")),
        float_of(now.get("mma_f16_acc_f16")),
    );
    let (Some(b), Some(n), Some(bc), Some(nc)) = vals else {
        return Ok((false, None, "The benchmark's result is incomplete".into()));
    };
    if !(b.is_finite() && bc.is_finite()) || b <= 0.0 || bc <= 0.0 {
        return Ok((false, None, "The locked baseline is not usable".into()));
    }
    if !(n.is_finite() && nc.is_finite()) {
        return Ok((false, None, "The benchmark's result is incomplete".into()));
    }
    if (nc / bc - 1.0).abs() > CONTROL_DRIFT {
        return Ok((
            false,
            None,
            format!(
                "The control rate (FP16 accumulate) is {} TFLOPS against {} locked: the card was busy or held back, so the comparison means nothing; measure again",
                fmt_f(nc, 1),
                fmt_f(bc, 1)
            ),
        ));
    }
    let ratio = n / b;
    if ratio < MIN_RATIO {
        return Ok((
            false,
            Some(ratio),
            format!("Tensor rate {}x of locked, under the {MIN_RATIO}x that shows the unlock", fmt_f(ratio, 2)),
        ));
    }
    let (gb, gn) = (gemm_at(baseline)?, gemm_at(now)?);
    let gemm = match (&gb, &gn) {
        (Some(gb), Some(gn)) if truthy(gb) && truthy(gn) => {
            format!("; cuBLAS 8192 {} TFLOPS against {}", fmt_value_f(gn, 1)?, fmt_value_f(gb, 1)?)
        }
        _ => String::new(),
    };
    Ok((
        true,
        Some(ratio),
        format!("Tensor rate {}x of locked ({} against {} TFLOPS{gemm}), results correct", fmt_f(ratio, 2), fmt_f(n, 1), fmt_f(b, 1)),
    ))
}

/// samples: (degrees C, hardware slowdown active) pairs. True when the card ran too hot, so the session is held lower.
pub fn thermal_verdict(samples: &[(Option<i128>, bool)]) -> bool {
    samples.iter().any(|(t, slow)| t.map(|t| t > HOT_C).unwrap_or(false) || *slow)
}

/// glibc's [[:space:]] in a UTF-8 locale (no no-break spaces), as the shell fragment's grep read the entry
fn grep_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{1680}' | '\u{2000}'..='\u{2006}' | '\u{2008}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{205f}' | '\u{3000}')
}

/// The entry file's text when it has the shape of exactly one entry of ours: a plain file (not a link) of UTF-8 text with
/// no NUL, ending in a newline; our id on its first line, a closing brace on its last, no other line opening an entry, and
/// one kernel line, ending in the option for one PCI address.
pub fn checked_entry(path: &str) -> Option<String> {
    let mut f = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut f).take(1 << 20).read_to_end(&mut bytes).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    let body = text.strip_suffix('\n')?;
    let lines: Vec<&str> = body.split('\n').collect();
    if !lines[0].ends_with(&format!("--id {ENTRY_ID} {{")) || lines[lines.len() - 1] != "}" {
        return None;
    }
    if lines.iter().filter(|l| l.starts_with("menuentry ")).count() != 1 {
        return None;
    }
    let after_space = |l: &str| l.trim_start_matches(grep_space).to_string();
    if lines[1..].iter().any(|l| {
        let l = after_space(l);
        l.starts_with("menuentry") || l.starts_with("submenu")
    }) {
        return None;
    }
    let is_kernel = |l: &str| after_space(l).strip_prefix("linux").and_then(|r| r.chars().next()).map(grep_space).unwrap_or(false);
    let kernels: Vec<&&str> = lines.iter().filter(|l| is_kernel(l)).collect();
    if kernels.len() != 1 {
        return None;
    }
    // ^[[:space:]]*linux[[:space:]].* turing_unlock\.targets=<address>$ on a kernel line (there is one)
    let k = kernels[0];
    let addr_at = k.len().checked_sub(12)?;
    let addr = k.get(addr_at..)?;
    let before = k.get(..addr_at)?;
    let lead = format!(" {PARAM}");
    if !is_bdf(addr) || !before.ends_with(&lead) {
        return None;
    }
    // the option must come after "linux" and its space: something of the line before it is the ".*"
    let head = &before[..before.len() - lead.len()];
    let rest = after_space(head);
    let ok = rest.strip_prefix("linux").and_then(|r| r.chars().next()).map(grep_space).unwrap_or(false);
    ok.then_some(text)
}

#[cfg(test)]
#[path = "gpuunlock_tests.rs"]
mod tests;
