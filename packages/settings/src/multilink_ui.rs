//! Settings > Wi-Fi & Network > Several connections at once: Heimdall's heimdall-multilink (Heimdall/docs/MULTILINK.md).
//!
//! - **Read**, without root: the service's `/run/heimdall-multilink/status.json` while it runs (once a second, while this page
//!   is open and the window is in front); while it is off, `heimdall-multilink status --json` (every few seconds: it looks at
//!   the links itself). The window is woken only when what was read changed, so a page with nothing moving draws nothing.
//! - **Change**: `pkexec /usr/bin/heimdall-multilink on|off|set LINK ROLE [--share N|none] [--cap MB|none] [--per day|month]`
//!   (polkit action org.heimdall.multilink: the administrator's password, remembered for a few minutes).

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use yggdrasil_ui::theme::Palette;

use crate::ui;

const BIN: &str = "/usr/bin/heimdall-multilink";
const STATUS: &str = "/run/heimdall-multilink/status.json";

pub fn installed() -> bool {
    std::path::Path::new(BIN).exists()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Link {
    pub name: String,
    pub role: String,
    pub up: bool,
    pub healthy: Option<bool>,
    pub weight: Option<u64>,
    pub rate: u64,
    pub share_now: u64,
    pub rtt_ms: Option<u64>,
    pub share_limit: Option<u64>,
    pub data_cap_mb: Option<u64>,
    pub period: String,
    pub used_bytes: u64,
    pub over_cap: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// active, paused, waiting or off
    pub state: String,
    pub reason: String,
    pub combined_rate: u64,
    pub links: Vec<Link>,
}

/// The service's status file, or `heimdall-multilink status --json` (the same shape, `state` "off").
pub fn parse(json: &str) -> Option<Status> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let s = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    let links = v["links"]
        .as_array()?
        .iter()
        .map(|l| Link {
            name: s(l, "name"),
            role: s(l, "role"),
            up: l["up"].as_bool().unwrap_or(false),
            healthy: match (l["healthy4"].as_bool(), l["healthy6"].as_bool()) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(false) || b.unwrap_or(false)),
            },
            weight: l["weight4"].as_u64().or(l["weight6"].as_u64()),
            rate: l["rate"].as_u64().unwrap_or(0),
            share_now: l["share_now"].as_u64().unwrap_or(0),
            rtt_ms: l["rtt_ms"].as_u64(),
            share_limit: l["share_limit"].as_u64(),
            data_cap_mb: l["data_cap_mb"].as_u64(),
            period: s(l, "period"),
            used_bytes: l["used_bytes"].as_u64().unwrap_or(0),
            over_cap: l["over_cap"].as_bool().unwrap_or(false),
        })
        .collect();
    Some(Status { state: s(&v, "state"), reason: s(&v, "reason"), combined_rate: v["combined_rate"].as_u64().unwrap_or(0), links })
}

/// What kind of connection an interface is, from the name the system gave it.
pub fn kind(name: &str) -> &'static str {
    let usb = name.starts_with("en") && name.contains('u') && name.find('s').is_some_and(|s| name[s..].contains('u'));
    if name.starts_with("wl") {
        "Wi-Fi"
    } else if usb || name.starts_with("usb") || name.starts_with("rndis") {
        "USB (a phone, or an adapter)"
    } else if name.starts_with("ww") {
        "Mobile data"
    } else if name.starts_with("bnep") {
        "Bluetooth (a phone)"
    } else if name.starts_with("en") || name.starts_with("eth") {
        "Ethernet"
    } else {
        "Network"
    }
}

/// Bytes per second as megabits per second, as connection speeds are given.
pub fn mbit(bytes_per_s: u64) -> String {
    let m = bytes_per_s as f64 * 8.0 / 1e6;
    if m >= 10.0 {
        format!("{m:.0} Mbit/s")
    } else {
        format!("{m:.1} Mbit/s")
    }
}

