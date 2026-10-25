//! yggdrasil-installer: installs the operating system from its live medium. The window is Yggdrasil's (the same look as Settings
//! and Minos, no other toolkit); the work is the OS's own program, named by an integration file of kind "installer":
//!
//!     kind = installer
//!     command = /usr/local/bin/labyrinth-install-system
//!
//! It is run as root (pkexec), gets the choices as one JSON object on its standard input (so no password is ever on a command
//! line), answers `--disks` with the disks it could install onto, and reports with one line each: `STEP n N text`, `INFO`,
//! `WARN`, `ERROR text`, `DONE`. See docs/INSTALLER.md.

mod partitions;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, vec2, Align, CornerRadius, Layout, Margin, RichText};
use yggdrasil_ui::chrome;
use yggdrasil_ui::theme::{self, Palette};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Welcome,
    Keyboard,
    TimeZone,
    Disk,
    Account,
    Summary,
    Installing,
    Done,
}

const PAGES: [Page; 6] = [Page::Welcome, Page::Keyboard, Page::TimeZone, Page::Disk, Page::Account, Page::Summary];

#[derive(Clone, Debug, PartialEq)]
struct Disk {
    path: String,
    size: u64,
    model: String,
    removable: bool,
    big_enough: bool,
    /// gpt, dos, or "" (no partition table)
    table: String,
    parts: Vec<partitions::Part>,
}

/// What the install program said, line by line.
#[derive(Clone, Debug, PartialEq)]
enum Report {
    Step(usize, usize, String),
    Info(String),
    Warn(String),
    Error(String),
    Done,
}

fn parse_report(line: &str) -> Option<Report> {
    let (kind, rest) = line.split_once(' ').unwrap_or((line, ""));
    match kind {
        "STEP" => {
            let mut it = rest.splitn(3, ' ');
            let n = it.next()?.parse().ok()?;
            let total = it.next()?.parse().ok()?;
            Some(Report::Step(n, total, it.next().unwrap_or("").to_string()))
        }
        "INFO" => Some(Report::Info(rest.to_string())),
        "WARN" => Some(Report::Warn(rest.to_string())),
        "ERROR" => Some(Report::Error(rest.to_string())),
        "DONE" => Some(Report::Done),
        _ => None,
    }
}

/// A login name from a full name: "Ada Lovelace" -> "ada".
fn login_from(name: &str) -> String {
    name.split_whitespace().next().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase()
}

fn login_ok(l: &str) -> bool {
    let mut c = l.chars();
    matches!(c.next(), Some('a'..='z' | '_')) && l.len() <= 32 && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn hostname_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 63 && !h.starts_with('-') && !h.ends_with('-') && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn size_text(b: u64) -> String {
    let gb = b as f64 / 1e9;
    if gb >= 1000.0 {
        format!("{:.1} TB", gb / 1000.0)
    } else {
        format!("{gb:.0} GB")
    }
}

fn run_lines(cmd: &str, args: &[&str]) -> Vec<String> {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect()).unwrap_or_default()
}

/// Readable names for the common languages (the rest show their code).
const COMMON: [(&str, &str); 16] = [
    ("en_US.UTF-8", "English (United States)"), ("en_GB.UTF-8", "English (United Kingdom)"), ("en_CA.UTF-8", "English (Canada)"),
    ("en_AU.UTF-8", "English (Australia)"), ("de_DE.UTF-8", "Deutsch"), ("fr_FR.UTF-8", "Français"), ("es_ES.UTF-8", "Español (España)"),
    ("es_MX.UTF-8", "Español (México)"), ("it_IT.UTF-8", "Italiano"), ("nl_NL.UTF-8", "Nederlands"), ("pt_BR.UTF-8", "Português (Brasil)"),
    ("pt_PT.UTF-8", "Português (Portugal)"), ("sv_SE.UTF-8", "Svenska"), ("pl_PL.UTF-8", "Polski"), ("ja_JP.UTF-8", "日本語"),
    ("zh_CN.UTF-8", "中文 (简体)"),
];

/// How a language shows in the list: its name for the common ones, else its code.
fn locale_label(code: &str) -> String {
    COMMON.iter().find(|(c, _)| *c == code).map(|(_, n)| format!("{n}   ({code})")).unwrap_or_else(|| code.to_string())
}

/// UTF-8 languages the system can set up (glibc's list), the common ones first.
fn locales() -> Vec<String> {
    let mut all: Vec<String> = std::fs::read_to_string("/usr/share/i18n/SUPPORTED")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|l| l.ends_with(".UTF-8"))
        .map(str::to_string)
        .collect();
    if all.is_empty() {
        all = vec!["en_US.UTF-8".into()];
    }
    let mut out: Vec<String> = COMMON.iter().map(|(c, _)| c.to_string()).filter(|c| all.contains(c)).collect();
    out.extend(all.into_iter().filter(|c| !COMMON.iter().any(|(k, _)| k == c)));
    out
}

fn keymaps() -> Vec<String> {
    let mut v = run_lines("localectl", &["list-x11-keymap-layouts"]);
    if v.is_empty() {
        v = ["us", "gb", "de", "fr", "es", "it", "se", "no", "dk", "fi", "nl", "pt", "br", "ca", "ch", "pl", "cz", "ru", "jp"].iter().map(|s| s.to_string()).collect();
    }
    v
}

