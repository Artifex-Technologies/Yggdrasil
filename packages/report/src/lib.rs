//! Report a problem: the zip a person sends to their system's support when something goes wrong, made by Settings > About >
//! Report a problem. It holds what the person wrote, what the computer is (make and model, processor, memory, graphics cards
//! and their drivers, screens, disks, sound), the software's versions, recent errors and what the operating system adds (its
//! `kind = report` integration: where reports go and its own checks). Names, network addresses, Wi-Fi names and serial
//! numbers are taken out first (`scrub`); pictures the person adds go as they are.

pub mod scrub;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use scrub::Personal;

/// What a report can be about (the form's first choice).
pub const AREAS: [&str; 10] = [
    "The desktop and windows",
    "Settings",
    "Files (Minos)",
    "Store (Agora)",
    "Games",
    "Installing or updating",
    "Network and internet",
    "Sound",
    "Screens",
    "Something else",
];

/// What the form says goes in a report.
pub const WHAT_IT_HOLDS: &str = "The report holds what you write here and what is needed to fix the problem: this computer's make and model, its processor, memory, graphics cards and drivers, screens, disks and sound, the software's versions, and recent error messages. Your name, the computer's name, network addresses, Wi-Fi names and serial numbers are taken out. Pictures you add go as they are.";

/// The most of one log kept (its end, where the problem is).
const LOG_TAIL: usize = 400 * 1024;

/// Where reports go and what the operating system adds, from its `kind = report` integration (docs/INTEGRATIONS.md).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Support {
    /// Whose support it is ("Labyrinth OS").
    pub name: String,
    /// Where Email it sends the report.
    pub email: Option<String>,
    /// More files for the report: (title, command), each command's output as `system/<title>.txt`.
    pub includes: Vec<(String, Vec<String>)>,
}

impl Support {
    pub fn from(i: &yggdrasil_conf::integrations::Integration) -> Support {
        let includes = i
            .entries("include.")
            .into_iter()
            .filter_map(|(_, v)| {
                let (title, cmd) = v.split_once('|')?;
                let argv: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
                (argv.first().is_some_and(|p| p.starts_with('/')) && !title.trim().is_empty()).then(|| (title.trim().to_string(), argv))
            })
            .collect();
        let email = i.get("email").filter(|e| e.contains('@') && !e.contains(char::is_whitespace)).map(str::to_string);
        Support { name: i.get("name").unwrap_or("").to_string(), email, includes }
    }

    /// The operating system's, if it has one.
    pub fn find() -> Option<Support> {
        yggdrasil_conf::integrations::first("report").map(|i| Support::from(&i))
    }
}

/// What the person asked for.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub area: String,
    pub text: String,
    pub pictures: Vec<PathBuf>,
}

/// The computer as a report reads it: its files under `root` (`/`, a made-up tree in tests), the commands it runs and its
/// environment.
pub struct Machine<'a> {
    pub root: PathBuf,
    pub run: &'a dyn Fn(&[&str]) -> Option<String>,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// The account's home folder.
    pub home: PathBuf,
    /// When, as the report says it ("2026-11-26 15:30 -07:00").
    pub now: String,
}

impl Machine<'_> {
    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.root.join(path.trim_start_matches('/'))).map(|s| s.trim().to_string()).unwrap_or_default()
    }

    fn sh(&self, argv: &[&str]) -> String {
        (self.run)(argv).unwrap_or_else(|| format!("(not available: {})\n", argv.join(" ")))
    }
}

/// One file of the zip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub name: String,
    pub data: Vec<u8>,
}

/// The names to take out on this computer: its name, the people's accounts (name and full name), the saved networks and
/// the paired Bluetooth devices.
pub fn personal(m: &Machine) -> Personal {
    let mut p = Personal::default();
    p.add(&m.read("/etc/hostname"), "<computer>");
    p.add(&m.read("/proc/sys/kernel/hostname"), "<computer>");
    for line in m.read("/etc/passwd").lines() {
        let f: Vec<&str> = line.split(':').collect();
        let uid: u32 = f.get(2).and_then(|u| u.parse().ok()).unwrap_or(0);
        if f.len() >= 5 && (1000..60000).contains(&uid) {
            p.add(f[0], "<user>");
            p.add(f[4].split(',').next().unwrap_or(""), "<name>");
        }
    }
    if let Some(user) = (m.env)("USER") {
        p.add(&user, "<user>");
    }
    for line in (m.run)(&["nmcli", "-t", "-f", "NAME,TYPE", "connection", "show"]).unwrap_or_default().lines() {
        if let Some((name, kind)) = line.rsplit_once(':') {
            if !matches!(kind, "loopback" | "bridge") {
                p.add(&name.replace("\\:", ":"), "<network>");
            }
        }
    }
    for line in (m.run)(&["bluetoothctl", "devices"]).unwrap_or_default().lines() {
        // "Device AA:BB:CC:DD:EE:FF Name of it"
        if let Some(name) = line.strip_prefix("Device ").and_then(|r| r.split_once(' ')).map(|(_, n)| n) {
            p.add(name, "<device>");
        }
    }
    p
}

