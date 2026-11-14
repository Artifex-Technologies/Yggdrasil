//! The Network step, right after Welcome: join a network before installing, or go on without one. Wired is shown when a cable is
//! in, the Wi-Fi networks in range when the computer has Wi-Fi (none of it when it has no adapter); a secured network asks for
//! its password, joined through NetworkManager's D-Bus call (yggdrasil-netjoin, the same as Settings': the password never on a
//! command line, a profile that failed is not kept). Nothing on this page blocks Next. When the computer is already online by
//! cable as Welcome is left, the page is not shown at all. A network joined here goes to the installed system with the rest of
//! the live session's NetworkManager profiles (yggdrasil-install-system).

use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, Align2, CornerRadius, FontId, RichText, Sense, Stroke};
use yggdrasil_netjoin::list::{nmcli, Snapshot, Wifi};
use yggdrasil_ui::theme::Palette;

/// What the page (and the summary) says while the computer is offline.
pub const OFFLINE_NOTE: &str = "Without a network the system may not get everything it needs: drivers and software for the devices \
     found here (Razer, Corsair, mice and keyboards), browsers that are not on this medium, and the latest updates. You can connect \
     later in Settings > Wi-Fi & Network; they come then.";

/// Where the computer is on the network.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Not read yet.
    Checking,
    /// Online by cable.
    Wired,
    /// Online through this Wi-Fi network.
    Wifi(String),
    /// Has network hardware, not connected.
    Offline,
    /// No wired or Wi-Fi adapter at all.
    NoHardware,
}

pub fn state(s: Option<&Snapshot>) -> State {
    let Some(s) = s else { return State::Checking };
    if s.wired_up() {
        State::Wired
    } else if let Some(n) = s.wifi_up() {
        State::Wifi(n)
    } else if s.devices.is_empty() {
        State::NoHardware
    } else {
        State::Offline
    }
}

impl State {
    /// The note about going on without a network: shown while offline (or with no network hardware), never while online or
    /// before the first look.
    pub fn note(&self) -> Option<&'static str> {
        matches!(self, State::Offline | State::NoHardware).then_some(OFFLINE_NOTE)
    }

    /// The summary's line.
    pub fn summary(&self) -> String {
        match self {
            State::Checking => String::new(),
            State::Wired => "Connected (wired)".into(),
            State::Wifi(n) => format!("Connected to “{n}” (Wi-Fi)"),
            State::Offline | State::NoHardware => "Not connected".into(),
        }
    }
}

/// How many of four bars a signal fills (as Settings and nmcli show it).
pub fn bars(signal: u8) -> usize {
    match signal {
        0..=25 => 1,
        26..=50 => 2,
        51..=75 => 3,
        _ => 4,
    }
}

/// Where the page's answers come from: NetworkManager, or (the demo, the tests) made-up ones.
#[derive(Clone, Copy)]
pub struct Backend {
    /// The adapters and the networks in range (may wait for a scan: asked on a thread).
    pub read: fn() -> Snapshot,
    /// The adapters only (quick: asked as Welcome is left, to know whether the page is needed).
    pub adapters: fn() -> Snapshot,
    /// Ask for a new scan (and wait a moment for it).
    pub rescan: fn(),
    /// Switch Wi-Fi on.
    pub radio_on: fn() -> Result<(), String>,
    /// Join a network (an empty password for an open one).
    pub join: fn(&str, &str) -> Result<(), String>,
}

fn system_read() -> Snapshot {
    Snapshot::read(nmcli)
}
fn system_adapters() -> Snapshot {
    Snapshot::adapters(nmcli)
}
fn system_rescan() {
    let _ = nmcli(&["device", "wifi", "rescan"]);
    std::thread::sleep(Duration::from_secs(3));
}
fn system_radio_on() -> Result<(), String> {
    nmcli(&["radio", "wifi", "on"]).map(|_| ()).ok_or_else(|| "Wi-Fi could not be switched on.".into())
}

/// NetworkManager on this computer.
pub const SYSTEM: Backend = Backend {
    read: system_read,
    adapters: system_adapters,
    rescan: system_rescan,
    radio_on: system_radio_on,
    join: yggdrasil_netjoin::join::connect,
};