/// How the letters are arranged: what people know a keyboard by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum KbType {
    Qwerty,
    Qwertz,
    Azerty,
    Dvorak,
    Colemak,
    Other,
}

const KB_TYPES: [(KbType, &str, &str); 6] = [
    (KbType::Qwerty, "QWERTY", "Most keyboards"),
    (KbType::Qwertz, "QWERTZ", "German, Swiss, Czech, Hungarian…"),
    (KbType::Azerty, "AZERTY", "French, Belgian"),
    (KbType::Dvorak, "Dvorak", "Letters arranged for less finger travel"),
    (KbType::Colemak, "Colemak", "QWERTY with the most-used letters moved home"),
    (KbType::Other, "Other alphabets", "Cyrillic, Greek, Arabic, Hebrew, Indic, Thai…"),
];

/// One keyboard to choose: an XKB layout, maybe with a variant (Dvorak and Colemak are variants of a layout).
#[derive(Clone, Debug, PartialEq)]
struct KbChoice {
    layout: String,
    variant: String,
    name: String,
    kind: KbType,
}

impl KbChoice {
    fn key(&self) -> String {
        format!("{}|{}", self.layout, self.variant)
    }
    fn label(&self) -> String {
        if self.variant.is_empty() { format!("{}   ({})", self.name, self.layout) } else { format!("{}   ({}, {})", self.name, self.layout, self.variant) }
    }
}

/// The arrangement a layout's letters follow (its default variant).
fn layout_kind(code: &str) -> KbType {
    const AZERTY: [&str; 2] = ["fr", "be"];
    const QWERTZ: [&str; 11] = ["de", "at", "ch", "cz", "sk", "hu", "si", "hr", "ba", "li", "lu"];
    const OTHER_SCRIPTS: [&str; 37] = ["ru", "ua", "by", "bg", "mk", "rs", "mn", "kz", "kg", "tj", "uz", "gr", "il", "ara", "eg", "iq", "sy", "ir", "af", "pk", "in", "bd",
        "np", "lk", "th", "la", "kh", "mm", "bt", "ge", "am", "et", "er", "mv", "ma", "jo", "tm"];
    if AZERTY.contains(&code) {
        KbType::Azerty
    } else if QWERTZ.contains(&code) {
        KbType::Qwertz
    } else if OTHER_SCRIPTS.contains(&code) {
        KbType::Other
    } else {
        KbType::Qwerty
    }
}