/// One link's line under its name.
pub fn describe(l: &Link, state: &str) -> String {
    let mut parts: Vec<String> = vec![kind(&l.name).to_string()];
    if !l.up {
        parts.push("not connected".into());
    } else if state == "active" && l.role != "off" {
        match l.healthy {
            Some(false) => parts.push("no internet through it now".into()),
            Some(true) => {
                parts.push(mbit(l.rate));
                match l.weight {
                    Some(w) => parts.push(format!("{w} % of new connections")),
                    None if l.role == "backup" || l.over_cap => parts.push("waiting as backup".into()),
                    None => {}
                }
            }
            None => parts.push("checking…".into()),
        }
    }
    if let Some(cap) = l.data_cap_mb {
        let per = if l.period == "day" { "today" } else { "this month" };
        parts.push(format!("{} of {cap} MB {per}{}", l.used_bytes / 1_000_000, if l.over_cap { ": cap reached" } else { "" }));
    }
    parts.join(" · ")
}

enum Msg {
    Status(Option<Status>),
    Done(Result<(), String>),
}

pub struct Panel {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    shown: Option<Status>,
    busy: bool,
    error: String,
    /// read again at once (after a change made here)
    poke: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    /// the link whose limits are open, and the cap being typed
    open: Option<String>,
    cap_text: String,
}

fn read() -> Option<Status> {
    if let Some(s) = std::fs::read_to_string(STATUS).ok().and_then(|t| parse(&t)) {
        return Some(s);
    }
    let o = Command::new(BIN).args(["status", "--json"]).stdin(std::process::Stdio::null()).output().ok()?;
    parse(&String::from_utf8_lossy(&o.stdout))
}

fn run(args: Vec<String>) -> Result<(), String> {
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    let o = Command::new("pkexec").arg(BIN).args(&args).output().map_err(|e| format!("pkexec: {e}"))?;
    match o.status.code() {
        Some(0) => Ok(()),
        // the password dialog was closed
        Some(126) | Some(127) => Err(String::new()),
        _ => Err(String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("It could not be changed").to_string()),
    }
}

