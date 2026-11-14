//! The Network step, right after Welcome: join a network before installing, or go on without one. Wired is shown when a cable is
//! in, the Wi-Fi networks in range when the computer has Wi-Fi (none of it when it has no adapter); a secured network asks for
//! its password, joined through NetworkManager's D-Bus call (yggdrasil-netjoin, the same as Settings': the password never on a
//! command line, a profile that failed is not kept). Nothing on this page blocks Next. As Welcome is left the page is decided
//! (`decision`): not shown when the computer is online by cable, or has no Wi-Fi adapter at all. The adapters are read again
//! with every look, so a USB Wi-Fi adapter plugged in while the page is open shows up with its networks; with two adapters the
//! list is what both hear, and a join goes to the one that hears the network best. A network joined here goes to the installed
//! system with the rest of the live session's NetworkManager profiles (yggdrasil-install-system).

use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, Align2, CornerRadius, FontId, RichText, Sense, Stroke};
use yggdrasil_netjoin::join::Request;
use yggdrasil_netjoin::list::{nmcli, Snapshot, Wifi};
use yggdrasil_netjoin::mac::{self, Adapter};
use yggdrasil_ui::theme::Palette;

/// What the page (and the summary) says while the computer is offline.
pub const OFFLINE_NOTE: &str = "Without a network the system may not get everything it needs: drivers and software for the devices \
     found here (Razer, Corsair, mice and keyboards), browsers that are not on this medium, and the latest updates. You can connect \
     later in Settings > Wi-Fi & Network; they come then.";

/// Added on the summary when the computer is offline and has no Wi-Fi adapter.
pub const PLUG_IN: &str = "To connect now, plug in a network cable or a USB Wi-Fi adapter.";

/// Whether the page is needed, from the adapters as Welcome is left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Online by cable, or no Wi-Fi adapter (the page would have nothing to offer).
    Skip,
    Show,
    /// A cable is in and NetworkManager is still connecting it (getting an address): ask again in a moment.
    Wait,
}

pub fn decision(s: &Snapshot) -> Decision {
    if !s.has_wifi() || s.wired_up() {
        Decision::Skip
    } else if s.devices.iter().any(|d| d.kind == "ethernet" && d.state.starts_with("connecting")) {
        Decision::Wait
    } else {
        Decision::Show
    }
}

/// The adapters, asked again every `every` while a cable is still connecting, for at most `limit`.
pub fn settle(adapters: fn() -> Snapshot, limit: Duration, every: Duration) -> Snapshot {
    let until = Instant::now() + limit;
    loop {
        let s = adapters();
        if decision(&s) != Decision::Wait || Instant::now() >= until {
            return s;
        }
        std::thread::sleep(every);
    }
}

/// The summary's amber note: the offline note, and how to connect now when there is no Wi-Fi adapter.
pub fn summary_note(s: Option<&Snapshot>) -> Option<String> {
    let note = state(s).note()?;
    Some(if s.is_some_and(|s| s.has_wifi()) { note.to_string() } else { format!("{note} {PLUG_IN}") })
}

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
    /// Join a network (a hidden one too; with this computer's real address when asked).
    pub join: fn(&Request) -> Result<(), String>,
}

/// The adapters' addresses (over D-Bus, read only), asked again only when the adapters change (a USB one plugged in).
fn addresses_for(s: &Snapshot) -> Vec<Adapter> {
    static LAST: std::sync::Mutex<(Vec<String>, Vec<Adapter>)> = std::sync::Mutex::new((Vec::new(), Vec::new()));
    let names: Vec<String> = s.devices.iter().map(|d| d.device.clone()).collect();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.0 != names || last.1.is_empty() {
        *last = (names, mac::adapters());
    }
    last.1.clone()
}

