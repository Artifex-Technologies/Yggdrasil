//! yggdrasil-gpu-run: run a program (a game) on a chosen graphics card, on a computer with more than one. The Python script's
//! behaviour (doc/gpu-run.txt is its help): the cards from /sys/class/drm in PCI order, their names from lspci and
//! vulkaninfo, the variables that make OpenGL, Vulkan and Proton draw on one, and `--frames-on` through gamescope.
//!
//! What it starts is what it was given: the command after the card number, run with execvpe as Python's os.execvpe ran it
//! (each folder of the PATH in turn, no shell, no fallback to /bin/sh for a file that is not a program), or gamescope
//! through env. A card number is read as Python's int() read it.

use std::ffi::{CString, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use pycompat::json::{self, Map, Value};

use crate::py;

pub const DOC: &str = include_str!("doc/gpu-run.txt");
const MESA_DRIVERS: &[&str] = &["amdgpu", "radeon", "i915", "xe", "nouveau"];
const NVIDIA: i128 = 0x10DE;

fn vendor_name(v: i128) -> &'static str {
    match v {
        0x10DE => "NVIDIA",
        0x1002 => "AMD",
        0x8086 => "Intel",
        _ => "Graphics",
    }
}

fn sysfs() -> String {
    py::env_or("YGGDRASIL_SYSFS", "/sys")
}

/// open(path).read().strip(), or the default on an OSError. None: not UTF-8 (the script stopped there).
fn read(path: &str, default: &str) -> Result<String, py::Crash> {
    match std::fs::read(path) {
        Ok(b) => String::from_utf8(b).map(|t| pycompat::strip(&py::universal(&t)).to_string()).map_err(|e| py::Crash(format!("UnicodeDecodeError: {path}: {e}"))),
        Err(_) => Ok(default.to_string()),
    }
}

/// The card's marketing name from the PCI database (lspci), e.g. 'GeForce RTX 2070'; empty if lspci is missing.
pub fn pci_name(slot: &str) -> String {
    let out = match timed(&["lspci", "-mm", "-s", slot], 5) {
        Some(o) => o,
        None => return String::new(),
    };
    let fields: Vec<String> = regex::Regex::new(r#""([^"]*)""#).unwrap().captures_iter(&out).map(|c| c[1].to_string()).collect();
    // "class" "vendor" "device" ...; the device often carries the name in brackets: "TU106 [GeForce RTX 2070 Rev. A]"
    if fields.len() >= 3 {
        return match regex::Regex::new(r"\[([^\]]+)\]").unwrap().captures(&fields[2]) {
            Some(m) => m[1].to_string(),
            None => fields[2].clone(),
        };
    }
    String::new()
}

/// A program's standard output as text, or None when it could not be started or took longer than `secs`.
fn timed(argv: &[&str], secs: u64) -> Option<String> {
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
    if !matches!(pycompat::process::wait_timeout(&mut child, std::time::Duration::from_secs(secs)), Ok(Some(_))) {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    Some(py::universal(&String::from_utf8_lossy(&reader.join().ok()?)))
}

/// One Vulkan device: (vendor id, device id, name).
pub type VkDevice = (i128, i128, String);

fn hex(s: &str) -> Result<i128, py::Crash> {
    pycompat::int16(s).ok_or_else(|| py::Crash(format!("ValueError: invalid literal for int() with base 16: {}", pycompat::repr(s))))
}

/// The devices of `vulkaninfo --summary` (or the given text).
pub fn vulkan_devices(text: Option<&str>) -> Result<Vec<VkDevice>, py::Crash> {
    let owned;
    let text = match text {
        Some(t) => t,
        None => match timed(&["vulkaninfo", "--summary"], 15) {
            Some(t) => {
                owned = t;
                &owned
            }
            None => return Ok(vec![]),
        },
    };
    let line_re = regex::Regex::new(&r"^\s*(vendorID|deviceID|deviceName)\s*=\s*(.+)".replace(r"\s", pycompat::PY_SPACE_CLASS)).unwrap();
    let mut devices = vec![];
    let mut cur: Vec<(String, String)> = vec![];
    let complete = |cur: &[(String, String)]| ["vendorID", "deviceID", "deviceName"].iter().all(|k| cur.iter().any(|(n, _)| n == k));
    let field = |cur: &[(String, String)], k: &str| cur.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap();
    for line in pycompat::splitlines(text) {
        let Some(m) = line_re.captures(line) else { continue };
        let (k, v) = (m[1].to_string(), pycompat::strip(&m[2]).to_string());
        if k == "vendorID" && !cur.is_empty() {
            if complete(&cur) {
                devices.push((hex(&field(&cur, "vendorID"))?, hex(&field(&cur, "deviceID"))?, field(&cur, "deviceName")));
            }
            cur.clear();
        }
        cur.push((k, v));
    }
    if complete(&cur) {
        devices.push((hex(&field(&cur, "vendorID"))?, hex(&field(&cur, "deviceID"))?, field(&cur, "deviceName")));
    }
    Ok(devices)
}

/// A graphics card.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub slot: String,
    pub vendor: i128,
    pub device: i128,
    pub driver: String,
    pub boot: bool,
    pub screens: i64,
    pub number: i64,
    pub name: String,
    pub vulkan_name: String,
}

impl Card {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("slot".into(), self.slot.clone().into());
        m.insert("vendor".into(), Value::from(self.vendor as i64));
        m.insert("device".into(), Value::from(self.device as i64));
        m.insert("driver".into(), self.driver.clone().into());
        m.insert("boot".into(), self.boot.into());
        m.insert("screens".into(), self.screens.into());
        m.insert("number".into(), self.number.into());
        m.insert("name".into(), self.name.clone().into());
        m.insert("vulkan_name".into(), self.vulkan_name.clone().into());
        Value::Object(m)
    }
}