/// Everything there is to choose, from the XKB list (base.lst): each layout, plus its Dvorak and Colemak variants and the two
/// international US ones many people type with. `available` (localectl's layouts) limits it when not empty.
fn keyboard_choices(lst: &str, available: &[String]) -> Vec<KbChoice> {
    let mut section = "";
    let mut names = std::collections::HashMap::new();
    let mut out = vec![];
    let ok = |l: &str| available.is_empty() || available.iter().any(|a| a == l);
    for line in lst.lines() {
        if let Some(h) = line.strip_prefix("! ") {
            section = if h.trim() == "layout" { "layout" } else if h.trim() == "variant" { "variant" } else { "" };
            continue;
        }
        let Some((code, rest)) = line.trim().split_once(char::is_whitespace) else { continue };
        let rest = rest.trim();
        match section {
            "layout" if ok(code) => {
                names.insert(code.to_string(), rest.to_string());
                out.push(KbChoice { layout: code.into(), variant: String::new(), name: rest.into(), kind: layout_kind(code) });
            }
            "variant" => {
                let Some((layout, name)) = rest.split_once(": ") else { continue };
                if !ok(layout) {
                    continue;
                }
                let kind = if name.contains("Dvorak") {
                    KbType::Dvorak
                } else if name.contains("Colemak") {
                    KbType::Colemak
                } else if layout == "us" && (code == "intl" || code == "altgr-intl") {
                    KbType::Qwerty
                } else {
                    continue;
                };
                out.push(KbChoice { layout: layout.into(), variant: code.into(), name: name.into(), kind });
            }
            _ => {}
        }
    }
    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

/// The live session types with this keyboard from now on (Hyprland), so the box on the page tries the layout chosen.
fn try_keyboard(c: &KbChoice) {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    let lua = format!("hl.config({{ input = {{ kb_layout = {:?}, kb_variant = {:?} }} }})", c.layout, c.variant);
    let _ = std::process::Command::new("hyprctl").args(["eval", &lua]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

/// The time zones a person would pick: Region/City ones and UTC (not the old aliases such as "W-SU", "Zulu" or "US/Pacific").
fn timezones() -> Vec<String> {
    let mut v: Vec<String> = run_lines("timedatectl", &["list-timezones"]).into_iter().filter(|z| real_zone(z)).collect();
    v.sort_by_key(|z| zone_label(z).to_lowercase());
    v
}

fn real_zone(z: &str) -> bool {
    const AREAS: [&str; 10] = ["Africa", "America", "Antarctica", "Arctic", "Asia", "Atlantic", "Australia", "Europe", "Indian", "Pacific"];
    z == "UTC" || z.split_once('/').map(|(area, rest)| AREAS.contains(&area) && !rest.is_empty()).unwrap_or(false)
}

/// "America/Argentina/Buenos_Aires" -> "Buenos Aires (America, Argentina)"
fn zone_label(z: &str) -> String {
    let parts: Vec<&str> = z.split('/').collect();
    if parts.len() < 2 {
        return z.replace('_', " ");
    }
    format!("{} ({})", parts[parts.len() - 1].replace('_', " "), parts[..parts.len() - 1].join(", ").replace('_', " "))
}

/// The live system's own time zone (what the medium was set to), as the first guess. No location service is asked.
fn current_timezone() -> String {
    std::fs::read_link("/etc/localtime").ok().and_then(|p| p.to_string_lossy().split("zoneinfo/").nth(1).map(str::to_string)).unwrap_or_else(|| "UTC".into())
}

fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("NAME=").map(|v| v.trim_matches('"').to_string()))
        .unwrap_or_else(|| "the system".into())
}

struct App {
    page: Page,
    command: Option<Vec<String>>,
    os: String,
    locales: Vec<String>,
    locale: String,
    kb_choices: Vec<KbChoice>,
    kb_type: KbType,
    keymap: String,
    keymap_variant: String,
    test_text: String,
    zones: Vec<String>,
    zone: String,
    zone_filter: String,
    disks: Vec<Disk>,
    disk: Option<String>,
    /// Partitions chosen by hand instead of erasing the whole disk.
    manual_mode: bool,
    manual: partitions::Manual,
    disk_error: Option<String>,
    encrypt: bool,
    crypt_pw: String,
    crypt_pw2: String,
    name: String,
    login: String,
    login_touched: bool,
    hostname: String,
    pw: String,
    pw2: String,
    agree: bool,
    reports: Option<Receiver<Report>>,
    step: (usize, usize, String),
    log: Vec<Report>,
    failed: Option<String>,
    /// Screenshots only (`--demo PAGE`): sample disks and names, and nothing can be installed.
    demo: bool,
}

/// The sample computer of `--demo`: a 1 TB disk with Windows on it and an empty 512 GB one.
fn demo_disks() -> Vec<Disk> {
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    let part = |n: u32, start: u64, size: u64, fs: &str, label: &str, efi: bool| partitions::Part {
        path: format!("/dev/nvme0n1p{n}"),
        number: n,
        start,
        size,
        fstype: fs.into(),
        label: label.into(),
        efi,
        mounted: false,
    };
    vec![
        Disk {
            path: "/dev/nvme0n1".into(),
            size: 1_000_204_886_016,
            model: "Sample NVMe 1TB".into(),
            removable: false,
            big_enough: true,
            table: "gpt".into(),
            parts: vec![
                part(1, MIB, 300 * MIB, "vfat", "SYSTEM", true),
                part(2, 301 * MIB, 16 * MIB, "", "", false),
                part(3, 317 * MIB, 400 * GIB, "ntfs", "Windows", false),
                part(4, 317 * MIB + 400 * GIB, 750 * MIB, "ntfs", "Recovery", false),
            ],
        },
        Disk { path: "/dev/sda".into(), size: 512_110_190_592, model: "Sample SSD 512GB".into(), removable: false, big_enough: true, table: String::new(), parts: vec![] },
    ]
}

impl App {
    /// `--demo welcome|keyboard|timezone|disk|disk-manual|account|summary`: a page filled with sample choices, for screenshots.
    fn demo(&mut self, what: &str) {
        self.demo = true;
        self.disks = demo_disks();
        self.disk = Some("/dev/nvme0n1".into());
        self.crypt_pw = "sample-passphrase".into();
        self.crypt_pw2 = self.crypt_pw.clone();
        self.name = "Ada Lovelace".into();
        self.login = "ada".into();
        self.login_touched = true;
        self.hostname = "ada-desktop".into();
        self.pw = "sample-password".into();
        self.pw2 = self.pw.clone();
        self.zone = "America/Denver".into();
        if what == "disk-manual" || what == "summary" {
            let d = &self.disks[0];
            let end = d.parts.iter().map(|p| p.start + p.size).max().unwrap_or(0);
            let free = (d.size - end - (1 << 20)) / (1 << 20) * (1 << 20);
            self.manual = partitions::Manual::demo(&d.parts, end, free);
            self.manual_mode = true;
        }
        self.page = match what {
            "keyboard" => Page::Keyboard,
            "timezone" => Page::TimeZone,
            "disk" | "disk-manual" => Page::Disk,
            "account" => Page::Account,
            "summary" => Page::Summary,
            _ => Page::Welcome,
        };
    }

    fn new() -> App {
        let command = yggdrasil_conf::integrations::first("installer").and_then(|i| i.command("command"));
        let os = os_name();
        let mut app = App {
            page: Page::Welcome,
            command,
            hostname: os.to_lowercase().split_whitespace().next().unwrap_or("computer").chars().filter(|c| c.is_ascii_alphanumeric()).collect(),
            os,
            locales: locales(),
            locale: "en_US.UTF-8".into(),
            kb_choices: keyboard_choices(&std::fs::read_to_string("/usr/share/X11/xkb/rules/base.lst").unwrap_or_default(), &keymaps()),
            kb_type: KbType::Qwerty,
            keymap: "us".into(),
            keymap_variant: String::new(),
            test_text: String::new(),
            zones: timezones(),
            zone: current_timezone(),
            zone_filter: String::new(),
            disks: vec![],
            disk: None,
            manual_mode: false,
            manual: partitions::Manual::default(),
            disk_error: None,
            encrypt: true,
            crypt_pw: String::new(),
            crypt_pw2: String::new(),
            name: String::new(),
            login: String::new(),
            login_touched: false,
            pw: String::new(),
            pw2: String::new(),
            agree: false,
            reports: None,
            step: (0, 0, String::new()),
            log: vec![],
            failed: None,
            demo: false,
        };
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--demo") {
            app.command = None;
            app.demo(args.get(i + 1).map(String::as_str).unwrap_or("welcome"));
            return app;
        }
        app.load_disks();
        app
    }

    fn load_disks(&mut self) {
        let Some(cmd) = &self.command else { return };
        let out = Command::new(&cmd[0]).args(&cmd[1..]).arg("--disks").output();
        match out.ok().and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok()) {
            Some(v) => {
                self.disks = v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|d| Disk {
                                path: d["path"].as_str().unwrap_or("").to_string(),
                                size: d["size"].as_u64().unwrap_or(0),
                                model: d["model"].as_str().unwrap_or("").to_string(),
                                removable: d["removable"].as_bool().unwrap_or(false),
                                big_enough: d["big_enough"].as_bool().unwrap_or(false),
                                table: d["table"].as_str().unwrap_or("").to_string(),
                                parts: d["partitions"]
                                    .as_array()
                                    .map(|a| {
                                        a.iter()
                                            .map(|p| partitions::Part {
                                                path: p["path"].as_str().unwrap_or("").to_string(),
                                                number: p["number"].as_u64().unwrap_or(0) as u32,
                                                start: p["start"].as_u64().unwrap_or(0),
                                                size: p["size"].as_u64().unwrap_or(0),
                                                fstype: p["fstype"].as_str().unwrap_or("").to_string(),
                                                label: p["label"].as_str().unwrap_or("").to_string(),
                                                efi: p["efi"].as_bool().unwrap_or(false),
                                                mounted: p["mounted"].as_bool().unwrap_or(false),
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.disk_error = None;
            }
            None => self.disk_error = Some("The disks could not be listed".into()),
        }
    }

    /// What is still missing on a page before Next (None: it can go on).
    fn chosen_disk(&self) -> Option<&Disk> {
        self.disks.iter().find(|d| Some(&d.path) == self.disk.as_ref())
    }

    fn missing(&self, page: Page) -> Option<String> {
        match page {
            Page::Disk => {
                if self.disk.is_none() {
                    Some("Choose a disk".into())
                } else if let Some(p) = self.chosen_disk().filter(|_| self.manual_mode).and_then(|d| self.manual.problem(d.size, &d.parts, &d.table)) {
                    Some(p)
                } else if self.encrypt && self.crypt_pw.chars().count() < 8 {
                    Some("The encryption password needs at least 8 characters".into())
                } else if self.encrypt && self.crypt_pw != self.crypt_pw2 {
                    Some("The two encryption passwords are not the same".into())
                } else {
                    None
                }
            }
            Page::Account => {
                if self.name.trim().is_empty() {
                    Some("Type your name".into())
                } else if !login_ok(&self.login) {
                    Some("The account name must start with a small letter (letters, digits, - and _)".into())
                } else if !hostname_ok(&self.hostname) {
                    Some("The computer's name may use letters, digits and -".into())
                } else if self.pw.is_empty() {
                    Some("Choose a password".into())
                } else if self.pw != self.pw2 {
                    Some("The two passwords are not the same".into())
                } else {
                    None
                }
            }
            Page::Summary if !self.agree => Some("Tick the box to confirm the changes to the disk".into()),
            _ => None,
        }
    }

    fn plan(&self) -> serde_json::Value {
        let mut plan = serde_json::json!({
            "disk": self.disk.clone().unwrap_or_default(),
            "mode": "erase",
            "encrypt": self.encrypt,
            "encrypt_password": if self.encrypt { self.crypt_pw.clone() } else { String::new() },
            "user": {"name": self.name.trim(), "login": self.login, "password": self.pw},
            "hostname": self.hostname,
            "timezone": self.zone,
            "locale": self.locale,
            "keymap": self.keymap,
            "keymap_variant": self.keymap_variant,
        });
        if self.manual_mode {
            if let (Some(d), Some(obj)) = (self.chosen_disk(), plan.as_object_mut()) {
                if let Some(m) = self.manual.plan(&d.parts).as_object() {
                    for (k, v) in m {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        plan
    }

    fn start_install(&mut self, ctx: &egui::Context) {
        if self.demo {
            return;
        }
        let Some(cmd) = self.command.clone() else { return };
        let plan = self.plan().to_string();
        let (tx, rx) = channel();
        self.reports = Some(rx);
        self.page = Page::Installing;
        let c = ctx.clone();
        std::thread::spawn(move || {
            // pkexec asks for the administrator password where there is one (the live medium lets it through)
            let child = Command::new("pkexec").args(&cmd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Report::Error(format!("the installer could not start: {e}")));
                    return;
                }
            };
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(plan.as_bytes());
            }
            let mut ended = false;
            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(r) = parse_report(&line) {
                        ended |= matches!(r, Report::Done | Report::Error(_));
                        let _ = tx.send(r);
                        c.request_repaint();
                    }
                }
            }
            let status = child.wait();
            if !ended {
                let _ = tx.send(Report::Error(match status {
                    Ok(s) if s.code() == Some(126) || s.code() == Some(127) => "administrator rights were not given".into(),
                    Ok(s) => format!("the installer stopped without saying why ({s})"),
                    Err(e) => e.to_string(),
                }));
            }
            c.request_repaint();
        });
    }

    fn poll(&mut self) {
        let Some(rx) = &self.reports else { return };
        while let Ok(r) = rx.try_recv() {
            match &r {
                Report::Step(n, total, text) => self.step = (*n, *total, text.clone()),
                Report::Error(e) => self.failed = Some(e.clone()),
                Report::Done => self.page = Page::Done,
                _ => {}
            }
            self.log.push(r);
        }
    }
}

fn heading(ui: &mut egui::Ui, pal: &Palette, title: &str, sub: &str) {
    ui.label(RichText::new(title).size(24.0).strong().color(pal.text));
    ui.add_space(2.0);
    ui.label(RichText::new(sub).color(pal.dim));
    ui.add_space(14.0);
}

fn field(ui: &mut egui::Ui, text: &mut String, hint: &str, secret: bool) -> egui::Response {
    ui.add(egui::TextEdit::singleline(text).hint_text(hint).password(secret).desired_width(360.0).margin(vec2(8.0, 6.0)))
}

/// A list to pick one line from (shown through `label`), the chosen one highlighted and scrolled into view when the list opens.
impl App {
    /// The keyboard chosen now.
    fn kb_chosen(&self) -> Option<&KbChoice> {
        self.kb_choices.iter().find(|c| c.layout == self.keymap && c.variant == self.keymap_variant)
    }

    fn choose_keyboard(&mut self, c: KbChoice) {
        try_keyboard(&c);
        self.keymap = c.layout;
        self.keymap_variant = c.variant;
    }
}

fn pick_list(ui: &mut egui::Ui, pal: &Palette, id: &str, items: &[String], chosen: &mut String, height: f32, label: impl Fn(&str) -> String) {
    let seen = egui::Id::new(("pick-list-shown", id));
    let first = !ui.ctx().data(|d| d.get_temp::<bool>(seen).unwrap_or(false));
    egui::ScrollArea::vertical().id_salt(id).max_height(height).show(ui, |ui| {
        for it in items {
            let on = it == chosen;
            let r = ui.add(egui::Button::new(RichText::new(label(it)).color(if on { pal.on_accent() } else { pal.text })).fill(if on { pal.accent } else { egui::Color32::TRANSPARENT }).min_size(vec2(360.0, 26.0)));
            if r.clicked() {
                *chosen = it.clone();
            }
            if on && first {
                r.scroll_to_me(Some(Align::Center));
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(seen, true));
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        theme::apply(&ctx, true, accent);
        let pal = theme::palette_for(true, accent);
        self.poll();
        let mut go: Option<Page> = None;
        let mut install = false;

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 28, right: 12, top: 8, bottom: 18 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Install {}", self.os)).color(pal.dim));
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                if !matches!(self.page, Page::Installing) {
                    chrome::window_controls(ui, &pal);
                }
            });
            // where in the steps we are
            if let Some(i) = PAGES.iter().position(|p| *p == self.page) {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    for (k, _) in PAGES.iter().enumerate() {
                        let c = if k <= i { pal.accent } else { pal.border };
                        let (r, _) = ui.allocate_exact_size(vec2(44.0, 4.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, CornerRadius::same(2), c);
                    }
                });
            }
            ui.add_space(16.0);
            let footer = 54.0;
            let body_h = ui.available_height() - footer;
            ui.allocate_ui(vec2(ui.available_width() - 16.0, body_h), |ui| {
                ui.set_min_height(body_h);
                // a long page (manual partitions, the summary of them) scrolls; the buttons below stay where they are
                egui::ScrollArea::vertical().id_salt(("page", self.page as u8)).max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
                match self.page {
                    Page::Welcome => {
                        heading(ui, &pal, &format!("Welcome to {}", self.os), "This puts it on this computer's disk. It takes a few minutes; nothing is sent anywhere.");
                        if self.command.is_none() && !self.demo {
                            ui.label(RichText::new("This system has no installer (no integration of kind \"installer\").").color(pal.red));
                        }
                        ui.label("Language");
                        let mut loc = self.locale.clone();
                        pick_list(ui, &pal, "locales", &self.locales, &mut loc, (body_h - 140.0).max(120.0), locale_label);
                        self.locale = loc;
                    }
                    Page::Keyboard => {
                        heading(ui, &pal, "Keyboard", "The layout of your keyboard. Type in the box to try it.");
                        // the arrangement first, then the keyboards of that kind
                        let mut new_type = None;
                        ui.horizontal_wrapped(|ui| {
                            for (t, name, what) in KB_TYPES {
                                let on = self.kb_type == t;
                                let b = egui::Button::new(RichText::new(name).color(if on { pal.on_accent() } else { pal.text })).fill(if on { pal.accent } else { pal.field }).min_size(vec2(84.0, 30.0));
                                if ui.add(b).on_hover_text(what).clicked() {
                                    new_type = Some(t);
                                }
                            }
                        });
                        if let Some(t) = new_type {
                            self.kb_type = t;
                            if self.kb_chosen().map(|c| c.kind != t).unwrap_or(true) {
                                // the usual one of that kind, so something of it is chosen at once
                                let usual = match t { KbType::Qwerty => ("us", ""), KbType::Qwertz => ("de", ""), KbType::Azerty => ("fr", ""), KbType::Dvorak => ("us", "dvorak"), KbType::Colemak => ("us", "colemak"), KbType::Other => ("", "") };
                                let pick = self.kb_choices.iter().find(|c| c.kind == t && c.layout == usual.0 && c.variant == usual.1).or_else(|| self.kb_choices.iter().find(|c| c.kind == t)).cloned();
                                if let Some(c) = pick {
                                    self.choose_keyboard(c);
                                }
                            }
                        }
                        ui.add_space(6.0);
                        let shown: Vec<KbChoice> = self.kb_choices.iter().filter(|c| c.kind == self.kb_type).cloned().collect();
                        let keys: Vec<String> = shown.iter().map(KbChoice::key).collect();
                        let mut km = format!("{}|{}", self.keymap, self.keymap_variant);
                        pick_list(ui, &pal, "keymaps", &keys, &mut km, (body_h - 190.0).max(120.0), |k| shown.iter().find(|c| c.key() == k).map(KbChoice::label).unwrap_or_else(|| k.to_string()));
                        if km != format!("{}|{}", self.keymap, self.keymap_variant) {
                            if let Some(c) = shown.iter().find(|c| c.key() == km).cloned() {
                                self.choose_keyboard(c);
                            }
                        }
                        ui.add_space(8.0);
                        field(ui, &mut self.test_text, "Try your keyboard here", false);
                    }
                    Page::TimeZone => {
                        heading(ui, &pal, "Time zone", "Where the clock should be set for. Type a city to find it.");
                        field(ui, &mut self.zone_filter, "Search, e.g. Denver or Berlin", false);
                        ui.add_space(6.0);
                        let q = self.zone_filter.to_lowercase();
                        let shown: Vec<String> = self.zones.iter().filter(|z| q.is_empty() || zone_label(z).to_lowercase().contains(&q)).cloned().collect();
                        let mut z = self.zone.clone();
                        pick_list(ui, &pal, "zones", &shown, &mut z, (body_h - 150.0).max(120.0), zone_label);
                        self.zone = z;
                    }
                    Page::Disk => {
                        heading(ui, &pal, "Where to install", "Choose the disk, then how: erase all of it, or choose partitions yourself (beside Windows, for one).");
                        for d in self.disks.clone() {
                            let on = self.disk.as_deref() == Some(d.path.as_str());
                            let label = format!("{}   {}   {}{}", if d.model.is_empty() { "Disk" } else { &d.model }, size_text(d.size), d.path, if d.removable { "   (removable)" } else { "" });
                            let b = egui::Button::new(RichText::new(label).color(if on { pal.on_accent() } else { pal.text })).fill(if on { pal.accent } else { pal.field }).min_size(vec2(460.0, 34.0));
                            if ui.add_enabled(d.big_enough, b).on_disabled_hover_text("Too small: at least 30 GB are needed").clicked() {
                                if self.disk.as_deref() != Some(d.path.as_str()) {
                                    self.manual = partitions::Manual::default();
                                }
                                self.disk = Some(d.path.clone());
                            }
                            ui.add_space(4.0);
                        }
                        if self.disks.is_empty() {
                            ui.label(RichText::new(self.disk_error.clone().unwrap_or_else(|| "No disk found to install onto".into())).color(pal.amber));
                        }
                        if ui.button("Look again").clicked() {
                            self.load_disks();
                        }
                        if let Some(d) = self.chosen_disk().cloned() {
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                for (manual, name, what) in [(false, "Erase the disk", "Labyrinth gets the whole disk: everything on it is erased"), (true, "Choose partitions", "Make, delete and pick partitions yourself")] {
                                    let on = self.manual_mode == manual;
                                    let b = egui::Button::new(RichText::new(name).color(if on { pal.on_accent() } else { pal.text })).fill(if on { pal.accent } else { pal.field }).min_size(vec2(160.0, 32.0));
                                    if ui.add(b).on_hover_text(what).clicked() {
                                        self.manual_mode = manual;
                                    }
                                }
                            });
                            if self.manual_mode {
                                ui.add_space(8.0);
                                partitions::ui(ui, &pal, &mut self.manual, d.size, &d.parts, &d.table);
                            }
                        }
                        ui.add_space(16.0);
                        ui.checkbox(&mut self.encrypt, if self.manual_mode { "Encrypt the system partition (/, recommended)" } else { "Encrypt the disk (recommended)" });
                        ui.label(RichText::new("Without this password nobody can read the disk, even with the computer in hand. It is asked each time the computer starts. If you forget it, the data cannot be recovered.").color(pal.dim));
                        if self.encrypt {
                            ui.add_space(6.0);
                            field(ui, &mut self.crypt_pw, "Encryption password (at least 8 characters)", true);
                            field(ui, &mut self.crypt_pw2, "The same again", true);
                        }
                    }
                    Page::Account => {
                        heading(ui, &pal, "Your account", "You sign in with it and use it to change system settings (with sudo).");
                        ui.label("Your name");
                        if field(ui, &mut self.name, "e.g. Ada Lovelace", false).changed() && !self.login_touched {
                            self.login = login_from(&self.name);
                        }
                        ui.label("Account name");
                        if field(ui, &mut self.login, "e.g. ada", false).changed() {
                            self.login_touched = true;
                        }
                        ui.label("Computer's name");
                        field(ui, &mut self.hostname, "e.g. my-laptop", false);
                        ui.add_space(6.0);
                        ui.label("Password");
                        field(ui, &mut self.pw, "Password", true);
                        field(ui, &mut self.pw2, "The same again", true);
                    }
                    Page::Summary => {
                        heading(ui, &pal, "Ready to install", "Check the choices. Nothing has been changed yet.");
                        let disk = self.chosen_disk();
                        let rows = [
                            ("Language", locale_label(&self.locale)),
                            ("Keyboard", self.kb_chosen().map(KbChoice::label).unwrap_or_else(|| self.keymap.clone())),
                            ("Time zone", zone_label(&self.zone)),
                            ("Disk", disk.map(|d| format!("{} ({}, {}), {}", if d.model.is_empty() { "Disk" } else { &d.model }, size_text(d.size), d.path, if self.manual_mode { "partitions chosen below" } else { "erased" })).unwrap_or_default()),
                            ("Encryption", if self.encrypt { "on".to_string() } else { "off".to_string() }),
                            ("Account", format!("{} ({})", self.name.trim(), self.login)),
                            ("Computer's name", self.hostname.clone()),
                        ];
                        egui::Grid::new("summary").num_columns(2).spacing(vec2(24.0, 8.0)).show(ui, |ui| {
                            for (k, v) in rows {
                                ui.label(RichText::new(k).color(pal.dim));
                                ui.label(v);
                                ui.end_row();
                            }
                        });
                        if self.manual_mode {
                            if let Some(d) = disk {
                                ui.add_space(10.0);
                                ui.label(RichText::new("Changes to the disk").color(pal.dim));
                                for line in self.manual.describe(d.size, &d.parts) {
                                    ui.label(format!("•  {line}"));
                                }
                            }
                        }
                        ui.add_space(16.0);
                        let agree = if self.manual_mode { "I understand that the changes listed above are made to the disk (deleted and formatted partitions lose what is on them)" } else { "I understand that everything on this disk will be erased" };
                        ui.checkbox(&mut self.agree, RichText::new(agree).color(pal.amber));
                    }
                    Page::Installing => {
                        heading(ui, &pal, "Installing", "This takes a few minutes. Please keep the computer on.");
                        let (n, total, text) = self.step.clone();
                        let frac = if total > 0 { (n.saturating_sub(1) as f32 + 0.5) / total as f32 } else { 0.0 };
                        ui.add(egui::ProgressBar::new(frac).desired_width(460.0).fill(pal.accent));
                        ui.add_space(6.0);
                        ui.label(if total > 0 { format!("{text} ({n} of {total})") } else { "Starting...".into() });
                        if let Some(Report::Info(i)) = self.log.iter().rev().find(|r| matches!(r, Report::Info(_))) {
                            ui.label(RichText::new(i).color(pal.dim));
                        }
                        if let Some(e) = &self.failed {
                            ui.add_space(12.0);
                            ui.label(RichText::new("The installation stopped").color(pal.red).strong());
                            ui.label(RichText::new(e).color(pal.red));
                            ui.label(RichText::new("Nothing else was changed after this point. You can go back, change something, and try again.").color(pal.dim));
                        }
                        let warns: Vec<&String> = self.log.iter().filter_map(|r| if let Report::Warn(w) = r { Some(w) } else { None }).collect();
                        for w in warns {
                            ui.label(RichText::new(format!("Note: {w}")).color(pal.amber));
                        }
                        ctx.request_repaint_after(std::time::Duration::from_millis(500));
                    }
                    Page::Done => {
                        heading(ui, &pal, "All done", &format!("{} is installed. Take out the USB stick and restart.", self.os));
                        let warns: Vec<&String> = self.log.iter().filter_map(|r| if let Report::Warn(w) = r { Some(w) } else { None }).collect();
                        for w in warns {
                            ui.label(RichText::new(format!("Note: {w}")).color(pal.amber));
                        }
                    }
                }
                });
            });
            // Back and Next
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);
                match self.page {
                    Page::Done => {
                        if ui.add(egui::Button::new(RichText::new("Restart now").color(pal.on_accent())).fill(pal.accent).min_size(vec2(120.0, 34.0))).clicked() {
                            let _ = Command::new("systemctl").arg("reboot").spawn();
                        }
                    }
                    Page::Installing => {
                        if self.failed.is_some() && ui.add(egui::Button::new("Back").min_size(vec2(90.0, 34.0))).clicked() {
                            self.failed = None;
                            self.log.clear();
                            self.step = (0, 0, String::new());
                            self.reports = None;
                            self.page = Page::Summary;
                        }
                    }
                    page => {
                        let i = PAGES.iter().position(|p| *p == page).unwrap_or(0);
                        let last = page == Page::Summary;
                        let missing = self.missing(page);
                        let next = egui::Button::new(RichText::new(if last { "Install" } else { "Next" }).color(pal.on_accent())).fill(if last { pal.red } else { pal.accent }).min_size(vec2(110.0, 34.0));
                        let r = ui.add_enabled(missing.is_none() && (self.command.is_some() || self.demo), next);
                        if let Some(m) = missing.clone() {
                            r.on_disabled_hover_text(m);
                        } else if r.clicked() {
                            if last {
                                install = true;
                            } else {
                                go = Some(PAGES[i + 1]);
                            }
                        }
                        if i > 0 && ui.add(egui::Button::new("Back").min_size(vec2(90.0, 34.0))).clicked() {
                            go = Some(PAGES[i - 1]);
                        }
                        if let Some(m) = missing {
                            ui.label(RichText::new(m).color(pal.dim));
                        }
                    }
                }
            });
        });
        if let Some(p) = go {
            self.page = p;
        }
        if install {
            self.start_install(&ctx);
        }
        chrome::outline(&ctx, &pal);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 600.0])
            .with_min_inner_size([620.0, 520.0])
            .with_title("Install")
            .with_app_id("yggdrasil-installer")
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native("Install", options, Box::new(|_| Ok(Box::new(App::new()))))
}

