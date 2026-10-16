//! Labyrinth Settings: the everyday settings in one clean app, in the style of GNOME Settings,
//! with the same look as Minos (labyrinth-ui). It changes Plasma's settings through the tools
//! Plasma itself uses (sys.rs). KDE's System Settings is hidden (labyrinth-plasma-setup) and its
//! command redirects here (/usr/local/bin/systemsettings); the few deep options not covered yet
//! open one KDE panel on its own (kcmshell6), never the whole KDE app.
//!
//! Reading the system can be slow (Wi-Fi scans, Bluetooth), so every page loads on a background
//! thread and every change runs on one too; the window never freezes.

mod appmgr;
mod apps_ui;
mod icons;
mod sys;
mod ui;

/// Tests that change HOME or XDG_* (process-wide) take this lock, so they run one at a time.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use eframe::egui::{self, vec2, CornerRadius, Margin, RichText, Sense};
use icons::Icon;
use labyrinth_ui::{chrome, theme};
use theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Network,
    Bluetooth,
    Displays,
    Sound,
    Appearance,
    Power,
    Firewall,
    DateTime,
    Apps,
    Shortcuts,
    About,
}

const PAGES: [(Page, &str, Icon); 11] = [
    (Page::Network, "Wi-Fi & Network", Icon::Wifi),
    (Page::Bluetooth, "Bluetooth", Icon::Bluetooth),
    (Page::Displays, "Displays", Icon::Display),
    (Page::Sound, "Sound", Icon::Sound),
    (Page::Appearance, "Appearance", Icon::Appearance),
    (Page::Power, "Power", Icon::Power),
    (Page::Firewall, "Firewall", Icon::Shield),
    (Page::DateTime, "Date & Time", Icon::Clock),
    (Page::Apps, "Apps", Icon::Apps),
    (Page::Shortcuts, "Keyboard Shortcuts", Icon::Keyboard),
    (Page::About, "About", Icon::Info),
];

enum Msg {
    About(sys::About),
    Network(sys::Network),
    Bluetooth(sys::Bluetooth),
    Displays(sys::Displays),
    Sound(sys::Sound),
    Appearance(sys::Appearance),
    Power(sys::Power),
    Firewall(bool),
    Time(sys::Time),
    Apps(Vec<sys::Component>),
    Installed(Vec<appmgr::AppInfo>),
    AppSizes(std::collections::HashMap<String, u64>),
    DirSize(std::path::PathBuf, u64),
    Done(Page, Result<(), String>),
}

#[derive(Default)]
struct Data {
    about: Option<sys::About>,
    network: Option<sys::Network>,
    bluetooth: Option<sys::Bluetooth>,
    displays: Option<sys::Displays>,
    sound: Option<sys::Sound>,
    appearance: Option<sys::Appearance>,
    power: Option<sys::Power>,
    firewall: Option<bool>,
    time: Option<sys::Time>,
    apps: Option<Vec<sys::Component>>,
    installed: Option<Vec<appmgr::AppInfo>>,
}

struct App {
    page: Page,
    search: String,
    data: Data,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    loading: Option<Page>,
    status: String,
    wifi_password: Option<(String, String)>,
    zone_filter: String,
    /// What the icon-pack install box holds: a folder, an archive or a link.
    icon_src: String,
    // Apps page
    app_tab: usize,
    app_sort: usize,
    app_sel: Option<String>,
    // Developer options: logo clicks (while the ` key is held) toward the unlock, and its confirmation
    dev_clicks: u8,
    dev_last: std::time::Instant,
    dev_prompt: bool,
    app_filter: String,
    app_system: bool,
    app_folder: String,
    app_confirm: Option<apps_ui::Confirm>,
    app_sizes: std::collections::HashMap<String, u64>,
    dir_sizes: std::collections::HashMap<std::path::PathBuf, u64>,
    owners: std::collections::HashMap<String, Option<(String, bool)>>,
    /// The shape and colouring chosen for the Labyrinth Brands pack (indexes into BRAND_SHAPES and BRAND_STYLES).
    brand_shape: usize,
    brand_style: usize,
    /// When the open page was last read, so it can be read again now and then and follow changes made elsewhere.
    refreshed: std::time::Instant,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let mut app = App {
            page: start_page(),
            search: String::new(),
            data: Data::default(),
            tx,
            rx,
            loading: None,
            status: String::new(),
            wifi_password: None,
            zone_filter: String::new(),
            icon_src: String::new(),
            app_tab: 0,
            app_sort: 0,
            app_sel: None,
            dev_clicks: 0,
            dev_last: std::time::Instant::now(),
            dev_prompt: false,
            app_filter: String::new(),
            app_system: false,
            app_folder: String::new(),
            app_confirm: None,
            app_sizes: Default::default(),
            dir_sizes: Default::default(),
            owners: Default::default(),
            brand_shape: 0,
            brand_style: 0,
            refreshed: std::time::Instant::now(),
        };
        // `labyrinth-settings apps:ID` opens that app's own page (links from the dock or the notch)
        if let Some(id) = std::env::args().nth(1).and_then(|a| a.strip_prefix("apps:").map(String::from)).filter(|s| !s.is_empty()) {
            app.app_sel = Some(id);
        }
        app.load(app.page, ctx);
        app
    }

    /// Read a page's state on a background thread.
    fn load(&mut self, page: Page, ctx: &egui::Context) {
        self.refreshed = std::time::Instant::now();
        self.loading = Some(page);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match page {
                Page::About => Msg::About(sys::about()),
                Page::Network => Msg::Network(sys::network()),
                Page::Bluetooth => Msg::Bluetooth(sys::bluetooth()),
                Page::Displays => Msg::Displays(sys::displays()),
                Page::Sound => Msg::Sound(sys::sound()),
                Page::Appearance => Msg::Appearance(sys::appearance()),
                Page::Power => Msg::Power(sys::power()),
                Page::Firewall => Msg::Firewall(sys::firewall_on()),
                Page::DateTime => Msg::Time(sys::time()),
                Page::Apps => {
                    let _ = tx.send(Msg::Installed(load_installed(&tx)));
                    Msg::Apps(sys::apps())
                }
                Page::Shortcuts => return,
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Run a change on a background thread, then reload the page.
    fn change(&mut self, ctx: &egui::Context, f: impl FnOnce() -> Result<(), String> + Send + 'static) {
        let tx = self.tx.clone();
        let page = self.page;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = f();
            let _ = tx.send(Msg::Done(page, r));
            ctx.request_repaint();
        });
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::About(v) => self.data.about = Some(v),
                Msg::Network(v) => self.data.network = Some(v),
                Msg::Bluetooth(v) => self.data.bluetooth = Some(v),
                Msg::Displays(v) => self.data.displays = Some(v),
                Msg::Sound(v) => self.data.sound = Some(v),
                Msg::Appearance(v) => self.data.appearance = Some(v),
                Msg::Power(v) => self.data.power = Some(v),
                Msg::Firewall(v) => self.data.firewall = Some(v),
                Msg::Time(v) => self.data.time = Some(v),
                Msg::Apps(v) => self.data.apps = Some(v),
                Msg::Installed(v) => self.data.installed = Some(v),
                Msg::AppSizes(m) => {
                    self.app_sizes = m;
                    continue;
                }
                Msg::DirSize(p, n) => {
                    self.dir_sizes.insert(p, n);
                    continue;
                }
                Msg::Done(page, r) => {
                    self.status = match r {
                        Ok(()) => String::new(),
                        Err(e) if e.is_empty() => "That did not work.".into(),
                        Err(e) => e,
                    };
                    self.load(page, ctx);
                    continue;
                }
            }
            self.loading = None;
        }
    }
}