fn realpath_name(p: &str) -> String {
    let real = std::fs::canonicalize(p).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| pycompat::normpath(p));
    py::basename(&real).to_string()
}

/// The graphics cards, in PCI order (numbered from 1), with the names `name_of` gives each slot.
pub fn cards_with(vulkan: Option<Vec<VkDevice>>, name_of: &dyn Fn(&str) -> String) -> Result<Vec<Card>, py::Crash> {
    let drm = py::join(&py::join(&sysfs(), "class"), "drm");
    let mut names: Vec<String> = match std::fs::read_dir(&drm) {
        Ok(rd) => rd.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect(),
        Err(_) => vec![],
    };
    names.sort();
    let card_re = regex::Regex::new(r"^card\d+$").unwrap();
    let mut found: Vec<Card> = vec![];
    for n in &names {
        if !card_re.is_match(n) {
            continue;
        }
        let dev = py::join(&py::join(&drm, n), "device");
        let vendor = read(&py::join(&dev, "vendor"), "")?;
        if vendor.is_empty() {
            continue;
        }
        let slot = realpath_name(&dev);
        let mut screens = 0;
        for o in &names {
            if o.starts_with(&format!("{n}-")) && read(&py::join(&py::join(&drm, o), "status"), "")? == "connected" {
                screens += 1;
            }
        }
        let driver_link = py::join(&dev, "driver");
        let card = Card {
            slot: slot.clone(),
            vendor: hex(&vendor)?,
            device: hex(&read(&py::join(&dev, "device"), "0")?)?,
            driver: if std::path::Path::new(&driver_link).exists() { realpath_name(&driver_link) } else { String::new() },
            boot: read(&py::join(&dev, "boot_vga"), "")? == "1",
            screens,
            number: 0,
            name: String::new(),
            vulkan_name: String::new(),
        };
        // a dict by slot: a later card with the same slot takes the earlier one's place
        match found.iter_mut().find(|c| c.slot == slot) {
            Some(c) => *c = card,
            None => found.push(card),
        }
    }
    let vk = match vulkan {
        Some(v) => v,
        None => vulkan_devices(None)?,
    };
    found.sort_by(|a, b| a.slot.cmp(&b.slot));
    for (i, c) in found.iter_mut().enumerate() {
        c.number = i as i64 + 1;
        let n = name_of(&c.slot);
        c.name = if n.is_empty() { format!("{} card", vendor_name(c.vendor)) } else { n };
        // two identical cards have the same Vulkan name: either picks the right model, which is what matters for the name filter
        c.vulkan_name = vk.iter().find(|d| d.0 == c.vendor && d.1 == c.device).map(|d| d.2.clone()).unwrap_or_default();
    }
    Ok(found)
}