fn system_read() -> Snapshot {
    let mut s = Snapshot::read(nmcli);
    s.addresses = addresses_for(&s);
    s.private_wifi = mac::nm_private_wifi();
    s
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
    join: yggdrasil_netjoin::join::join,
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
    // made-up, locally administered addresses
    let adapter = |d: &str, kind: &str, a: &str| Adapter {
        device: d.into(),
        kind: kind.into(),
        address: a.into(),
        permanent: a.into(),
        ..Adapter::default()
    };
    Snapshot {
        wifi_on: true,
        devices: vec![dev("eth0", "ethernet", "unavailable"), dev("wlan0", "wifi", "disconnected")],
        networks: vec![
            net("Example-5G", 84, true),
            net("Example-Guest", 66, false),
            net("Cafe-Example", 47, true),
            net("Example-2G", 22, true),
        ],
        addresses: vec![adapter("eth0", "ethernet", "02:00:00:00:00:10"), adapter("wlan0", "wifi", "02:00:00:00:00:01")],
        private_wifi: true,
    }
}
fn demo_rescan() {
    std::thread::sleep(Duration::from_millis(600));
}
fn demo_radio_on() -> Result<(), String> {
    Err("This is a demo: nothing is changed.".into())
}
fn demo_join(_: &Request) -> Result<(), String> {
    std::thread::sleep(Duration::from_millis(800));
    Err("This is a demo: nothing is joined.".into())
}
pub const DEMO: Backend =
    Backend { read: demo_snapshot, adapters: demo_snapshot, rescan: demo_rescan, radio_on: demo_radio_on, join: demo_join };

enum Msg {
    Read(Snapshot),
    /// The adapters only (the watch on the other pages): the networks and addresses stay as they were.
    Adapters(Snapshot),
    Joined(String, Result<(), String>, Snapshot),
    Radio(Result<(), String>, Snapshot),
}

pub struct Net {
    pub backend: Backend,
    pub snap: Option<Snapshot>,
    read_at: Option<Instant>,
    /// The one read, scan or join under way.
    pending: Option<Receiver<Msg>>,
    /// The open form: a network's password (and address choice), or a hidden network's name, security and password. The
    /// password leaves with the join and is not kept.
    pub form: Option<Request>,
    /// The network being joined.
    pub joining: Option<String>,
    /// The last join's outcome: (it worked, what to say).
    pub message: Option<(bool, String)>,
    /// Online by cable, or without a Wi-Fi adapter, as Welcome was left: the page is not shown.
    pub skip_page: bool,
    /// Skipped for want of a Wi-Fi adapter: a later look that finds one (a USB adapter plugged in) offers the page again.
    skipped_without_wifi: bool,
    /// The decision as Welcome is left, under way on a thread.
    deciding: Option<Receiver<Snapshot>>,
    /// A Wi-Fi adapter appeared since the last look (a USB one plugged in): the other pages offer to join a network.
    pub new_adapter: bool,
    /// A join worked (the page returns to where the person came from, when they came from further on).
    pub joined: bool,
    /// A form was just opened: the page scrolls it into view (on this many frames: the first one may not know the page's full
    /// height yet).
    pub scroll_to_form: u8,
}

impl Net {
    pub fn new(backend: Backend) -> Net {
        Net {
            backend,
            snap: None,
            read_at: None,
            pending: None,
            form: None,
            joining: None,
            message: None,
            skip_page: false,
            skipped_without_wifi: false,
            deciding: None,
            new_adapter: false,
            joined: false,
            scroll_to_form: 0,
        }
    }

    /// Whether the computer is online now.
    pub fn online(&self) -> bool {
        matches!(self.state(), State::Wired | State::Wifi(_))
    }