/// Everything the zip holds (names inside its folder), every text scrubbed.
pub fn gather(req: &Request, m: &Machine, support: Option<&Support>) -> Vec<File> {
    let p = personal(m);
    let text = |name: &str, body: String| File { name: name.to_string(), data: scrub::scrub(&body, &p).into_bytes() };
    let mut files = vec![
        text("report.txt", summary(req, m, support)),
        text("device.txt", device(m)),
        text("logs/this-start.txt", tail(m.sh(&["journalctl", "-b", "0", "-p", "warning", "-o", "short-iso", "--no-pager", "-n", "3000"]))),
        text("logs/last-start.txt", tail(m.sh(&["journalctl", "-b", "-1", "-p", "err", "-o", "short-iso", "--no-pager", "-n", "1000"]))),
        text("logs/crashes.txt", m.sh(&["coredumpctl", "list", "--no-pager", "--since", "-7d"])),
    ];
    for (name, body) in session_logs(m) {
        files.push(text(&format!("logs/{name}"), body));
    }
    for (title, argv) in support.map(|s| s.includes.as_slice()).unwrap_or(&[]) {
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let safe: String = title.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
        files.push(text(&format!("system/{safe}.txt"), m.sh(&args)));
    }
    for (i, pic) in req.pictures.iter().enumerate() {
        if let Ok(data) = std::fs::read(pic) {
            let ext = pic.extension().and_then(|e| e.to_str()).unwrap_or("png");
            files.push(File { name: format!("pictures/picture-{}.{ext}", i + 1), data });
        }
    }
    files
}

fn summary(req: &Request, m: &Machine, support: Option<&Support>) -> String {
    let os = os_release(m);
    let get = |k: &str| os.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap_or_default();
    let env = |k: &str| (m.env)(k).unwrap_or_default();
    let session = if (m.env)("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        "Hyprland".to_string()
    } else if !env("XDG_CURRENT_DESKTOP").is_empty() {
        env("XDG_CURRENT_DESKTOP")
    } else {
        "not known".to_string()
    };
    format!(
        "Problem report{for_whom}\nMade: {now}\nAbout: {area}\n\nWhat happened:\n{text}\n\nSystem: {name} {version}{build}\nDesktop: Yggdrasil, session: {session} ({kind})\nKernel: {kernel}\nUp for: {up}\n",
        for_whom = support.filter(|s| !s.name.is_empty()).map(|s| format!(" for {}", s.name)).unwrap_or_default(),
        now = m.now,
        area = if req.area.is_empty() { "not said" } else { &req.area },
        text = req.text.trim(),
        name = if get("PRETTY_NAME").is_empty() { get("NAME") } else { get("PRETTY_NAME") },
        version = get("VERSION_ID"),
        build = if get("BUILD_ID").is_empty() { String::new() } else { format!(" (build {})", get("BUILD_ID")) },
        kind = env("XDG_SESSION_TYPE"),
        kernel = m.read("/proc/sys/kernel/osrelease"),
        up = m.read("/proc/uptime").split_whitespace().next().and_then(|s| s.parse::<f64>().ok()).map(|s| format!("{:.0} minutes", s / 60.0)).unwrap_or_default(),
    )
}

fn os_release(m: &Machine) -> Vec<(String, String)> {
    let text = Some(m.read("/etc/os-release")).filter(|t| !t.is_empty()).unwrap_or_else(|| m.read("/usr/lib/os-release"));
    text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches('"').to_string())).collect()
}

/// The operating system's id ("labyrinth"), for the zip's name.
pub fn os_id(m: &Machine) -> String {
    let id = os_release(m).into_iter().find(|(k, _)| k == "ID").map(|(_, v)| v).unwrap_or_default();
    let id: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    if id.is_empty() {
        "system".into()
    } else {
        id
    }
}