#[cfg(test)]
mod tests {

    #[test]
    fn keyboards_are_offered_by_how_the_letters_are_arranged() {
        let lst = "! model\n  pc105  Generic 105-key PC\n\n! layout\n  us              English (US)\n  de              German\n  fr              French\n  ru              Russian\n\n! variant\n  dvorak          us: English (Dvorak)\n  colemak         us: English (Colemak)\n  intl            us: English (US, intl., with dead keys)\n  nodeadkeys      de: German (no dead keys)\n";
        let all = super::keyboard_choices(lst, &[]);
        let kind = |l: &str, v: &str| all.iter().find(|c| c.layout == l && c.variant == v).map(|c| c.kind);
        assert_eq!(kind("us", ""), Some(super::KbType::Qwerty));
        assert_eq!(kind("de", ""), Some(super::KbType::Qwertz));
        assert_eq!(kind("fr", ""), Some(super::KbType::Azerty));
        assert_eq!(kind("ru", ""), Some(super::KbType::Other));
        assert_eq!(kind("us", "dvorak"), Some(super::KbType::Dvorak));
        assert_eq!(kind("us", "colemak"), Some(super::KbType::Colemak));
        assert_eq!(kind("us", "intl"), Some(super::KbType::Qwerty));
        assert_eq!(kind("de", "nodeadkeys"), None, "other variants are not listed");
        assert_eq!(kind("pc105", ""), None, "models are not keyboards");
        // only what the system has, when it says
        assert!(super::keyboard_choices(lst, &["us".to_string()]).iter().all(|c| c.layout == "us"));
        assert_eq!(all.iter().find(|c| c.variant == "dvorak").unwrap().label(), "English (Dvorak)   (us, dvorak)");
    }