pub fn cards() -> Result<Vec<Card>, py::Crash> {
    cards_with(None, &pci_name)
}

/// The variables that make OpenGL, Vulkan and Proton draw on `card` (in the script's order).
pub fn environment(card: &Card, all: &[Card]) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![];
    if all.len() < 2 {
        return env; // one card: nothing to choose
    }
    let set = |env: &mut Vec<(String, String)>, k: &str, v: String| match env.iter_mut().find(|(n, _)| n == k) {
        Some(e) => e.1 = v,
        None => env.push((k.into(), v)),
    };
    if card.vendor == NVIDIA && card.driver == "nvidia" {
        set(&mut env, "__NV_PRIME_RENDER_OFFLOAD", "1".into());
        set(&mut env, "__GLX_VENDOR_LIBRARY_NAME", "nvidia".into());
        set(&mut env, "__VK_LAYER_NV_optimus", "NVIDIA_only".into());
    } else {
        set(&mut env, "DRI_PRIME", format!("pci-{}", card.slot.replace([':', '.'], "_")));
        set(&mut env, "MESA_VK_DEVICE_SELECT", format!("{}:{}", hex4(card.vendor), hex4(card.device)));
        if all.iter().any(|c| c.vendor == NVIDIA && c.driver == "nvidia") {
            set(&mut env, "__VK_LAYER_NV_optimus", "non_NVIDIA_only".into());
        }
    }
    if !card.vulkan_name.is_empty() {
        set(&mut env, "DXVK_FILTER_DEVICE_NAME", card.vulkan_name.clone());
        set(&mut env, "VKD3D_FILTER_DEVICE_NAME", card.vulkan_name.clone());
    }
    let _ = MESA_DRIVERS;
    env
}

/// f"{n:04x}" (a negative number keeps its sign in front)
fn hex4(n: i128) -> String {
    if n < 0 {
        format!("-{:04x}", -n)
    } else {
        format!("{n:04x}")
    }
}

/// The argv that runs `command` on card `game` inside gamescope on card `frames`, with lsfg-vk on gamescope only.
pub fn frames_command(game: &Card, frames: &Card, all: &[Card], command: &[String], multiplier: i128) -> Vec<String> {
    let mut outer = environment(frames, all);
    let inner = environment(game, all);
    for (k, v) in [("LSFGVK_ENV", "1".to_string()), ("LSFGVK_MULTIPLIER", multiplier.to_string())] {
        match outer.iter_mut().find(|(n, _)| n == k) {
            Some(e) => e.1 = v,
            None => outer.push((k.into(), v)),
        }
    }
    let unset: Vec<&String> = outer.iter().map(|(k, _)| k).filter(|k| !inner.iter().any(|(n, _)| n == *k)).collect();
    let mut argv = vec!["env".to_string()];
    argv.extend(outer.iter().map(|(k, v)| format!("{k}={v}")));
    argv.extend(["gamescope", "--backend", "sdl", "--prefer-vk-device"].map(String::from));
    argv.push(format!("{}:{}", hex4(frames.vendor), hex4(frames.device)));
    argv.extend(["-f", "--", "env"].map(String::from));
    for k in unset {
        argv.push("-u".into());
        argv.push(k.clone());
    }
    argv.push("DISABLE_LSFGVK=1".into());
    argv.extend(inner.iter().map(|(k, v)| format!("{k}={v}")));
    argv.extend(command.iter().cloned());
    argv
}