/// The device: make and model, processor, memory, graphics cards (driver, memory windows, link), screens, disks, sound,
/// network devices (their kind and state only).
fn device(m: &Machine) -> String {
    let dmi = |f: &str| m.read(&format!("/sys/class/dmi/id/{f}"));
    let mut s = String::new();
    let mut line = |k: &str, v: String| {
        if !v.is_empty() {
            s.push_str(&format!("{k}: {v}\n"));
        }
    };
    line("Make", dmi("sys_vendor"));
    line(
        "Model",
        [dmi("product_name"), dmi("product_version")]
            .into_iter()
            .filter(|x| !x.is_empty() && x != "To Be Filled By O.E.M.")
            .collect::<Vec<_>>()
            .join(" "),
    );
    line("Board", [dmi("board_vendor"), dmi("board_name")].join(" ").trim().to_string());
    line("Firmware", [dmi("bios_vendor"), dmi("bios_version"), dmi("bios_date")].join(" ").trim().to_string());
    line("Kind", chassis(&dmi("chassis_type")).to_string());
    let cpuinfo = m.read("/proc/cpuinfo");
    let model = cpuinfo.lines().find_map(|l| l.strip_prefix("model name").map(|r| r.trim_start_matches([' ', '\t', ':']).to_string()));
    let threads = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
    line("Processor", model.map(|m| format!("{m} ({threads} threads)")).unwrap_or_default());
    let meminfo = m.read("/proc/meminfo");
    let kib = |k: &str| {
        meminfo.lines().find_map(|l| l.strip_prefix(k)).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse::<u64>().ok())
    };
    line("Memory", kib("MemTotal:").map(|k| format!("{:.1} GiB", k as f64 / 1048576.0)).unwrap_or_default());
    line("Swap", kib("SwapTotal:").map(|k| format!("{:.1} GiB", k as f64 / 1048576.0)).unwrap_or_default());
    s.push_str("\nGraphics:\n");
    s.push_str(&graphics(m));
    s.push_str("\nScreens:\n");
    s.push_str(&screens(m));
    s.push_str("\nDisks:\n");
    s.push_str(&m.sh(&["lsblk", "-d", "-o", "NAME,MODEL,SIZE,ROTA,TRAN,TYPE"]));
    s.push_str(&m.sh(&["df", "-hT", "-x", "tmpfs", "-x", "devtmpfs", "-x", "efivarfs", "-x", "squashfs", "-x", "overlay"]));
    s.push_str("\nSound:\n");
    s.push_str(&m.sh(&["pactl", "info"]));
    s.push_str(&m.sh(&["pactl", "list", "short", "sinks"]));
    s.push_str(&m.sh(&["pactl", "list", "short", "sources"]));
    s.push_str("\nNetwork devices:\n");
    s.push_str(&m.sh(&["nmcli", "-t", "-f", "DEVICE,TYPE,STATE", "device"]));
    s
}

fn chassis(t: &str) -> &'static str {
    match t.parse::<u32>().unwrap_or(0) {
        3..=7 | 13 | 15 | 16 | 24 => "desktop",
        8..=12 | 14 | 30..=32 => "laptop",
        17 | 23 | 28 | 29 => "server",
        _ => "",
    }
}