/// `labyrinth-settings displays` opens on a page (for links from the top bar or other apps).
fn start_page() -> Page {
    let arg = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    let arg = arg.split(':').next().unwrap_or_default().to_string();
    PAGES.iter().find(|(p, name, _)| format!("{p:?}").to_lowercase() == arg || name.to_lowercase().starts_with(&arg) && !arg.is_empty())
        .map(|(p, _, _)| *p)
        .unwrap_or(Page::Network)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.receive(&ctx);
        // Read the open page again every few seconds, so a change made somewhere else (Fylgja's dark mode, the
        // panel, a command) shows up without closing Settings. Not while something is being dragged or typed, not
        // for pages that are slow to read (Wi-Fi scans, Bluetooth discovery, the package list), and not when
        // the window is in the background.
        let refreshable = matches!(self.page, Page::About | Page::Displays | Page::Sound | Page::Appearance | Page::Power | Page::Firewall | Page::DateTime);
        if refreshable {
            let idle = self.loading.is_none() && !ctx.input(|i| i.pointer.any_down()) && ctx.memory(|m| m.focused().is_none()) && ctx.input(|i| i.viewport().focused != Some(false));
            if idle && self.refreshed.elapsed() >= std::time::Duration::from_secs(4) {
                let page = self.page;
                self.load(page, &ctx);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(4));
        } else if self.page == Page::Apps && self.app_tab == 0 && self.app_confirm.is_none() {
            // the app list: running or not, new launches, and changes made from the command line
            let idle = self.loading.is_none() && !ctx.input(|i| i.pointer.any_down()) && ctx.memory(|m| m.focused().is_none()) && ctx.input(|i| i.viewport().focused != Some(false));
            if idle && self.refreshed.elapsed() >= std::time::Duration::from_secs(4) {
                self.refreshed = std::time::Instant::now();
                let (tx, c) = (self.tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let theme = sys::run("kreadconfig6", &["--file", "kdeglobals", "--group", "Icons", "--key", "Theme"]).map(|s| s.trim().to_string()).unwrap_or_default();
                    let _ = tx.send(Msg::Installed(appmgr::list_apps(&theme)));
                    c.request_repaint();
                });
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(4));
        }
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        let dark = self.data.appearance.as_ref().map(|a| a.dark).unwrap_or(true);
        theme::apply(&ctx, dark, accent);
        let pal = theme::palette_for(dark, accent);

        // No header bar: the search box tops the sidebar and the window buttons sit at the top right
        // of the page; empty space at the top moves the window (like Ghostty's single row).
        // Sidebar: search, then the pages.
        egui::Panel::left("pages")
            .default_size(250.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(Margin::symmetric(10, 10)))
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search settings").desired_width(f32::INFINITY).margin(vec2(8.0, 6.0)));
                ui.add_space(8.0);
                let q = self.search.to_lowercase();
                for (p, name, icon) in PAGES {
                    if !q.is_empty() && !name.to_lowercase().contains(&q) {
                        continue;
                    }
                    let sel = self.page == p;
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
                    if sel {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
                    }
                    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), vec2(18.0, 18.0));
                    icons::paint(ui.painter(), icon_rect, icon, if sel { pal.accent } else { pal.text });
                    ui.painter().text(egui::pos2(rect.left() + 40.0, rect.center().y), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(14.0), pal.text);
                    if resp.clicked() && !sel {
                        self.page = p;
                        self.status.clear();
                        self.load(p, &ctx);
                    }
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 32, right: 12, top: 6, bottom: 18 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                    chrome::window_controls(ui, &pal);
                });
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    // Room for the scroll bar on the right, so cards are never cut off under it.
                    ui.set_max_width((ui.available_width() - 20.0).min(760.0));
                    if !self.status.is_empty() {
                        ui.colored_label(pal.red, &self.status);
                    }
                    match self.page {
                        Page::About => self.page_about(ui, &pal, &ctx),
                        Page::Network => self.page_network(ui, &pal, &ctx),
                        Page::Bluetooth => self.page_bluetooth(ui, &pal, &ctx),
                        Page::Displays => self.page_displays(ui, &pal, &ctx),
                        Page::Sound => self.page_sound(ui, &pal, &ctx),
                        Page::Appearance => self.page_appearance(ui, &pal, &ctx),
                        Page::Power => self.page_power(ui, &pal, &ctx),
                        Page::Firewall => self.page_firewall(ui, &pal, &ctx),
                        Page::DateTime => self.page_time(ui, &pal, &ctx),
                        Page::Apps => self.page_apps(ui, &pal, &ctx),
                        Page::Shortcuts => page_shortcuts(ui, &pal),
                    }
                });
            });

        self.wifi_dialog(&ctx, &pal);
        chrome::outline(&ctx, &pal);
        // Keep live values (volume, connections) fresh while the window is open.
        ctx.request_repaint_after(Duration::from_secs(5));
    }
}

/// The installed apps, and (in the background) how much space each takes.
fn load_installed(tx: &std::sync::mpsc::Sender<Msg>) -> Vec<appmgr::AppInfo> {
    let theme = sys::run("kreadconfig6", &["--file", "kdeglobals", "--group", "Icons", "--key", "Theme"]).map(|s| s.trim().to_string()).unwrap_or_default();
    let apps = appmgr::list_apps(&theme);
    let (tx, list) = (tx.clone(), apps.clone());
    std::thread::spawn(move || {
        let sizes = list.iter().map(|a| (a.id.clone(), appmgr::data_dirs(a).iter().map(|(p, _)| appmgr::dir_size(p)).sum::<u64>())).collect();
        let _ = tx.send(Msg::AppSizes(sizes));
    });
    apps
}