impl Panel {
    pub fn new(ctx: &egui::Context) -> Panel {
        let (tx, rx) = channel();
        let p = Panel {
            tx,
            rx,
            shown: None,
            busy: false,
            error: String::new(),
            poke: Arc::default(),
            alive: Arc::new(AtomicBool::new(true)),
            paused: Arc::default(),
            open: None,
            cap_text: String::new(),
        };
        let (tx, ctx, alive, paused, poke) = (p.tx.clone(), ctx.clone(), p.alive.clone(), p.paused.clone(), p.poke.clone());
        let _ = std::thread::Builder::new().name("multilink".into()).spawn(move || {
            let mut last: Option<Option<Status>> = None;
            let mut last_read = Instant::now() - Duration::from_secs(60);
            while alive.load(Ordering::Acquire) {
                // the status file once a second; the slower look of `status --json` every 5 seconds while it is off
                let off = last.as_ref().is_some_and(|s| s.as_ref().is_none_or(|s| s.state == "off"));
                let every = if off { Duration::from_secs(5) } else { Duration::from_secs(1) };
                let due = last_read.elapsed() >= every || poke.swap(false, Ordering::AcqRel);
                if due && !paused.load(Ordering::Acquire) {
                    last_read = Instant::now();
                    let s = read();
                    if last.as_ref() != Some(&s) {
                        last = Some(s.clone());
                        let _ = tx.send(Msg::Status(s));
                        ctx.request_repaint();
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        p
    }

    /// Hold the reads while the window is in the background.
    pub fn pause(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }

    fn act(&mut self, ctx: &egui::Context, args: Vec<String>) {
        self.busy = true;
        self.error.clear();
        let (tx, ctx, poke) = (self.tx.clone(), ctx.clone(), self.poke.clone());
        std::thread::spawn(move || {
            let r = run(args);
            poke.store(true, Ordering::Release);
            let _ = tx.send(Msg::Done(r));
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Status(s) => self.shown = s,
                Msg::Done(r) => {
                    self.busy = false;
                    if let Err(e) = r {
                        self.error = e;
                    }
                }
            }
        }
        if !self.error.is_empty() {
            ui.colored_label(pal.red, &self.error);
        }
        let Some(st) = self.shown.clone() else {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "Looking at the connections…", None, |_| {}));
            return;
        };
        let on = st.state != "off";
        let mut want: Option<Vec<String>> = None;
        ui::group(ui, pal, None, |ui| {
            let sub = match st.state.as_str() {
                "active" => {
                    let n = st.links.iter().filter(|l| l.weight.is_some()).count();
                    format!(
                        "New connections are spread over {n} connection{} · together {} now",
                        if n == 1 { "" } else { "s" },
                        mbit(st.combined_rate)
                    )
                }
                "paused" => format!("Paused: {}", st.reason),
                "waiting" => "Waiting for a second connection (Wi-Fi, Ethernet, or a phone over USB)".into(),
                _ => "Off: each connection works as usual, one at a time".into(),
            };
            ui::row(ui, pal, true, "Use several connections at once", Some(&sub), |ui| {
                let mut v = on;
                ui.add_enabled_ui(!self.busy, |ui| {
                    if ui::switch(ui, pal, &mut v).changed() {
                        want = Some(vec![if v { "on" } else { "off" }.into()]);
                    }
                });
            });
        });
        if st.links.is_empty() {
            ui::footnote(ui, pal, "No connection to the internet is on right now.");
        } else {
            ui::group(ui, pal, Some("Connections"), |ui| {
                for (i, l) in st.links.iter().enumerate() {
                    let sub = describe(l, &st.state);
                    let roles = ["Use", "Backup only", "Off"];
                    let sel = match l.role.as_str() {
                        "backup" => 1,
                        "off" => 2,
                        _ => 0,
                    };
                    let mut toggle = false;
                    ui::row(ui, pal, i == 0, &l.name, Some(&sub), |ui| {
                        ui.add_enabled_ui(!self.busy, |ui| {
                            if let Some(k) = ui::segmented(ui, pal, &roles, sel) {
                                want = Some(vec!["set".into(), l.name.clone(), ["use", "backup", "off"][k].into()]);
                            }
                            let label = if self.open.as_deref() == Some(&l.name) { "Done" } else { "Limits" };
                            if ui::quiet(ui, pal, label).clicked() {
                                toggle = true;
                            }
                        });
                    });
                    if toggle {
                        if self.open.as_deref() == Some(&l.name) {
                            self.open = None;
                        } else {
                            self.open = Some(l.name.clone());
                            self.cap_text = l.data_cap_mb.map(|c| c.to_string()).unwrap_or_default();
                        }
                    }
                    if self.open.as_deref() == Some(&l.name) {
                        let shares = ["No limit", "25 %", "50 %", "75 %"];
                        let cur = match l.share_limit {
                            Some(25) => 1,
                            Some(50) => 2,
                            Some(75) => 3,
                            _ => 0,
                        };
                        ui::row(ui, pal, false, "Largest share of new connections", Some("For a link that should carry less"), |ui| {
                            ui.add_enabled_ui(!self.busy, |ui| {
                                if let Some(k) = ui::segmented(ui, pal, &shares, cur) {
                                    let v = ["none", "25", "50", "75"][k];
                                    want = Some(vec!["set".into(), l.name.clone(), l.role.clone(), "--share".into(), v.into()]);
                                }
                            });
                        });
                        let per = if l.period == "day" { 0 } else { 1 };
                        ui::row(
                            ui,
                            pal,
                            false,
                            "Data cap",
                            Some("For mobile data: past it, this link is kept for backup only until the next day or month. Empty: no cap"),
                            |ui| {
                                ui.add_enabled_ui(!self.busy, |ui| {
                                    let mut cap = None;
                                    if ui::action(ui, pal, "Set").clicked() {
                                        cap = Some(per);
                                    }
                                    if let Some(k) = ui::segmented(ui, pal, &["Day", "Month"], per) {
                                        cap = Some(k);
                                    }
                                    ui.label("MB a");
                                    ui::field(ui, pal, &mut self.cap_text, "MB", 70.0, false);
                                    if let Some(k) = cap {
                                        let t = self.cap_text.trim();
                                        let v = if t.is_empty() || t == "0" { "none".to_string() } else { t.to_string() };
                                        if v == "none" || v.parse::<u64>().is_ok() {
                                            want = Some(vec![
                                                "set".into(),
                                                l.name.clone(),
                                                l.role.clone(),
                                                "--cap".into(),
                                                v,
                                                "--per".into(),
                                                ["day", "month"][k].into(),
                                            ]);
                                        } else {
                                            self.error = "The data cap is a number of megabytes.".into();
                                        }
                                    }
                                });
                            },
                        );
                    }
                }
            });
        }
        ui::footnote(
            ui,
            pal,
            "It helps when the connections reach the internet by different ways: two providers, or your provider and a phone's \
             mobile data. Wi-Fi and Ethernet from the same router share that router's one line, so together they are no faster.",
        );
        ui::footnote(
            ui,
            pal,
            "Each new connection goes over one of them and stays there, so one download over one connection is never faster; \
             many at once (a game store, a web page, a download manager) are. Sites see a different address per connection and \
             may ask you to log in again. With a VPN on there is nothing to spread: it pauses until the VPN is off. Off, \
             stopped or crashed, the normal routing is back at once. Untrusted Wi-Fi keeps all its protections.",
        );
        ui::footnote(
            ui,
            pal,
            "A phone as the second connection: USB tethering (iPhone: Personal Hotspot with the cable, tap Trust; it needs \
             usbmuxd. Android: USB tethering). Set a data cap for mobile data.",
        );
        if let Some(args) = want {
            self.act(ctx, args);
        }
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIVE: &str = r#"{"state":"active","reason":"","since":1,"new_links":"use","combined_rate":5250000,"links":[
        {"name":"wlan0","role":"use","up":true,"ipv4":true,"ipv6":true,"healthy4":true,"healthy6":true,"weight4":70,"weight6":null,
         "rtt_ms":12,"rate":4000000,"peak":5000000,"share_now":76,"share_limit":null,"data_cap_mb":null,"period":"month","used_bytes":0,"over_cap":false},
        {"name":"enp0s20f0u1","role":"use","up":true,"ipv4":true,"ipv6":false,"healthy4":true,"healthy6":false,"weight4":30,"weight6":null,
         "rtt_ms":40,"rate":1250000,"peak":1300000,"share_now":24,"share_limit":null,"data_cap_mb":2000,"period":"month","used_bytes":512000000,"over_cap":false}]}"#;

    #[test]
    fn the_status_reads() {
        let s = parse(ACTIVE).unwrap();
        assert_eq!(s.state, "active");
        assert_eq!(s.links.len(), 2);
        assert_eq!(describe(&s.links[0], &s.state), "Wi-Fi · 32 Mbit/s · 70 % of new connections");
        assert_eq!(
            describe(&s.links[1], &s.state),
            "USB (a phone, or an adapter) · 10 Mbit/s · 30 % of new connections · 512 of 2000 MB this month"
        );
        assert_eq!(mbit(s.combined_rate), "42 Mbit/s");
    }

    #[test]
    fn off_and_broken_answers() {
        let off = parse(r#"{"state":"off","reason":"","combined_rate":0,"links":[{"name":"wlan0","role":"use","up":true,"ipv4":true,"ipv6":true,"share_limit":null,"data_cap_mb":null,"period":"month"}]}"#).unwrap();
        assert_eq!(describe(&off.links[0], &off.state), "Wi-Fi");
        assert!(parse("not json").is_none());
    }

    #[test]
    fn kinds_from_names() {
        assert_eq!(kind("wlp3s0"), "Wi-Fi");
        assert_eq!(kind("eno2"), "Ethernet");
        assert_eq!(kind("enp5s0"), "Ethernet");
        assert_eq!(kind("enp0s20f0u1c4i2"), "USB (a phone, or an adapter)");
        assert_eq!(kind("bnep0"), "Bluetooth (a phone)");
        assert_eq!(mbit(150_000), "1.2 Mbit/s");
    }
}