/// The made-up computer of `--demo`: a Wi-Fi adapter, an Ethernet port without a cable, and networks with made-up names.
/// Nothing is asked of NetworkManager and nothing is joined.
pub fn demo_snapshot() -> Snapshot {
    let dev = |d: &str, k: &str, s: &str| yggdrasil_netjoin::list::NetDevice {
        device: d.into(),
        kind: k.into(),
        state: s.into(),
        connection: String::new(),
    };
    let net = |n: &str, signal: u8, secured: bool| Wifi { in_use: false, ssid: n.into(), signal, secured };
    Snapshot {
        wifi_on: true,
        devices: vec![dev("eth0", "ethernet", "unavailable"), dev("wlan0", "wifi", "disconnected")],
        networks: vec![
            net("Example-5G", 84, true),
            net("Example-Guest", 66, false),
            net("Cafe-Example", 47, true),
            net("Example-2G", 22, true),
        ],
    }
}
fn demo_rescan() {
    std::thread::sleep(Duration::from_millis(600));
}
fn demo_radio_on() -> Result<(), String> {
    Err("This is a demo: nothing is changed.".into())
}
fn demo_join(_: &str, _: &str) -> Result<(), String> {
    std::thread::sleep(Duration::from_millis(800));
    Err("This is a demo: nothing is joined.".into())
}
pub const DEMO: Backend =
    Backend { read: demo_snapshot, adapters: demo_snapshot, rescan: demo_rescan, radio_on: demo_radio_on, join: demo_join };

enum Msg {
    Read(Snapshot),
    Joined(String, Result<(), String>, Snapshot),
    Radio(Result<(), String>, Snapshot),
}

pub struct Net {
    pub backend: Backend,
    pub snap: Option<Snapshot>,
    read_at: Option<Instant>,
    /// The one read, scan or join under way.
    pending: Option<Receiver<Msg>>,
    /// The secured network whose password box is open, and what is typed in it.
    pub chosen: Option<String>,
    pub password: String,
    /// The network being joined.
    pub joining: Option<String>,
    /// The last join's outcome: (it worked, what to say).
    pub message: Option<(bool, String)>,
    /// Online by cable as Welcome was left: the page is not shown.
    pub skip_page: bool,
}

impl Net {
    pub fn new(backend: Backend) -> Net {
        Net {
            backend,
            snap: None,
            read_at: None,
            pending: None,
            chosen: None,
            password: String::new(),
            joining: None,
            message: None,
            skip_page: false,
        }
    }

    pub fn state(&self) -> State {
        state(self.snap.as_ref())
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Leaving Welcome: whether the page is needed. Asked of the adapters there and then (not of an older look), so a cable
    /// plugged in a moment ago counts; the page then stays as decided while it is on screen.
    pub fn decide(&mut self) {
        let a = (self.backend.adapters)();
        self.skip_page = a.wired_up();
        if self.skip_page {
            // the summary says "Connected (wired)" from this until the next look
            let networks = self.snap.take().map(|s| s.networks).unwrap_or_default();
            self.snap = Some(Snapshot { networks, ..a });
        }
    }

    fn run(&mut self, ctx: &egui::Context, work: impl FnOnce() -> Msg + Send + 'static) {
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }

    /// A new look at the network, on a thread (nothing when one is under way).
    pub fn refresh(&mut self, ctx: &egui::Context) {
        if self.pending.is_none() {
            let read = self.backend.read;
            self.read_at = Some(Instant::now());
            self.run(ctx, move || Msg::Read(read()));
        }
    }

    /// Takes in what a thread found; looks again when the last look is older than `every`.
    pub fn tick(&mut self, ctx: &egui::Context, every: Duration) {
        self.poll();
        if self.read_at.is_none_or(|t| t.elapsed() >= every) {
            self.refresh(ctx);
        }
        ctx.request_repaint_after(every);
    }

    pub fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        let Ok(m) = rx.try_recv() else { return };
        self.pending = None;
        match m {
            Msg::Read(s) => self.snap = Some(s),
            Msg::Radio(r, s) => {
                self.snap = Some(s);
                if let Err(e) = r {
                    self.message = Some((false, e));
                }
            }
            Msg::Joined(ssid, r, s) => {
                self.snap = Some(s);
                self.joining = None;
                self.read_at = Some(Instant::now());
                self.message = Some(match r {
                    Ok(()) => {
                        self.chosen = None;
                        (true, format!("Connected to “{ssid}”. The installed system keeps this network."))
                    }
                    Err(e) => (false, e),
                });
            }
        }
    }