    #[test]
    fn only_region_city_zones_are_offered_and_read_as_places() {
        assert!(super::real_zone("America/Denver") && super::real_zone("UTC"));
        assert!(!super::real_zone("W-SU") && !super::real_zone("Zulu") && !super::real_zone("US/Pacific") && !super::real_zone("Etc/GMT+5"));
        assert_eq!(super::zone_label("America/Argentina/Buenos_Aires"), "Buenos Aires (America, Argentina)");
        assert_eq!(super::zone_label("Europe/Berlin"), "Berlin (Europe)");
        assert_eq!(super::zone_label("UTC"), "UTC");
    }

    use super::*;

    #[test]
    fn the_demo_never_installs_and_its_manual_plan_is_valid() {
        let mut app = App::new();
        app.demo("disk-manual");
        assert!(app.demo && app.command.is_none() && app.page == Page::Disk && app.manual_mode);
        let d = app.chosen_disk().unwrap().clone();
        assert_eq!(app.manual.problem(d.size, &d.parts, &d.table), None, "{:?}", app.manual.problem(d.size, &d.parts, &d.table));
        assert_eq!(app.missing(Page::Disk), None);
    }

    #[test]
    fn reports_are_read() {
        assert_eq!(parse_report("STEP 3 9 Encrypting"), Some(Report::Step(3, 9, "Encrypting".into())));
        assert_eq!(parse_report("INFO copied 40%"), Some(Report::Info("copied 40%".into())));
        assert_eq!(parse_report("ERROR the disk is busy"), Some(Report::Error("the disk is busy".into())));
        assert_eq!(parse_report("DONE"), Some(Report::Done));
        assert_eq!(parse_report("random output"), None);
    }

    #[test]
    fn names_are_checked_as_the_installer_does() {
        assert_eq!(login_from("Ada Lovelace"), "ada");
        assert_eq!(login_from("  Zoë-Ann X"), "zoann");
        assert!(login_ok("ada") && login_ok("_svc") && login_ok("a-1"));
        assert!(!login_ok("Ada") && !login_ok("1ada") && !login_ok(""));
        assert!(hostname_ok("my-laptop") && !hostname_ok("-x") && !hostname_ok("a b"));
        assert_eq!(size_text(1_000_204_886_016), "1.0 TB");
        assert_eq!(size_text(512_110_190_592), "512 GB");
    }
}
