//! yggdrasil-installer: installs the operating system from its live medium. The window is Yggdrasil's (the same look as Settings
//! and Minos, no other toolkit); the work is the OS's own program, named by an integration file of kind "installer":
//!
//!     kind = installer
//!     command = /usr/local/bin/labyrinth-install-system
//!
//! It is run as root (pkexec), gets the choices as one JSON object on its standard input (so no password is ever on a command
//! line), answers `--disks` with the disks it could install onto, and reports with one line each: `STEP n N text`, `INFO`,
//! `WARN`, `ERROR text`, `DONE`. See docs/INSTALLER.md.

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

fn timezones() -> Vec<String> {
    run_lines("timedatectl", &["list-timezones"])
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
    keymaps: Vec<String>,
    keymap: String,
    test_text: String,
    zones: Vec<String>,
    zone: String,
    zone_filter: String,
    disks: Vec<Disk>,
    disk: Option<String>,
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
}

impl App {
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
            keymaps: keymaps(),
            keymap: "us".into(),
            test_text: String::new(),
            zones: timezones(),
            zone: current_timezone(),
            zone_filter: String::new(),
            disks: vec![],
            disk: None,
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
        };
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
    fn missing(&self, page: Page) -> Option<&'static str> {
        match page {
            Page::Disk => {
                if self.disk.is_none() {
                    Some("Choose a disk")
                } else if self.encrypt && self.crypt_pw.chars().count() < 8 {
                    Some("The encryption password needs at least 8 characters")
                } else if self.encrypt && self.crypt_pw != self.crypt_pw2 {
                    Some("The two encryption passwords are not the same")
                } else {
                    None
                }
            }
            Page::Account => {
                if self.name.trim().is_empty() {
                    Some("Type your name")
                } else if !login_ok(&self.login) {
                    Some("The account name must start with a small letter (letters, digits, - and _)")
                } else if !hostname_ok(&self.hostname) {
                    Some("The computer's name may use letters, digits and -")
                } else if self.pw.is_empty() {
                    Some("Choose a password")
                } else if self.pw != self.pw2 {
                    Some("The two passwords are not the same")
                } else {
                    None
                }
            }
            Page::Summary if !self.agree => Some("Tick the box to confirm that the disk may be erased"),
            _ => None,
        }
    }

    fn plan(&self) -> serde_json::Value {
        serde_json::json!({
            "disk": self.disk.clone().unwrap_or_default(),
            "mode": "erase",
            "encrypt": self.encrypt,
            "encrypt_password": if self.encrypt { self.crypt_pw.clone() } else { String::new() },
            "user": {"name": self.name.trim(), "login": self.login, "password": self.pw},
            "hostname": self.hostname,
            "timezone": self.zone,
            "locale": self.locale,
            "keymap": self.keymap,
        })
    }

    fn start_install(&mut self, ctx: &egui::Context) {
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
                match self.page {
                    Page::Welcome => {
                        heading(ui, &pal, &format!("Welcome to {}", self.os), "This puts it on this computer's disk. It takes a few minutes; nothing is sent anywhere.");
                        if self.command.is_none() {
                            ui.label(RichText::new("This system has no installer (no integration of kind \"installer\").").color(pal.red));
                        }
                        ui.label("Language");
                        let mut loc = self.locale.clone();
                        pick_list(ui, &pal, "locales", &self.locales, &mut loc, (body_h - 140.0).max(120.0), locale_label);
                        self.locale = loc;
                    }
                    Page::Keyboard => {
                        heading(ui, &pal, "Keyboard", "The layout of your keyboard. Type in the box to try it.");
                        let mut km = self.keymap.clone();
                        pick_list(ui, &pal, "keymaps", &self.keymaps, &mut km, (body_h - 150.0).max(120.0), |k| k.to_string());
                        self.keymap = km;
                        ui.add_space(8.0);
                        field(ui, &mut self.test_text, "Try your keyboard here", false);
                    }
                    Page::TimeZone => {
                        heading(ui, &pal, "Time zone", "Where the clock should be set for. Type a city to find it.");
                        field(ui, &mut self.zone_filter, "Search, e.g. Denver or Berlin", false);
                        ui.add_space(6.0);
                        let q = self.zone_filter.to_lowercase().replace(' ', "_");
                        let shown: Vec<String> = self.zones.iter().filter(|z| q.is_empty() || z.to_lowercase().contains(&q)).cloned().collect();
                        let mut z = self.zone.clone();
                        pick_list(ui, &pal, "zones", &shown, &mut z, (body_h - 150.0).max(120.0), |z| z.replace('_', " "));
                        self.zone = z;
                    }
                    Page::Disk => {
                        heading(ui, &pal, "Where to install", "The whole disk you choose is used: everything on it is erased.");
                        for d in self.disks.clone() {
                            let on = self.disk.as_deref() == Some(d.path.as_str());
                            let label = format!("{}   {}   {}{}", if d.model.is_empty() { "Disk" } else { &d.model }, size_text(d.size), d.path, if d.removable { "   (removable)" } else { "" });
                            let b = egui::Button::new(RichText::new(label).color(if on { pal.on_accent() } else { pal.text })).fill(if on { pal.accent } else { pal.field }).min_size(vec2(460.0, 34.0));
                            if ui.add_enabled(d.big_enough, b).on_disabled_hover_text("Too small: at least 30 GB are needed").clicked() {
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
                        ui.add_space(16.0);
                        ui.checkbox(&mut self.encrypt, "Encrypt the disk (recommended)");
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
                        let disk = self.disks.iter().find(|d| Some(&d.path) == self.disk.as_ref());
                        let rows = [
                            ("Language", locale_label(&self.locale)),
                            ("Keyboard", self.keymap.clone()),
                            ("Time zone", self.zone.clone()),
                            ("Disk", disk.map(|d| format!("{} ({}, {}), erased", if d.model.is_empty() { "Disk" } else { &d.model }, size_text(d.size), d.path)).unwrap_or_default()),
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
                        ui.add_space(16.0);
                        ui.checkbox(&mut self.agree, RichText::new("I understand that everything on this disk will be erased").color(pal.amber));
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
                        let r = ui.add_enabled(missing.is_none() && self.command.is_some(), next);
                        if let Some(m) = missing {
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
    use super::*;

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