    pub fn rescan(&mut self, ctx: &egui::Context) {
        if self.pending.is_none() {
            let (rescan, read) = (self.backend.rescan, self.backend.read);
            self.read_at = Some(Instant::now());
            self.run(ctx, move || {
                rescan();
                Msg::Read(read())
            });
        }
    }

    pub fn radio_on(&mut self, ctx: &egui::Context) {
        if self.pending.is_none() {
            let (on, read) = (self.backend.radio_on, self.backend.read);
            self.message = None;
            self.run(ctx, move || {
                let r = on();
                std::thread::sleep(Duration::from_secs(2));
                Msg::Radio(r, read())
            });
        }
    }

    /// Join `ssid` with what is typed (nothing for an open network). The password leaves this page with the join and is not kept.
    pub fn join(&mut self, ctx: &egui::Context, ssid: &str) {
        if self.pending.is_some() && self.joining.is_none() {
            // a look is under way: its answer is dropped, the join's own look follows it
            self.pending = None;
        }
        if self.pending.is_some() {
            return;
        }
        let pw = std::mem::take(&mut self.password);
        let (join, read) = (self.backend.join, self.backend.read);
        let ssid = ssid.to_string();
        self.joining = Some(ssid.clone());
        self.message = None;
        self.run(ctx, move || {
            let r = join(&ssid, &pw);
            Msg::Joined(ssid, r, read())
        });
    }
}

/// What the page asks of the installer.
#[derive(PartialEq, Eq, Debug)]
pub enum Act {
    None,
    /// "Continue without a network": on to the next step.
    Skip,
}

/// Four bars and, for a secured network, a lock, drawn at the right end of `rect`.
fn marks(p: &egui::Painter, rect: egui::Rect, pal: &Palette, w: &Wifi, fg: egui::Color32) {
    let lit = bars(w.signal);
    let base = egui::pos2(rect.right() - 34.0, rect.center().y + 8.0);
    for i in 0..4 {
        let h = 5.0 + 4.0 * i as f32;
        let x = base.x + 5.0 * i as f32;
        let bar = egui::Rect::from_min_max(egui::pos2(x, base.y - h), egui::pos2(x + 3.0, base.y));
        p.rect_filled(bar, CornerRadius::same(1), if i < lit { fg } else { pal.border });
    }
    if w.secured {
        let c = egui::pos2(rect.right() - 52.0, rect.center().y + 3.0);
        let body = egui::Rect::from_center_size(c, vec2(11.0, 8.0));
        p.rect_filled(body, CornerRadius::same(2), fg);
        let shackle = egui::Rect::from_min_max(egui::pos2(c.x - 3.5, c.y - 10.0), egui::pos2(c.x + 3.5, c.y - 2.0));
        p.rect_stroke(shackle, CornerRadius::same(3), Stroke::new(1.6, fg), egui::StrokeKind::Middle);
    }
}

fn wifi_row(ui: &mut egui::Ui, pal: &Palette, w: &Wifi, open: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(460.0, 36.0), Sense::click());
    let p = ui.painter();
    let fill = if w.in_use {
        pal.accent
    } else if r.hovered() || open {
        pal.hover
    } else {
        pal.field
    };
    p.rect_filled(rect, CornerRadius::same(8), fill);
    let fg = if w.in_use { pal.on_accent() } else { pal.text };
    let label = if w.in_use { format!("✔  {}", w.ssid) } else { w.ssid.clone() };
    p.text(rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(14.0), fg);
    if w.in_use {
        p.text(rect.right_center() - vec2(66.0, 0.0), Align2::RIGHT_CENTER, "Connected", FontId::proportional(12.0), fg);
    }
    marks(p, rect, pal, w, fg);
    if r.hovered() && !w.in_use {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r
}