/// Every display controller (PCI class 0x03): its maker and name from pci.ids, the driver in use, NVIDIA's driver version,
/// the memory windows (a window as large as the card's memory is Resizable BAR at work) and the PCIe link.
fn graphics(m: &Machine) -> String {
    let ids = m.read("/usr/share/hwdata/pci.ids");
    let mut out = String::new();
    let base = m.root.join("sys/bus/pci/devices");
    let mut devs: Vec<PathBuf> = std::fs::read_dir(&base).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    devs.sort();
    for d in devs {
        let get = |f: &str| std::fs::read_to_string(d.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
        if !get("class").starts_with("0x03") {
            continue;
        }
        let (vendor, device) = (get("vendor"), get("device"));
        let driver = std::fs::read_link(d.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "none".into());
        let (vname, dname) = pci_name(&ids, &vendor, &device);
        out.push_str(&format!(
            "- {vname} {dname} [{}:{}], driver {driver}",
            vendor.trim_start_matches("0x"),
            device.trim_start_matches("0x")
        ));
        if get("boot_vga") == "1" {
            out.push_str(", the screen at start");
        }
        out.push('\n');
        let windows: Vec<String> = get("resource")
            .lines()
            .take(6)
            .filter_map(|l| {
                let f: Vec<u64> = l.split_whitespace().filter_map(|x| u64::from_str_radix(x.trim_start_matches("0x"), 16).ok()).collect();
                (f.len() >= 2 && f[1] > f[0]).then(|| human(f[1] - f[0] + 1))
            })
            .collect();
        if !windows.is_empty() {
            out.push_str(&format!("  memory windows (BARs): {}\n", windows.join(", ")));
        }
        let speed = get("current_link_speed");
        if !speed.is_empty() {
            out.push_str(&format!(
                "  PCIe link: {} x{} (can do {} x{})\n",
                speed,
                get("current_link_width"),
                get("max_link_speed"),
                get("max_link_width")
            ));
        }
    }
    let nvidia = m.read("/proc/driver/nvidia/version");
    if let Some(l) = nvidia.lines().next() {
        out.push_str(&format!("NVIDIA driver: {l}\n"));
    }
    if out.is_empty() {
        out.push_str("(no graphics card found)\n");
    }
    out
}

/// A vendor's and a device's names from pci.ids (the numbers when it does not know them).
fn pci_name(ids: &str, vendor: &str, device: &str) -> (String, String) {
    let v = vendor.trim_start_matches("0x").to_ascii_lowercase();
    let d = device.trim_start_matches("0x").to_ascii_lowercase();
    let mut vname = None;
    for line in ids.lines() {
        if vname.is_none() {
            if let Some(rest) = line.strip_prefix(&v).filter(|r| r.starts_with("  ")) {
                if !line.starts_with('\t') {
                    vname = Some(rest.trim().to_string());
                }
            }
        } else if !line.starts_with('\t') && !line.starts_with('#') && !line.is_empty() {
            break;
        } else if let Some(rest) = line.strip_prefix('\t').and_then(|l| l.strip_prefix(&d)).filter(|r| r.starts_with("  ")) {
            return (vname.unwrap_or_default(), rest.trim().to_string());
        }
    }
    (vname.unwrap_or_else(|| format!("vendor {v}")), format!("device {d}"))
}

/// Every connected screen: its connector, the monitor's maker and name from its EDID (never its serial number) and its
/// preferred mode.
fn screens(m: &Machine) -> String {
    let base = m.root.join("sys/class/drm");
    let mut conns: Vec<PathBuf> = std::fs::read_dir(&base)
        .map(|r| r.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().contains('-'))).collect())
        .unwrap_or_default();
    conns.sort();
    let mut out = String::new();
    for c in &conns {
        let get = |f: &str| std::fs::read_to_string(c.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
        if get("status") != "connected" {
            continue;
        }
        let name = c.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mode = get("modes").lines().next().unwrap_or("").to_string();
        let monitor = std::fs::read(c.join("edid")).ok().map(|e| edid_name(&e)).unwrap_or_default();
        // (not the connector's "enabled": NVIDIA's driver leaves it "disabled" on a screen in use)
        out.push_str(&format!("- {name}: {monitor}, {mode}\n"));
    }
    if out.is_empty() {
        out.push_str("(no connected screen found)\n");
    }
    out
}

/// "ACR XV345CUR" from an EDID: the three-letter maker and the monitor's name (descriptor 0xFC); never its serial (0xFF).
fn edid_name(e: &[u8]) -> String {
    if e.len() < 128 {
        return String::new();
    }
    let id = u16::from_be_bytes([e[8], e[9]]);
    let letter = |n: u16| char::from(b'A' - 1 + (n & 0x1f) as u8);
    let maker: String = [letter(id >> 10), letter(id >> 5), letter(id)].iter().collect();
    let name = (0..4)
        .map(|i| &e[54 + i * 18..72 + i * 18])
        .find(|d| d[0] == 0 && d[1] == 0 && d[3] == 0xfc)
        .map(|d| String::from_utf8_lossy(&d[5..18]).split('\n').next().unwrap_or("").trim().to_string())
        .unwrap_or_default();
    format!("{maker} {name}").trim().to_string()
}

/// The ends of the session's own logs: Yggdrasil's state folder's *.log files and Hyprland's log while it runs.
fn session_logs(m: &Machine) -> Vec<(String, String)> {
    let mut out = vec![];
    let state = (m.env)("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| m.home.join(".local/state")).join("yggdrasil");
    let mut logs: Vec<PathBuf> = std::fs::read_dir(&state)
        .map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "log")).collect())
        .unwrap_or_default();
    logs.sort();
    if let (Some(run), Some(sig)) = ((m.env)("XDG_RUNTIME_DIR"), (m.env)("HYPRLAND_INSTANCE_SIGNATURE")) {
        logs.push(Path::new(&run).join("hypr").join(sig).join("hyprland.log"));
    }
    for l in logs {
        if let Ok(data) = std::fs::read(&l) {
            let name = l.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push((format!("{name}.txt"), tail(String::from_utf8_lossy(&data).into_owned())));
        }
    }
    out
}

