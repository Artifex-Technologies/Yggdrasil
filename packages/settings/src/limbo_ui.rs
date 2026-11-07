//! Settings > Security > Limbo: the sandboxes (Limbo's guests). Written against Limbo's protocol (Limbo/docs/SETTINGS_PAGE.md,
//! "Protocol"), through the `limbo` command so the page never speaks HTTP itself:
//! - `limbo settings show --json` (user.sock: read-only, never a password) gives the status, the instances and the licenses;
//! - a change runs `limbo instance ... --json` as the person; when Limbo answers `password_required` (the switch is on) the
//!   same command runs again through `pkexec`, which asks for the password;
//! - root-only things (the switch, adding or removing a license) always go through `pkexec limbo ...`; a license key goes on
//!   stdin, never on a command line.
//!
//! Quarantine verdicts are not here: Heimdall decides what is held, and its page (Settings > Security) shows held guests too. A
//! held guest's row says so. Text that came from a guest (none is shown here today) would be shown as plain text.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;

use crate::ui;

const LIMBO: &str = "/usr/bin/limbo";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub version: String,
    pub kvm: bool,
    pub ask_password: bool,
    pub total: i64,
    pub images: i64,
    pub guests: i64,
    pub evidence: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Instance {
    pub name: String,
    pub kind: String,
    pub os: String,
    pub state: String,
    pub running: bool,
    pub cpus: i64,
    pub memory_mb: i64,
    pub network: String,
    pub own_bytes: i64,
    pub base_bytes: i64,
    pub owner: String,
    pub mine: bool,
    pub held_by_heimdall: bool,
    pub held: bool,
    pub pending: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct License {
    pub id: String,
    pub name: String,
    pub masked: String,
    pub assigned: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Ready { status: Status, instances: Vec<Instance>, licenses: Vec<License> },
    /// Limbo answered with an error (code, the line to show)
    Error(String, String),
}

/// `limbo settings show --json`'s answer (or the error object it prints on failure).
pub fn parse(json: &str) -> Option<Shown> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Some(Shown::Error(v["code"].as_str().unwrap_or("").to_string(), e.to_string()));
    }
    let s = &v["status"];
    let space = &s["space"];
    let status = Status {
        version: s["version"].as_str().unwrap_or("").to_string(),
        kvm: s["kvm"].as_bool().unwrap_or(false),
        ask_password: s["ask_password"].as_bool().unwrap_or(true),
        total: space["total_bytes"].as_i64().unwrap_or(0),
        images: space["images_bytes"].as_i64().unwrap_or(0),
        guests: space["guests_bytes"].as_i64().unwrap_or(0),
        evidence: space["evidence_bytes"].as_i64().unwrap_or(0),
    };
    let instances = v["instances"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|i| Instance {
                    name: i["name"].as_str().unwrap_or("").to_string(),
                    kind: i["kind"].as_str().unwrap_or("").to_string(),
                    os: i["os"].as_str().unwrap_or("").to_string(),
                    state: i["state"].as_str().unwrap_or("").to_string(),
                    running: i["running"].as_bool().unwrap_or(false),
                    cpus: i["cpus"].as_i64().unwrap_or(0),
                    memory_mb: i["memory_mb"].as_i64().unwrap_or(0),
                    network: i["network"].as_str().unwrap_or("").to_string(),
                    own_bytes: i["disk"]["own_bytes"].as_i64().unwrap_or(0),
                    base_bytes: i["disk"]["base_bytes"].as_i64().unwrap_or(0),
                    owner: i["owner"].as_str().unwrap_or("").to_string(),
                    mine: i["mine"].as_bool().unwrap_or(false),
                    held_by_heimdall: i["held_by_heimdall"].as_bool().unwrap_or(false),
                    held: !i["hold"].is_null() && i.get("hold").is_some(),
                    pending: i["pending"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    let licenses = v["licenses"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|l| License {
                    id: l["id"].as_str().unwrap_or("").to_string(),
                    name: l["name"].as_str().unwrap_or("").to_string(),
                    masked: l["masked"].as_str().unwrap_or("").to_string(),
                    assigned: l["assigned"].as_array().map(|x| x.iter().filter_map(|g| g.as_str().map(String::from)).collect()).unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Shown::Ready { status, instances, licenses })
}

/// Bytes as people read them (GB with one decimal from 1 GB on).
pub fn size(b: i64) -> String {
    let b = b.max(0) as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} kB", b / 1e3)
    }
}

/// Memory as it is sold: 4096 MB is "4 GB".
pub fn memory(mb: i64) -> String {
    if mb >= 1024 && mb % 1024 == 0 {
        format!("{} GB", mb / 1024)
    } else if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

/// The line under an instance's name.
pub fn describe(i: &Instance) -> String {
    let os = match i.os.as_str() {
        "windows" => "Windows",
        "linux" => "Linux",
        o => o,
    };
    let state = if i.held_by_heimdall {
        "held by Heimdall (Settings > Security)".to_string()
    } else if i.held {
        "held".to_string()
    } else if i.running {
        "running".to_string()
    } else {
        "stopped".to_string()
    };
    let net = match i.network.as_str() {
        "none" => "no network",
        "allowlist" => "a few sites only",
        "internet-only" => "internet, not your network",
        n => n,
    };
    let mut s = format!("{os} · {state} · {} processor{} · {} memory · {net} · its own changes {}", i.cpus, if i.cpus == 1 { "" } else { "s" }, memory(i.memory_mb), size(i.own_bytes));
    if i.pending {
        s.push_str(" · changes apply at its next start");
    }
    if !i.mine && !i.owner.is_empty() {
        s.push_str(&format!(" · belongs to {}", i.owner));
    }
    s
}

/// Does an answer ask for the password (the switch is on)?
pub fn wants_password(out: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(out).ok().and_then(|v| v["code"].as_str().map(|c| c == "password_required")).unwrap_or(false)
}

enum Msg {
    Shown(Option<Shown>),
    Done(Result<(), String>),
    Report(Result<std::path::PathBuf, String>),
}

pub struct Panel {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    shown: Option<Shown>,
    busy: bool,
    status: String,
    loaded: Option<Instant>,
    /// the license being added (name, key) and whether the form is open; the instance whose space is about to be freed
    lic_name: String,
    lic_key: String,
    adding: bool,
    free_confirm: Option<String>,
    /// Where the last report was saved.
    saved: Option<String>,
}

pub fn installed() -> bool {
    std::path::Path::new(LIMBO).exists()
}

fn show_json() -> String {
    // a debug build may read the answer from $YGGDRASIL_LIMBO_FIXTURE (to look at the page without Limbo); release builds never do
    if cfg!(debug_assertions) {
        if let Some(t) = std::env::var_os("YGGDRASIL_LIMBO_FIXTURE").and_then(|f| std::fs::read_to_string(f).ok()) {
            return t;
        }
    }
    Command::new(LIMBO).args(["settings", "show", "--json"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

/// Runs `limbo ARGS --json` as the person, and through pkexec when Limbo asks for the password (or `root` says it must).
fn run(args: &[String], root: bool, stdin: Option<String>) -> Result<(), String> {
    let go = |pk: bool| -> Result<String, String> {
        let mut c = if pk { Command::new("pkexec") } else { Command::new(LIMBO) };
        if pk {
            c.arg(LIMBO);
        }
        c.args(args).arg("--json").stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
        let mut child = c.spawn().map_err(|_| if pk { "pkexec is not available".to_string() } else { "Limbo is not installed".to_string() })?;
        if let (Some(text), Some(mut w)) = (&stdin, child.stdin.take()) {
            let _ = w.write_all(text.as_bytes());
        }
        let o = child.wait_with_output().map_err(|e| e.to_string())?;
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        if o.status.success() {
            return Ok(out);
        }
        if pk && matches!(o.status.code(), Some(126) | Some(127)) {
            return Err("Not changed: the password was not given.".into());
        }
        Err(out)
    };
    match go(root) {
        Ok(_) => Ok(()),
        Err(out) if !root && wants_password(&out) => go(true).map(|_| ()).map_err(|e| one_line(&e)),
        Err(out) => Err(one_line(&out)),
    }
}

/// Where a report is kept: ~/Documents (else the home folder), named by the date, a number added if one exists.
pub fn report_path(home: &std::path::Path, date: &str, exists: &dyn Fn(&std::path::Path) -> bool) -> std::path::PathBuf {
    let docs = home.join("Documents");
    let dir = if exists(&docs) { docs } else { home.to_path_buf() };
    let mut p = dir.join(format!("Limbo report {date}.md"));
    let mut n = 2;
    while exists(&p) {
        p = dir.join(format!("Limbo report {date} ({n}).md"));
        n += 1;
    }
    p
}

/// `limbo report` for the last seven days, through pkexec (the audit log and the quarantine records are root's), kept as
/// Markdown in the person's Documents and opened with their Markdown app.
fn save_report() -> Result<std::path::PathBuf, String> {
    let o = Command::new("pkexec").args([LIMBO, "report", "--since", "7d"]).stdin(Stdio::null()).output().map_err(|_| "pkexec is not available".to_string())?;
    if !o.status.success() {
        if matches!(o.status.code(), Some(126) | Some(127)) {
            return Err("No report: the password was not given.".into());
        }
        let err = String::from_utf8_lossy(&o.stderr);
        return Err(format!("No report: {}", err.lines().next().unwrap_or("Limbo did not answer")));
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).ok_or("No home folder")?;
    let date = Command::new("date").arg("+%Y-%m-%d").output().map(|d| String::from_utf8_lossy(&d.stdout).trim().to_string()).unwrap_or_default();
    let path = report_path(&home, &date, &|p| p.exists());
    std::fs::write(&path, &o.stdout).map_err(|e| format!("Not saved: {e}"))?;
    let _ = Command::new("xdg-open").arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    Ok(path)
}

fn one_line(out: &str) -> String {
    match parse(out) {
        Some(Shown::Error(_, line)) => line,
        _ => out.lines().next().unwrap_or("Limbo did not answer").to_string(),
    }
}

impl Panel {
    pub fn new(ctx: &egui::Context) -> Panel {
        let (tx, rx) = channel();
        let mut p = Panel { tx, rx, shown: None, busy: false, status: String::new(), loaded: None, lic_name: String::new(), lic_key: String::new(), adding: false, free_confirm: None, saved: None };
        p.load(ctx);
        p
    }

    fn load(&mut self, ctx: &egui::Context) {
        self.loaded = Some(Instant::now());
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            // an answer that is not Limbo's JSON: a Limbo from before this page (it has no `settings show`)
            let out = show_json();
            let shown = parse(&out).or_else(|| Some(Shown::Error("old".into(), out.lines().next().unwrap_or("").to_string())));
            let _ = tx.send(Msg::Shown(shown));
            ctx.request_repaint();
        });
    }

    fn act(&mut self, ctx: &egui::Context, args: Vec<String>, root: bool, stdin: Option<String>) {
        self.busy = true;
        self.status.clear();
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Done(run(&args, root, stdin)));
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Shown(s) => self.shown = s,
                Msg::Done(r) => {
                    self.busy = false;
                    if let Err(e) = r {
                        self.status = e;
                    }
                    self.load(ctx);
                }
                Msg::Report(r) => {
                    self.busy = false;
                    match r {
                        Ok(p) => self.saved = Some(p.display().to_string()),
                        Err(e) => self.status = e,
                    }
                }
            }
        }
        // the guests start and stop elsewhere too (Sarina's runs, Heimdall's holds): read again now and then while shown
        if !self.busy && self.loaded.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.load(ctx);
        }
        ctx.request_repaint_after(Duration::from_secs(5));
        if !self.status.is_empty() {
            ui.colored_label(pal.red, &self.status);
        }
        let Some(shown) = self.shown.clone() else {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "Asking Limbo…", None, |_| {}));
            return;
        };
        let (status, instances, licenses) = match shown {
            Shown::Ready { status, instances, licenses } => (status, instances, licenses),
            Shown::Error(code, line) => {
                let (title, sub) = match code.as_str() {
                    "old" => ("This Limbo is older than this page".to_string(), "Update Limbo (limbo-git) to see its guests and figures here".to_string()),
                    "" if line.contains("permission denied") => ("Limbo's figures are for its users".to_string(), "Join the limbo-users group to see them without the password: sudo gpasswd -a YOUR-NAME limbo-users, then sign in again".to_string()),
                    _ if line.contains("no such file") || line.contains("connection refused") => ("Limbo is not running".to_string(), "Check it with: systemctl status limbo".to_string()),
                    _ => ("Limbo did not answer".to_string(), line),
                };
                ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, &title, Some(&sub), |_| {}));
                return;
            }
        };
        let mut job: Option<(Vec<String>, bool, Option<String>)> = None;
        ui::group(ui, pal, Some("Now"), |ui| {
            ui::row(ui, pal, true, "Limbo", Some(&format!("Version {}", status.version)), |ui| {
                ui.label("Running");
            });
            ui::row(ui, pal, false, "Hardware virtualization (KVM)", Some(if status.kvm { "Guests run at full speed" } else { "Not available: switch on VT-x or AMD-V in the firmware setup" }), |ui| {
                ui.label(if status.kvm { "Available" } else { "Off" });
            });
            let sub = format!("Base images {} · guests' own changes {} · kept as evidence {}", size(status.images), size(status.guests), size(status.evidence));
            ui::row(ui, pal, false, "Space used", Some(&sub), |ui| {
                ui.label(size(status.total));
            });
        });
        ui.add_space(8.0);
        ui::group(ui, pal, None, |ui| {
            let mut ask = status.ask_password;
            ui::row(ui, pal, true, "Ask for my password for Limbo changes", Some("Off: you start, stop and change your own guests without it. Quarantine decisions and anything Heimdall restricted always ask"), |ui| {
                if ui.add_enabled_ui(!self.busy, |ui| ui::switch(ui, pal, &mut ask)).inner.changed() {
                    job = Some((vec!["settings".into(), "password".into(), if ask { "on" } else { "off" }.into()], true, None));
                }
            });
        });
        ui.add_space(8.0);
        ui::group(ui, pal, None, |ui| {
            let sub = match &self.saved {
                Some(p) => format!("Saved: {p}"),
                None => "What the guests did and every hold of the last seven days, in plain words (Markdown), for you or for someone helping you".to_string(),
            };
            ui::row(ui, pal, true, "Evidence report", Some(&sub), |ui| {
                if ui.add_enabled(!self.busy, egui::Button::new("Save a report…")).clicked() {
                    self.busy = true;
                    self.status.clear();
                    let (tx, ctx) = (self.tx.clone(), ctx.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send(Msg::Report(save_report()));
                        ctx.request_repaint();
                    });
                }
            });
        });
        ui.add_space(8.0);
        let mine: Vec<&Instance> = instances.iter().filter(|i| i.kind == "instance").collect();
        let others: Vec<&Instance> = instances.iter().filter(|i| i.kind != "instance").collect();
        ui::group(ui, pal, Some("Guests"), |ui| {
            if mine.is_empty() {
                ui::row(ui, pal, true, "No guests of your own yet", Some("An administrator adds one with: sudo limbo instance add MANIFEST"), |_| {});
            }
            for (n, i) in mine.iter().enumerate() {
                ui::row(ui, pal, n == 0, &i.name, Some(&describe(i)), |ui| {
                    let can = !self.busy && !i.held && (i.mine || i.owner.is_empty());
                    if i.running {
                        if ui.add_enabled(can, egui::Button::new("Stop")).clicked() {
                            job = Some((vec!["instance".into(), "stop".into(), i.name.clone()], false, None));
                        }
                    } else {
                        if ui.add_enabled(can, egui::Button::new("Start")).clicked() {
                            job = Some((vec!["instance".into(), "start".into(), i.name.clone()], false, None));
                        }
                        if i.own_bytes > 0 && ui.add_enabled(can, egui::Button::new("Free space…")).clicked() {
                            self.free_confirm = Some(i.name.clone());
                        }
                    }
                });
            }
        });
        if !others.is_empty() {
            ui.add_space(8.0);
            ui::group(ui, pal, Some("Runs and test guests"), |ui| {
                for (n, i) in others.iter().enumerate() {
                    let who = if i.kind == "run" { "A test or analysis run (Sarina's or Heimdall's)" } else { "Started by hand" };
                    ui::row(ui, pal, n == 0, &i.name, Some(&format!("{who} · {}", describe(i))), |_| {});
                }
            });
        }
        ui.add_space(8.0);
        ui::group(ui, pal, Some("Windows licenses"), |ui| {
            if licenses.is_empty() {
                ui::row(ui, pal, true, "None added", Some("Only for programs that need a real Windows guest; Wine never needs one"), |_| {});
            }
            for (n, l) in licenses.iter().enumerate() {
                let sub = if l.assigned.is_empty() { l.masked.clone() } else { format!("{} · used by {}", l.masked, l.assigned.join(", ")) };
                ui::row(ui, pal, n == 0, &l.name, Some(&sub), |ui| {
                    if ui.add_enabled(!self.busy, egui::Button::new("Remove")).clicked() {
                        job = Some((vec!["license".into(), "remove".into(), l.id.clone()], true, None));
                    }
                });
            }
            ui::row(ui, pal, false, "Add a license", Some("Kept where only the administrator can read it, shown masked, never logged or given to Sarina or Heimdall"), |ui| {
                if ui::quiet(ui, pal, if self.adding { "Cancel" } else { "Add…" }).clicked() {
                    self.adding = !self.adding;
                    self.lic_key.clear();
                }
            });
            if self.adding {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui::field(ui, pal, &mut self.lic_name, "A name, e.g. Windows 11 Pro", 220.0, false);
                    ui::field(ui, pal, &mut self.lic_key, "XXXXX-XXXXX-XXXXX-XXXXX-XXXXX", 300.0, true);
                    let ok = !self.lic_name.trim().is_empty() && self.lic_key.trim().len() >= 29 && !self.busy;
                    if ui.add_enabled(ok, egui::Button::new("Add")).clicked() {
                        // the key goes on stdin, never on a command line (every account can read those)
                        job = Some((vec!["license".into(), "add".into(), "--name".into(), self.lic_name.trim().to_string()], true, Some(format!("{}\n", self.lic_key.trim()))));
                        self.lic_key.clear();
                        self.adding = false;
                    }
                });
                ui.add_space(6.0);
            }
        });
        ui.label(RichText::new("Held guests and quarantined files are on Heimdall's page (Settings > Security). Changes that need it ask for your password.").size(11.5).color(pal.dim));
        if let Some(name) = self.free_confirm.clone() {
            let mut close = false;
            egui::Window::new(format!("Free the space of {name}?")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
                ui.label("Everything changed inside this guest since it was made is deleted: it starts again as new.");
                ui.horizontal(|ui| {
                    if ui.button("Free space").clicked() {
                        job = Some((vec!["instance".into(), "free-space".into(), name.clone(), "--yes".into()], false, None));
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.free_confirm = None;
            }
        }
        if let Some((args, root, stdin)) = job {
            self.act(ctx, args, root, stdin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW: &str = r#"{"status": {"version": "0.1.0-dev", "running": true, "kvm": true, "ask_password": true,
        "space": {"total_bytes": 21474836480, "images_bytes": 8589934592, "guests_bytes": 1073741824, "evidence_bytes": 0, "exchange_bytes": 4096},
        "instances": {"total": 2, "running": 1, "held": 0}},
      "instances": [
        {"name": "lab", "kind": "instance", "os": "linux", "state": "running", "running": true, "address": "", "cpus": 2, "memory_mb": 4096,
         "network": "none", "disk": {"base_bytes": 2147483648, "own_bytes": 52428800, "virtual_bytes": 0, "disk_gb": 0},
         "shared_folder": "off", "clipboard": "off", "devices": [], "disposable": false, "owner": "ada", "mine": true,
         "hold": null, "held_by_heimdall": false, "pending": false, "license": "", "overrides": {}},
        {"name": "run-x1", "kind": "run", "os": "linux", "state": "held", "running": false, "cpus": 1, "memory_mb": 2048, "network": "none",
         "disk": {"base_bytes": 1, "own_bytes": 0}, "owner": "", "mine": false,
         "hold": {"id": "q-1", "status": "open", "level": "isolate", "by": "heimdall", "since": "2026-11-05T08:00:00Z"}, "held_by_heimdall": true}],
      "licenses": [{"id": "lic-1", "name": "Windows 11 Pro", "masked": "XXXXX-XXXXX-XXXXX-XXXXX-7K2QF", "added": "x", "assigned": ["win11"]}]}"#;

    #[test]
    fn the_answer_parses() {
        let Some(Shown::Ready { status, instances, licenses }) = parse(SHOW) else { panic!("not parsed") };
        assert_eq!((status.kvm, status.ask_password, status.total), (true, true, 21474836480));
        assert_eq!(instances.len(), 2);
        assert!(instances[0].mine && instances[0].running && !instances[0].held);
        assert!(instances[1].held && instances[1].held_by_heimdall);
        assert_eq!(licenses[0].assigned, vec!["win11".to_string()]);
        assert!(describe(&instances[1]).contains("held by Heimdall"));
        assert!(describe(&instances[0]).contains("2 processors") && describe(&instances[0]).contains("no network"));
    }

    #[test]
    fn errors_and_the_password() {
        let e = r#"{"error": "password_required: Limbo changes need the password on this computer.", "code": "password_required"}"#;
        assert!(wants_password(e));
        assert!(!wants_password(r#"{"error": "x", "code": "not_owner"}"#));
        assert!(!wants_password("not json"));
        assert_eq!(parse(e), Some(Shown::Error("password_required".into(), "password_required: Limbo changes need the password on this computer.".into())));
    }

    #[test]
    fn reports_go_to_documents_and_never_over_an_earlier_one() {
        use std::path::{Path, PathBuf};
        let home = Path::new("/home/ada");
        let taken = |list: &'static [&'static str]| move |p: &Path| list.iter().any(|x| Path::new(x) == p);
        assert_eq!(report_path(home, "2026-11-07", &taken(&["/home/ada/Documents"])), PathBuf::from("/home/ada/Documents/Limbo report 2026-11-07.md"));
        assert_eq!(
            report_path(home, "2026-11-07", &taken(&["/home/ada/Documents", "/home/ada/Documents/Limbo report 2026-11-07.md"])),
            PathBuf::from("/home/ada/Documents/Limbo report 2026-11-07 (2).md")
        );
        assert_eq!(report_path(home, "2026-11-07", &taken(&[])), PathBuf::from("/home/ada/Limbo report 2026-11-07.md"), "no Documents folder");
    }

    #[test]
    fn sizes() {
        assert_eq!(size(21474836480), "21.5 GB");
        assert_eq!(size(52428800), "52 MB");
        assert_eq!(size(4096), "4 kB");
        assert_eq!(memory(4096), "4 GB");
        assert_eq!(memory(1536), "1.5 GB");
        assert_eq!(memory(512), "512 MB");
    }
}