/// The page itself.
pub fn page(ui: &mut egui::Ui, pal: &Palette, net: &mut Net) -> Act {
    let ctx = ui.ctx().clone();
    let st = net.state();
    let mut act = Act::None;
    match &st {
        State::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Looking for networks…").color(pal.dim));
            });
        }
        State::Wired => {
            ui.label(RichText::new("✔  Connected by cable (Ethernet)").strong().color(pal.green));
            ui.label(RichText::new("You are online. Press Next to go on.").color(pal.dim));
        }
        State::Wifi(n) => {
            ui.label(RichText::new(format!("✔  Connected to “{n}”")).strong().color(pal.green));
            ui.label(RichText::new("You are online. Press Next to go on.").color(pal.dim));
        }
        State::NoHardware => {
            ui.label(RichText::new("No network hardware was found on this computer.").color(pal.dim));
        }
        State::Offline => {}
    }
    let snap = net.snap.clone().unwrap_or_default();
    if !snap.wired_up() && snap.cable() {
        ui.add_space(4.0);
        ui.label(RichText::new("A network cable is plugged in; waiting for it to connect…").color(pal.dim));
    }
    if snap.has_wifi() {
        ui.add_space(12.0);
        ui.label(RichText::new("Wi-Fi").strong().color(pal.dim));
        ui.add_space(4.0);
        if !snap.wifi_on {
            ui.label("Wi-Fi is switched off.");
            if ui.add_enabled(!net.busy(), egui::Button::new("Turn on Wi-Fi").min_size(vec2(140.0, 30.0))).clicked() {
                net.radio_on(&ctx);
            }
        } else {
            if snap.networks.is_empty() {
                ui.label(RichText::new("No networks found yet.").color(pal.dim));
            }
            let mut click: Option<Wifi> = None;
            let mut join: Option<String> = None;
            let mut cancel = false;
            let joining = net.joining.is_some();
            for w in &snap.networks {
                let open = net.chosen.as_deref() == Some(w.ssid.as_str());
                if wifi_row(ui, pal, w, open).clicked() && !w.in_use && !joining {
                    click = Some(w.clone());
                }
                if open && !joining {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut net.password)
                                .hint_text(format!("Password for “{}”", w.ssid))
                                .password(true)
                                .desired_width(250.0)
                                .margin(vec2(8.0, 6.0)),
                        );
                        if ui.memory(|m| m.focused().is_none()) {
                            r.request_focus();
                        }
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let b = egui::Button::new(RichText::new("Join").color(pal.on_accent())).fill(pal.accent).min_size(vec2(70.0, 30.0));
                        if (ui.add_enabled(!net.password.is_empty(), b).clicked() || enter) && !net.password.is_empty() {
                            join = Some(w.ssid.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                }
                ui.add_space(4.0);
            }
            if cancel {
                net.chosen = None;
                net.password.clear();
            }
            if let Some(w) = click {
                net.message = None;
                if w.secured {
                    if net.chosen.as_deref() != Some(w.ssid.as_str()) {
                        net.password.clear();
                    }
                    net.chosen = Some(w.ssid);
                } else {
                    net.chosen = None;
                    join = Some(w.ssid);
                }
            }
            if let Some(ssid) = join {
                net.join(&ctx, &ssid);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!net.busy(), egui::Button::new("Scan again")).clicked() {
                    net.rescan(&ctx);
                }
                if net.busy() && net.joining.is_none() && net.snap.is_some() {
                    ui.spinner();
                }
            });
        }
    }
    if let Some(n) = &net.joining {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("Joining “{n}”…"));
        });
    }
    if let Some((ok, m)) = &net.message {
        ui.add_space(8.0);
        let text = if *ok { m.clone() } else { format!("{m} Nothing was saved; check it and try again.") };
        ui.add(egui::Label::new(RichText::new(text).color(if *ok { pal.green } else { pal.red })).wrap());
    }
    if let Some(note) = st.note() {
        ui.add_space(14.0);
        ui.add(egui::Label::new(RichText::new(note).color(pal.amber)).wrap());
        ui.add_space(8.0);
        if ui.add(egui::Button::new("Continue without a network").min_size(vec2(220.0, 32.0))).clicked() {
            act = Act::Skip;
        }
    }
    act
}

#[cfg(test)]
mod tests {
    use super::*;
    use yggdrasil_netjoin::list::NetDevice;

    fn dev(kind: &str, state: &str, conn: &str) -> NetDevice {
        NetDevice { device: format!("{kind}0"), kind: kind.into(), state: state.into(), connection: conn.into() }
    }

    fn snap(devices: Vec<NetDevice>, networks: Vec<Wifi>) -> Snapshot {
        Snapshot { wifi_on: true, devices, networks }
    }