/// A log's last LOG_TAIL bytes, from the start of a line.
fn tail(s: String) -> String {
    if s.len() <= LOG_TAIL {
        return s;
    }
    let mut at = s.len() - LOG_TAIL;
    while !s.is_char_boundary(at) {
        at += 1;
    }
    let from = s[at..].find('\n').map_or(at, |n| at + n + 1);
    format!("(only the last part)\n{}", &s[from..])
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v.fract() == 0.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// The zip: every file inside one folder named `folder`, compressed.
pub fn zip(files: &[File], folder: &str) -> std::io::Result<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(0o644);
        for f in files {
            z.start_file(format!("{folder}/{}", f.name), opts).map_err(std::io::Error::other)?;
            z.write_all(&f.data)?;
        }
        z.finish().map_err(std::io::Error::other)?;
    }
    Ok(buf.into_inner())
}

/// Runs a command with nothing on its input and a time limit; its output, or None when it is missing, fails to start or
/// takes longer than `limit`.
pub fn run(argv: &[&str], limit: Duration) -> Option<String> {
    let (prog, args) = argv.split_first()?;
    let mut child = Command::new(prog).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut s);
        s
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok().map(|s| String::from_utf8_lossy(&s).into_owned())
}

/// Makes the report on this computer and saves it in the Downloads folder (else the home folder); its path.
pub fn make(req: &Request, support: Option<&Support>) -> Result<PathBuf, String> {
    let run_cmd = |argv: &[&str]| run(argv, Duration::from_secs(15));
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let home = PathBuf::from(std::env::var("HOME").map_err(|_| "no home folder".to_string())?);
    let now = jiff::Zoned::now();
    let m = Machine {
        root: PathBuf::from("/"),
        run: &run_cmd,
        env: &env,
        home: home.clone(),
        now: now.strftime("%Y-%m-%d %H:%M %:z").to_string(),
    };
    let folder = format!("{}-report-{}", os_id(&m), now.strftime("%Y-%m-%d-%H%M"));
    let data = zip(&gather(req, &m, support), &folder).map_err(|e| format!("the report could not be packed: {e}"))?;
    let downloads = run(&["xdg-user-dir", "DOWNLOAD"], Duration::from_secs(5))
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| p.is_dir() && *p != home)
        .unwrap_or_else(|| home.clone());
    let path = downloads.join(format!("{folder}.zip"));
    std::fs::write(&path, data).map_err(|e| format!("the report could not be saved in {}: {e}", downloads.display()))?;
    Ok(path)
}

/// How Email it went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mail {
    /// A mail program opened a new message with the report attached.
    Attached,
    /// The mail program (often a browser's webmail) opened a new message; the person attaches the file, which is shown.
    AttachYourself,
    /// Nothing handles e-mail here.
    NoMailApp,
}

/// Mail programs that take an attachment from `xdg-email --attach`; webmail in a browser does not.
const ATTACHES: [&str; 7] = ["thunderbird", "betterbird", "evolution", "kmail", "kontact", "claws", "geary"];

/// Opens a new message to `to` with the report: attached where the mail program takes it, else with a line asking to attach
/// it, and the file shown in the file manager.
pub fn email(to: &str, report: &Path, subject: &str, body: &str) -> Mail {
    let handler =
        run(&["xdg-mime", "query", "default", "x-scheme-handler/mailto"], Duration::from_secs(5)).unwrap_or_default().to_lowercase();
    let file = report.to_string_lossy();
    if ATTACHES.iter().any(|c| handler.contains(c))
        && Command::new("xdg-email").args(["--attach", &file, "--subject", subject, "--body", body, to]).spawn().is_ok()
    {
        return Mail::Attached;
    }
    if handler.trim().is_empty() {
        return Mail::NoMailApp;
    }
    let name = report.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let body = format!("{body}\n\nPlease attach the report file {name} (in the folder that just opened) before sending.");
    let url = format!("mailto:{to}?subject={}&body={}", encode(subject), encode(&body));
    if Command::new("xdg-open").arg(url).spawn().is_err() {
        return Mail::NoMailApp;
    }
    show(report);
    Mail::AttachYourself
}

/// Shows the report's folder in the file manager.
pub fn show(report: &Path) {
    let dir = report.parent().unwrap_or(report);
    if Command::new("minos").arg(dir).spawn().is_err() {
        let _ = Command::new("xdg-open").arg(dir).spawn();
    }
}

/// Percent-encoding for a mailto: link.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests;