const BRAND_SHAPES: [(&str, &str); 5] = [("glyph", "Logo only"), ("squircle", "Squircle"), ("rounded", "Rounded square"), ("circle", "Circle"), ("hexagon", "Hexagon")];
const BRAND_STYLES: [(&str, &str); 3] = [("brand", "Brand colours"), ("dark", "Dark tile"), ("purple", "Labyrinth purple")];

fn loading(ui: &mut egui::Ui) {
    ui.add_space(40.0);
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label("Loading…");
    });
}

impl App {
    fn page_about(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(a) = self.data.about.clone() else { return loading(ui) };
        let developer = labyrinth_conf::PanelConf::load().developer;
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            // Five clicks on the logo while the ` key is held offer Developer options, like tapping the build number on Android.
            let logo = ui.add(egui::Image::new("file:///usr/share/icons/hicolor/256x256/apps/labyrinthos.png").fit_to_exact_size(vec2(96.0, 96.0)).sense(Sense::click()));
            if logo.clicked() {
                if ctx.input(|i| i.key_down(egui::Key::Backtick)) {
                    if self.dev_last.elapsed() > std::time::Duration::from_secs(3) {
                        self.dev_clicks = 0;
                    }
                    self.dev_clicks += 1;
                    self.dev_last = std::time::Instant::now();
                    if self.dev_clicks >= 5 {
                        self.dev_clicks = 0;
                        self.dev_prompt = !developer;
                    }
                } else {
                    self.dev_clicks = 0;
                }
            }
            ui.label(RichText::new(if a.os.is_empty() { "Labyrinth OS" } else { &a.os }).size(22.0).strong());
        });
        if self.dev_prompt {
            egui::Window::new("Turn on Developer options?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
                ui.set_max_width(400.0);
                ui.label("They add more technical settings: per-mode CPU and graphics limits under Power, and choosing which kind of connection an app may use under Apps. Nothing changes until you use them.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Turn on").clicked() {
                        self.dev_prompt = false;
                        let mut c = labyrinth_conf::PanelConf::load();
                        c.developer = true;
                        self.change(ctx, move || c.save());
                    }
                    if ui.button("Not now").clicked() {
                        self.dev_prompt = false;
                    }
                });
            });
        }
        if developer {
            ui::group(ui, pal, None, |ui| {
                ui::row(ui, pal, true, "Developer options", Some("On: more technical settings show under Power and Apps"), |ui| {
                    if ui::quiet(ui, pal, "Turn off").clicked() {
                        let mut c = labyrinth_conf::PanelConf::load();
                        c.developer = false;
                        self.change(ctx, move || c.save());
                    }
                });
            });
        }
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Device name", None, |ui| { ui.label(&a.hostname); });
            ui::row(ui, pal, false, "Processor", None, |ui| { ui.label(&a.cpu); });
            ui::row(ui, pal, false, "Memory", None, |ui| { ui.label(format!("{:.0} GB", a.memory_gib)); });
            for (i, g) in a.gpus.iter().enumerate() {
                ui::row(ui, pal, false, if i == 0 { "Graphics" } else { "" }, None, |ui| { ui.label(g); });
            }
            ui::row(ui, pal, false, "Disk", None, |ui| { ui.label(format!("{} free of {}", ui::gib(a.disk_free), ui::gib(a.disk_total))); });
            ui::row(ui, pal, false, "Kernel", None, |ui| { ui.label(&a.kernel); });
        });
    }

    fn page_network(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(n) = self.data.network.clone() else { return loading(ui) };
        let wired: Vec<&sys::NetDevice> = n.devices.iter().filter(|d| d.kind == "ethernet").collect();
        if !wired.is_empty() {
            ui::group(ui, pal, Some("Wired"), |ui| {
                for (i, d) in wired.iter().enumerate() {
                    let on = d.state.starts_with("connected");
                    let sub = if on { format!("Connected · {}", d.connection) } else { "Cable unplugged or off".into() };
                    ui::row(ui, pal, i == 0, &format!("Ethernet ({})", d.device), Some(&sub), |_| {});
                }
            });
        }
        let mut wifi_on = n.wifi_on;
        let has_wifi = n.devices.iter().any(|d| d.kind == "wifi");
        let mut toggled = false;
        ui::group(ui, pal, Some("Wi-Fi"), |ui| {
            ui::row(ui, pal, true, "Wi-Fi", Some(if has_wifi { "Connect to wireless networks" } else { "No Wi-Fi adapter found" }), |ui| {
                if has_wifi && ui::switch(ui, pal, &mut wifi_on).changed() {
                    toggled = true;
                }
            });
        });
        if toggled {
            let arg = if wifi_on { "on" } else { "off" };
            self.change(ctx, move || sys::act("nmcli", &["radio", "wifi", arg]));
        }
        if n.wifi_on && has_wifi {
            let mut connect: Option<(String, bool)> = None;
            let mut disconnect = false;
            ui::group(ui, pal, Some("Networks"), |ui| {
                if n.networks.is_empty() {
                    ui::row(ui, pal, true, "No networks found", None, |_| {});
                }
                for (i, w) in n.networks.iter().enumerate() {
                    let bars = match w.signal { 0..=25 => "▂", 26..=50 => "▂▄", 51..=75 => "▂▄▆", _ => "▂▄▆█" };
                    let sub = format!("{bars}  {}{}", if w.secured { "Secured" } else { "Open" }, if w.in_use { " · Connected" } else { "" });
                    ui::row(ui, pal, i == 0, &w.ssid, Some(&sub), |ui| {
                        if w.in_use {
                            if ui::quiet(ui, pal, "Disconnect").clicked() {
                                disconnect = true;
                            }
                        } else if ui::action(ui, pal, "Connect").clicked() {
                            connect = Some((w.ssid.clone(), w.secured));
                        }
                    });
                }
            });
            if let Some((ssid, secured)) = connect {
                if secured {
                    self.wifi_password = Some((ssid, String::new()));
                } else {
                    self.change(ctx, move || sys::act("nmcli", &["device", "wifi", "connect", &ssid]));
                }
            }
            if disconnect {
                let dev = n.devices.iter().find(|d| d.kind == "wifi").map(|d| d.device.clone()).unwrap_or_default();
                self.change(ctx, move || sys::act("nmcli", &["device", "disconnect", &dev]));
            }
            ui.add_space(8.0);
            if ui::quiet(ui, pal, "Scan again").clicked() {
                self.change(ctx, || {
                    let _ = sys::act("nmcli", &["device", "wifi", "rescan"]);
                    std::thread::sleep(Duration::from_secs(3));
                    Ok(())
                });
            }
        }
    }

    fn wifi_dialog(&mut self, ctx: &egui::Context, pal: &Palette) {
        let mut done: Option<bool> = None;
        if let Some((ssid, pw)) = &mut self.wifi_password {
            egui::Window::new(format!("Connect to “{ssid}”")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
                ui.label("Password");
                let r = ui.add(egui::TextEdit::singleline(pw).password(true).desired_width(280.0));
                r.request_focus();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui::action(ui, pal, "Connect").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                        done = Some(true);
                    }
                    if ui::quiet(ui, pal, "Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
        }
        if let Some(ok) = done {
            if let Some((ssid, pw)) = self.wifi_password.take() {
                if ok {
                    self.change(ctx, move || sys::act("nmcli", &["device", "wifi", "connect", &ssid, "password", &pw]));
                }
            }
        }
    }

    fn page_bluetooth(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(b) = self.data.bluetooth.clone() else { return loading(ui) };
        if !b.present {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "No Bluetooth adapter found", None, |_| {}));
            return;
        }
        let mut on = b.powered;
        let mut toggled = false;
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Bluetooth", Some("Keyboards, mice, headphones and phones"), |ui| {
                toggled = ui::switch(ui, pal, &mut on).changed();
            });
        });
        if toggled {
            let arg = if on { "on" } else { "off" };
            self.change(ctx, move || sys::act("bluetoothctl", &["power", arg]));
        }
        if b.powered {
            let mut act: Option<(String, &'static str)> = None;
            ui::group(ui, pal, Some("Devices"), |ui| {
                if b.devices.is_empty() {
                    ui::row(ui, pal, true, "No devices yet", Some("Use “Add a device” to pair one"), |_| {});
                }
                for (i, d) in b.devices.iter().enumerate() {
                    let mut sub = if d.connected { "Connected".to_string() } else { "Not connected".to_string() };
                    if let Some(p) = d.battery {
                        sub.push_str(&format!(" · Battery {p}%"));
                    }
                    ui::row(ui, pal, i == 0, &d.name, Some(&sub), |ui| {
                        if ui::quiet(ui, pal, "Forget").clicked() {
                            act = Some((d.mac.clone(), "remove"));
                        }
                        if d.connected {
                            if ui::quiet(ui, pal, "Disconnect").clicked() {
                                act = Some((d.mac.clone(), "disconnect"));
                            }
                        } else if ui::action(ui, pal, "Connect").clicked() {
                            act = Some((d.mac.clone(), "connect"));
                        }
                    });
                }
            });
            if let Some((mac, verb)) = act {
                self.change(ctx, move || sys::act("bluetoothctl", &[verb, &mac]));
            }
            ui.add_space(8.0);
            if ui::action(ui, pal, "Add a device…").clicked() {
                sys::spawn("bluedevil-wizard", &[]);
            }
        }
    }

    fn page_displays(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(d) = self.data.displays.clone() else { return loading(ui) };
        let mut cmd: Option<Vec<String>> = None;
        let mut tiling: Option<String> = None;
        for o in d.outputs.iter().filter(|o| o.enabled) {
            let Some(cur) = o.current_mode().cloned() else { continue };
            let title = if o.primary { format!("{} · Main display", o.name) } else { o.name.clone() };
            ui::group(ui, pal, Some(&title), |ui| {
                ui::row(ui, pal, true, "Resolution", None, |ui| {
                    egui::ComboBox::from_id_salt(format!("res-{}", o.name)).selected_text(format!("{} × {}", cur.width, cur.height)).show_ui(ui, |ui| {
                        for (w, h) in o.resolutions() {
                            if ui.selectable_label(w == cur.width && h == cur.height, format!("{w} × {h}")).clicked() {
                                if let Some(m) = o.rates(w, h).first() {
                                    cmd = Some(vec![format!("output.{}.mode.{}", o.name, m.id)]);
                                }
                            }
                        }
                    });
                });
                ui::row(ui, pal, false, "Refresh rate", None, |ui| {
                    egui::ComboBox::from_id_salt(format!("hz-{}", o.name)).selected_text(format!("{:.0} Hz", cur.refresh)).show_ui(ui, |ui| {
                        for m in o.rates(cur.width, cur.height) {
                            if ui.selectable_label(m.id == cur.id, format!("{:.2} Hz", m.refresh)).clicked() {
                                cmd = Some(vec![format!("output.{}.mode.{}", o.name, m.id)]);
                            }
                        }
                    });
                });
                ui::row(ui, pal, false, "Scale", Some("Makes text and windows larger"), |ui| {
                    egui::ComboBox::from_id_salt(format!("scale-{}", o.name)).selected_text(format!("{:.0} %", o.scale * 100.0)).show_ui(ui, |ui| {
                        for s in [1.0, 1.25, 1.5, 1.75, 2.0] {
                            if ui.selectable_label((s - o.scale).abs() < 0.01, format!("{:.0} %", s * 100.0)).clicked() {
                                cmd = Some(vec![format!("output.{}.scale.{}", o.name, s)]);
                            }
                        }
                    });
                });
                if !o.primary {
                    ui::row(ui, pal, false, "Main display", Some("The top bar and dock live here"), |ui| {
                        if ui::quiet(ui, pal, "Make main").clicked() {
                            cmd = Some(vec![format!("output.{}.priority.1", o.name)]);
                        }
                    });
                }
                let mut is_tiling = d.tiling.split(',').any(|t| t.trim() == o.name);
                ui::row(ui, pal, false, "Tiling screen", Some("Windows tile automatically, Omarchy-style, with their own workspaces"), |ui| {
                    if ui::switch(ui, pal, &mut is_tiling).changed() {
                        tiling = Some(if is_tiling { o.name.clone() } else { "off".into() });
                    }
                });
            });
        }
        if d.outputs.is_empty() {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "No displays reported", None, |_| {}));
        }
        if let Some(args) = cmd {
            self.change(ctx, move || {
                let a: Vec<&str> = args.iter().map(String::as_str).collect();
                sys::act("kscreen-doctor", &a)
            });
        }
        if let Some(t) = tiling {
            self.change(ctx, move || sys::act("labyrinth-tiling", &[&t]));
        }
        ui.add_space(8.0);
        if ui::quiet(ui, pal, "Arrange displays…").clicked() {
            sys::spawn("kcmshell6", &["kcm_kscreen"]);
        }
    }

    fn page_sound(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(s) = self.data.sound.clone() else { return loading(ui) };
        let mut cmds: Vec<Vec<String>> = vec![];
        for (title, devices, default, kind) in [("Output", &s.outputs, &s.default_output, "sink"), ("Input", &s.inputs, &s.default_input, "source")] {
            ui::group(ui, pal, Some(title), |ui| {
                if devices.is_empty() {
                    ui::row(ui, pal, true, "No devices", None, |_| {});
                    return;
                }
                let cur = devices.iter().find(|d| &d.name == default).or(devices.first()).unwrap();
                ui::row(ui, pal, true, "Device", None, |ui| {
                    egui::ComboBox::from_id_salt(format!("dev-{kind}")).selected_text(&cur.description).width((ui.available_width() * 0.6).clamp(160.0, 320.0)).show_ui(ui, |ui| {
                        for d in devices {
                            if ui.selectable_label(d.name == cur.name, &d.description).clicked() {
                                cmds.push(vec![format!("set-default-{kind}"), d.name.clone()]);
                            }
                        }
                    });
                });
                let mut vol = cur.volume as f32;
                ui::row(ui, pal, false, "Volume", None, |ui| {
                    let r = ui::slider(ui, pal, &mut vol);
                    if r.drag_stopped() || (r.changed() && !r.dragged()) {
                        cmds.push(vec![format!("set-{kind}-volume"), cur.name.clone(), format!("{}%", vol as u32)]);
                    }
                });
                let mut muted = cur.muted;
                ui::row(ui, pal, false, "Mute", None, |ui| {
                    if ui::switch(ui, pal, &mut muted).changed() {
                        cmds.push(vec![format!("set-{kind}-mute"), cur.name.clone(), if muted { "1".into() } else { "0".into() }]);
                    }
                });
            });
        }
        if !cmds.is_empty() {
            self.change(ctx, move || {
                for c in cmds {
                    let a: Vec<&str> = c.iter().map(String::as_str).collect();
                    sys::act("pactl", &a)?;
                }
                Ok(())
            });
        }
    }

    fn page_appearance(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(a) = self.data.appearance.clone() else { return loading(ui) };
        let mut style: Option<bool> = None;
        let mut accent: Option<[u8; 3]> = None;
        let mut wallpaper: Option<String> = None;
        let mut icon_use: Option<String> = None;
        let mut icon_remove: Option<String> = None;
        let mut icon_install: Option<String> = None;
        let mut brand_build = false;
        let mut panel_switch: Option<bool> = None;
        let mut conf = a.panel;
        let mut conf_changed = false;
        ui::group(ui, pal, Some("Style"), |ui| {
            ui::row(ui, pal, true, "Look", None, |ui| {
                if let Some(i) = ui::segmented(ui, pal, &["Light", "Dark"], if a.dark { 1 } else { 0 }) {
                    style = Some(i == 1);
                }
            });
            ui::row(ui, pal, false, "Accent colour", Some("Selections, switches, the active window border"), |ui| {
                for (name, rgb) in sys::ACCENTS.iter().rev() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                    let c = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                    ui.painter().circle_filled(rect.center(), 10.0, c);
                    if a.accent == Some(*rgb) || (a.accent.is_none() && *name == "Labyrinth purple") {
                        ui.painter().circle_stroke(rect.center(), 12.5, egui::Stroke::new(2.0, pal.text));
                    }
                    if resp.on_hover_text(*name).clicked() {
                        accent = Some(*rgb);
                    }
                }
            });
        });
        ui::group(ui, pal, Some("Icons"), |ui| {
            for (i, pack) in a.icon_packs.iter().enumerate() {
                let current = pack.id == a.icon_theme;
                let sub = if pack.user { "Installed by you" } else { "Comes with the system" };
                ui::row(ui, pal, i == 0, &pack.name, Some(sub), |ui| {
                    if current {
                        ui.label(RichText::new("In use").color(pal.accent));
                    } else if ui::action(ui, pal, "Use").clicked() {
                        icon_use = Some(pack.id.clone());
                    }
                    if pack.user && !current && ui::quiet(ui, pal, "Remove").clicked() {
                        icon_remove = Some(pack.id.clone());
                    }
                    for p in pack.preview.iter().rev() {
                        ui.add(egui::Image::new(format!("file://{}", p.display())).fit_to_exact_size(vec2(28.0, 28.0)));
                    }
                });
            }
            ui::row(ui, pal, a.icon_packs.is_empty(), "Labyrinth Brands", Some("App logos from Simple Icons in the shape and colours you pick; apps it has no logo for keep the other pack's icon. Make it, then choose it above"), |ui| {
                if ui::action(ui, pal, "Make").clicked() {
                    brand_build = true;
                }
                egui::ComboBox::from_id_salt("brand-style").selected_text(BRAND_STYLES[self.brand_style].1).show_ui(ui, |ui| {
                    for (i, (_, l)) in BRAND_STYLES.iter().enumerate() {
                        if ui.selectable_label(i == self.brand_style, *l).clicked() {
                            self.brand_style = i;
                        }
                    }
                });
                egui::ComboBox::from_id_salt("brand-shape").selected_text(BRAND_SHAPES[self.brand_shape].1).show_ui(ui, |ui| {
                    for (i, (_, l)) in BRAND_SHAPES.iter().enumerate() {
                        if ui.selectable_label(i == self.brand_shape, *l).clicked() {
                            self.brand_shape = i;
                        }
                    }
                });
            });
            ui::row(ui, pal, false, "Install an icon pack", Some("A folder, an archive (.tar.gz, .tar.xz, .zip) or an https link. It goes in your own icons folder; nothing needs a password"), |ui| {
                if ui::action(ui, pal, "Install").clicked() && !self.icon_src.trim().is_empty() {
                    icon_install = Some(self.icon_src.trim().to_string());
                }
                ui.add(egui::TextEdit::singleline(&mut self.icon_src).hint_text("~/Downloads/pack.tar.gz or https://...").desired_width((ui.available_width() - 10.0).clamp(160.0, 340.0)));
            });
        });
        ui::group(ui, pal, Some("Top bar and dock"), |ui| {
            let mut on = a.panel_on;
            ui::row(ui, pal, true, "Use the Labyrinth top bar and dock", Some("Replaces Plasma's panels. It has no system tray, quick settings or global menu yet."), |ui| {
                if ui::switch(ui, pal, &mut on).changed() {
                    panel_switch = Some(on);
                }
            });
        });
        let modes = ["Match theme", "Dark / light"];
        let mode_index = |m: labyrinth_conf::ColorMode| if m == labyrinth_conf::ColorMode::Theme { 0 } else { 1 };
        let mode_of = |i: usize| if i == 0 { labyrinth_conf::ColorMode::Theme } else { labyrinth_conf::ColorMode::Neutral };
        ui::group(ui, pal, Some("Dock"), |ui| {
            let positions = ["Bottom", "Left", "Right"];
            let pos_index = match conf.dock_pos {
                labyrinth_conf::DockPos::Bottom => 0,
                labyrinth_conf::DockPos::Left => 1,
                labyrinth_conf::DockPos::Right => 2,
            };
            ui::row(ui, pal, true, "Position", Some("Which edge of the screen; left and right are vertical"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &positions, pos_index) {
                    conf.dock_pos = [labyrinth_conf::DockPos::Bottom, labyrinth_conf::DockPos::Left, labyrinth_conf::DockPos::Right][i];
                    conf_changed = true;
                }
            });
            let mut gap = conf.dock_gap as f32;
            ui::row(ui, pal, false, "Distance from edge", Some("How far the dock floats off the screen edge; 0 puts it right on the edge"), |ui| {
                let r = ui::slider_range(ui, pal, &mut gap, 0.0..=labyrinth_conf::DOCK_GAP_MAX as f32, " px");
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    conf.dock_gap = gap as u32;
                    conf_changed = true;
                }
            });
            let mut size = conf.dock_icon as f32;
            ui::row(ui, pal, false, "Size", Some("Icon size"), |ui| {
                let r = ui::slider_range(ui, pal, &mut size, labyrinth_conf::DOCK_ICON_MIN as f32..=labyrinth_conf::DOCK_ICON_MAX as f32, " px");
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    conf.dock_icon = size as u32;
                    conf_changed = true;
                }
            });
            let mut opacity = conf.dock_opacity as f32;
            ui::row(ui, pal, false, "Background", Some("Opacity of the dock's shelf; 0% leaves just the icons"), |ui| {
                let r = ui::slider(ui, pal, &mut opacity);
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    conf.dock_opacity = opacity as u32;
                    conf_changed = true;
                }
            });
            ui::row(ui, pal, false, "Colour", Some("Match the theme's purple, or follow only dark and light"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &modes, mode_index(conf.dock_color)) {
                    conf.dock_color = mode_of(i);
                    conf_changed = true;
                }
            });
        });
        ui::group(ui, pal, Some("Top bar"), |ui| {
            let mut opacity = conf.bar_opacity as f32;
            ui::row(ui, pal, true, "Background", Some("Opacity of the bar; 0% leaves just its text"), |ui| {
                let r = ui::slider(ui, pal, &mut opacity);
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    conf.bar_opacity = opacity as u32;
                    conf_changed = true;
                }
            });
            ui::row(ui, pal, false, "Colour", Some("Match the theme's purple, or follow only dark and light"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &modes, mode_index(conf.bar_color)) {
                    conf.bar_color = mode_of(i);
                    conf_changed = true;
                }
            });
        });
        ui::group(ui, pal, Some("Start menu"), |ui| {
            let themes = ["Windows", "Folders"];
            let current = if conf.start_theme == labyrinth_conf::StartTheme::Folders { 1 } else { 0 };
            ui::row(ui, pal, true, "Theme", Some("Windows: pinned apps and every app in a list. Folders: narrow, apps in folders by kind, and a power button"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &themes, current) {
                    conf.start_theme = if i == 1 { labyrinth_conf::StartTheme::Folders } else { labyrinth_conf::StartTheme::Windows };
                    conf_changed = true;
                }
            });
        });
        ui::group(ui, pal, Some("Notch (Fylgja)"), |ui| {
            ui::row(ui, pal, true, "Clipboard history", Some("Keeps what you copy (text) in memory only, to paste again from the notch or with Super+V. It is forgotten when you log out, and password managers are skipped"), |ui| {
                if ui::switch(ui, pal, &mut conf.clipboard_history).changed() {
                    conf_changed = true;
                }
            });
        });
        ui::group(ui, pal, Some("Notifications"), |ui| {
            ui::row(ui, pal, true, "Pop-ups", Some("A new notification slides in at the bottom right for a few seconds. They are all kept in the Notification Center either way"), |ui| {
                if ui::switch(ui, pal, &mut conf.notify_toasts).changed() {
                    conf_changed = true;
                }
            });
            ui::row(ui, pal, false, "Do not disturb", Some("No pop-ups (critical alerts still show). Also a button at the bottom of the notification list. Everything is still kept"), |ui| {
                if ui::switch(ui, pal, &mut conf.notify_dnd).changed() {
                    conf_changed = true;
                }
            });
            ui::row(ui, pal, false, "Notification Center pane", Some("Off: notifications are in Fylgja's Notifications section. On: a pane down the right edge, opened with the calendar (click the date)"), |ui| {
                if ui::switch(ui, pal, &mut conf.notify_pane).changed() {
                    conf_changed = true;
                }
            });
            let levels = ["Off", "Critical", "Warnings", "Everything"];
            let cur = match conf.heimdall_alerts {
                labyrinth_conf::AlertLevel::Off => 0,
                labyrinth_conf::AlertLevel::Critical => 1,
                labyrinth_conf::AlertLevel::Warning => 2,
                labyrinth_conf::AlertLevel::Info => 3,
            };
            ui::row(ui, pal, false, "Heimdall alerts", Some("Which of Heimdall's security alerts show up as notifications. Everything includes each new place a program connects to"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &levels, cur) {
                    conf.heimdall_alerts = [labyrinth_conf::AlertLevel::Off, labyrinth_conf::AlertLevel::Critical, labyrinth_conf::AlertLevel::Warning, labyrinth_conf::AlertLevel::Info][i];
                    conf_changed = true;
                }
            });
        });
        ui::group(ui, pal, Some("Wallpaper"), |ui| {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for p in &a.wallpapers {
                    let uri = format!("file://{}", p.display());
                    let img = egui::Image::new(uri).fit_to_exact_size(vec2(160.0, 90.0)).corner_radius(CornerRadius::same(8)).sense(Sense::click());
                    if ui.add(img).on_hover_text(p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()).clicked() {
                        wallpaper = Some(p.display().to_string());
                    }
                }
            });
            ui.add_space(6.0);
        });
        if conf_changed {
            self.change(ctx, move || conf.save());
        }
        if let Some(on) = panel_switch {
            self.change(ctx, move || sys::act("labyrinth-panel-mode", &[if on { "on" } else { "off" }]));
        }
        if let Some(dark) = style {
            self.change(ctx, move || {
                let scheme = if dark { "WhiteSurDark" } else { "WhiteSur" };
                sys::act("plasma-apply-colorscheme", &[scheme])?;
                // Kvantum draws KDE apps' widgets; keep it in step with the colour scheme.
                let home = std::env::var("HOME").unwrap_or_default();
                let _ = std::fs::create_dir_all(format!("{home}/.config/Kvantum"));
                let _ = std::fs::write(format!("{home}/.config/Kvantum/kvantum.kvconfig"), format!("[General]\ntheme={scheme}\n"));
                // Minos's icon follows: its dark or light version
                let _ = std::process::Command::new("labyrinth-minos-icon").args(["-", if dark { "dark" } else { "light" }]).status();
                Ok(())
            });
        }
        if let Some([r, g, b]) = accent {
            self.change(ctx, move || {
                sys::act("plasma-apply-colorscheme", &["--accent-color", &format!("#{r:02x}{g:02x}{b:02x}")])?;
                // Minos's icon takes the new accent
                let _ = std::process::Command::new("labyrinth-minos-icon").args([format!("{r},{g},{b}"), String::new()]).status();
                Ok(())
            });
        }
        if let Some(p) = wallpaper {
            self.change(ctx, move || sys::act("plasma-apply-wallpaperimage", &[&p]));
        }
        if brand_build {
            let (shape, style) = (BRAND_SHAPES[self.brand_shape].0, BRAND_STYLES[self.brand_style].0);
            self.change(ctx, move || sys::build_brand_pack(shape, style));
        }
        if let Some(id) = icon_use {
            self.change(ctx, move || sys::set_icon_pack(&id));
        }
        if let Some(id) = icon_remove {
            self.change(ctx, move || sys::remove_icon_pack(&id));
        }
        if let Some(src) = icon_install {
            self.icon_src.clear();
            self.change(ctx, move || sys::install_icon_pack(&src).map(|_| ()));
        }
    }

    fn page_power(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(p) = self.data.power.clone() else { return loading(ui) };
        let mut profile: Option<String> = None;
        let mut timer: Option<(&'static str, &'static str, i64)> = None;
        if !p.profiles.is_empty() {
            ui::group(ui, pal, Some("Power mode"), |ui| {
                let labels: Vec<String> = p.profiles.iter().map(|x| match x.as_str() { "performance" => "Performance".into(), "balanced" => "Balanced".into(), "power-saver" => "Power saver".into(), o => o.to_string() }).collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                let sel = p.profiles.iter().position(|x| *x == p.profile).unwrap_or(0);
                ui::row(ui, pal, true, "Mode", Some("Performance for games and builds; Power saver for quiet"), |ui| {
                    if let Some(i) = ui::segmented(ui, pal, &refs, sel) {
                        profile = Some(p.profiles[i].clone());
                    }
                });
            });
        }
        let choices: [(i64, &str); 8] = [(0, "Never"), (1, "1 minute"), (2, "2 minutes"), (5, "5 minutes"), (10, "10 minutes"), (15, "15 minutes"), (30, "30 minutes"), (60, "1 hour")];
        let label = |m: i64| choices.iter().find(|c| c.0 == m).map(|c| c.1.to_string()).unwrap_or(format!("{m} minutes"));
        ui::group(ui, pal, Some("When idle"), |ui| {
            for (first, title, group, key, cur) in [(true, "Turn off the screen after", "Display", "TurnOffDisplayIdleTimeoutSec", p.screen_off_min), (false, "Lock the screen after", "Lock", "Timeout", p.lock_min), (false, "Sleep after", "SuspendAndShutdown", "AutoSuspendIdleTimeoutSec", p.sleep_min)] {
                ui::row(ui, pal, first, title, None, |ui| {
                    egui::ComboBox::from_id_salt(key).selected_text(label(cur)).show_ui(ui, |ui| {
                        for (m, l) in choices {
                            if ui.selectable_label(m == cur, l).clicked() {
                                timer = Some((group, key, m));
                            }
                        }
                    });
                });
            }
        });
        // Developer options: limits for each power mode
        let mut limit: Option<(String, &'static str, Option<i64>)> = None;
        if let Some(l) = p.limits.clone() {
            for mode in &p.profiles {
                let label = match mode.as_str() { "performance" => "Performance", "balanced" => "Balanced", "power-saver" => "Power saver", o => o };
                let (cpu, gpu) = l.modes.get(mode).copied().unwrap_or((None, None));
                ui::group(ui, pal, Some(&format!("{label} mode limits")), |ui| {
                    let mut c = cpu.unwrap_or(100) as f32;
                    ui::row(ui, pal, true, "Processor", Some("The most it may clock, as a share of its top speed"), |ui| {
                        if cpu.is_some() && ui::quiet(ui, pal, "Reset").clicked() {
                            limit = Some((mode.clone(), "cpu", None));
                        }
                        let r = ui::slider_range(ui, pal, &mut c, 20.0..=100.0, " %");
                        if r.drag_stopped() || (r.changed() && !r.dragged()) {
                            limit = Some((mode.clone(), "cpu", if c as i64 >= 100 { None } else { Some(c as i64) }));
                        }
                    });
                    if let Some(g) = &l.gpu {
                        let mut w = gpu.unwrap_or(g.default) as f32;
                        ui::row(ui, pal, false, &g.name, Some(&format!("The most power the graphics card may draw (it allows {}-{} W, default {} W)", g.min, g.max, g.default)), |ui| {
                            if gpu.is_some() && ui::quiet(ui, pal, "Reset").clicked() {
                                limit = Some((mode.clone(), "gpu", None));
                            }
                            let r = ui::slider_range(ui, pal, &mut w, g.min as f32..=g.max as f32, " W");
                            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                                limit = Some((mode.clone(), "gpu", if w as i64 == g.default { None } else { Some(w as i64) }));
                            }
                        });
                    }
                });
            }
            ui.label(RichText::new("These only hold things back; nothing here overclocks. Changing a limit asks for your password.").size(11.5).color(pal.dim));
        }
        if let Some((mode, which, v)) = limit {
            self.change(ctx, move || {
                let arg = format!("{which}={}", v.map(|n| n.to_string()).unwrap_or_else(|| "default".into()));
                sys::act("pkexec", &["/usr/local/bin/labyrinth-powerlimit", "set", &mode, &arg])
            });
        }
        if let Some(name) = profile {
            self.change(ctx, move || sys::act("powerprofilesctl", &["set", &name]));
        }
        if let Some(("Lock", _, minutes)) = timer {
            // the screen locker's own setting (kscreenlockerrc), not PowerDevil's
            self.change(ctx, move || {
                sys::act("kwriteconfig6", &["--file", "kscreenlockerrc", "--group", "Daemon", "--key", "Autolock", if minutes == 0 { "false" } else { "true" }])?;
                if minutes > 0 {
                    sys::act("kwriteconfig6", &["--file", "kscreenlockerrc", "--group", "Daemon", "--key", "Timeout", &minutes.to_string()])?;
                }
                Ok(())
            });
        } else if let Some((group, key, minutes)) = timer {
            self.change(ctx, move || {
                let (when, flag) = if minutes == 0 { ("-1".to_string(), "false") } else { ((minutes * 60).to_string(), "true") };
                let enable_key = if group == "Display" { "TurnOffDisplayWhenIdle" } else { "AutoSuspendAction" };
                sys::act("kwriteconfig6", &["--file", "powerdevilrc", "--group", "AC", "--group", group, "--key", key, "--", &when])?;
                if group == "Display" {
                    sys::act("kwriteconfig6", &["--file", "powerdevilrc", "--group", "AC", "--group", group, "--key", enable_key, flag])?;
                } else {
                    // 1 = sleep, 0 = do nothing
                    sys::act("kwriteconfig6", &["--file", "powerdevilrc", "--group", "AC", "--group", group, "--key", enable_key, if minutes == 0 { "0" } else { "1" }])?;
                }
                let _ = sys::act("qdbus6", &["org.kde.Solid.PowerManagement", "/org/kde/Solid/PowerManagement", "org.kde.Solid.PowerManagement.reparseConfiguration"]);
                Ok(())
            });
        }
    }

    fn page_firewall(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(on) = self.data.firewall else { return loading(ui) };
        let mut v = on;
        let mut toggled = false;
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Firewall", Some("Blocks incoming connections; everything you start can still reach out"), |ui| {
                toggled = ui::switch(ui, pal, &mut v).changed();
            });
        });
        if toggled {
            let arg = if v { "enable" } else { "disable" };
            self.change(ctx, move || sys::act("pkexec", &["ufw", arg]));
        }
        ui.add_space(8.0);
        if ui::quiet(ui, pal, "Rules and exceptions…").clicked() {
            sys::spawn("kcmshell6", &["kcm_firewall"]);
        }
    }

    fn page_time(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(t) = self.data.time.clone() else { return loading(ui) };
        let mut ntp = t.ntp;
        let mut ntp_changed = false;
        let mut clock = labyrinth_conf::PanelConf::load();
        let mut clock_changed = false;
        let mut zone: Option<String> = None;
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Set the time automatically", Some("From time servers on the internet"), |ui| {
                ntp_changed = ui::switch(ui, pal, &mut ntp).changed();
            });
            ui::row(ui, pal, false, "24-hour clock", Some("In the top bar and the calendar; off shows AM/PM"), |ui| {
                clock_changed = ui::switch(ui, pal, &mut clock.clock_24h).changed();
            });
            ui::row(ui, pal, false, "Time zone", Some(&t.timezone), |ui| {
                egui::ComboBox::from_id_salt("zone").selected_text(&t.timezone).width((ui.available_width() * 0.55).clamp(160.0, 260.0)).height(360.0).show_ui(ui, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.zone_filter).hint_text("Search, e.g. Denver"));
                    let q = self.zone_filter.to_lowercase();
                    for z in t.zones.iter().filter(|z| q.is_empty() || z.to_lowercase().contains(&q)).take(400) {
                        if ui.selectable_label(*z == t.timezone, z).clicked() {
                            zone = Some(z.clone());
                        }
                    }
                });
            });
        });
        if ntp_changed {
            let arg = if ntp { "true" } else { "false" };
            self.change(ctx, move || sys::act("timedatectl", &["set-ntp", arg]));
        }
        if let Some(z) = zone {
            self.change(ctx, move || sys::act("timedatectl", &["set-timezone", &z]));
        }
        if clock_changed {
            self.change(ctx, move || clock.save());
        }
    }

    fn page_apps_minos(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let mut minos = labyrinth_conf::PanelConf::load();
        let mut minos_changed = false;
        ui::group(ui, pal, Some("Minos"), |ui| {
            ui::row(ui, pal, true, "Search inside subfolders", Some("Searching in Minos looks through every folder below the one you are in; off searches only that folder"), |ui| {
                minos_changed = ui::switch(ui, pal, &mut minos.search_subfolders).changed();
            });
            ui::row(ui, pal, false, "Show hidden items", Some("Files and folders whose names start with a dot"), |ui| {
                minos_changed |= ui::switch(ui, pal, &mut minos.show_hidden).changed();
            });
        });
        if minos_changed {
            self.change(ctx, move || minos.save());
        }
    }

    fn page_apps_software(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(apps) = self.data.apps.clone() else { return loading(ui) };
        ui::group(ui, pal, Some("Labyrinth's optional software"), |ui| {
            for (i, c) in apps.iter().enumerate() {
                let sub: String = c.description.chars().take(110).collect::<String>() + if c.description.chars().count() > 110 { "…" } else { "" };
                ui::row(ui, pal, i == 0, &c.name, Some(&sub), |ui| {
                    // Installing shows its progress (and asks for the password) in a terminal.
                    if c.installed {
                        if ui::quiet(ui, pal, "Remove").clicked() {
                            sys::spawn("ghostty", &["-e", "labyrinth-apps", "remove", &c.id]);
                        }
                    } else if ui::action(ui, pal, "Install").clicked() {
                        sys::spawn("ghostty", &["-e", "labyrinth-apps", "install", &c.id]);
                    }
                });
            }
        });
    }
}