/// The card numbered `arg` (read as int() reads a number), if there is one.
pub fn pick<'a>(arg: &str, all: &'a [Card]) -> Option<&'a Card> {
    let n = pycompat::int(arg)?;
    all.iter().find(|c| c.number as i128 == n)
}

/// os.execvpe(file, args, env): a name with a slash is run as it is, else each folder of env's PATH in turn (os.defpath
/// without one). Returns only when nothing could be run: str() of the OSError Python raised then ("[Errno 2] No such file or
/// directory: b'/usr/bin/x'": the path of the last folder tried, or of the first whose error was not "no such file", as
/// bytes; a name with a slash as text). `named`: os.execvpe's errors name the path, os.execvp's (used for gamescope) do not.
/// An empty program name is Python's ValueError (the script stopped there).
fn execvpe(file: &str, args: &[OsString], env: &[(OsString, OsString)], named: bool) -> Result<String, py::Crash> {
    if args.first().map(|a| a.is_empty()).unwrap_or(true) {
        return Err(py::Crash("ValueError: execve: argv first element cannot be empty".into()));
    }
    let cargs: Vec<CString> = args.iter().map(|a| CString::new(a.as_bytes()).unwrap_or_default()).collect();
    let mut argv: Vec<*const libc::c_char> = cargs.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    let cenv: Vec<CString> = env
        .iter()
        .map(|(k, v)| {
            let mut kv = k.clone().into_vec();
            kv.push(b'=');
            kv.extend_from_slice(v.as_bytes());
            CString::new(kv).unwrap_or_default()
        })
        .collect();
    let mut envp: Vec<*const libc::c_char> = cenv.iter().map(|a| a.as_ptr()).collect();
    envp.push(std::ptr::null());
    let try_one = |path: &str| -> i32 {
        match CString::new(path) {
            Ok(c) => {
                unsafe { libc::execve(c.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            }
            // a NUL in the name: Python's ValueError
            Err(_) => libc::EINVAL,
        }
    };
    let shown = |code: i32, name: String| {
        if named {
            format!("[Errno {code}] {}: {name}", py::strerror(code))
        } else {
            format!("[Errno {code}] {}", py::strerror(code))
        }
    };
    if file.contains('/') {
        let code = try_one(file);
        return Ok(shown(code, pycompat::repr(file)));
    }
    let path = env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.to_string_lossy().into_owned()).unwrap_or_else(|| "/bin:/usr/bin".into());
    let mut saved: Option<(i32, String)> = None;
    let mut last = (libc::ENOENT, String::new());
    for dir in path.split(':') {
        let full = py::join(dir, file);
        let code = try_one(&full);
        if code != libc::ENOENT && code != libc::ENOTDIR && saved.is_none() {
            saved = Some((code, full.clone()));
        }
        last = (code, full);
    }
    let (code, full) = saved.unwrap_or(last);
    Ok(shown(code, pycompat::repr_bytes(full.as_bytes())))
}

/// `argv` as text decides; `raw` (the same arguments as the system gave them) is what a started program gets, byte for byte.
fn dispatch(argv: &[String], raw: &[OsString]) -> Result<i32, py::Crash> {
    if argv.is_empty() || argv[0] == "-h" || argv[0] == "--help" {
        println!("{}", pycompat::strip(DOC));
        return Ok(if argv.is_empty() { 2 } else { 0 });
    }
    if argv[0] == "list" {
        let cs = cards()?;
        if argv.iter().any(|a| a == "--json") {
            println!("{}", json::dumps_indent(&Value::from(cs.iter().map(Card::to_json).collect::<Vec<_>>()), 1));
        } else {
            for c in &cs {
                let notes: Vec<String> = [
                    if c.boot { "started on this card".to_string() } else { String::new() },
                    if c.screens != 0 { format!("{} screen(s)", c.screens) } else { String::new() },
                ]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect();
                let driver = if c.driver.is_empty() { "no driver" } else { &c.driver };
                let tail = if notes.is_empty() { String::new() } else { format!("  {}", notes.join(", ")) };
                println!("{}  {}  ({driver}, {}){tail}", c.number, c.name, c.slot);
            }
        }
        return Ok(0);
    }
    if argv[0] == "env" {
        let cs = cards()?;
        let arg = argv.get(1).map(String::as_str).unwrap_or("");
        let Some(c) = pick(arg, &cs) else {
            let shown = if argv.len() > 1 { pycompat::repr_list(&argv[1..]) } else { String::new() };
            eprintln!("yggdrasil-gpu-run: no card {shown}; see: yggdrasil-gpu-run list");
            return Ok(2);
        };
        for (k, v) in environment(c, &cs) {
            println!("{k}={v}");
        }
        return Ok(0);
    }
    let cs = cards()?;
    let c = pick(&argv[0], &cs);
    if let Some(c) = c {
        if argv.len() > 2 && argv[1] == "--frames-on" {
            let f = pick(&argv[2], &cs);
            let mut rest = &argv[3..];
            let mut multiplier: i128 = 2;
            if rest.len() >= 2 && rest[0] == "--multiplier" && pycompat::isdigit(&rest[1]) {
                // isdigit() takes a superscript two, int() does not: the script stopped there
                let m = pycompat::int(&rest[1]).ok_or_else(|| py::Crash(format!("ValueError: invalid literal for int() with base 10: {}", pycompat::repr(&rest[1]))))?;
                if (2..=20).contains(&m) {
                    multiplier = m;
                    rest = &rest[2..];
                }
            }
            let ok = matches!(f, Some(f) if f.number != c.number) && !rest.is_empty();
            if !ok {
                eprintln!("yggdrasil-gpu-run: usage: yggdrasil-gpu-run N --frames-on M [--multiplier X] COMMAND (two different cards)");
                return Ok(2);
            }
            let start = argv.len() - rest.len();
            let mut a2: Vec<OsString> = frames_command(c, f.unwrap(), &cs, &[], multiplier).into_iter().map(OsString::from).collect();
            a2.extend(raw[start..].iter().cloned());
            let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
            let e = execvpe("env", &a2, &env, false)?;
            eprintln!("yggdrasil-gpu-run: cannot start gamescope: {e}");
            return Ok(127);
        }
    }
    let Some(c) = c.filter(|_| argv.len() >= 2) else {
        let nums: Vec<String> = cs.iter().map(|x| x.number.to_string()).collect();
        let shown = if nums.is_empty() { "none found".to_string() } else { nums.join(", ") };
        eprintln!("yggdrasil-gpu-run: usage: yggdrasil-gpu-run N COMMAND (cards: {shown})");
        return Ok(2);
    };
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (k, v) in environment(c, &cs) {
        let (k, v) = (OsString::from(k), OsString::from(v));
        match env.iter_mut().find(|(n, _)| *n == k) {
            Some(e) => e.1 = v,
            None => env.push((k, v)),
        }
    }
    let e = execvpe(&argv[1], &raw[1..], &env, true)?;
    eprintln!("yggdrasil-gpu-run: cannot start {}: {e}", argv[1]);
    Ok(127)
}

/// The program: `argv` without its own name (as sys.argv[1:]).
pub fn main(raw: &[OsString]) -> i32 {
    let argv: Vec<String> = raw.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    match dispatch(&argv, raw) {
        Ok(c) => c,
        Err(c) => py::crashed("yggdrasil-gpu-run", &c),
    }
}

#[cfg(test)]
mod tests;