    #[test]
    fn the_state_follows_the_adapters() {
        assert_eq!(state(None), State::Checking);
        assert_eq!(
            state(Some(&snap(vec![dev("ethernet", "connected", "Wired 1"), dev("wifi", "disconnected", "")], vec![]))),
            State::Wired
        );
        let wifi = snap(
            vec![dev("ethernet", "unavailable", ""), dev("wifi", "connected", "Example-5G")],
            vec![Wifi { in_use: true, ssid: "Example-5G".into(), signal: 80, secured: true }],
        );
        assert_eq!(state(Some(&wifi)), State::Wifi("Example-5G".into()));
        assert_eq!(state(Some(&snap(vec![dev("wifi", "disconnected", "")], vec![]))), State::Offline);
        assert_eq!(state(Some(&snap(vec![dev("ethernet", "disconnected", "")], vec![]))), State::Offline, "a cable, no Wi-Fi");
        assert_eq!(state(Some(&Snapshot::default())), State::NoHardware);
    }

    #[test]
    fn the_note_shows_while_offline_only() {
        assert_eq!(State::Offline.note(), Some(OFFLINE_NOTE));
        assert_eq!(State::NoHardware.note(), Some(OFFLINE_NOTE), "no network hardware: the same note");
        assert_eq!(State::Wired.note(), None);
        assert_eq!(State::Wifi("Example-5G".into()).note(), None);
        assert_eq!(State::Checking.note(), None, "not before the first look");
        for word in ["drivers", "Razer", "Corsair", "browsers", "updates", "Settings > Wi-Fi & Network"] {
            assert!(OFFLINE_NOTE.contains(word), "{word}");
        }
        assert_eq!(State::Wired.summary(), "Connected (wired)");
        assert_eq!(State::Offline.summary(), "Not connected");
    }

    #[test]
    fn signal_bars() {
        assert_eq!([0, 25, 26, 50, 51, 75, 76, 100].map(bars), [1, 1, 2, 2, 3, 3, 4, 4]);
    }

    #[test]
    fn the_demo_has_made_up_networks_and_joins_nothing() {
        let s = demo_snapshot();
        assert!(s.networks.iter().all(|w| w.ssid.contains("Example")));
        assert_eq!(state(Some(&s)), State::Offline);
        assert!(s.has_wifi() && !s.cable());
        assert!((DEMO.join)("Example-5G", "x").is_err());
        assert!((DEMO.radio_on)().is_err());
    }

    // ---- joining, with a made-up NetworkManager behind the page (never the real one)

    fn fake_read() -> Snapshot {
        snap(vec![dev("wifi", "disconnected", "")], vec![Wifi { in_use: false, ssid: "Example-5G".into(), signal: 70, secured: true }])
    }
    fn fake_join(ssid: &str, pw: &str) -> Result<(), String> {
        assert_eq!(ssid, "Example-5G");
        if pw == "right" {
            Ok(())
        } else {
            Err("The password was not accepted.".into())
        }
    }
    const FAKE: Backend = Backend { read: fake_read, adapters: fake_read, rescan: || {}, radio_on: || Ok(()), join: fake_join };

    fn wait(net: &mut Net) {
        let until = Instant::now() + Duration::from_secs(5);
        while net.busy() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
            net.poll();
        }
        assert!(!net.busy());
    }

    #[test]
    fn a_wrong_password_says_so_and_is_not_kept() {
        let ctx = egui::Context::default();
        let mut net = Net::new(FAKE);
        net.chosen = Some("Example-5G".into());
        net.password = "wrong".into();
        net.join(&ctx, "Example-5G");
        assert!(net.password.is_empty(), "the password leaves the page with the join");
        assert_eq!(net.joining.as_deref(), Some("Example-5G"));
        wait(&mut net);
        assert_eq!(net.joining, None);
        assert_eq!(net.message, Some((false, "The password was not accepted.".into())));
        assert_eq!(net.chosen.as_deref(), Some("Example-5G"), "the box stays open to try again");
        net.password = "right".into();
        net.join(&ctx, "Example-5G");
        wait(&mut net);
        assert!(net.message.as_ref().is_some_and(|(ok, m)| *ok && m.contains("Example-5G")));
        assert_eq!(net.chosen, None);
        assert!(net.password.is_empty());
    }
}