fn page_shortcuts(ui: &mut egui::Ui, pal: &Palette) {
    let groups: [(&str, &[(&str, &str)]); 3] = [
        ("Everywhere", &[
            ("Win", "Launcher"), ("Win + E", "Minos"), ("Win + R", "Run"), ("Win + I", "Settings"), ("Win + L", "Lock"),
            ("Win + V", "Clipboard history"), ("Win + Shift + S", "Screenshot of a region"), ("Ctrl + Shift + Esc", "Task manager"),
            ("Win + Return", "Terminal"), ("F12", "Drop-down terminal"), ("Win + F", "Full screen"),
            ("Win + Shift + Alt + ← / →", "Window to the other monitor"), ("Alt + Tab", "Switch windows"),
        ]),
        ("Everyday screen", &[
            ("Win + ← / →", "Snap left / right"), ("Win + ↑ / ↓", "Maximize / restore"), ("Win + Shift + ← / →", "Window to the other monitor"),
            ("Win + 1…9", "Open dock app 1–9"), ("Win + W", "Overview"), ("Win + Space", "Dock and top bar over a full-screen app"),
            ("Win + − / =", "Zoom out / in"),
        ]),
        ("Tiling screen", &[
            ("Win + arrows", "Move focus"), ("Win + Shift + arrows", "Swap tiles"), ("Win + 1…9", "Switch workspace"),
            ("Win + Shift + 1…9", "Send window to workspace"), ("Win + W", "Close window"), ("Win + Space", "Launcher"),
            ("Win + T", "Float or tile the window"), ("Win + − / =", "Shrink / grow the tile"),
        ]),
    ];
    for (title, rows) in groups {
        ui::group(ui, pal, Some(title), |ui| {
            for (i, (keys, what)) in rows.iter().enumerate() {
                ui::row(ui, pal, i == 0, what, None, |ui| {
                    egui::Frame::new().fill(pal.hover).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
                        ui.label(RichText::new(*keys).monospace());
                    });
                });
            }
        });
    }
    ui.add_space(8.0);
    if ui::quiet(ui, pal, "Change shortcuts…").clicked() {
        sys::spawn("kcmshell6", &["kcm_keys"]);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1060.0, 720.0])
            .with_min_inner_size([560.0, 400.0])
            .with_title("Settings")
            .with_app_id("labyrinth-settings")
            // Draws its own title bar like Minos (labyrinth-ui chrome).
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "Settings",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
    )
}