    pub fn state(&self) -> State {
        state(self.snap.as_ref())
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Leaving Welcome: whether the page is needed, asked of the adapters there and then (not of an older look), so a cable
    /// plugged in a moment ago counts; up to 5 s while a cable is still connecting. On a thread: Welcome shows a spinner until
    /// `decided` says it is done. The page then stays as decided while it is on screen.
    pub fn start_deciding(&mut self, ctx: &egui::Context) {
        if self.deciding.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.deciding = Some(rx);
        let (adapters, ctx) = (self.backend.adapters, ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(settle(adapters, Duration::from_secs(5), Duration::from_millis(250)));
            ctx.request_repaint();
        });
    }

    pub fn deciding(&self) -> bool {
        self.deciding.is_some()
    }

    /// True once, when the decision started by `start_deciding` has come.
    pub fn decided(&mut self) -> bool {
        let Some(s) = self.deciding.as_ref().and_then(|rx| rx.try_recv().ok()) else { return false };
        self.deciding = None;
        self.apply(s);
        true
    }

    /// The decision without a thread (the tests).
    #[cfg(test)]
    pub fn decide_now(&mut self, limit: Duration, every: Duration) {
        self.apply(settle(self.backend.adapters, limit, every));
    }

    fn apply(&mut self, a: Snapshot) {
        self.skip_page = decision(&a) == Decision::Skip;
        self.skipped_without_wifi = self.skip_page && !a.has_wifi();
        if self.skip_page {
            // the summary speaks from this until the next look
            let networks = self.snap.take().map(|s| s.networks).unwrap_or_default();
            self.snap = Some(Snapshot { networks, ..a });
        }
    }

    fn take(&mut self, s: Snapshot) {
        let wifi = |s: &Snapshot| s.devices.iter().filter(|d| d.kind == "wifi").count();
        if self.snap.as_ref().is_some_and(|old| wifi(&s) > wifi(old)) {
            self.new_adapter = true;
        }
        if self.skipped_without_wifi && s.has_wifi() {
            // a Wi-Fi adapter appeared after the page was skipped for want of one: the page is offered again
            self.skip_page = false;
            self.skipped_without_wifi = false;
        }
        self.snap = Some(s);
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

    /// On the pages after this one: the adapters only (quick, no scan), every `every`, so a cable or a USB Wi-Fi adapter plugged
    /// in later is noticed (the notes go, the summary's line changes, a page offers to join).
    pub fn watch(&mut self, ctx: &egui::Context, every: Duration) {
        self.poll();
        if self.pending.is_none() && self.read_at.is_none_or(|t| t.elapsed() >= every) {
            let adapters = self.backend.adapters;
            self.read_at = Some(Instant::now());
            self.run(ctx, move || Msg::Adapters(adapters()));
        }
        ctx.request_repaint_after(every);
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
            Msg::Read(s) => self.take(s),
            Msg::Adapters(a) => {
                let old = self.snap.clone().unwrap_or_default();
                let addresses =
                    if a.devices.iter().map(|d| &d.device).eq(old.devices.iter().map(|d| &d.device)) { old.addresses } else { vec![] };
                self.take(Snapshot { networks: old.networks, addresses, private_wifi: old.private_wifi, ..a });
            }
            Msg::Radio(r, s) => {
                self.take(s);
                if let Err(e) = r {
                    self.message = Some((false, e));
                }
            }
            Msg::Joined(ssid, r, s) => {
                self.take(s);
                self.joining = None;
                self.read_at = Some(Instant::now());
                self.message = Some(match r {
                    Ok(()) => {
                        self.form = None;
                        self.joined = true;
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

    /// Join what the form says (an open network: `Request::new(name, "")`). The password leaves this page with the join and is
    /// not kept; the form stays open (empty password) to try again when it did not work.
    pub fn join(&mut self, ctx: &egui::Context, r: Request) {
        if self.pending.is_some() && self.joining.is_none() {
            // a look is under way: its answer is dropped, the join's own look follows it
            self.pending = None;
        }
        if self.pending.is_some() {
            return;
        }
        if let Some(f) = &mut self.form {
            f.password.clear();
        }
        let (join, read) = (self.backend.join, self.backend.read);
        let ssid = r.ssid.clone();
        self.joining = Some(ssid.clone());
        self.message = None;
        self.run(ctx, move || {
            let done = join(&r);
            Msg::Joined(ssid, done, read())
        });
    }
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

fn row_frame(ui: &mut egui::Ui, pal: &Palette, accent: bool, open: bool) -> (egui::Rect, egui::Response) {
    let (rect, r) = ui.allocate_exact_size(vec2(460.0, 36.0), Sense::click());
    let fill = if accent {
        pal.accent
    } else if r.hovered() || open {
        pal.hover
    } else {
        pal.field
    };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    if r.hovered() && !accent {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    (rect, r)
}

fn wifi_row(ui: &mut egui::Ui, pal: &Palette, w: &Wifi, open: bool) -> egui::Response {
    let (rect, r) = row_frame(ui, pal, w.in_use, open);
    let p = ui.painter();
    let fg = if w.in_use { pal.on_accent() } else { pal.text };
    let label = if w.in_use { format!("✔  {}", w.ssid) } else { w.ssid.clone() };
    p.text(rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(14.0), fg);
    if w.in_use {
        p.text(rect.right_center() - vec2(66.0, 0.0), Align2::RIGHT_CENTER, "Connected", FontId::proportional(12.0), fg);
    }
    marks(p, rect, pal, w, fg);
    r
}

/// An address with a Copy button beside it.
fn address_line(ui: &mut egui::Ui, pal: &Palette, text: &str, mac: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{text} {mac}")).color(pal.dim));
        if ui.small_button("Copy").on_hover_text("Copy the address").clicked() {
            ui.ctx().copy_text(mac.to_string());
        }
    });
}

enum FormAct {
    None,
    Join,
    Cancel,
}

/// The form under a network's row, or the hidden network's: its name and security (hidden), the password (when it asks for
/// one), the address the network will see with the choice of this computer's real one (while the system gives each network a
/// private address), Join and Cancel.
fn form(ui: &mut egui::Ui, pal: &Palette, f: &mut Request, snap: &Snapshot, scroll: &mut u8) -> FormAct {
    let mut act = FormAct::None;
    let asks = if f.hidden { f.secured } else { snap.networks.iter().find(|w| w.ssid == f.ssid).is_none_or(|w| w.secured) };
    let shown = egui::Frame::new().inner_margin(egui::Margin { left: 12, right: 0, top: 6, bottom: 6 }).show(ui, |ui| {
        let mut enter = false;
        if f.hidden {
            let name =
                ui.add(egui::TextEdit::singleline(&mut f.ssid).hint_text("Network name").desired_width(300.0).margin(vec2(8.0, 6.0)));
            if f.ssid.is_empty() && ui.memory(|m| m.focused().is_none()) {
                name.request_focus();
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Security");
                for (secured, label) in [(false, "None"), (true, "WPA/WPA2/WPA3 Personal")] {
                    let on = f.secured == secured;
                    let b = egui::Button::new(RichText::new(label).color(if on { pal.on_accent() } else { pal.text })).fill(if on {
                        pal.accent
                    } else {
                        pal.field
                    });
                    if ui.add(b).clicked() {
                        f.secured = secured;
                    }
                }
            });
            ui.add_space(4.0);
        }
        if asks {
            let hint = if f.hidden || f.ssid.is_empty() { "Password".to_string() } else { format!("Password for “{}”", f.ssid) };
            let r = ui.add(
                egui::TextEdit::singleline(&mut f.password).hint_text(hint).password(true).desired_width(300.0).margin(vec2(8.0, 6.0)),
            );
            if ui.memory(|m| m.focused().is_none()) && !(f.hidden && f.ssid.is_empty()) {
                r.request_focus();
            }
            enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(4.0);
        }
        if snap.private_wifi {
            ui.checkbox(&mut f.real_address, "Use this computer's real address (for networks that reserve or allow addresses)");
        }
        let seen = mac::seen_by_network(&snap.addresses, snap.private_wifi, f.real_address);
        let many = seen.len() > 1;
        for (label, m) in seen {
            match m {
                Some(m) => address_line(
                    ui,
                    pal,
                    &if many { format!("{label}: address (MAC)") } else { "This computer's Wi-Fi address (MAC):".into() },
                    &m,
                ),
                None => {
                    let what = if many {
                        format!("{label}: a private address of its own for this network")
                    } else {
                        "A private address of its own for this network (made as it joins).".into()
                    };
                    ui.label(RichText::new(what).color(pal.dim));
                }
            }
        }
        ui.add_space(4.0);
        let ready = !f.ssid.trim().is_empty() && (!asks || !f.password.is_empty());
        ui.horizontal(|ui| {
            let b = egui::Button::new(RichText::new("Join").color(pal.on_accent())).fill(pal.accent).min_size(vec2(70.0, 30.0));
            if (ui.add_enabled(ready, b).clicked() || enter) && ready {
                act = FormAct::Join;
            }
            if ui.button("Cancel").clicked() {
                act = FormAct::Cancel;
            }
        });
    });
    if *scroll > 0 {
        *scroll -= 1;
        shown.response.scroll_to_me(Some(egui::Align::BOTTOM));
    }
    act
}

/// The page itself.
pub fn page(ui: &mut egui::Ui, pal: &Palette, net: &mut Net) {
    let ctx = ui.ctx().clone();
    let st = net.state();
    let snap = net.snap.clone().unwrap_or_default();
    let address_of = |device: &str| snap.addresses.iter().find(|a| a.device == device).map(|a| a.address.clone()).filter(|a| !a.is_empty());
    match &st {
        State::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Looking for networks…").color(pal.dim));
            });
        }
        State::Wired => {
            ui.label(RichText::new("✔  Connected by cable (Ethernet)").strong().color(pal.green));
            let wired = snap.devices.iter().find(|d| d.kind == "ethernet" && d.state.starts_with("connected"));
            if let Some(m) = wired.and_then(|d| address_of(&d.device)) {
                address_line(ui, pal, "Wired address (MAC):", &m);
            }
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
    if !snap.wired_up() && snap.cable() {
        ui.add_space(4.0);
        ui.label(RichText::new("A network cable is plugged in; waiting for it to connect…").color(pal.dim));
        if let Some(m) = snap
            .devices
            .iter()
            .find(|d| d.kind == "ethernet" && yggdrasil_netjoin::list::cable_in(&d.state))
            .and_then(|d| address_of(&d.device))
        {
            address_line(ui, pal, "Wired address (MAC):", &m);
        }
    }
    if snap.has_wifi() {
        ui.add_space(12.0);
        let adapters = snap.devices.iter().filter(|d| d.kind == "wifi").count();
        ui.label(RichText::new("Wi-Fi").strong().color(pal.dim));
        if adapters > 1 {
            ui.label(
                RichText::new(format!(
                    "{adapters} Wi-Fi adapters: the networks either one hears; a network is joined on the one that hears it best."
                ))
                .color(pal.dim),
            );
        }
        // each adapter's own address, for networks that reserve or allow addresses
        for a in snap.addresses.iter().filter(|a| a.kind == "wifi" && !a.real().is_empty()) {
            address_line(ui, pal, &format!("{} · address (MAC)", a.label()), a.real());
        }
        if snap.private_wifi && !snap.addresses.is_empty() {
            ui.label(
                RichText::new(
                    "Networks see a private address of their own unless “Use this computer's real address” is ticked when joining.",
                )
                .color(pal.dim),
            );
        }
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
            let mut act = FormAct::None;
            let joining = net.joining.is_some();
            for w in &snap.networks {
                let open = net.form.as_ref().is_some_and(|f| !f.hidden && f.ssid == w.ssid);
                if wifi_row(ui, pal, w, open).clicked() && !w.in_use && !joining {
                    click = Some(w.clone());
                }
                if let (true, false, Some(f)) = (open, joining, net.form.as_mut()) {
                    act = form(ui, pal, f, &snap, &mut net.scroll_to_form);
                }
                ui.add_space(4.0);
            }
            // a network that does not broadcast its name: joined by its name
            let hidden_open = net.form.as_ref().is_some_and(|f| f.hidden);
            let (rect, r) = row_frame(ui, pal, false, hidden_open);
            ui.painter().text(
                rect.left_center() + vec2(12.0, 0.0),
                Align2::LEFT_CENTER,
                "Hidden network…",
                FontId::proportional(14.0),
                pal.text,
            );
            if r.clicked() && !joining {
                net.message = None;
                net.form = Some(Request { hidden: true, secured: true, ..Request::default() });
                net.scroll_to_form = 2;
            }
            if let (true, false, Some(f)) = (hidden_open, joining, net.form.as_mut()) {
                act = form(ui, pal, f, &snap, &mut net.scroll_to_form);
            }
            match act {
                FormAct::Join => {
                    if let Some(f) = net.form.clone() {
                        net.join(&ctx, f);
                    }
                }
                FormAct::Cancel => net.form = None,
                FormAct::None => {}
            }
            if let Some(w) = click {
                net.message = None;
                if w.secured || snap.private_wifi {
                    // a password to type, or the address to choose first
                    if !net.form.as_ref().is_some_and(|f| !f.hidden && f.ssid == w.ssid) {
                        net.form = Some(Request::new(&w.ssid, ""));
                        net.scroll_to_form = 2;
                    }
                } else {
                    net.form = None;
                    net.join(&ctx, Request::new(&w.ssid, ""));
                }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use yggdrasil_netjoin::list::NetDevice;

    fn dev(kind: &str, state: &str, conn: &str) -> NetDevice {
        NetDevice { device: format!("{kind}0"), kind: kind.into(), state: state.into(), connection: conn.into() }
    }

    fn snap(devices: Vec<NetDevice>, networks: Vec<Wifi>) -> Snapshot {
        Snapshot { wifi_on: true, devices, networks, ..Snapshot::default() }
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
        assert!((DEMO.join)(&Request::new("Example-5G", "x")).is_err());
        assert!(s.addresses.iter().all(|a| a.real().starts_with("02:00:00:00:00:")), "made-up, locally administered addresses");
        assert!((DEMO.radio_on)().is_err());
    }

    #[test]
    fn the_rules_as_welcome_is_left() {
        let wifi = || dev("wifi", "disconnected", "");
        assert_eq!(decision(&snap(vec![dev("ethernet", "connected", "Wired 1"), wifi()], vec![])), Decision::Skip, "online by cable");
        assert_eq!(
            decision(&snap(vec![dev("ethernet", "connecting (getting IP configuration)", "Wired 1"), wifi()], vec![])),
            Decision::Wait,
            "a cable in, still getting its address"
        );
        assert_eq!(decision(&snap(vec![dev("ethernet", "connecting (prepare)", "")], vec![])), Decision::Skip, "no Wi-Fi: no wait");
        assert_eq!(decision(&snap(vec![dev("ethernet", "connected", "Wired 1")], vec![])), Decision::Skip);
        assert_eq!(decision(&snap(vec![dev("ethernet", "unavailable", "")], vec![])), Decision::Skip, "no Wi-Fi, no cable");
        assert_eq!(decision(&Snapshot::default()), Decision::Skip, "no network hardware at all");
        assert_eq!(decision(&snap(vec![dev("ethernet", "unavailable", ""), wifi()], vec![])), Decision::Show);
        assert_eq!(
            decision(&snap(vec![dev("ethernet", "disconnected", ""), wifi()], vec![])),
            Decision::Show,
            "a cable that did not connect"
        );
        assert_eq!(decision(&snap(vec![wifi()], vec![])), Decision::Show);
    }

    static CONNECTING_CALLS: AtomicUsize = AtomicUsize::new(0);
    /// A cable that is still connecting for the first two looks, then connected.
    fn connects_on_the_third_look() -> Snapshot {
        let state = if CONNECTING_CALLS.fetch_add(1, Ordering::SeqCst) < 2 { "connecting (getting IP configuration)" } else { "connected" };
        snap(vec![dev("ethernet", state, "Wired 1"), dev("wifi", "disconnected", "")], vec![])
    }
    fn never_connects() -> Snapshot {
        snap(vec![dev("ethernet", "connecting (getting IP configuration)", "Wired 1"), dev("wifi", "disconnected", "")], vec![])
    }

    #[test]
    fn a_cable_still_connecting_is_waited_for_a_moment() {
        let ms = Duration::from_millis;
        let mut net = Net::new(Backend { adapters: connects_on_the_third_look, ..FAKE });
        net.decide_now(ms(5000), ms(1));
        assert!(net.skip_page, "it connected while Welcome waited: no page");
        assert_eq!(CONNECTING_CALLS.load(Ordering::SeqCst), 3);
        assert_eq!(net.state(), State::Wired);
        let mut net = Net::new(Backend { adapters: never_connects, ..FAKE });
        let t = Instant::now();
        net.decide_now(ms(60), ms(5));
        assert!(t.elapsed() >= ms(60) && t.elapsed() < ms(2000), "waits its limit, no longer");
        assert!(!net.skip_page, "still connecting after the wait: the page is shown");
    }

    static PLUG_CALLS: AtomicUsize = AtomicUsize::new(0);
    /// The built-in adapter alone for the first look; then a USB adapter beside it, hearing a network of its own.
    fn usb_adapter_plugged_in() -> Snapshot {
        let net = |n: &str, signal: u8| Wifi { in_use: false, ssid: n.into(), signal, secured: true };
        if PLUG_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            snap(vec![dev("wifi", "disconnected", "")], vec![net("Example-5G", 40)])
        } else {
            let usb = NetDevice { device: "wlan1".into(), kind: "wifi".into(), state: "disconnected".into(), connection: String::new() };
            snap(vec![dev("wifi", "disconnected", ""), usb], vec![net("Example-5G", 70), net("Example-Far", 55)])
        }
    }

    #[test]
    fn an_adapter_plugged_in_while_the_page_is_open_shows_up() {
        let ctx = egui::Context::default();
        let mut net = Net::new(Backend { read: usb_adapter_plugged_in, ..FAKE });
        let wifi_count = |n: &Net| n.snap.as_ref().map(|s| s.devices.iter().filter(|d| d.kind == "wifi").count());
        net.tick(&ctx, Duration::ZERO);
        wait(&mut net);
        assert_eq!(wifi_count(&net), Some(1));
        assert_eq!(net.snap.as_ref().unwrap().networks.len(), 1);
        // the next look (the page's 4 s refresh) reads the adapters again
        net.tick(&ctx, Duration::ZERO);
        wait(&mut net);
        assert_eq!(wifi_count(&net), Some(2), "the USB adapter is there");
        let names: Vec<String> = net.snap.as_ref().unwrap().networks.iter().map(|w| w.ssid.clone()).collect();
        assert_eq!(names, ["Example-5G", "Example-Far"], "the networks either adapter hears");
    }

    // ---- joining, with a made-up NetworkManager behind the page (never the real one)

    fn fake_read() -> Snapshot {
        snap(vec![dev("wifi", "disconnected", "")], vec![Wifi { in_use: false, ssid: "Example-5G".into(), signal: 70, secured: true }])
    }
    fn fake_join(r: &Request) -> Result<(), String> {
        let (ssid, pw) = (r.ssid.as_str(), r.password.as_str());
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
        net.form = Some(Request::new("Example-5G", "wrong"));
        net.join(&ctx, net.form.clone().unwrap());
        assert_eq!(net.form.as_ref().unwrap().password, "", "the password leaves the page with the join");
        assert_eq!(net.joining.as_deref(), Some("Example-5G"));
        wait(&mut net);
        assert_eq!(net.joining, None);
        assert_eq!(net.message, Some((false, "The password was not accepted.".into())));
        assert_eq!(net.form.as_ref().map(|f| f.ssid.as_str()), Some("Example-5G"), "the form stays open to try again");
        assert!(!net.joined);
        net.form.as_mut().unwrap().password = "right".into();
        net.join(&ctx, net.form.clone().unwrap());
        wait(&mut net);
        assert!(net.message.as_ref().is_some_and(|(ok, m)| *ok && m.contains("Example-5G")));
        assert_eq!(net.form, None);
        assert!(net.joined, "a join that worked: the page may return to where the person came from");
    }

    static ASKED: std::sync::Mutex<Vec<Request>> = std::sync::Mutex::new(Vec::new());
    fn recording_join(r: &Request) -> Result<(), String> {
        ASKED.lock().unwrap().push(r.clone());
        Ok(())
    }

    #[test]
    fn a_hidden_network_and_the_real_address_go_to_the_join() {
        let ctx = egui::Context::default();
        let mut net = Net::new(Backend { join: recording_join, ..FAKE });
        let hidden = Request { ssid: "Example-Hidden".into(), password: "pw".into(), hidden: true, secured: true, real_address: true };
        net.form = Some(hidden.clone());
        net.join(&ctx, hidden.clone());
        wait(&mut net);
        assert_eq!(ASKED.lock().unwrap().as_slice(), [hidden], "name, security, password and the address choice, as typed");
        assert!(net.joined && net.form.is_none());
    }

    static WATCH_CALLS: AtomicUsize = AtomicUsize::new(0);
    /// What the watch on the later pages sees: offline with the built-in adapter, then a USB Wi-Fi adapter, then a cable.
    fn later_on() -> Snapshot {
        let usb = NetDevice { device: "wlan1".into(), kind: "wifi".into(), state: "disconnected".into(), connection: String::new() };
        match WATCH_CALLS.fetch_add(1, Ordering::SeqCst) {
            0 => snap(vec![dev("ethernet", "unavailable", ""), dev("wifi", "disconnected", "")], vec![]),
            1 => snap(vec![dev("ethernet", "unavailable", ""), dev("wifi", "disconnected", ""), usb], vec![]),
            _ => snap(vec![dev("ethernet", "connected", "Wired 1"), dev("wifi", "disconnected", ""), usb], vec![]),
        }
    }

    #[test]
    fn the_later_pages_notice_an_adapter_or_a_cable() {
        let ctx = egui::Context::default();
        let mut net = Net::new(Backend { adapters: later_on, ..FAKE });
        let known = Wifi { in_use: false, ssid: "Example-5G".into(), signal: 70, secured: true };
        net.snap = Some(Snapshot { networks: vec![known.clone()], private_wifi: true, ..later_on() });
        net.watch(&ctx, Duration::ZERO);
        wait(&mut net);
        assert!(net.new_adapter, "a USB Wi-Fi adapter plugged in");
        assert_eq!(net.state(), State::Offline);
        let s = net.snap.clone().unwrap();
        assert_eq!(s.networks, [known], "the watch asks for the adapters only: the networks stay");
        assert!(s.private_wifi);
        net.new_adapter = false;
        net.watch(&ctx, Duration::ZERO);
        wait(&mut net);
        assert_eq!(net.state(), State::Wired, "a cable plugged in and connected");
        assert!(net.online() && net.state().note().is_none(), "the notes go");
        assert!(!net.new_adapter);
    }
}
