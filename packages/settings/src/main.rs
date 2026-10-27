//! Settings: the everyday settings in one clean app, in the style of GNOME Settings,
//! with the same look as Minos (yggdrasil-ui). It changes the system through the usual command-line
//! tools and Yggdrasil's own settings files (sys.rs).
//!
//! Reading the system can be slow (Wi-Fi scans, Bluetooth), so every page loads on a background
//! thread and every change runs on one too; the window never freezes.

mod controls;
mod printers;
mod appmgr;
mod defaults;
mod apps_ui;
mod icons;
mod restore;
mod distress;
mod security;
mod workspaces;
mod zones_ui;
mod sys;
mod ui;
mod users;

/// Tests that change HOME or XDG_* (process-wide) take this lock, so they run one at a time.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use eframe::egui::{self, vec2, CornerRadius, Margin, RichText, Sense};
use icons::Icon;
use yggdrasil_ui::{chrome, theme};
use theme::Palette;
use yggdrasil_conf::nightlight;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Network,
    Bluetooth,
    Displays,
    Sound,
    Appearance,
    Power,
    Users,
    Security,
    Restore,
    Firewall,
    DateTime,
    Apps,
    Printers,
    DefaultApps,
    /// The assistant the operating system provides (only when it has one: its assistant integration).
    Assistant,
    Shortcuts,
    About,
    /// Shown under About while Developer options are on.
    Developer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SecSub {
    Main,
    /// The operating system's security monitor (its `security` integration).
    Monitor,
    Logins,
    Sudo,
    Distress,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppSub {
    Main,
    Icons,
    Bar,
    Notifs,
    Wallpaper,
    Minos,
    Lock,
    Workspaces,
    Boot,
}

const PAGES: [(Page, &str, Icon); 18] = [
    (Page::Network, "Wi-Fi & Network", Icon::Wifi),
    (Page::Bluetooth, "Bluetooth", Icon::Bluetooth),
    (Page::Displays, "Displays", Icon::Display),
    (Page::Sound, "Sound", Icon::Sound),
    (Page::Appearance, "Appearance", Icon::Appearance),
    (Page::Power, "Power", Icon::Power),
    (Page::Users, "Users", Icon::User),
    (Page::Security, "Security", Icon::Shield),
    (Page::Restore, "Restore points", Icon::Restart),
    (Page::Firewall, "Firewall", Icon::Shield),
    (Page::DateTime, "Date & Time", Icon::Clock),
    (Page::Printers, "Printers", Icon::Printer),
    (Page::Apps, "Apps", Icon::Apps),
    (Page::DefaultApps, "Default apps", Icon::Apps),
    (Page::Assistant, "Assistant", Icon::Sound),
    (Page::Shortcuts, "Keyboard Shortcuts", Icon::Keyboard),
    (Page::About, "About", Icon::Info),
    (Page::Developer, "Developer Mode", Icon::Keyboard),
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
    Account(users::Account, String, Vec<users::Account>, Vec<(String, String)>),
    AccountDone(Result<String, String>),
    Security(security::Overview),
    Restore(restore::State),
    Scan(security::ScanReport),
    Logins((Vec<security::LogEntry>, Vec<security::LogEntry>)),
    SudoLog(Vec<security::LogEntry>),
    Password(Result<(), String>),
    Time(sys::Time),
    Apps(Vec<sys::Component>),
    Installed(Vec<appmgr::AppInfo>),
    Defaults(Vec<defaults::Row>),
    Assistant(Vec<controls::State>),
    Printers(printers::State),
    AppSizes(std::collections::HashMap<String, u64>),
    DirSize(std::path::PathBuf, u64),
    Done(Page, Result<(), String>),
}

#[derive(Default)]
struct Data {
    accounts: Option<Vec<users::Account>>,
    /// The optional groups this system has: (name, what it allows).
    opt_groups: Vec<(String, String)>,
    restore: Option<restore::State>,
    security: Option<security::Overview>,
    account: Option<(users::Account, String)>,
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
    defaults: Option<Vec<defaults::Row>>,
    assistant: Option<Vec<controls::State>>,
    printers: Option<printers::State>,
}

#[derive(Default)]
struct AddForm {
    open: bool,
    full: String,
    login: String,
    /// The login name was typed by hand: stop suggesting one from the full name.
    login_touched: bool,
    admin: bool,
    pw: String,
    pw2: String,
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
    /// Network > Private addresses: the address being typed for a network (by its profile's id).
    mac_typed: std::collections::HashMap<String, String>,
    // Security page
    sec_filter: usize,
    /// The alert whose explanation is open (its time and detector).
    sec_open: Option<(i64, String)>,
    sec_sub: SecSub,
    demo_scroll: u32,
    demo_offset: f32,
    distress: distress::State,
    /// Password logs: (sign-ins, failed attempts) and sudo history, read when the page opens.
    logins: Option<(Vec<security::LogEntry>, Vec<security::LogEntry>)>,
    sudo_log: Option<Vec<security::LogEntry>>,
    log_busy: bool,
    scan_busy: bool,
    scan_report: Option<security::ScanReport>,
    // Users page: the password form
    pw_open: bool,
    pw_cur: String,
    pw_new: String,
    pw_confirm: String,
    pw_msg: Option<(String, bool)>,
    pw_busy: bool,
    // Users page: adding and removing other accounts
    acct_form: AddForm,
    acct_busy: bool,
    acct_msg: Option<(String, bool)>,
    /// The account asked to be removed (waiting for the confirmation) and whether its home folder goes too.
    acct_remove: Option<(String, bool)>,
    /// The account shown in detail on the Users page (None: your own).
    acct_sel: Option<String>,
    /// Setting another account's password: open, the new one twice.
    other_pw: (bool, String, String),
    /// The full name being edited: (account, text).
    name_edit: Option<(String, String)>,
    zone_filter: String,
    /// What the icon-pack install box holds: a folder, an archive or a link.
    icon_src: String,
    // Apps page
    app_tab: usize,
    /// Which part of Appearance is open.
    app_sub: AppSub,
    /// The lock screen previews are being (or have been) drawn for this visit to the page.
    lock_previews_started: bool,
    /// Appearance > Boot screen: what `bootscreen get` said (and when), the picture being chosen, and a change under way.
    boot_info: Option<(std::time::Instant, serde_json::Value)>,
    boot_pic: String,
    splash_pic: String,
    /// Bluetooth > Add a device: searching until, what was found (and when it was last asked), the one being paired.
    bt_scan_until: Option<std::time::Instant>,
    bt_found: (std::time::Instant, Vec<(String, String)>),
    bt_pairing: Option<String>,
    bt_pairing_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Bluetooth > Controllers: the controller whose options are open (its address or serial), and Add a controller: None when
    /// not adding, Some(None) while picking the kind, Some(Some(model)) while looking for it.
    pad_sel: Option<String>,
    pad_add: Option<Option<gamepads::Model>>,
    boot_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ws_conf: workspaces::Conf,
    app_sort: usize,
    app_sel: Option<String>,
    /// What you decided for the open app in Ask mode, refreshed every few seconds.
    ask_cache: Option<(std::time::Instant, String, Vec<appmgr::Decision>)>,
    // Developer options: logo clicks (while the ` key is held) toward the unlock, and its confirmation
    dev_clicks: u8,
    dev_last: std::time::Instant,
    dev_prompt: bool,
    /// Power > Graphics card: the unlock's confirmation while open (with the date of Sarina's last backup), and its checkbox.
    unlock_confirm: Option<Option<String>>,
    unlock_read: bool,
    app_filter: String,
    app_system: bool,
    app_folder: String,
    app_confirm: Option<apps_ui::Confirm>,
    app_sizes: std::collections::HashMap<String, u64>,
    dir_sizes: std::collections::HashMap<std::path::PathBuf, u64>,
    owners: std::collections::HashMap<String, Option<(String, bool)>>,
    /// The shape and colouring chosen for the Liki pack (indexes into BRAND_SHAPES and BRAND_STYLES).
    brand_shape: usize,
    brand_style: usize,
    /// When the open page was last read, so it can be read again now and then and follow changes made elsewhere.
    refreshed: std::time::Instant,
    /// Displays > Night Light: its settings, whether it is on now, and the thread that runs hyprsunset (read when first shown).
    night: Option<(nightlight::Settings, bool, Sender<nightlight::Cmd>)>,
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
            mac_typed: Default::default(),
            sec_filter: 1,
            sec_open: None,
            demo_scroll: if start_sub().starts_with("distress-demo-") { 8 } else { 0 },
            demo_offset: if start_sub() == "distress-demo-warn" { 100_000.0 } else { 430.0 },
            sec_sub: match start_sub().as_str() {
                "monitor" | "heimdall" => SecSub::Monitor,
                "logins" => SecSub::Logins,
                "sudo" => SecSub::Sudo,
                "distress-demo" | "distress-demo-warn" | "distress-demo-mid" | "distress" => SecSub::Distress,
                _ => SecSub::Main,
            },
            distress: match start_sub().as_str() {
                "distress-demo" => distress::State::demo(false),
                "distress-demo-warn" => distress::State::demo(true),
                "distress-demo-mid" => distress::State::demo(false),
                _ => distress::State::default(),
            },
            logins: None,
            sudo_log: None,
            log_busy: false,
            scan_busy: false,
            scan_report: None,
            pw_open: false,
            pw_cur: String::new(),
            pw_new: String::new(),
            pw_confirm: String::new(),
            pw_msg: None,
            pw_busy: false,
            acct_form: AddForm::default(),
            acct_busy: false,
            acct_msg: None,
            acct_remove: None,
            acct_sel: None,
            other_pw: (false, String::new(), String::new()),
            name_edit: None,
            zone_filter: String::new(),
            icon_src: String::new(),
            app_tab: 0,
            app_sub: match start_sub().as_str() {
                "icons" => AppSub::Icons,
                "bar" => AppSub::Bar,
                "notifs" => AppSub::Notifs,
                "wallpaper" => AppSub::Wallpaper,
                "minos" => AppSub::Minos,
                "lock" => AppSub::Lock,
                "workspaces" => AppSub::Workspaces,
                "boot" => AppSub::Boot,
                _ => AppSub::Main,
            },
            lock_previews_started: false,
            boot_info: None,
            boot_pic: String::new(),
            splash_pic: String::new(),
            bt_scan_until: None,
            bt_found: (std::time::Instant::now(), vec![]),
            bt_pairing: None,
            bt_pairing_done: Default::default(),
            pad_sel: None,
            pad_add: None,
            boot_busy: Default::default(),
            ws_conf: workspaces::load(),
            app_sort: 0,
            app_sel: None,
            ask_cache: None,
            dev_clicks: 0,
            dev_last: std::time::Instant::now(),
            dev_prompt: false,
            unlock_confirm: None,
            unlock_read: false,
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
            night: None,
        };
        // `yggdrasil-settings apps:ID` opens that app's own page (links from the dock or the notch)
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
                Page::Users => {
                    let me = users::account();
                    let (all, groups) = users::all();
                    Msg::Account(me, users::keyring_state(), all, groups)
                }
                Page::Security => Msg::Security(security::overview()),
                Page::Restore => Msg::Restore(restore::load()),
                Page::DateTime => Msg::Time(sys::time()),
                Page::Apps => {
                    let _ = tx.send(Msg::Installed(load_installed(&tx)));
                    Msg::Apps(sys::apps())
                }
                Page::DefaultApps => Msg::Defaults(defaults::load()),
                Page::Assistant => Msg::Assistant(controls::load(yggdrasil_conf::integrations::first("assistant").map(|i| i.controls()).unwrap_or_default())),
                Page::Printers => Msg::Printers(printers::load()),
                Page::Shortcuts | Page::Developer => return,
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
                Msg::Account(a, k, all, groups) => {
                    self.data.account = Some((a, k));
                    self.data.accounts = Some(all);
                    self.data.opt_groups = groups;
                }
                Msg::AccountDone(r) => {
                    self.acct_busy = false;
                    match r {
                        Ok(m) => {
                            self.acct_msg = Some((m, true));
                            self.acct_form = AddForm::default();
                            self.acct_remove = None;
                            self.name_edit = None;
                            self.load(Page::Users, ctx);
                        }
                        Err(e) => self.acct_msg = Some((e, false)),
                    }
                    continue;
                }
                Msg::Security(o) => self.data.security = Some(o),
                Msg::Logins(l) => {
                    self.logins = Some(l);
                    self.log_busy = false;
                }
                Msg::SudoLog(l) => {
                    self.sudo_log = Some(l);
                    self.log_busy = false;
                }
                Msg::Restore(r) => self.data.restore = Some(r),
                Msg::Defaults(r) => self.data.defaults = Some(r),
                Msg::Assistant(a) => self.data.assistant = Some(a),
                Msg::Printers(p) => self.data.printers = Some(p),
                Msg::Scan(r) => {
                    self.scan_busy = false;
                    self.scan_report = Some(r);
                    continue;
                }
                Msg::Password(r) => {
                    self.pw_busy = false;
                    match r {
                        Ok(()) => {
                            self.pw_msg = Some(("Password changed. Your keyring follows it.".into(), true));
                            self.pw_open = false;
                            self.load(Page::Users, ctx);
                        }
                        Err(e) => self.pw_msg = Some((e, false)),
                    }
                    continue;
                }
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

/// `yggdrasil-settings security:monitor` (or `appearance:icons`) opens a page inside the page.
fn start_sub() -> String {
    std::env::args().nth(1).unwrap_or_default().to_lowercase().split_once(':').map(|(_, s)| s.to_string()).unwrap_or_default()
}

/// `yggdrasil-settings displays` opens on a page (for links from the top bar or other apps).
fn start_page() -> Page {
    let arg = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    let arg = arg.split(':').next().unwrap_or_default().to_string();
    PAGES.iter().find(|(p, name, _)| format!("{p:?}").to_lowercase() == arg || name.to_lowercase().starts_with(&arg) && !arg.is_empty())
        .map(|(p, _, _)| *p)
        .unwrap_or(Page::Network)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        self.receive(&ctx);
        // Read the open page again every few seconds, so a change made somewhere else (Fylgja's dark mode, the
        // panel, a command) shows up without closing Settings. Not while something is being dragged or typed, not
        // for pages that are slow to read (Wi-Fi scans, Bluetooth discovery, the package list), and not when
        // the window is in the background.
        let refreshable = matches!(self.page, Page::About | Page::Displays | Page::Sound | Page::Appearance | Page::Power | Page::Firewall | Page::DateTime | Page::Security);
        if refreshable {
            let idle = self.loading.is_none() && !ctx.input(|i| i.pointer.any_down()) && ctx.memory(|m| m.focused().is_none()) && ctx.input(|i| i.viewport().focused != Some(false));
            if idle && self.refreshed.elapsed() >= std::time::Duration::from_secs(4) {
                let page = self.page;
                self.load(page, &ctx);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(4));
        } else if self.page == Page::Bluetooth
            && self.bt_scan_until.is_none()
            && self.data.bluetooth.as_ref().is_some_and(|b| !b.pads.is_empty() || !b.paired_pads.is_empty())
        {
            // controllers listed: their battery (and whether they are connected) stays current
            let idle = self.loading.is_none() && !ctx.input(|i| i.pointer.any_down()) && ctx.input(|i| i.viewport().focused != Some(false));
            if idle && self.refreshed.elapsed() >= std::time::Duration::from_secs(5) {
                let page = self.page;
                self.load(page, &ctx);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        } else if self.page == Page::Apps && self.app_tab == 0 && self.app_confirm.is_none() {
            // the app list: running or not, new launches, and changes made from the command line
            let idle = self.loading.is_none() && !ctx.input(|i| i.pointer.any_down()) && ctx.memory(|m| m.focused().is_none()) && ctx.input(|i| i.viewport().focused != Some(false));
            if idle && self.refreshed.elapsed() >= std::time::Duration::from_secs(4) {
                self.refreshed = std::time::Instant::now();
                let (tx, c) = (self.tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let theme = sys::current_icon_theme();
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
                let developer_on = yggdrasil_conf::PanelConf::load().developer;
                for (p, name, icon) in PAGES {
                    let assistant_name = if p == Page::Assistant { yggdrasil_conf::integrations::first("assistant").and_then(|i| i.get("name").map(str::to_string)) } else { None };
                    let name: &str = assistant_name.as_deref().unwrap_or(name);
                    if p == Page::Developer && !developer_on || p == Page::Assistant && yggdrasil_conf::integrations::first("assistant").is_none() {
                        continue;
                    }
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
                    if resp.clicked() && p == Page::Appearance {
                        self.app_sub = AppSub::Main;
                    }
                    if resp.clicked() {
                        self.distress.close();
                    }
                    if resp.clicked() && p == Page::Security {
                        self.sec_sub = SecSub::Main;
                    }
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
                let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
                if self.demo_scroll > 0 {
                    // the preview for pictures: start scrolled to the bottom
                    self.demo_scroll -= 1;
                    area = area.vertical_scroll_offset(self.demo_offset);
                    ctx.request_repaint();
                }
                area.show(ui, |ui| {
                    // Room for the scroll bar on the right, so cards are never cut off under it.
                    ui.set_max_width((ui.available_width() - 20.0).min(760.0));
                    if !self.status.is_empty() {
                        ui.colored_label(pal.red, &self.status);
                    }
                    match self.page {
                        Page::About => self.page_about(ui, &pal, &ctx),
                        Page::Network => {
                            self.page_network(ui, &pal, &ctx);
                            self.mac_section(ui, &pal, &ctx);
                        }
                        Page::Bluetooth => self.page_bluetooth(ui, &pal, &ctx),
                        Page::Displays => self.page_displays(ui, &pal, &ctx),
                        Page::Sound => self.page_sound(ui, &pal, &ctx),
                        Page::Appearance => self.page_appearance(ui, &pal, &ctx),
                        Page::Power => self.page_power(ui, &pal, &ctx),
                        Page::Firewall => self.page_firewall(ui, &pal, &ctx),
                        Page::Users => self.page_users(ui, &pal, &ctx),
                        Page::Security => self.page_security(ui, &pal, &ctx),
                        Page::Restore => self.page_restore(ui, &pal, &ctx),
                        Page::DefaultApps => self.page_default_apps(ui, &pal, &ctx),
                        Page::Assistant => self.page_assistant(ui, &pal, &ctx),
                        Page::Printers => self.page_printers(ui, &pal, &ctx),
                        Page::DateTime => self.page_time(ui, &pal, &ctx),
                        Page::Apps => self.page_apps(ui, &pal, &ctx),
                        Page::Shortcuts => page_shortcuts(ui, &pal),
                        Page::Developer => self.page_developer(ui, &pal, &ctx),
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
    let theme = sys::current_icon_theme();
    let apps = appmgr::list_apps(&theme);
    let (tx, list) = (tx.clone(), apps.clone());
    std::thread::spawn(move || {
        let sizes = list.iter().map(|a| (a.id.clone(), appmgr::data_dirs(a).iter().map(|(p, _)| appmgr::dir_size(p)).sum::<u64>())).collect();
        let _ = tx.send(Msg::AppSizes(sizes));
    });
    apps
}

const BRAND_SHAPES: [(&str, &str); 5] = [("glyph", "Logo only"), ("squircle", "Squircle"), ("rounded", "Rounded square"), ("circle", "Circle"), ("hexagon", "Hexagon")];
const BRAND_STYLES: [(&str, &str); 3] = [("brand", "Brand colors"), ("dark", "Dark tile"), ("purple", "Accent colour")];

fn loading(ui: &mut egui::Ui) {
    ui.add_space(40.0);
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label("Loading…");
    });
}

impl App {
    /// Appearance > Workspaces: the default profile and one for each workspace.
    fn workspaces_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        use workspaces::{Default as Dflt, Profile};
        let mut c = self.ws_conf.clone();
        ui::group(ui, pal, None, |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Standard: windows float and you move and size them. Tiling: windows are laid out on their own, side by side, like Hyprland. This is used by Yggdrasil's own compositor (it takes effect once that replaces KWin); the choice is kept until then.").size(12.5).color(pal.dim));
            ui.add_space(6.0);
        });
        ui::group(ui, pal, Some("Default"), |ui| {
            let sel = match c.default {
                Dflt::Fixed(Profile::Standard) => 0,
                Dflt::Fixed(Profile::Tiling) => 1,
                Dflt::Ask => 2,
            };
            ui::row(ui, pal, true, "Every workspace starts as", Some("Ask each time shows a question when you open an empty workspace"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &["Standard", "Tiling", "Ask each time"], sel) {
                    c.default = [Dflt::Fixed(Profile::Standard), Dflt::Fixed(Profile::Tiling), Dflt::Ask][i];
                }
            });
        });
        ui::group(ui, pal, Some("Each workspace"), |ui| {
            for n in 0..4 {
                let label = |p: Option<Profile>| match p {
                    None => "Use the default",
                    Some(Profile::Standard) => "Always standard",
                    Some(Profile::Tiling) => "Always tiling",
                };
                ui::row(ui, pal, n == 0, &format!("Workspace {}", n + 1), None, |ui| {
                    egui::ComboBox::from_id_salt(("ws", n)).selected_text(label(c.per[n])).show_ui(ui, |ui| {
                        for p in [None, Some(Profile::Standard), Some(Profile::Tiling)] {
                            if ui.selectable_label(c.per[n] == p, label(p)).clicked() {
                                c.per[n] = p;
                            }
                        }
                    });
                });
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Super+T floats or tiles the focused window on a tiling workspace. Super+Ctrl+1 to 4 switch workspaces.").size(12.0).color(pal.dim));
        if c != self.ws_conf {
            self.ws_conf = c.clone();
            self.change(ctx, move || workspaces::save(&c));
        }
    }

    /// Appearance > Lock screen: the designs, each drawn by yggdrasil-lock into a picture (nothing is locked), and which one is in use.
    /// Appearance > Boot screen: the operating system's animation, the start-up messages as text, or your own picture (Plymouth).
    fn boot_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        const HELPER: &str = "/usr/lib/yggdrasil/bootscreen";
        let busy = self.boot_busy.load(Ordering::Acquire);
        if !busy && self.boot_info.as_ref().map(|(t, _)| t.elapsed() > std::time::Duration::from_secs(5)).unwrap_or(true) {
            let v: serde_json::Value = sys::run(HELPER, &["get"]).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            self.boot_info = Some((std::time::Instant::now(), v));
        }
        let info = self.boot_info.as_ref().map(|(_, v)| v.clone()).unwrap_or_default();
        if info.is_null() && busy {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "Changing the boot screen", Some("The start-up image is rebuilt, which takes about a minute..."), |_| {}));
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
            return;
        }
        if info.is_null() {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "The boot screen cannot be changed here", Some("Plymouth is not installed"), |_| {}));
            // the screen after signing in is Yggdrasil's own: it needs no Plymouth
            self.splash_section(ui, pal);
            return;
        }
        let ready = info["ready"].as_bool().unwrap_or(false);
        let anim = info["animation"].as_str().unwrap_or("spinner").to_string();
        let idx = match info["choice"].as_str() {
            Some("messages") => 1,
            Some("picture") => 2,
            _ => 0,
        };
        let mut pick: Option<usize> = None;
        ui::group(ui, pal, None, |ui| {
            // until Plymouth is switched on nothing is chosen yet: any of the three switches it on
            let shown = if ready { idx } else { usize::MAX };
            ui::row(ui, pal, true, "While the computer starts", Some(match shown {
                1 => "The start-up messages, as text",
                2 => "Your own picture",
                0 => "The animation",
                _ => "Not set up yet: pick one",
            }), |ui| {
                if !busy {
                    pick = ui::segmented(ui, pal, &["Animation", "Messages", "Your picture"], shown);
                }
            });
            let preview = format!("/usr/share/plymouth/themes/{anim}/preview.png");
            if std::path::Path::new(&preview).is_file() {
                ui.add_space(6.0);
                ui.add(egui::Image::new(format!("file://{preview}")).fit_to_exact_size(vec2(320.0, 180.0)).corner_radius(8));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui::field(ui, pal, &mut self.boot_pic, "Your picture: /home/you/Pictures/boot.png", 340.0, false);
                let ok = std::path::Path::new(self.boot_pic.trim()).is_file();
                if ui.add_enabled(ok && !busy, egui::Button::new("Use this picture")).clicked() {
                    pick = Some(3);
                }
            });
        });
        ui.add_space(4.0);
        let note = if busy {
            "Changing the boot screen: the start-up image is rebuilt, which takes about a minute..."
        } else if !ready {
            "The first change switches the boot screen on (it asks for your password and takes about a minute). It shows from the next start."
        } else {
            "A change shows from the next start. Press Escape while it shows to see the messages."
        };
        ui.label(RichText::new(note).color(pal.dim));
        // the same choice once more: brings a newer Settings' boot set-up into the start-up image (e.g. NVIDIA's early start)
        if ready && !busy && idx != 2 && ui::quiet(ui, pal, "Apply again").on_hover_text("Rebuild the start-up image with this choice (about a minute)").clicked() {
            pick = Some(idx);
        }
        if let Some(k) = pick {
            let args: Vec<String> = match k {
                0 => vec!["animation".into()],
                1 => vec!["messages".into()],
                2 if !std::path::Path::new(self.boot_pic.trim()).is_file() => {
                    self.status = "Type the path of a picture first, then Use this picture".into();
                    vec![]
                }
                _ => vec!["picture".into(), self.boot_pic.trim().to_string()],
            };
            if !args.is_empty() {
                let flag = self.boot_busy.clone();
                flag.store(true, Ordering::Release);
                self.boot_info = None;
                self.change(ctx, move || {
                    let mut a: Vec<&str> = vec![HELPER, "set"];
                    a.extend(args.iter().map(String::as_str));
                    let r = sys::act("pkexec", &a);
                    flag.store(false, Ordering::Release);
                    r
                });
            }
        }
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        self.splash_section(ui, pal);
    }

    /// Appearance > Boot screen > After signing in: what shows until the desktop is ready (yggdrasil-splash reads
    /// ~/.config/yggdrasil/splash.conf; no password needed, it is yours).
    fn splash_section(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let file = sys::config_dir().join("yggdrasil").join("splash.conf");
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let get = |k: &str| sys::ini_get(&format!("[x]\n{text}"), "x", k).map(|v| v.split('#').next().unwrap_or("").trim().to_string());
        let idx = match get("mode").as_deref() {
            Some("messages") => 1,
            Some("picture") => 2,
            Some("off") => 3,
            _ => 0,
        };
        if self.splash_pic.is_empty() {
            self.splash_pic = get("picture").unwrap_or_default();
        }
        let mut write: Option<(&str, String)> = None;
        ui.add_space(10.0);
        ui::group(ui, pal, Some("After signing in"), |ui| {
            ui::row(ui, pal, true, "Until the desktop is ready", Some(match idx {
                1 => "What is starting, as text",
                2 => "Your own picture",
                3 => "Nothing: a black screen",
                _ => "The animation",
            }), |ui| {
                if let Some(k) = ui::segmented(ui, pal, &["Animation", "Messages", "Your picture", "Nothing"], idx) {
                    let mode = ["animation", "messages", "picture", "off"][k];
                    if k == 2 && !std::path::Path::new(self.splash_pic.trim()).is_file() {
                        self.status = "Type the path of a picture first, then Use this picture".into();
                    } else {
                        write = Some((mode, self.splash_pic.trim().to_string()));
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui::field(ui, pal, &mut self.splash_pic, "Your picture: /home/you/Pictures/start.png", 340.0, false);
                let ok = std::path::Path::new(self.splash_pic.trim()).is_file();
                if ui.add_enabled(ok, egui::Button::new("Use this picture")).clicked() {
                    write = Some(("picture", self.splash_pic.trim().to_string()));
                }
            });
        });
        if let Some((mode, pic)) = write {
            let body = format!("# What shows between signing in and the desktop (Settings > Appearance > Boot screen)\nmode = {mode}\npicture = {pic}\n");
            self.status = match std::fs::create_dir_all(file.parent().unwrap_or(std::path::Path::new("/"))).and_then(|_| std::fs::write(&file, body)) {
                Ok(()) => "Shows the next time you sign in".into(),
                Err(e) => format!("Could not save it: {e}"),
            };
        }
    }

    fn lock_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        const DESIGNS: [(&str, &str, &str); 4] = [
            ("split", "Split", "Wallpaper on the left, a frosted sign-in panel on the right"),
            ("classic", "Classic", "Clock and password field, centred on a dark background"),
            ("zen", "Zen", "The wallpaper, a small clock and a quiet password field"),
            ("poster", "Poster", "A huge stacked clock, the field tucked in the corner"),
        ];
        let cache = yggdrasil_conf::dirs::cache().join("lock-previews");
        let conf = yggdrasil_conf::dirs::config().join("lock.conf");
        let current = std::fs::read_to_string(&conf).ok().and_then(|t| t.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == "design").map(|(_, v)| v.trim().to_string())).unwrap_or_else(|| "split".into());
        if !self.lock_previews_started {
            self.lock_previews_started = true;
            let (dir, ctx2) = (cache.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = std::fs::create_dir_all(&dir);
                for (id, _, _) in DESIGNS {
                    let out = dir.join(format!("{id}.png"));
                    let _ = std::process::Command::new("yggdrasil-lock").args(["--preview", id, "--size", "1920x1080", "--typed", "6", "--out"]).arg(&out).status();
                    ctx2.forget_image(&format!("file://{}", out.display()));
                }
                ctx2.request_repaint();
            });
        }
        ui::group(ui, pal, None, |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Pick how the lock screen looks (Super+L locks the screen).").color(pal.dim));
            ui.add_space(6.0);
        });
        for (id, name, about) in DESIGNS {
            ui::group(ui, pal, None, |ui| {
                ui.horizontal(|ui| {
                    let img = cache.join(format!("{id}.png"));
                    let (r, _) = ui.allocate_exact_size(vec2(224.0, 126.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(8), pal.bg);
                    if img.is_file() {
                        ui.put(r, egui::Image::new(format!("file://{}", img.display())).fit_to_exact_size(r.size()).corner_radius(8));
                    }
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new(name).size(16.0).strong());
                        ui.add(egui::Label::new(RichText::new(about).size(12.5).color(pal.dim)).wrap());
                        ui.add_space(8.0);
                        if current == id {
                            ui.label(RichText::new("In use").color(pal.accent));
                        } else if ui::action(ui, pal, "Use").clicked() {
                            if let Some(d) = conf.parent() {
                                let _ = std::fs::create_dir_all(d);
                            }
                            let _ = std::fs::write(&conf, format!("# Which lock screen design yggdrasil-lock draws (set in Settings > Appearance > Lock screen)\ndesign = {id}\n"));
                        }
                    });
                });
            });
        }
    }

    /// Developer Mode: the developer options.
    fn page_developer(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Developer options", Some("On: more technical settings show under Power (limits for each power mode) and Apps (which kind of connection an app may use)"), |ui| {
                if ui::quiet(ui, pal, "Turn off").clicked() {
                    let mut c = yggdrasil_conf::PanelConf::load();
                    c.developer = false;
                    self.page = Page::About;
                    self.change(ctx, move || c.save());
                }
            });
        });
    }

    fn page_about(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(a) = self.data.about.clone() else { return loading(ui) };
        let developer = yggdrasil_conf::PanelConf::load().developer;
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            // Five clicks on the logo while the ` key is held offer Developer options, like tapping the build number on Android.
            // the system's logo (LOGO in os-release); without one, a disc with its initial
            let logo = if a.logo.is_empty() {
                let (rect, r) = ui.allocate_exact_size(vec2(96.0, 96.0), Sense::click());
                ui.painter().circle_filled(rect.center(), 46.0, pal.accent);
                let initial = a.os.chars().next().unwrap_or('L').to_uppercase().to_string();
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(44.0), egui::Color32::WHITE);
                r
            } else {
                ui.add(egui::Image::new(format!("file://{}", a.logo)).fit_to_exact_size(vec2(96.0, 96.0)).sense(Sense::click()))
            };
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
            ui.label(RichText::new(if a.os.is_empty() { "Linux" } else { &a.os }).size(22.0).strong());
            ui.label(RichText::new("Desktop: Yggdrasil").size(13.0).color(pal.dim));
        });
        if self.dev_prompt {
            egui::Window::new("Turn on Developer options?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
                ui.set_max_width(400.0);
                ui.label("They add more technical settings: per-mode CPU and graphics limits under Power, and choosing which kind of connection an app may use under Apps. Nothing changes until you use them.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Turn on").clicked() {
                        self.dev_prompt = false;
                        let mut c = yggdrasil_conf::PanelConf::load();
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
                        let mut c = yggdrasil_conf::PanelConf::load();
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
                    let sub = format!("{}{}", if w.secured { "Secured" } else { "Open" }, if w.in_use { " · Connected" } else { "" });
                    ui::row(ui, pal, i == 0, &w.ssid, Some(&sub), |ui| {
                        if w.in_use {
                            if ui::quiet(ui, pal, "Disconnect").clicked() {
                                disconnect = true;
                            }
                        } else if ui::action(ui, pal, "Connect").clicked() {
                            connect = Some((w.ssid.clone(), w.secured));
                        }
                        ui.add_space(8.0);
                        ui::signal_bars(ui, pal, w.signal);
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

    /// Network > Private addresses: per saved network, which hardware (MAC) address it sees, as on phones. Private (a random one
    /// for that network, the same every time), a new random one each time, the real one, or one typed in.
    fn mac_section(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(n) = self.data.network.clone() else { return };
        if n.saved.is_empty() {
            return;
        }
        let mut set: Option<(String, bool, sys::MacMode)> = None;
        ui.add_space(8.0);
        ui::group(ui, pal, Some("Private addresses"), |ui| {
            for (i, s) in n.saved.iter().enumerate() {
                let (idx, sub) = match &s.mac {
                    sys::MacMode::Private => (0, "A random address of its own for this network, the same every time".to_string()),
                    sys::MacMode::Random => (1, "A new random address every time it connects".to_string()),
                    sys::MacMode::Real => (2, if s.wifi { "This computer's real address (Heimdall keeps it only on networks you trust)".to_string() } else { "This computer's real address".to_string() }),
                    sys::MacMode::Custom(a) => (3, format!("Uses {a}")),
                };
                let title = if s.wifi { s.name.clone() } else { format!("{} (wired)", s.name) };
                ui::row(ui, pal, i == 0, &title, Some(&sub), |ui| {
                    if let Some(k) = ui::segmented(ui, pal, &["Private", "New each time", "Real", "Custom"], idx) {
                        match k {
                            0 => set = Some((s.uuid.clone(), s.wifi, sys::MacMode::Private)),
                            1 => set = Some((s.uuid.clone(), s.wifi, sys::MacMode::Random)),
                            2 => set = Some((s.uuid.clone(), s.wifi, sys::MacMode::Real)),
                            _ => {
                                let start = if let sys::MacMode::Custom(a) = &s.mac { a.clone() } else { sys::random_mac() };
                                self.mac_typed.entry(s.uuid.clone()).or_insert(start);
                            }
                        }
                    }
                });
                // typing an address: shown once Custom was picked (or is in use)
                let typing = self.mac_typed.contains_key(&s.uuid) || matches!(s.mac, sys::MacMode::Custom(_));
                if typing {
                    let text = self.mac_typed.entry(s.uuid.clone()).or_insert_with(|| if let sys::MacMode::Custom(a) = &s.mac { a.clone() } else { sys::random_mac() });
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui::field(ui, pal, text, "02:1A:2B:3C:4D:5E", 190.0, false);
                        let ok = sys::valid_mac(text.trim());
                        if ui::quiet(ui, pal, "Random").on_hover_text("Make up an address, like macchanger -r").clicked() {
                            *text = sys::random_mac();
                        }
                        if ui.add_enabled(ok, egui::Button::new("Use this address")).clicked() {
                            set = Some((s.uuid.clone(), s.wifi, sys::MacMode::Custom(text.trim().to_uppercase())));
                        }
                        if !ok {
                            ui.label(RichText::new("Six pairs like 02:1A:2B:3C:4D:5E").color(pal.dim));
                        }
                    });
                }
            }
            ui.add_space(4.0);
            ui.label(RichText::new("A change is used from the next time the computer connects to that network.").color(pal.dim));
        });
        if let Some((uuid, wifi, mode)) = set {
            if !matches!(mode, sys::MacMode::Custom(_)) {
                self.mac_typed.remove(&uuid);
            }
            self.change(ctx, move || sys::set_mac(&uuid, wifi, &mode));
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
                    // game controllers have their own list below
                    let none = if b.paired_pads.is_empty() { "No devices yet" } else { "No other devices" };
                    ui::row(ui, pal, true, none, Some("Use “Add a device” to pair one"), |_| {});
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
            if self.pad_add.is_none() {
                self.bt_add(ui, pal, ctx);
            }
            ui.add_space(8.0);
            self.bt_controllers(ui, pal, ctx, &b);
        }
    }

    /// Bluetooth > Controllers: game controllers (connected over USB or Bluetooth, or paired and off) with their battery; one's
    /// options open below it: light bar colour, player light, a rumble test, disconnect and forget. The lights chosen are kept in
    /// ~/.config/yggdrasil/controllers.json, which the Controllers plugin reads to put them back on every connection.
    fn bt_controllers(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, b: &sys::Bluetooth) {
        const COLOURS: [(&str, [u8; 3]); 6] = [("Purple", [124, 84, 214]), ("Blue", [40, 110, 255]), ("Red", [255, 40, 40]), ("Green", [34, 197, 94]), ("White", [255, 255, 255]), ("Off", [0, 0, 0])];
        enum Do {
            Select(String),
            Light(gamepads::Pad, [u8; 3]),
            Player(gamepads::Pad, u8),
            Rumble(gamepads::Pad),
            Bt(String, &'static str),
        }
        let mut todo: Option<Do> = None;
        // the paired Bluetooth controllers that are off (the connected ones are listed from the kernel, with their battery)
        let off: Vec<&sys::BtDevice> = b.paired_pads.iter().filter(|d| !b.pads.iter().any(|p| p.mac() == Some(d.mac.to_lowercase().as_str()))).collect();
        ui::group(ui, pal, Some("Controllers"), |ui| {
            if b.pads.is_empty() && off.is_empty() {
                ui::row(ui, pal, true, "No controllers yet", Some("Use “Add a controller” to pair one, or plug one in with a cable"), |_| {});
            }
            for (i, p) in b.pads.iter().enumerate() {
                let key = p.uniq.clone();
                let open = self.pad_sel.as_deref() == Some(key.as_str());
                let mut sub = p.connection.label().to_string();
                if let Some(bat) = &p.battery {
                    sub.push_str(&format!(" · {}", bat.describe()));
                }
                ui::row(ui, pal, i == 0, &p.title(), Some(&sub), |ui| {
                    if ui::quiet(ui, pal, if open { "Close" } else { "Options" }).clicked() {
                        todo = Some(Do::Select(if open { String::new() } else { key.clone() }));
                    }
                    if let Some(pct) = p.battery.as_ref().and_then(|bat| bat.estimate()) {
                        let charging = p.battery.as_ref().is_some_and(|bat| bat.charging());
                        let (r, _) = ui.allocate_exact_size(egui::vec2(90.0, 8.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 4.0, pal.field);
                        let col = if charging || pct > 40 { pal.green } else if pct > 20 { pal.amber } else { pal.red };
                        let mut fill = r;
                        fill.set_width((r.width() * pct as f32 / 100.0).max(8.0));
                        ui.painter().rect_filled(fill, 4.0, col);
                    }
                });
                if !open {
                    continue;
                }
                if p.leds.has_lightbar() {
                    let current = b.pad_settings.lightbar_for(p);
                    ui::row(ui, pal, false, "Light bar", if p.leds.writable { None } else { Some("Install Controller support from Agora to change the lights") }, |ui| {
                        ui.add_enabled_ui(p.leds.writable, |ui| {
                            for (name, rgb) in COLOURS.iter().rev() {
                                let (r, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                                let c = if *rgb == [0, 0, 0] { pal.field } else { egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]) };
                                ui.painter().circle_filled(r.center(), 10.0, c);
                                if current == Some(*rgb) || resp.hovered() {
                                    ui.painter().circle_stroke(r.center(), 11.5, egui::Stroke::new(2.0, pal.text));
                                }
                                if resp.on_hover_text(*name).clicked() {
                                    todo = Some(Do::Light(p.clone(), *rgb));
                                }
                            }
                        });
                    });
                }
                if !p.leds.players.is_empty() {
                    let now = b.pad_settings.player_for(p).unwrap_or(0) as usize;
                    ui::row(ui, pal, false, "Player light", None, |ui| {
                        ui.add_enabled_ui(p.leds.writable, |ui| {
                            if let Some(n) = ui::segmented(ui, pal, &["Off", "1", "2", "3", "4", "5"], now) {
                                todo = Some(Do::Player(p.clone(), n as u8));
                            }
                        });
                    });
                }
                if let Some(mode) = p.mode() {
                    // an 8BitDo: the mode it was switched into decides which driver takes it
                    let note = match mode {
                        "Xbox mode" => "Recommended. Over Bluetooth it works best with Xbox controller support from Agora",
                        "Switch mode" => "Driven like a Switch Pro Controller",
                        _ => "Basic buttons only; Xbox mode (Start + X when pairing) works best",
                    };
                    ui::row(ui, pal, false, &format!("Mode: {mode}"), Some(note), |_| {});
                    let fw_note = if p.model == gamepads::Model::XboxCompatible { "If it is an 8BitDo: use a Chromium-based browser such as Vivaldi; connect the controller by USB" } else { "Use a Chromium-based browser such as Vivaldi; connect the controller by USB" };
                    ui::row(ui, pal, false, "Firmware updates", Some(fw_note), |ui| {
                        if ui::quiet(ui, pal, "Open 8BitDo's updater").clicked() {
                            let _ = std::process::Command::new("xdg-open").arg("https://web.8bitdo.com").stdin(std::process::Stdio::null()).spawn();
                        }
                    });
                }
                ui::row(ui, pal, false, "Rumble", Some("A short buzz, to check it works"), |ui| {
                    ui.add_enabled_ui(p.event.is_some(), |ui| {
                        if ui::quiet(ui, pal, "Test").clicked() {
                            todo = Some(Do::Rumble(p.clone()));
                        }
                    });
                });
                if let Some(mac) = p.mac() {
                    let mac = mac.to_uppercase();
                    ui::row(ui, pal, false, &mac, Some("Bluetooth address"), |ui| {
                        if ui::quiet(ui, pal, "Forget").clicked() {
                            todo = Some(Do::Bt(mac.clone(), "remove"));
                        }
                        if ui::quiet(ui, pal, "Disconnect").clicked() {
                            todo = Some(Do::Bt(mac.clone(), "disconnect"));
                        }
                    });
                }
            }
            for (i, d) in off.iter().enumerate() {
                ui::row(ui, pal, i == 0 && b.pads.is_empty(), &d.name, Some("Not connected · press its PS, Xbox or Home button"), |ui| {
                    if ui::quiet(ui, pal, "Forget").clicked() {
                        todo = Some(Do::Bt(d.mac.clone(), "remove"));
                    }
                    if ui::action(ui, pal, "Connect").clicked() {
                        todo = Some(Do::Bt(d.mac.clone(), "connect"));
                    }
                });
            }
            if !std::path::Path::new("/usr/bin/yggdrasil-controllers").exists() {
                ui::row(ui, pal, false, "Controller support", Some("Battery in the top bar, and your lights kept between connections"), |ui| {
                    if ui::quiet(ui, pal, "Get it in Agora").clicked() {
                        sys::spawn("agora", &[]);
                    }
                });
            }
        });
        match todo {
            Some(Do::Select(k)) => self.pad_sel = (!k.is_empty()).then_some(k),
            Some(Do::Light(p, rgb)) => {
                let mut st = b.pad_settings.clone();
                st.per_pad.entry(p.uniq.clone()).or_default().lightbar = Some(gamepads::color_hex(rgb));
                self.change(ctx, move || {
                    st.save().map_err(|e| e.to_string())?;
                    gamepads::set_lightbar(&p, rgb)
                });
            }
            Some(Do::Player(p, n)) => {
                let mut st = b.pad_settings.clone();
                st.per_pad.entry(p.uniq.clone()).or_default().player = Some(n);
                self.change(ctx, move || {
                    st.save().map_err(|e| e.to_string())?;
                    gamepads::set_player(&p, n)
                });
            }
            Some(Do::Rumble(p)) => {
                // it blocks for the length of the buzz: on a thread, not reloading the page
                std::thread::spawn(move || {
                    let _ = gamepads::rumble(&p, 400);
                });
            }
            Some(Do::Bt(mac, verb)) => {
                if verb == "remove" {
                    self.pad_sel = None;
                }
                self.change(ctx, move || sys::act("bluetoothctl", &[verb, &mac]));
            }
            None => {}
        }
        ui.add_space(8.0);
        self.pad_add_flow(ui, pal, ctx);
    }

    /// Bluetooth > Controllers > Add a controller: which kind, how to put it in pairing mode, then the same search and pairing as
    /// Add a device.
    fn pad_add_flow(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        use gamepads::Model;
        const KINDS: [(&str, Model); 6] = [("PlayStation 4", Model::DualShock4), ("PlayStation 5", Model::DualSense), ("Xbox", Model::Xbox), ("Switch Pro", Model::SwitchPro), ("8BitDo", Model::EightBitDo), ("Other", Model::Other)];
        match self.pad_add {
            None => {
                if ui::action(ui, pal, "Add a controller…").clicked() {
                    self.pad_add = Some(None);
                }
            }
            Some(None) => {
                let mut pick = None;
                ui::group(ui, pal, Some("Which controller?"), |ui| {
                    for (i, (name, m)) in KINDS.iter().enumerate() {
                        ui::row(ui, pal, i == 0, name, None, |ui| {
                            if ui::action(ui, pal, "Choose").clicked() {
                                pick = Some(*m);
                            }
                        });
                    }
                });
                if ui::quiet(ui, pal, "Cancel").clicked() {
                    self.pad_add = None;
                }
                if let Some(m) = pick {
                    self.pad_add = Some(Some(m));
                    sys::bt_scan(30);
                    self.bt_scan_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
                    self.bt_found = (std::time::Instant::now(), vec![]);
                }
            }
            Some(Some(m)) => {
                ui::group(ui, pal, Some("Pairing mode"), |ui| {
                    ui::row(ui, pal, true, m.pairing_hint(), Some("Then pick it below when it turns up"), |_| {});
                    if m == Model::Xbox || m == Model::EightBitDo {
                        ui::row(ui, pal, false, "Xbox controllers (and 8BitDo in Xbox mode) work best with Xbox controller support from Agora (xpadneo)", None, |_| {});
                    }
                });
                ui.add_space(6.0);
                self.bt_add(ui, pal, ctx);
                if self.bt_scan_until.is_none() {
                    // Done in the list: the flow is over
                    self.pad_add = None;
                }
            }
        }
    }

    /// Bluetooth > Add a device: looks for devices nearby for 20 seconds and lists them as they turn up; Pair pairs, trusts
    /// and connects one (a code to compare comes up in Yggdrasil's own box).
    fn bt_add(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let searching = self.bt_scan_until.map(|t| std::time::Instant::now() < t).unwrap_or(false);
        if self.bt_scan_until.is_none() {
            if ui::action(ui, pal, "Add a device…").on_hover_text("Put the device in pairing mode first").clicked() {
                sys::bt_scan(20);
                self.bt_scan_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(20));
                self.bt_found = (std::time::Instant::now(), vec![]);
            }
            return;
        }
        if self.bt_found.0.elapsed() > std::time::Duration::from_millis(1500) {
            self.bt_found = (std::time::Instant::now(), sys::bt_nearby());
        }
        let mut pair: Option<String> = None;
        ui::group(ui, pal, Some(if searching { "Looking for devices nearby…" } else { "Devices nearby" }), |ui| {
            for (i, (mac, name)) in self.bt_found.1.iter().enumerate() {
                ui::row(ui, pal, i == 0, name, Some(mac), |ui| {
                    if self.bt_pairing.as_deref() == Some(mac.as_str()) {
                        ui.label(RichText::new("Pairing…").color(pal.dim));
                    } else if ui::action(ui, pal, "Pair").clicked() {
                        pair = Some(mac.clone());
                    }
                });
            }
            if self.bt_found.1.is_empty() {
                ui.label(RichText::new(if searching { "Nothing yet. Put the device in pairing mode." } else { "Nothing found. Is the device in pairing mode?" }).color(pal.dim));
            }
        });
        ui.horizontal(|ui| {
            if !searching && ui::quiet(ui, pal, "Look again").clicked() {
                sys::bt_scan(20);
                self.bt_scan_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(20));
            }
            if ui::quiet(ui, pal, "Done").clicked() {
                self.bt_scan_until = None;
                self.bt_found.1.clear();
            }
        });
        if let Some(mac) = pair {
            self.bt_pairing = Some(mac.clone());
            let busy = self.bt_pairing_done.clone();
            busy.store(false, std::sync::atomic::Ordering::Release);
            self.change(ctx, move || {
                let r = sys::bt_pair(&mac);
                busy.store(true, std::sync::atomic::Ordering::Release);
                r
            });
        }
        if self.bt_pairing_done.load(std::sync::atomic::Ordering::Acquire) {
            self.bt_pairing = None;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(700));
    }

    fn page_displays(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(d) = self.data.displays.clone() else { return loading(ui) };
        let mut cmd: Option<Vec<String>> = None;
        let mut tiling: Option<(String, bool)> = None;
        let hypr = sys::hyprland();
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
                if !o.primary && !hypr {
                    ui::row(ui, pal, false, "Main display", Some("The top bar and dock live here"), |ui| {
                        if ui::quiet(ui, pal, "Make main").clicked() {
                            cmd = Some(vec![format!("output.{}.priority.1", o.name)]);
                        }
                    });
                }
                let mut is_tiling = d.tiling.split(',').any(|t| t.trim() == o.name);
                ui::row(ui, pal, false, "Tiling screen", Some("Windows tile automatically, Omarchy-style, with their own workspaces"), |ui| {
                    if ui::switch(ui, pal, &mut is_tiling).changed() {
                        tiling = Some((o.name.clone(), is_tiling));
                    }
                });
            });
        }
        if d.outputs.is_empty() {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "No displays reported", None, |_| {}));
        }
        if let Some(args) = cmd {
            self.change(ctx, move || {
                // the choices as Hyprland settings: "output.NAME.mode.ID" or "output.NAME.scale.S"
                let parts: Vec<&str> = args[0].splitn(4, '.').collect();
                match parts.as_slice() {
                    ["output", name, "mode", id] => sys::hypr_set(name, Some(id), None),
                    ["output", name, "scale", s] => sys::hypr_set(name, None, s.parse().ok()),
                    _ => Err("not available here yet".into()),
                }
            });
        }
        if let Some((name, on)) = tiling {
            self.change(ctx, move || sys::set_tiling(&name, on));
        }
        if hypr {
            self.night_light(ui, pal);
            zones_ui::group(ui, pal);
        }
    }

    /// Displays > Night Light: the same settings as the notch's drop-down; yggdrasil_conf::nightlight writes them and runs hyprsunset.
    fn night_light(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        use nightlight::{fmt_time, MAX_FADE, MAX_TEMP, MIN_FADE, MIN_TEMP};
        let (n, on, tx) = self.night.get_or_insert_with(|| (nightlight::load(), nightlight::is_on(), nightlight::spawn()));
        let before = n.clone();
        let mut toggled = None;
        ui::group(ui, pal, Some("Night Light"), |ui| {
            ui::row(ui, pal, true, "Night Light", Some("Warmer colours, easier on the eyes in the evening"), |ui| {
                if ui::switch(ui, pal, on).changed() {
                    toggled = Some(*on);
                }
            });
            let sub = if n.schedule { format!("On from {} to {}", fmt_time(n.on), fmt_time(n.off)) } else { "Off: it stays as you switch it".to_string() };
            ui::row(ui, pal, false, "Schedule", Some(&sub), |ui| {
                ui::switch(ui, pal, &mut n.schedule);
            });
            ui.add_enabled_ui(n.schedule, |ui| {
                ui::row(ui, pal, false, "Turns on", None, |ui| time_field(ui, &mut n.on));
                ui::row(ui, pal, false, "Turns off", None, |ui| time_field(ui, &mut n.off));
            });
            ui::row(ui, pal, false, "Temperature", Some("Lower is warmer"), |ui| {
                let mut t = n.temp as f32;
                if ui::slider_range(ui, pal, &mut t, MIN_TEMP as f32..=MAX_TEMP as f32, " K").changed() {
                    n.temp = ((t / 100.0).round() * 100.0) as u32;
                }
            });
            ui.add_enabled_ui(n.schedule, |ui| {
                ui::row(ui, pal, false, "Gradual fade", Some("Eases in from the turn-on time and back out from the turn-off time"), |ui| {
                    ui::switch(ui, pal, &mut n.fade);
                });
                ui.add_enabled_ui(n.fade, |ui| {
                    ui::row(ui, pal, false, "Over", None, |ui| {
                        let mut m = n.minutes as f32;
                        if ui::slider_range(ui, pal, &mut m, MIN_FADE as f32..=MAX_FADE as f32, " min").changed() {
                            n.minutes = ((m / 5.0).round() * 5.0) as u32;
                        }
                    });
                });
            });
            // the two ramps, painted in the colours the screen goes through
            if let Some(ramps) = nightlight::ramps(n) {
                for (i, (from_t, to_t, from_k, to_k)) in ramps.into_iter().enumerate() {
                    let title = if i == 0 { "Fading in" } else { "Fading out" };
                    ui::row(ui, pal, false, title, Some(&format!("{} to {}", fmt_time(from_t), fmt_time(to_t))), |ui| ramp_bar(ui, pal, from_k, to_k));
                }
            }
        });
        if let Some(on) = toggled {
            let _ = tx.send(nightlight::Cmd::Toggle(on));
            if !n.schedule {
                n.light = on;
            }
        }
        if *n != before {
            if n.schedule != before.schedule {
                // without a schedule the light stays as it is now
                n.light = *on;
            }
            let _ = tx.send(nightlight::Cmd::Set(n.clone()));
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
        let mut minos_live = false;
        let mut conf = a.panel;
        let mut conf_changed = false;
        if self.app_sub != AppSub::Main {
            let title = match self.app_sub {
                AppSub::Icons => "Icons",
                AppSub::Bar => "Top bar and dock",
                AppSub::Notifs => "Notifications",
                AppSub::Minos => "Minos",
                AppSub::Lock => "Lock screen",
                AppSub::Workspaces => "Workspaces",
                AppSub::Boot => "Boot screen",
                _ => "Wallpaper",
            };
            ui.horizontal(|ui| {
                if ui::quiet(ui, pal, "\u{2039}  Appearance").clicked() {
                    self.app_sub = AppSub::Main;
                }
                ui.label(RichText::new(title).size(18.0).strong());
            });
            ui.add_space(6.0);
        }
        if self.app_sub == AppSub::Main {
        ui::group(ui, pal, Some("Style"), |ui| {
            ui::row(ui, pal, true, "Look", None, |ui| {
                if let Some(i) = ui::segmented(ui, pal, &["Light", "Dark"], if a.dark { 1 } else { 0 }) {
                    style = Some(i == 1);
                }
            });
            ui::row(ui, pal, false, "Accent color", Some("Selections, switches, the active window border"), |ui| {
                for (name, rgb) in sys::ACCENTS.iter().rev() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                    let c = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                    ui.painter().circle_filled(rect.center(), 10.0, c);
                    if a.accent == Some(*rgb) || (a.accent.is_none() && *rgb == yggdrasil_conf::integrations::default_accent()) {
                        ui.painter().circle_stroke(rect.center(), 12.5, egui::Stroke::new(2.0, pal.text));
                    }
                    if resp.on_hover_text(*name).clicked() {
                        accent = Some(*rgb);
                    }
                }
            });
        });
        }
        if self.app_sub == AppSub::Main {
            ui::group(ui, pal, None, |ui| {
                if ui::nav_row(ui, pal, true, "Top bar and dock", Some("The bar, the dock, the start menu and the notch: how they look and what they do"), |_| {}) {
                        self.app_sub = AppSub::Bar;
                }
                if ui::nav_row(ui, pal, false, "Icons", Some("Icon packs"), |_| {}) {
                        self.app_sub = AppSub::Icons;
                }
                if ui::nav_row(ui, pal, false, "Notifications", Some("Pop-ups, the Notification Center and Do not disturb"), |_| {}) {
                        self.app_sub = AppSub::Notifs;
                }
                if ui::nav_row(ui, pal, false, "Wallpaper", Some("The desktop picture"), |_| {}) {
                        self.app_sub = AppSub::Wallpaper;
                }
                if ui::nav_row(ui, pal, false, "Lock screen", Some("How the screen looks while it is locked"), |_| {}) {
                        self.app_sub = AppSub::Lock;
                        self.lock_previews_started = false;
                }
                if ui::nav_row(ui, pal, false, "Workspaces", Some("How windows behave on each workspace: float, or tile on their own"), |_| {}) {
                    self.app_sub = AppSub::Workspaces;
                    self.ws_conf = workspaces::load();
                }
                if ui::nav_row(ui, pal, false, "Boot screen", Some("What shows while the computer starts, and after you sign in"), |_| {}) {
                    self.app_sub = AppSub::Boot;
                    self.boot_info = None;
                }
                if ui::nav_row(ui, pal, false, "Minos", Some("The file manager: make its window see-through"), |_| {}) {
                        self.app_sub = AppSub::Minos;
                }
            });
        }
        if self.app_sub == AppSub::Minos {
        ui::group(ui, pal, Some("Window"), |ui| {
            ui::row(ui, pal, true, "See-through window", Some("Lets the desktop show through Minos. Applies to open windows within a second, no restart"), |ui| {
                minos_live |= ui::switch(ui, pal, &mut conf.minos_transparent).changed();
            });
            ui::row(ui, pal, false, "Transparency", Some("How solid the window is. The sample next to it shows the result"), |ui| {
                ui.add_enabled_ui(conf.minos_transparent, |ui| {
                    let mut v = conf.minos_opacity as f32;
                    if ui::slider_range(ui, pal, &mut v, yggdrasil_conf::MINOS_OPACITY_MIN as f32..=100.0, " %").changed() {
                        conf.minos_opacity = v as u32;
                        minos_live = true;
                    }
                });
                // a small sample window over a busy backdrop, so the effect is visible without opening Minos
                let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 70.0), egui::Sense::hover());
                let pt = ui.painter_at(rect);
                pt.rect_filled(rect, 8.0, egui::Color32::from_rgb(92, 56, 170));
                for (i, c) in [egui::Color32::from_rgb(235, 120, 180), egui::Color32::from_rgb(90, 190, 230), egui::Color32::from_rgb(250, 210, 90)].into_iter().enumerate() {
                    pt.circle_filled(rect.left_top() + egui::vec2(24.0 + i as f32 * 34.0, 22.0 + (i % 2) as f32 * 26.0), 15.0, c);
                }
                let shown = if conf.minos_transparent { conf.minos_opacity } else { 100 };
                let dark = theme::palette(true).translucent(shown);
                let win = rect.shrink2(egui::vec2(14.0, 10.0));
                pt.rect_filled(win, 6.0, dark.bg);
                pt.rect_filled(egui::Rect::from_min_size(win.min, egui::vec2(win.width() * 0.3, win.height())), 6.0, dark.sidebar);
                pt.rect_stroke(win, 6.0, egui::Stroke::new(1.0, dark.border), egui::StrokeKind::Inside);
            });
        });
        }
        if self.app_sub == AppSub::Lock {
            self.lock_page(ui, pal, ctx);
        }
        if self.app_sub == AppSub::Workspaces {
            self.workspaces_page(ui, pal, ctx);
        }
        if self.app_sub == AppSub::Boot {
            self.boot_page(ui, pal, ctx);
        }
        if self.app_sub == AppSub::Icons {
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
            ui::row(ui, pal, a.icon_packs.is_empty(), "Liki", Some("Yggdrasil's own icon pack: app logos from Simple Icons in the shape and colours you pick. An app with no logo gets the default app icon. Make it, then choose it above"), |ui| {
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
            ui::row(ui, pal, false, "Install an icon pack", Some("A folder, an archive (.tar.gz, .tar.xz, .zip), an Android icon pack (.apk) or an https link. It goes in your own icons folder; nothing needs a password. An .apk is only read, never run, and gives icons to the programs installed here"), |ui| {
                if ui::action(ui, pal, "Install").clicked() && !self.icon_src.trim().is_empty() {
                    icon_install = Some(self.icon_src.trim().to_string());
                }
                ui.add(egui::TextEdit::singleline(&mut self.icon_src).hint_text("~/Downloads/pack.tar.gz, pack.apk or https://...").desired_width((ui.available_width() - 10.0).clamp(160.0, 340.0)));
            });
        });
        }
        let modes = ["Match theme", "Dark / light"];
        let mode_index = |m: yggdrasil_conf::ColorMode| if m == yggdrasil_conf::ColorMode::Theme { 0 } else { 1 };
        let mode_of = |i: usize| if i == 0 { yggdrasil_conf::ColorMode::Theme } else { yggdrasil_conf::ColorMode::Neutral };
        if self.app_sub == AppSub::Bar {
        ui::group(ui, pal, Some("Dock"), |ui| {
            let positions = ["Bottom", "Left", "Right"];
            let pos_index = match conf.dock_pos {
                yggdrasil_conf::DockPos::Bottom => 0,
                yggdrasil_conf::DockPos::Left => 1,
                yggdrasil_conf::DockPos::Right => 2,
            };
            ui::row(ui, pal, true, "Position", Some("Which edge of the screen; left and right are vertical"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &positions, pos_index) {
                    conf.dock_pos = [yggdrasil_conf::DockPos::Bottom, yggdrasil_conf::DockPos::Left, yggdrasil_conf::DockPos::Right][i];
                    conf_changed = true;
                }
            });
            let mut gap = conf.dock_gap as f32;
            ui::row(ui, pal, false, "Distance from edge", Some("How far the dock floats off the screen edge; 0 puts it right on the edge"), |ui| {
                let r = ui::slider_range(ui, pal, &mut gap, 0.0..=yggdrasil_conf::DOCK_GAP_MAX as f32, " px");
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    conf.dock_gap = gap as u32;
                    conf_changed = true;
                }
            });
            let mut size = conf.dock_icon as f32;
            ui::row(ui, pal, false, "Size", Some("Icon size"), |ui| {
                let r = ui::slider_range(ui, pal, &mut size, yggdrasil_conf::DOCK_ICON_MIN as f32..=yggdrasil_conf::DOCK_ICON_MAX as f32, " px");
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
            ui::row(ui, pal, false, "Color", Some("Match the theme's purple, or follow only dark and light"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &modes, mode_index(conf.dock_color)) {
                    conf.dock_color = mode_of(i);
                    conf_changed = true;
                }
            });
        });
        }
        if self.app_sub == AppSub::Bar {
        ui::group(ui, pal, Some("Top bar"), |ui| {
            let mut opacity = conf.bar_opacity as f32;
            ui::row(ui, pal, true, "Background", Some("Opacity of the bar; 0% leaves just its text"), |ui| {
                let r = ui::slider(ui, pal, &mut opacity);
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    conf.bar_opacity = opacity as u32;
                    conf_changed = true;
                }
            });
            ui::row(ui, pal, false, "Color", Some("Match the theme's purple, or follow only dark and light"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &modes, mode_index(conf.bar_color)) {
                    conf.bar_color = mode_of(i);
                    conf_changed = true;
                }
            });
        });
        }
        if self.app_sub == AppSub::Bar {
        ui::group(ui, pal, Some("Start menu"), |ui| {
            let themes = ["Windows", "Folders"];
            let current = if conf.start_theme == yggdrasil_conf::StartTheme::Folders { 1 } else { 0 };
            ui::row(ui, pal, true, "Theme", Some("Windows: pinned apps and every app in a list. Folders: narrow, apps in folders by kind, and a power button"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &themes, current) {
                    conf.start_theme = if i == 1 { yggdrasil_conf::StartTheme::Folders } else { yggdrasil_conf::StartTheme::Windows };
                    conf_changed = true;
                }
            });
        });
        }
        if self.app_sub == AppSub::Main {
        ui::group(ui, pal, Some("Accessibility"), |ui| {
            let sizes = ["100%", "125%", "150%", "200%"];
            let cur = match conf.text_scale { 0..=112 => 0, 113..=137 => 1, 138..=174 => 2, _ => 3 };
            ui::row(ui, pal, true, "Text and control size", Some("Makes everything in the desktop's own apps (Settings, Minos, the admin password box, Calculator) larger. The top bar, dock and start menu follow the screen scale in Displays for now"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &sizes, cur) {
                    conf.text_scale = [100, 125, 150, 200][i];
                    conf_changed = true;
                }
            });
            ui::row(ui, pal, false, "Reduce motion", Some("No fades or slides in the desktop's own apps"), |ui| {
                if ui::switch(ui, pal, &mut conf.reduce_motion).changed() {
                    conf_changed = true;
                }
            });
        });
        }
        if self.app_sub == AppSub::Bar {
        ui::group(ui, pal, Some("Clipboard"), |ui| {
            ui::row(ui, pal, true, "Clipboard history", Some("Keeps what you copy (texts, and a few pictures) in memory only, to copy again from the Clipboard window (Super+V). It is forgotten when you log out or turn this off, and password managers are skipped. Snippets in that window are kept either way"), |ui| {
                if ui::switch(ui, pal, &mut conf.clipboard_history).changed() {
                    conf_changed = true;
                }
            });
        });
        }
        if self.app_sub == AppSub::Notifs {
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
            let cur = match conf.security_alerts {
                yggdrasil_conf::AlertLevel::Off => 0,
                yggdrasil_conf::AlertLevel::Critical => 1,
                yggdrasil_conf::AlertLevel::Warning => 2,
                yggdrasil_conf::AlertLevel::Info => 3,
            };
            if let Some(monitor) = yggdrasil_conf::integrations::security().map(security::monitor_name) {
                ui::row(ui, pal, false, &format!("{monitor} alerts"), Some(&format!("Which of {monitor}'s security alerts show up as notifications. Everything includes each new place a program connects to")), |ui| {
                    if let Some(i) = ui::segmented(ui, pal, &levels, cur) {
                        conf.security_alerts = [yggdrasil_conf::AlertLevel::Off, yggdrasil_conf::AlertLevel::Critical, yggdrasil_conf::AlertLevel::Warning, yggdrasil_conf::AlertLevel::Info][i];
                        conf_changed = true;
                    }
                });
            }
        });
        }
        if self.app_sub == AppSub::Wallpaper {
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
        }
        if minos_live {
            // saved at once (a tiny file), so the slider and the sample follow the drag and Minos follows within a second
            let _ = conf.save();
            if let Some(ap) = self.data.appearance.as_mut() {
                ap.panel = conf;
            }
            ctx.request_repaint();
        }
        if conf_changed {
            self.change(ctx, move || conf.save());
        }
        if let Some(dark) = style {
            self.change(ctx, move || {
                // Yggdrasil's own colour scheme and GTK colours (ygg-theme), in the accent already chosen
                sys::act("ygg-theme", &["apply", if dark { "--dark" } else { "--light" }])?;
                // Minos's icon follows: its dark or light version
                let _ = std::process::Command::new("liki-minos-icon").args(["-", if dark { "dark" } else { "light" }]).status();
                Ok(())
            });
        }
        if let Some([r, g, b]) = accent {
            self.change(ctx, move || {
                let rgb = format!("{r},{g},{b}");
                // the desktop's record of the choice, the colour scheme and the GTK colours in the new accent (ygg-theme writes them all)
                sys::act("ygg-theme", &["apply", "--accent", &rgb])?;
                // Minos's icon takes the new accent (when another pack is in use), and so does the Liki pack
                let _ = std::process::Command::new("liki-minos-icon").args([format!("{r},{g},{b}"), String::new()]).status();
                let _ = std::process::Command::new("liki-build").arg("--rebuild").status();
                Ok(())
            });
        }
        if let Some(p) = wallpaper {
            // the panel draws the wallpaper: the picture named in ~/.config/yggdrasil/wallpaper
            self.change(ctx, move || yggdrasil_conf::set_wallpaper(&p));
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
        // Developer options add the selected mode's limits under the mode itself: the processor's clock, then the graphics
        // card's power (and, on a card in the unlock's support table, the unlock). Choosing another mode shows that mode's.
        let mut limit: Option<(String, &'static str, Option<i64>)> = None;
        let mut unlock: Option<&'static str> = None;
        if !p.profiles.is_empty() {
            ui::group(ui, pal, Some("Power mode"), |ui| {
                let name = |x: &str| match x { "performance" => "Performance".to_string(), "balanced" => "Balanced".into(), "power-saver" => "Power saver".into(), o => o.to_string() };
                let labels: Vec<String> = p.profiles.iter().map(|x| name(x)).collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                let sel = p.profiles.iter().position(|x| *x == p.profile).unwrap_or(0);
                ui::row(ui, pal, true, "Mode", Some("Performance for games and builds; Power saver for quiet"), |ui| {
                    if let Some(i) = ui::segmented(ui, pal, &refs, sel) {
                        profile = Some(p.profiles[i].clone());
                    }
                });
                let Some(l) = &p.limits else { return };
                let mode = p.profile.clone();
                let in_mode = format!("in {} mode", name(&mode));
                let (cpu, gpu) = l.modes.get(&mode).copied().unwrap_or((None, None));
                let mut c = cpu.unwrap_or(100) as f32;
                // the row fills from the right: the slider first, so both sliders line up and the reset button sits to their left
                ui::row(ui, pal, false, "Processor", Some(&format!("The most it may clock {in_mode}, as a share of its top speed")), |ui| {
                    let r = ui::slider_range(ui, pal, &mut c, 20.0..=100.0, " %");
                    if r.drag_stopped() || (r.changed() && !r.dragged()) {
                        limit = Some((mode.clone(), "cpu", if c as i64 >= 100 { None } else { Some(c as i64) }));
                    }
                    if cpu.is_some() && ui::icon_button(ui, pal, Icon::Restart, "Back to the default", true, false).clicked() {
                        limit = Some((mode.clone(), "cpu", None));
                    }
                });
                let Some(g) = &l.gpu else { return };
                let top = g.hold.unwrap_or(g.max).max(g.min);
                let mut w = gpu.unwrap_or(g.default).min(top) as f32;
                let sub = match (g.hold, gpu) {
                    (Some(h), Some(mine)) if mine > h => format!("{}: held at {h} W while the unlock is tried; your {mine} W comes back after", g.name),
                    (Some(h), _) => format!("{}: at most {h} W while the unlock is tried", g.name),
                    _ => format!("{}: the most power it may draw {in_mode} ({}-{} W, default {} W)", g.name, g.min, g.max, g.default),
                };
                ui::row(ui, pal, false, "Graphics card", Some(&sub), |ui| {
                    let r = ui::slider_range(ui, pal, &mut w, g.min as f32..=top as f32, " W");
                    if r.drag_stopped() || (r.changed() && !r.dragged()) {
                        limit = Some((mode.clone(), "gpu", if w as i64 == g.default { None } else { Some(w as i64) }));
                    }
                    if gpu.is_some() && ui::icon_button(ui, pal, Icon::Restart, "Back to the default", true, false).clicked() {
                        limit = Some((mode.clone(), "gpu", None));
                    }
                });
                if let Some(u) = p.unlock.as_ref().filter(|u| u.offered()) {
                    let since = u.since.as_deref().and_then(|t| t.get(11..16)).unwrap_or("this start");
                    let (sub, button): (String, Option<(&str, &'static str)>) = match u.state.as_str() {
                        "supported" => match &u.blocker {
                            None => ("This card matches a tested one, but the unlock has not been run on this computer. Experimental.".into(), Some(("Arm for next restart…", "confirm"))),
                            Some(b) => (format!("This card matches a tested one. Not available yet: {b}."), None),
                        },
                        "secure-boot" => ("Needs Secure Boot off: the unlock runs a boot program that is not signed.".into(), None),
                        "armed" => ("The next restart will try the unlock once, then boot normally. If the screen stays black for more than a minute, hold the power button and start again: the computer boots normally and this switch turns itself off.".into(), Some(("Cancel", "disarm"))),
                        "unlocked" => (format!("Unlocked since {since}. {}. A restart or sleep locks it again.", u.reason), None),
                        "throttled" => (format!("{}. A restart or sleep locks it again.", u.reason), None),
                        _ => (u.reason.clone(), Some(("Allow another try…", "reenable"))),
                    };
                    ui::row(ui, pal, false, "Tensor-rate unlock", Some(&sub), |ui| {
                        if let Some((label, what)) = button {
                            if ui::quiet(ui, pal, label).clicked() {
                                unlock = Some(what);
                            }
                        }
                    });
                }
                if g.hold.is_some() && !p.unlock.as_ref().is_some_and(|u| u.state == "armed") {
                    ui::row(ui, pal, false, "Power hold", Some(&format!("Every mode stays at or below {} W since the unlock was tried. End it when the measurements are done.", g.hold.unwrap_or(g.default))), |ui| {
                        if ui::quiet(ui, pal, "End the hold…").clicked() {
                            unlock = Some("release");
                        }
                    });
                }
            });
            if p.limits.is_some() {
                ui.label(RichText::new("These limits only hold things back; nothing here overclocks. Changing one asks for your password.").size(11.5).color(pal.dim));
            }
        }
        let choices: [(i64, &str); 10] = [(0, "Never"), (1, "1 minute"), (2, "2 minutes"), (5, "5 minutes"), (10, "10 minutes"), (15, "15 minutes"), (30, "30 minutes"), (60, "1 hour"), (120, "2 hours"), (240, "4 hours")];
        let label = |m: i64| choices.iter().find(|c| c.0 == m).map(|c| c.1.to_string()).unwrap_or(format!("{m} minutes"));
        let mut idle = p.idle.clone();
        let mut idle_changed = false;
        ui::group(ui, pal, Some("When idle"), |ui| {
            let rows: [(&str, &str, i64, bool); 6] = [
                ("saver", "Screensaver after", p.idle.screensaver, true),
                ("dim", "Dim the screen after", p.idle.dim, true),
                ("off", "Turn off the screen after", p.idle.screen_off, true),
                ("lock", "Lock the screen after", p.idle.lock, true),
                ("sleep", "Sleep after", p.idle.sleep, p.idle.can_sleep),
                ("hib", "Hibernate after", p.idle.hibernate, p.idle.can_hibernate),
            ];
            for (n, (key, title, cur, ok)) in rows.into_iter().enumerate() {
                let note = if p.idle.sleep_note.is_empty() { "Not available on this computer".to_string() } else { p.idle.sleep_note.clone() };
                let sub = if ok { None } else { Some(note.as_str()) };
                ui::row(ui, pal, n == 0, title, sub, |ui| {
                    ui.add_enabled_ui(ok, |ui| {
                        egui::ComboBox::from_id_salt(key).selected_text(if ok { label(cur) } else { "Not available".to_string() }).show_ui(ui, |ui| {
                            for (m, l) in choices {
                                if ui.selectable_label(m == cur, l).clicked() {
                                    match key {
                                        "saver" => idle.screensaver = m,
                                        "dim" => idle.dim = m,
                                        "off" => idle.screen_off = m,
                                        "lock" => idle.lock = m,
                                        "sleep" => idle.sleep = m,
                                        _ => idle.hibernate = m,
                                    }
                                    idle_changed = true;
                                }
                            }
                        });
                    });
                });
            }
            ui::row(ui, pal, false, "Brightness while dimmed", Some("The screen is dimmed by drawing over it, on every monitor. It comes back at the first key or mouse move. Nothing dims while a video plays"), |ui| {
                let mut v = idle.dim_level as f32;
                let r = ui::slider_range(ui, pal, &mut v, 10.0..=90.0, " %");
                // saved when the slider is let go (or a click on the track), not on every pixel of a drag
                if (r.drag_stopped() || (r.changed() && !r.dragged())) && (v as i64) != idle.dim_level {
                    idle.dim_level = v as i64;
                    idle_changed = true;
                }
            });
            ui::row(ui, pal, false, "Keyboard lights off while dimmed", Some("Also while the screen is off or shows the screensaver; they come back with you. Razer keyboards (openrazer) and laptop keyboard backlights"), |ui| {
                if ui::switch(ui, pal, &mut idle.lights_off).changed() {
                    idle_changed = true;
                }
            });
        });
        if idle_changed {
            self.change(ctx, move || sys::write_idle(&idle));
        }
        if let Some((mode, which, v)) = limit {
            self.change(ctx, move || {
                let arg = format!("{which}={}", v.map(|n| n.to_string()).unwrap_or_else(|| "default".into()));
                sys::act("pkexec", &["/usr/lib/yggdrasil/powerlimit", "set", &mode, &arg])
            });
        }
        match unlock {
            Some("confirm") => {
                self.unlock_confirm = Some(sys::sarina_backup());
                self.unlock_read = false;
            }
            Some(what) => self.change(ctx, move || sys::act("pkexec", &["/usr/lib/yggdrasil/gpuunlock", what])),
            None => {}
        }
        if let Some(backup) = self.unlock_confirm.clone() {
            let hold = p.limits.as_ref().and_then(|l| l.gpu.as_ref()).map(|g| g.default).unwrap_or(0);
            let mut arm = false;
            egui::Window::new("Arm the graphics card unlock?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label("At the next restart, before Linux starts, a boot program changes one setting inside the graphics card, once. Then the computer starts as usual. Linux never writes to the card.");
                ui.add_space(6.0);
                for line in [
                    "Experimental: it has been run on very few cards.".to_string(),
                    "The screen may stay black. After a minute, hold the power button and start again: the computer starts normally and the unlock switches itself off.".into(),
                    "A snapshot of the system (\"before GPU unlock\") is taken first.".into(),
                    format!("Every power mode is held at {hold} W until the measurements are done."),
                    "Stop Sarina and close games first: the first try is made on an idle card.".into(),
                    match &backup { Some(d) => format!("Last Proton Drive backup of Sarina: {d}."), None => "No Proton Drive backup of Sarina was found.".into() },
                ] {
                    ui.label(format!("•  {line}"));
                }
                ui.add_space(8.0);
                ui.checkbox(&mut self.unlock_read, "I have read this and I am at the computer");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.unlock_read, egui::Button::new("Arm")).clicked() {
                        arm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.unlock_confirm = None;
                    }
                });
            });
            if arm {
                self.unlock_confirm = None;
                self.change(ctx, || sys::act("pkexec", &["/usr/lib/yggdrasil/gpuunlock", "arm"]));
            }
        }
        if let Some(name) = profile {
            self.change(ctx, move || sys::set_profile(&name));
        }
    }

    /// The security monitor's page: its state, alerts, connections, and what it declares (quarantine, the malware scan, settings).
    fn page_monitor(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(o) = self.data.security.clone() else { return loading(ui) };
        if o.name.is_empty() {
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let day = |a: &&security::Alert| now - a.time < 86400;
        let count = |sev: &str| o.alerts.iter().filter(day).filter(|a| a.severity == sev).count();
        ui::group(ui, pal, Some(&o.name), |ui| {
            let (title, sub) = if o.running { ("Watching".to_string(), format!("Last 24 hours: {} critical, {} warnings, {} notes", count("critical"), count("warning"), count("info"))) } else { ("Not running".to_string(), format!("{} does not answer on this computer", o.name)) };
            ui::row(ui, pal, true, &title, Some(&sub), |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 6.0, if !o.running { pal.red } else if count("critical") > 0 { pal.red } else if count("warning") > 0 { pal.amber } else { pal.green });
            });
        });
        let mut release = false;
        if let Some(why) = &o.contained {
            ui.add_space(8.0);
            ui::group(ui, pal, Some("The assistant is contained"), |ui| {
                ui::row(ui, pal, true, "Stopped and locked out", Some(&format!("{why}. It stays stopped, also after a restart, until you release it")), |ui| {
                    if o.has_release && ui::quiet(ui, pal, "Restore and release…").clicked() {
                        release = true;
                    }
                });
            });
            ui.label(RichText::new("Releasing first puts back the approved versions of its files, then lets it start again. It asks for your password.").size(11.5).color(pal.dim));
        }
        if release {
            self.change(ctx, security::release);
        }
        let mut held_act: Option<(bool, String)> = None;
        if o.has_held && !o.held.is_empty() {
            ui.add_space(8.0);
            ui::group(ui, pal, Some(&format!("Held by {}", o.name)), |ui| {
                for h in &o.held {
                    let name = h.original.rsplit('/').next().unwrap_or(&h.original).to_string();
                    let before = if h.seen_before { "it was on this computer before" } else { "never seen before it was held" };
                    ui::row(ui, pal, false, &name, Some(&format!("{} · {} · {} · {}", h.reason, before, security::ago(h.held_at, now), h.original)), |ui| {
                        if ui::quiet(ui, pal, "It's malware").clicked() {
                            held_act = Some((true, h.id.clone()));
                        }
                        if ui::quiet(ui, pal, "Release").clicked() {
                            held_act = Some((false, h.id.clone()));
                        }
                    });
                }
            });
            ui.label(RichText::new("Release puts the file back exactly as it was. It's malware keeps it held and teaches it to the scans, so every copy is caught. Both ask for your password; Sarina's daily review says what she thinks of each file.").size(11.5).color(pal.dim));
        }
        if let Some((confirm, id)) = held_act {
            self.change(ctx, move || security::held_act(confirm, &id));
        }
        ui.add_space(8.0);
        ui::group(ui, pal, Some("Recent alerts"), |ui| {
            let levels = ["Everything", "Warnings and worse", "Critical only"];
            ui::row(ui, pal, true, "Show", None, |ui| {
                if let Some(i) = ui::segmented(ui, pal, &levels, self.sec_filter) {
                    self.sec_filter = i;
                }
            });
            let keep = |a: &&security::Alert| match self.sec_filter {
                0 => true,
                1 => a.severity != "info",
                _ => a.severity == "critical",
            };
            let shown: Vec<&security::Alert> = o.alerts.iter().filter(keep).take(25).collect();
            if shown.is_empty() {
                ui::row(ui, pal, false, "Nothing to show", Some("No alerts at this level"), |_| {});
            }
            for a in shown {
                let color = match a.severity.as_str() {
                    "critical" => pal.red,
                    "warning" => pal.amber,
                    _ => pal.dim,
                };
                let text: String = a.summary.chars().take(160).collect();
                let key = (a.time, a.detector.clone());
                let open = self.sec_open.as_ref() == Some(&key);
                let mut toggle = false;
                ui::row(ui, pal, false, &text, Some(&format!("{} · {}", a.detector, security::ago(a.time, now))), |ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, color);
                    // the monitor's own explanation, in plain words (no AI needed)
                    if a.explain.is_some() && ui.button(if open { "Hide" } else { "Explain" }).clicked() {
                        toggle = true;
                    }
                });
                if toggle {
                    self.sec_open = if open { None } else { Some(key) };
                }
                if let (true, Some([what, serious, done, todo])) = (open, a.explain.as_ref()) {
                    egui::Frame::new().inner_margin(egui::Margin { left: 14, right: 14, top: 2, bottom: 10 }).show(ui, |ui| {
                        for (label, body) in [("What happened", what), ("How serious", serious), ("What was done", done), ("What to do", todo)] {
                            ui.label(egui::RichText::new(label).strong().size(12.5).color(pal.text));
                            ui.add(egui::Label::new(egui::RichText::new(body).size(12.5).color(pal.dim)).wrap());
                            ui.add_space(4.0);
                        }
                    });
                }
            }
        });
        ui.add_space(8.0);
        let conns: Vec<&security::Alert> = o.alerts.iter().filter(|a| a.detector == "connection-watch" && !a.process.is_empty()).take(12).collect();
        ui::group(ui, pal, Some("Who connected where (new destinations)"), |ui| {
            if conns.is_empty() {
                ui::row(ui, pal, true, "Nothing new", Some("Each program is listed the first time it talks to an address"), |_| {});
            }
            for (i, a) in conns.iter().enumerate() {
                ui::row(ui, pal, i == 0, &a.process, Some(&format!("{} · {}", a.remote, security::ago(a.time, now))), |_| {});
            }
        });
        ui.add_space(8.0);
        if !o.controls.is_empty() {
            ui.add_space(8.0);
            self.controls_group(ui, pal, ctx, "Settings", &o.controls);
        }
        let mut scan: Option<String> = None;
        let mut restore_id: Option<String> = None;
        let mut delete_id: Option<String> = None;
        if o.has_quarantine {
        ui.add_space(8.0);
        ui::group(ui, pal, Some("Quarantine"), |ui| {
            if o.quarantine.is_empty() {
                ui::row(ui, pal, true, "Nothing in quarantine", None, |_| {});
            }
            for q in &o.quarantine {
                let name = q.original.rsplit('/').next().unwrap_or(&q.original).to_string();
                ui::row(ui, pal, false, &name, Some(&format!("{} · {} · from {}", q.signature, q.when, q.original)), |ui| {
                    if ui::quiet(ui, pal, "Delete").clicked() {
                        delete_id = Some(q.id.clone());
                    }
                    if ui::quiet(ui, pal, "Restore").clicked() {
                        restore_id = Some(q.id.clone());
                    }
                });
            }
        });
        }
        if o.has_scan {
        ui.add_space(8.0);
        ui::group(ui, pal, Some("Malware scan"), |ui| {
            let sub = match (o.clamav, o.signatures) {
                (false, _) => "ClamAV is not installed (sudo pacman -S clamav)".to_string(),
                (true, Some(t)) => format!("ClamAV, signatures from {}", security::ago(t, now)),
                (true, None) => "ClamAV is installed; signatures are not downloaded yet (sudo freshclam)".to_string(),
            };
            ui::row(ui, pal, true, "Scan the Downloads folder", Some(&sub), |ui| {
                if ui.add_enabled(o.clamav && !self.scan_busy, egui::Button::new(if self.scan_busy { "Scanning…" } else { "Scan now" })).clicked() {
                    scan = Some(format!("{}/Downloads", std::env::var("HOME").unwrap_or_default()));
                }
            });
            if let Some(r) = &self.scan_report {
                if !r.error.is_empty() {
                    ui::row(ui, pal, false, "The scan did not run", Some(&r.error), |_| {});
                } else {
                    let title = if r.infected.is_empty() { "No malware found".to_string() } else { format!("{} infected file(s)", r.infected.len()) };
                    ui::row(ui, pal, false, &title, Some(&r.summary), |_| {});
                    for f in &r.infected {
                        ui::row(ui, pal, false, f, Some("Nothing was moved or deleted: decide what to do with it"), |_| {});
                    }
                }
            }
        });
        }
        if let Some(id) = restore_id {
            self.change(ctx, move || security::quarantine_restore(&id));
        }
        if let Some(id) = delete_id {
            self.change(ctx, move || security::quarantine_delete(&id));
        }
        if let Some(folder) = scan {
            self.scan_busy = true;
            let (tx, ctx2) = (self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Scan(security::scan_folder(&folder)));
                ctx2.request_repaint();
            });
        }
    }

    fn page_security(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        if self.sec_sub != SecSub::Main {
            let monitor = yggdrasil_conf::integrations::security().map(security::monitor_name).unwrap_or_default();
            let title = match self.sec_sub {
                SecSub::Monitor => monitor.as_str(),
                SecSub::Logins => "Login history",
                SecSub::Distress => "Distress password",
                _ => "Sudo history",
            };
            ui.horizontal(|ui| {
                if ui::quiet(ui, pal, "\u{2039}  Security").clicked() {
                    self.sec_sub = SecSub::Main;
                    self.distress.close();
                }
                ui.label(RichText::new(title).size(18.0).strong());
            });
            ui.add_space(6.0);
        }
        match self.sec_sub {
            SecSub::Monitor => return self.page_monitor(ui, pal, ctx),
            SecSub::Logins => return self.page_log(ui, pal, ctx, false),
            SecSub::Sudo => return self.page_log(ui, pal, ctx, true),
            SecSub::Distress => return self.distress.page(ui, pal, ctx),
            SecSub::Main => {}
        }
        let Some(o) = self.data.security.clone() else { return loading(ui) };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let recent = |sev: &str| o.alerts.iter().filter(|a| now - a.time < 86400 && a.severity == sev).count();
        let mut open: Option<SecSub> = None;
        if !o.name.is_empty() {
        ui::group(ui, pal, None, |ui| {
            let sub = if o.running { format!("Watching · last 24 hours: {} critical, {} warnings", recent("critical"), recent("warning")) } else { "Not running".to_string() };
            if ui::nav_row(ui, pal, true, &o.name, Some(&sub), |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 6.0, if !o.running || recent("critical") > 0 { pal.red } else if recent("warning") > 0 { pal.amber } else { pal.green });
            }) {
                open = Some(SecSub::Monitor);
            }
        });
        ui.add_space(8.0);
        }
        ui::group(ui, pal, Some("Password log"), |ui| {
            if ui::nav_row(ui, pal, true, "Login history", Some("Who signed in, and every failed attempt at the lock screen, login or administrator question"), |_| {}) {
                    open = Some(SecSub::Logins);
            }
            if ui::nav_row(ui, pal, false, "Sudo history", Some("What sudo was used for, by whom, and failed attempts"), |_| {}) {
                    open = Some(SecSub::Sudo);
            }
        });
        ui.add_space(8.0);
        // the distress password: only where the security integration names its helper and the helper is installed
        if distress::helper().is_some() {
        ui::group(ui, pal, Some("Protection"), |ui| {
            if ui::nav_row(ui, pal, true, "Distress password", Some("A second password that quietly does something else when it is typed. Needs your password to open"), |_| {}) {
                open = Some(SecSub::Distress);
            }
        });
        }
        if let Some(s) = open {
            self.sec_sub = s;
            self.logins = None;
            self.sudo_log = None;
            self.log_busy = false;
        }
    }

    /// The password logs: login history (sign-ins and failures) or sudo history, last 30 days.
    fn page_log(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, sudo: bool) {
        if !self.log_busy && ((sudo && self.sudo_log.is_none()) || (!sudo && self.logins.is_none())) {
            self.log_busy = true;
            let (tx, ctx2) = (self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(if sudo { Msg::SudoLog(security::sudo_history()) } else { Msg::Logins(security::login_history()) });
                ctx2.request_repaint();
            });
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let show = |ui: &mut egui::Ui, title: &str, list: &[security::LogEntry], empty: &str| {
            ui::group(ui, pal, Some(title), |ui| {
                if list.is_empty() {
                    ui::row(ui, pal, true, empty, None, |_| {});
                }
                for (i, e) in list.iter().take(100).enumerate() {
                    let when = if e.time > 0 { format!(" · {}", security::ago(e.time, now)) } else { String::new() };
                    let t: String = e.title.chars().take(150).collect();
                    ui::row(ui, pal, i == 0, &t, Some(&format!("{}{}", e.detail, when)), |ui| {
                        if e.failed {
                            let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                            ui.painter().circle_filled(rect.center(), 5.0, pal.red);
                        }
                    });
                }
            });
        };
        if sudo {
            let Some(l) = self.sudo_log.clone() else { return loading(ui) };
            let failed: Vec<_> = l.iter().filter(|e| e.failed).cloned().collect();
            show(ui, "Failed attempts (last 30 days)", &failed, "No failed sudo attempts");
            ui.add_space(8.0);
            show(ui, "Everything sudo ran (last 30 days)", &l, "Nothing recorded");
        } else {
            let Some((ok, bad)) = self.logins.clone() else { return loading(ui) };
            show(ui, "Failed attempts (last 30 days)", &bad, "No failed attempts");
            ui.add_space(8.0);
            show(ui, "Sign-ins", &ok, "Nothing recorded");
        }
    }

    fn page_printers(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(st) = self.data.printers.clone() else { return loading(ui) };
        if !st.ready {
            ui::group(ui, pal, None, |ui| {
                ui::row(ui, pal, true, "Printing is not running", Some("The print system (CUPS) is not installed or not started. If it is installed: run sudo systemctl enable --now cups.socket"), |_| {});
            });
            return;
        }
        enum Do {
            Default(String),
            Test(String),
            Remove(String),
            Opt(String, String, String),
            Cancel(String),
            Add(String, String),
        }
        let mut todo: Option<Do> = None;
        if st.printers.is_empty() {
            ui::group(ui, pal, None, |ui| {
                ui::row(ui, pal, true, "No printers yet", Some("Printers on your network appear below. A USB printer appears here or below once it is plugged in and switched on"), |_| {});
            });
        }
        for p in &st.printers {
            ui.add_space(4.0);
            ui::group(ui, pal, Some(&p.name), |ui| {
                let state = match p.state.as_str() {
                    "printing" => "Printing",
                    "stopped" => "Stopped",
                    _ => "Ready",
                };
                let sub = if p.message.is_empty() { format!("{state}{}", if p.default { " · default printer" } else { "" }) } else { format!("{state} · {}", p.message) };
                ui::row(ui, pal, true, state, Some(&sub), |ui| {
                    if ui::quiet(ui, pal, "Remove…").clicked() {
                        todo = Some(Do::Remove(p.name.clone()));
                    }
                    if ui::quiet(ui, pal, "Print test page").clicked() {
                        todo = Some(Do::Test(p.name.clone()));
                    }
                    if !p.default && ui::quiet(ui, pal, "Make default").clicked() {
                        todo = Some(Do::Default(p.name.clone()));
                    }
                });
                for m in &p.markers {
                    let level = m.level.map(|l| format!("{l}%")).unwrap_or_else(|| "level not reported".into());
                    ui::row(ui, pal, false, &m.name, Some(&level), |ui| {
                        if let Some(l) = m.level {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(140.0, 10.0), Sense::hover());
                            ui.painter().rect_filled(rect, CornerRadius::same(5), pal.hover);
                            let mut fill = rect;
                            fill.set_width(rect.width() * l as f32 / 100.0);
                            let c = u32::from_str_radix(m.color.trim_start_matches('#'), 16).ok().filter(|_| m.color.len() == 7).map(|v| egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)).unwrap_or(pal.accent);
                            let c = if c == egui::Color32::BLACK { pal.text } else { c };
                            ui.painter().rect_filled(fill, CornerRadius::same(5), if l <= 10 { pal.red } else { c });
                        }
                    });
                }
                // the everyday settings first; the rest are in CUPS's own pages (localhost:631)
                for o in p.options.iter().filter(|o| matches!(o.key.as_str(), "PageSize" | "Duplex" | "sides" | "print-quality" | "ColorModel" | "print-color-mode" | "MediaType" | "InputSlot" | "cupsPrintQuality" | "Resolution")) {
                    ui::row(ui, pal, false, &o.label, None, |ui| {
                        egui::ComboBox::from_id_salt(("printer-opt", &p.name, &o.key)).selected_text(o.current.clone()).width(180.0).show_ui(ui, |ui| {
                            for v in &o.values {
                                if ui.selectable_label(*v == o.current, v).clicked() && *v != o.current {
                                    todo = Some(Do::Opt(p.name.clone(), o.key.clone(), v.clone()));
                                }
                            }
                        });
                    });
                }
            });
        }
        if !st.jobs.is_empty() {
            ui.add_space(12.0);
            ui::group(ui, pal, Some("Waiting to print"), |ui| {
                for (i, j) in st.jobs.iter().enumerate() {
                    ui::row(ui, pal, i == 0, &j.id, Some(&format!("{} · {} bytes, from {}", j.printer, j.size, j.owner)), |ui| {
                        if ui::quiet(ui, pal, "Cancel").clicked() {
                            todo = Some(Do::Cancel(j.id.clone()));
                        }
                    });
                }
            });
        }
        ui.add_space(12.0);
        ui::group(ui, pal, Some("Found on your network"), |ui| {
            if st.found.is_empty() {
                ui::row(ui, pal, true, "Nothing new found", Some("Printers that are on and on the same network show up here within a few seconds. Most need no driver"), |_| {});
            }
            for (i, (name, uri)) in st.found.iter().enumerate() {
                ui::row(ui, pal, i == 0, name, Some(uri), |ui| {
                    if ui::action(ui, pal, "Add").clicked() {
                        todo = Some(Do::Add(name.clone(), uri.clone()));
                    }
                });
            }
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Everything else a printer offers (its driver, sharing, cleaning) is on CUPS's own page: http://localhost:631").size(12.0).color(pal.dim));
        if let Some(d) = todo {
            match d {
                Do::Default(n) => self.change(ctx, move || printers::set_default(&n)),
                Do::Test(n) => self.change(ctx, move || printers::test_page(&n)),
                Do::Remove(n) => self.change(ctx, move || printers::remove(&n)),
                Do::Opt(n, k, v) => self.change(ctx, move || printers::set_option(&n, &k, &v)),
                Do::Cancel(j) => self.change(ctx, move || printers::cancel(&j)),
                Do::Add(n, u) => self.change(ctx, move || printers::add(&n, &u)),
            }
        }
    }

    /// The assistant the operating system provides: Yggdrasil's own part (answers read aloud, the wave), then the controls the
    /// assistant declares in its integration file.
    fn page_assistant(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let name = yggdrasil_conf::integrations::first("assistant").and_then(|i| i.get("name").map(str::to_string)).unwrap_or_else(|| "The assistant".into());
        let mut conf = yggdrasil_conf::PanelConf::load();
        let mut conf_changed = false;
        ui::group(ui, pal, Some("Speaking"), |ui| {
            ui::row(ui, pal, true, "Say answers aloud", Some(&format!("After you ask {name} by voice (Super+S), the answer is said aloud. Starting to speak again, or Esc, stops it")), |ui| {
                if ui::switch(ui, pal, &mut conf.assistant_speaks).changed() {
                    conf_changed = true;
                }
            });
            let waves = ["Copper", "Siri", "Accent"];
            ui::row(ui, pal, false, "Wave colors", Some("The glowing wave that grows out of the notch while you talk and while the answer is spoken"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &waves, conf.voice_wave.min(2) as usize) {
                    conf.voice_wave = i as u8;
                    conf_changed = true;
                }
            });
        });
        if conf_changed {
            self.change(ctx, move || conf.save());
        }
        let Some(states) = self.data.assistant.clone() else { return loading(ui) };
        if states.is_empty() {
            return;
        }
        ui.add_space(8.0);
        self.controls_group(ui, pal, ctx, &name, &states);
    }

    /// The switches, choices and buttons an integration declares, as one group.
    fn controls_group(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, title: &str, states: &[controls::State]) {
        let mut todo: Option<Box<dyn FnOnce() -> Result<(), String> + Send>> = None;
        ui::group(ui, pal, Some(title), |ui| {
            for (i, st) in states.iter().enumerate() {
                let c = st.control.clone();
                let (label, description) = (c.label.clone(), c.description.clone());
                let desc = if description.is_empty() { None } else { Some(description.as_str()) };
                ui::row(ui, pal, i == 0, &label, desc, |ui| match &st.value {
                    controls::Value::Switch(on) => {
                        let mut v = *on;
                        if ui::switch(ui, pal, &mut v).changed() {
                            todo = Some(Box::new(move || controls::set_switch(&c, v)));
                        }
                    }
                    controls::Value::Choice { options, current } => {
                        egui::ComboBox::from_id_salt(("choice", c.id.clone())).selected_text(current.as_str()).width(220.0).show_ui(ui, |ui| {
                            for o in options {
                                if ui.selectable_label(o == current, o).clicked() {
                                    let (c, o) = (c.clone(), o.clone());
                                    todo = Some(Box::new(move || controls::set_choice(&c, &o)));
                                }
                            }
                        });
                    }
                    controls::Value::Action => {
                        if ui::quiet(ui, pal, "Run").clicked() {
                            let c = c.clone();
                            todo = Some(Box::new(move || controls::act(&c).map(|_| ())));
                        }
                    }
                });
            }
        });
        if let Some(f) = todo {
            self.change(ctx, f);
        }
    }

    fn page_default_apps(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(rows) = self.data.defaults.clone() else { return loading(ui) };
        let mut pick: Option<(usize, String)> = None;
        ui::group(ui, pal, Some("Open files with"), |ui| {
            for (n, r) in rows.iter().enumerate() {
                let k = &defaults::KINDS[r.kind];
                let shown = r.choices.iter().find(|c| c.id == r.current).map(|c| c.name.clone()).unwrap_or_else(|| if r.current.is_empty() { "Not set".into() } else { r.current.trim_end_matches(".desktop").to_string() });
                ui::row(ui, pal, n == 0, k.name, Some(k.what), |ui| {
                    egui::ComboBox::from_id_salt(("default-app", r.kind)).selected_text(shown).width(200.0).show_ui(ui, |ui| {
                        if r.choices.is_empty() {
                            ui.label("No installed program says it can open these");
                        }
                        for c in &r.choices {
                            if ui.selectable_label(c.id == r.current, &c.name).clicked() {
                                pick = Some((r.kind, c.id.clone()));
                            }
                        }
                    });
                });
            }
        });
        if let Some((kind, id)) = pick {
            self.change(ctx, move || defaults::set(kind, &id));
        }
    }

    fn page_restore(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(state) = self.data.restore.clone() else { return loading(ui) };
        match state {
            restore::State::Unavailable(why) => {
                ui::group(ui, pal, None, |ui| {
                    ui::row(ui, pal, true, "Restore points are not available", Some(&why), |_| {});
                });
            }
            restore::State::NoPermission => {
                let mut allow = false;
                ui::group(ui, pal, None, |ui| {
                    ui::row(ui, pal, true, "Restore points exist, but you are not allowed to see them yet", Some("One administrator question lets you list, create and delete them here"), |ui| {
                        allow = ui::quiet(ui, pal, "Allow me…").clicked();
                    });
                });
                if allow {
                    self.change(ctx, || restore::allow_me());
                }
            }
            restore::State::Ready(list) => {
                let mut make = false;
                let mut remove: Option<u64> = None;
                ui::group(ui, pal, Some("Restore points"), |ui| {
                    ui::row(ui, pal, true, "Make one now", Some("A copy of the system as it is: go back to it if an update or a change goes wrong. Your files in the home folder are not part of it"), |ui| {
                        make = ui::quiet(ui, pal, "Make restore point").clicked();
                    });
                    for s in list.iter().take(40) {
                        let sub = format!("{}{}", s.date, if s.important { " · kept until you delete it" } else { "" });
                        ui::row(ui, pal, false, &restore::title(s), Some(&sub), |ui| {
                            if s.cleanup.is_empty() && ui.button("Delete").clicked() {
                                remove = Some(s.number);
                            }
                        });
                    }
                });
                ui.add_space(8.0);
                ui::group(ui, pal, Some("Going back to one"), |ui| {
                    ui::row(ui, pal, true, "From the boot menu", Some("Restart, choose \"Arch Linux snapshots\" in the boot menu, pick the restore point and start it. If it is what you want, make it permanent with: sudo snapper rollback"), |_| {});
                });
                if make {
                    self.change(ctx, || restore::create("Restore point made in Settings"));
                }
                if let Some(n) = remove {
                    self.change(ctx, move || restore::delete(n));
                }
            }
        }
    }

    /// Runs one change through the account helper (with the administrator question), then reloads the page.
    fn acct_action(&mut self, ctx: &egui::Context, args: Vec<String>, stdin: Option<String>, ok: String) {
        self.acct_busy = true;
        self.acct_msg = None;
        let (tx, ctx2) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            let r = users::manage(&a, stdin.as_deref()).map(|_| ok);
            let _ = tx.send(Msg::AccountDone(r));
            ctx2.request_repaint();
        });
    }

    /// Every account on this computer, to choose one; then adding one.
    fn accounts_section(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, me: &str, all: &[users::Account], selected: &str) {
        let mut pick: Option<String> = None;
        let mut open_form = false;
        ui::group(ui, pal, Some("Accounts"), |ui| {
            // yours first, then the others in the order the system has them
            let mut order: Vec<&users::Account> = all.iter().filter(|a| a.login == me).collect();
            order.extend(all.iter().filter(|a| a.login != me));
            for (i, a) in order.iter().enumerate() {
                if account_row(ui, pal, i == 0, a, a.login == me, a.login == selected) {
                    pick = Some(a.login.clone());
                }
            }
            ui::row(ui, pal, order.is_empty(), "Add an account", Some("Standard users cannot change the system; administrators can"), |ui| {
                if ui.button(if self.acct_form.open { "Cancel" } else { "Add account…" }).clicked() {
                    open_form = true;
                }
            });
            if self.acct_form.open {
                ui.add_space(6.0);
                ui.label(RichText::new("Full name").color(pal.dim));
                let r = ui.add(egui::TextEdit::singleline(&mut self.acct_form.full).desired_width(f32::INFINITY));
                if r.changed() && !self.acct_form.login_touched {
                    self.acct_form.login = users::suggest_login(&self.acct_form.full);
                }
                ui.add_space(4.0);
                ui.label(RichText::new("User name (lowercase letters, digits, - and _)").color(pal.dim));
                if ui.add(egui::TextEdit::singleline(&mut self.acct_form.login).desired_width(f32::INFINITY)).changed() {
                    self.acct_form.login_touched = true;
                }
                let bad_login = !self.acct_form.login.is_empty() && !users::valid_login(&self.acct_form.login);
                let taken = all.iter().any(|a| a.login == self.acct_form.login);
                if bad_login {
                    ui.label(RichText::new("Use lowercase letters, digits, - and _, starting with a letter.").small().color(pal.red));
                } else if taken {
                    ui.label(RichText::new("That user name is already used.").small().color(pal.red));
                }
                ui.add_space(4.0);
                ui::row(ui, pal, false, "Administrator", Some("Can install software and change the system"), |ui| {
                    ui::switch(ui, pal, &mut self.acct_form.admin);
                });
                ui.label(RichText::new("Password").color(pal.dim));
                ui.add(egui::TextEdit::singleline(&mut self.acct_form.pw).password(true).desired_width(f32::INFINITY));
                let (word, frac) = users::strength(&self.acct_form.pw);
                if !word.is_empty() {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().min(320.0), 6.0), Sense::hover());
                    ui.painter().rect_filled(rect, 3.0, pal.field);
                    let mut fill = rect;
                    fill.set_width(rect.width() * frac);
                    ui.painter().rect_filled(fill, 3.0, theme::usage_color(pal, 1.0 - frac));
                    ui.label(RichText::new(word).small().color(pal.dim));
                }
                ui.add_space(4.0);
                ui.label(RichText::new("Password again").color(pal.dim));
                ui.add(egui::TextEdit::singleline(&mut self.acct_form.pw2).password(true).desired_width(f32::INFINITY));
                if !self.acct_form.pw2.is_empty() && self.acct_form.pw2 != self.acct_form.pw {
                    ui.label(RichText::new("The two passwords do not match.").small().color(pal.red));
                }
                ui.label(RichText::new("They can change it after they log in. It also becomes the password of their keyring.").small().color(pal.dim));
                ui.add_space(8.0);
                let f = &self.acct_form;
                let ready = !self.acct_busy && users::valid_login(&f.login) && !taken && f.pw.chars().count() >= 8 && f.pw == f.pw2;
                if ui.add_enabled(ready, egui::Button::new(if self.acct_busy { "Creating…" } else { "Create account" })).clicked() {
                    let (login, full, admin, pw) = (f.login.clone(), f.full.clone(), f.admin, f.pw.clone());
                    self.acct_busy = true;
                    self.acct_msg = None;
                    self.acct_form.pw.clear();
                    self.acct_form.pw2.clear();
                    let (tx, ctx2) = (self.tx.clone(), ctx.clone());
                    std::thread::spawn(move || {
                        let mut args = vec!["add", login.as_str()];
                        if !full.trim().is_empty() {
                            args.push("--full-name");
                            args.push(full.as_str());
                        }
                        if admin {
                            args.push("--admin");
                        }
                        let r = users::manage(&args, Some(&pw)).map(|_| format!("Account {login} created."));
                        let _ = tx.send(Msg::AccountDone(r));
                        ctx2.request_repaint();
                    });
                }
                ui.add_space(6.0);
            }
        });
        if open_form {
            self.acct_form.open = !self.acct_form.open;
            self.acct_msg = None;
        }
        if let Some(login) = pick {
            self.acct_sel = Some(login);
            self.name_edit = None;
            self.other_pw = (false, String::new(), String::new());
            self.acct_msg = None;
        }
        let _ = ctx;
    }

    fn page_users(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some((me, keyring)) = self.data.account.clone() else { return loading(ui) };
        let all = self.data.accounts.clone().unwrap_or_default();
        let sel_login = self.acct_sel.clone().filter(|l| all.iter().any(|a| &a.login == l)).unwrap_or_else(|| me.login.clone());
        self.accounts_section(ui, pal, ctx, &me.login, &all, &sel_login);
        let sel = all.iter().find(|a| a.login == sel_login).cloned().unwrap_or_else(|| me.clone());
        let mine = sel.login == me.login;
        let title = if sel.full_name.is_empty() { sel.login.clone() } else { sel.full_name.clone() };
        let busy = self.acct_busy;
        let mut actions: Vec<(Vec<String>, Option<String>, String)> = vec![];

        // the account itself
        ui.add_space(8.0);
        let mut edit_name: Option<Option<(String, String)>> = None;
        ui::group(ui, pal, Some(&title), |ui| {
            if let Some((m, ok)) = &self.acct_msg {
                ui.label(RichText::new(m.as_str()).color(if *ok { pal.green } else { pal.red }));
                ui.add_space(4.0);
            }
            if sel.protected {
                ui::row(ui, pal, true, "Kept by the operating system", Some("Settings shows this account but does not change it"), |_| {});
            }
            let editing = self.name_edit.as_ref().filter(|(l, _)| *l == sel.login).map(|(_, t)| t.clone());
            ui::row(ui, pal, !sel.protected, "Full name", Some(if sel.full_name.is_empty() { "None" } else { &sel.full_name }), |ui| {
                match editing.clone() {
                    Some(mut text) => {
                        if ui.add_enabled(!busy && !text.trim().is_empty(), egui::Button::new("Save")).clicked() {
                            actions.push((vec!["name".into(), sel.login.clone(), text.trim().to_string()], None, format!("{} renamed.", sel.login)));
                        }
                        if ui.button("Cancel").clicked() {
                            edit_name = Some(None);
                        }
                        if ui.add(egui::TextEdit::singleline(&mut text).desired_width(200.0)).changed() {
                            edit_name = Some(Some((sel.login.clone(), text)));
                        }
                    }
                    None => {
                        if !sel.protected && ui.button("Change…").clicked() {
                            edit_name = Some(Some((sel.login.clone(), sel.full_name.clone())));
                        }
                    }
                }
            });
            ui::row(ui, pal, false, "Account name", Some(&sel.login), |_| {});
            ui::row(ui, pal, false, "Home folder", Some(&sel.home), |_| {});
        });
        if let Some(e) = edit_name {
            self.name_edit = e;
        }

        if mine {
        let mut start = false;
        ui::group(ui, pal, Some("Password"), |ui| {
            ui::row(ui, pal, true, "Login password", Some("Also the password of your keyring (saved passwords and keys for your apps): they change together"), |ui| {
                if ui.button(if self.pw_open { "Cancel" } else { "Change…" }).clicked() {
                    self.pw_open = !self.pw_open;
                    self.pw_msg = None;
                }
            });
            if self.pw_open {
                ui.add_space(6.0);
                ui.label(RichText::new("Current password").color(pal.dim));
                ui.add(egui::TextEdit::singleline(&mut self.pw_cur).password(true).desired_width(f32::INFINITY));
                ui.add_space(4.0);
                ui.label(RichText::new("New password").color(pal.dim));
                ui.add(egui::TextEdit::singleline(&mut self.pw_new).password(true).desired_width(f32::INFINITY));
                let (word, frac) = users::strength(&self.pw_new);
                if !word.is_empty() {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().min(320.0), 6.0), Sense::hover());
                    ui.painter().rect_filled(rect, 3.0, pal.field);
                    let mut fill = rect;
                    fill.set_width(rect.width() * frac);
                    ui.painter().rect_filled(fill, 3.0, theme::usage_color(pal, 1.0 - frac));
                    ui.label(RichText::new(word).small().color(pal.dim));
                }
                ui.add_space(4.0);
                ui.label(RichText::new("New password again").color(pal.dim));
                ui.add(egui::TextEdit::singleline(&mut self.pw_confirm).password(true).desired_width(f32::INFINITY));
                ui.add_space(8.0);
                let mismatch = !self.pw_confirm.is_empty() && self.pw_confirm != self.pw_new;
                if mismatch {
                    ui.label(RichText::new("The two new passwords do not match.").color(pal.red));
                }
                let ready = !self.pw_busy && !self.pw_cur.is_empty() && self.pw_new.chars().count() >= 8 && self.pw_new == self.pw_confirm && self.pw_new != self.pw_cur;
                if ui.add_enabled(ready, egui::Button::new(if self.pw_busy { "Changing…" } else { "Change password" })).clicked() {
                    start = true;
                }
                if self.pw_new == self.pw_cur && !self.pw_new.is_empty() {
                    ui.label(RichText::new("The new password must be different from the current one.").small().color(pal.dim));
                }
                ui.add_space(6.0);
            }
            if let Some((m, ok)) = &self.pw_msg {
                ui.label(RichText::new(m.as_str()).color(if *ok { pal.green } else { pal.red }));
                ui.add_space(6.0);
            }
        });
        if start {
            self.pw_busy = true;
            self.pw_msg = None;
            let (cur, new) = (std::mem::take(&mut self.pw_cur), std::mem::take(&mut self.pw_new));
            self.pw_confirm.clear();
            let (tx, ctx2) = (self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let r = users::change_password(&cur, &new);
                let _ = tx.send(Msg::Password(r));
                ctx2.request_repaint();
            });
        }
        ui.add_space(8.0);
        let (state, hint, fixable) = match keyring.as_str() {
            "unlocked" => ("Unlocked", "Saved passwords and keys are available to your apps", false),
            "locked" => ("Locked", "Still locked with a previous password: it needs one question answered", true),
            "noservice" => ("Not running", "The keyring service is not running", false),
            "nokeyring" => ("None yet", "It is created the next time you log in", false),
            _ => ("Unknown", "The keyring check is not installed", false),
        };
        ui::group(ui, pal, Some("Keyring"), |ui| {
            ui::row(ui, pal, true, state, Some(hint), |ui| {
                if fixable && ui.button("Fix…").clicked() {
                    sys::spawn("yggdrasil-keyring-sync", &[]);
                }
            });
        });
        } else if !sel.protected {
            // someone else's password: set a new one (an administrator may; their keyring keeps the old one)
            let mut set_pw: Option<String> = None;
            ui.add_space(8.0);
            ui::group(ui, pal, Some("Password"), |ui| {
                ui::row(ui, pal, true, "Set a new password", Some("For when they forgot theirs. Their keyring keeps the old password: it asks for it once at their next sign-in"), |ui| {
                    if ui.button(if self.other_pw.0 { "Cancel" } else { "Set…" }).clicked() {
                        self.other_pw = (!self.other_pw.0, String::new(), String::new());
                    }
                });
                if self.other_pw.0 {
                    ui.add_space(6.0);
                    ui.label(RichText::new("New password").color(pal.dim));
                    ui.add(egui::TextEdit::singleline(&mut self.other_pw.1).password(true).desired_width(f32::INFINITY));
                    ui.add_space(4.0);
                    ui.label(RichText::new("New password again").color(pal.dim));
                    ui.add(egui::TextEdit::singleline(&mut self.other_pw.2).password(true).desired_width(f32::INFINITY));
                    if !self.other_pw.2.is_empty() && self.other_pw.1 != self.other_pw.2 {
                        ui.label(RichText::new("The two passwords do not match.").small().color(pal.red));
                    }
                    ui.add_space(6.0);
                    let ready = !busy && self.other_pw.1.chars().count() >= 8 && self.other_pw.1 == self.other_pw.2;
                    if ui.add_enabled(ready, egui::Button::new("Set password")).clicked() {
                        set_pw = Some(self.other_pw.1.clone());
                    }
                    ui.add_space(6.0);
                }
            });
            if let Some(pw) = set_pw {
                self.other_pw = (false, String::new(), String::new());
                actions.push((vec!["password".into(), sel.login.clone()], Some(pw), format!("New password set for {}.", sel.login)));
            }
        }

        // what the account may do
        if !sel.protected {
            ui.add_space(8.0);
            let groups = self.data.opt_groups.clone();
            ui::group(ui, pal, Some("Access"), |ui| {
                let mut admin = sel.admin;
                let sub = if mine && sel.admin { "You cannot remove your own administrator rights (another administrator can)" } else { "Can install software and change the system (with sudo)" };
                ui::row(ui, pal, true, "Administrator", Some(sub), |ui| {
                    ui.add_enabled_ui(!busy && !(mine && sel.admin), |ui| {
                        if ui::switch(ui, pal, &mut admin).changed() {
                            actions.push((vec!["admin".into(), sel.login.clone(), if admin { "on" } else { "off" }.into()], None, format!("{} is now {}.", sel.login, if admin { "an administrator" } else { "a standard user" })));
                        }
                    });
                });
                if sel.admin {
                    let mut asks = !sel.sudo_nopassword;
                    let sub = if asks {
                        "Recommended: nothing run as this account can change the system without the password"
                    } else {
                        "Off: anything run as this account can change the system without asking. Only for a computer nobody else uses"
                    };
                    ui::row(ui, pal, false, "Sudo asks for the password", Some(sub), |ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            if ui::switch(ui, pal, &mut asks).changed() {
                                actions.push((vec!["sudo".into(), sel.login.clone(), if asks { "password" } else { "nopassword" }.into()], None, format!("sudo {} {} for the password.", if asks { "asks" } else { "no longer asks" }, sel.login)));
                            }
                        });
                    });
                }
                if !mine {
                    let mut can = sel.can_login;
                    ui::row(ui, pal, false, "Can sign in", Some(if can { "The account can sign in" } else { "Signing in is switched off; the account and its files stay" }), |ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            if ui::switch(ui, pal, &mut can).changed() {
                                actions.push((vec!["login".into(), sel.login.clone(), if can { "on" } else { "off" }.into()], None, format!("{} {}.", sel.login, if can { "can sign in again" } else { "can no longer sign in" })));
                            }
                        });
                    });
                }
                for (g, what) in &groups {
                    let mut on = sel.groups.contains(g);
                    ui::row(ui, pal, false, what, Some(&format!("Group {g}; takes effect at the next sign-in")), |ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            if ui::switch(ui, pal, &mut on).changed() {
                                actions.push((vec!["group".into(), sel.login.clone(), g.clone(), if on { "on" } else { "off" }.into()], None, format!("{} {} {g}.", sel.login, if on { "joined" } else { "left" })));
                            }
                        });
                    });
                }
            });
        }

        if !mine && !sel.protected {
            ui.add_space(8.0);
            ui::group(ui, pal, None, |ui| {
                ui::row(ui, pal, true, "Remove this account", Some("They will no longer be able to sign in; you choose whether their files go too"), |ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Remove…")).clicked() {
                        self.acct_remove = Some((sel.login.clone(), false));
                    }
                });
            });
        }
        for (args, stdin, ok) in actions {
            self.acct_action(ctx, args, stdin, ok);
        }
        if let Some((login, delete_home)) = self.acct_remove.clone() {
            let mut keep = true;
            let mut go = false;
            let mut home = delete_home;
            egui::Window::new(format!("Remove {login}?")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
                ui.set_max_width(380.0);
                ui.label("They will no longer be able to log in. This cannot be undone.");
                ui.add_space(6.0);
                ui.checkbox(&mut home, "Also delete their files (home folder)");
                ui.label(RichText::new(if home { "Their documents, pictures and settings will be erased." } else { "Their files stay on the disk, in /home, until you delete them." }).small().color(pal.dim));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                    if ui.button(if home { "Remove and delete files" } else { "Remove account" }).clicked() {
                        go = true;
                    }
                });
            });
            self.acct_remove = if keep && !go { Some((login.clone(), home)) } else { None };
            if go {
                self.acct_busy = true;
                let (tx, ctx2) = (self.tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let mut args = vec!["remove", login.as_str()];
                    if home {
                        args.push("--delete-home");
                    }
                    let r = users::manage(&args, None).map(|_| format!("{login} was removed."));
                    let _ = tx.send(Msg::AccountDone(r));
                    ctx2.request_repaint();
                });
            }
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
    }

    fn page_time(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(t) = self.data.time.clone() else { return loading(ui) };
        let mut ntp = t.ntp;
        let mut ntp_changed = false;
        let mut clock = yggdrasil_conf::PanelConf::load();
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
        let mut minos = yggdrasil_conf::PanelConf::load();
        let mut minos_changed = false;
        ui::group(ui, pal, Some("Minos: where it opens"), |ui| {
            use yggdrasil_conf::StartIn;
            ui::row(ui, pal, true, "First new window", Some("The first Minos window you open: your home folder, or the tabs you had when you last closed it"), |ui| {
                let cur = (minos.minos_first_window == StartIn::Last) as usize;
                if let Some(i) = ui::segmented(ui, pal, &["Home", "Where you left off"], cur) {
                    minos.minos_first_window = [StartIn::Home, StartIn::Last][i];
                    minos_changed = true;
                }
            });
            ui::row(ui, pal, false, "New window while one is open", Some("Home, or a copy of the folder the open window is in"), |ui| {
                let cur = (minos.minos_more_windows == StartIn::Here) as usize;
                if let Some(i) = ui::segmented(ui, pal, &["Home", "Copy of where I am"], cur) {
                    minos.minos_more_windows = [StartIn::Home, StartIn::Here][i];
                    minos_changed = true;
                }
            });
            ui::row(ui, pal, false, "New tab", Some("Home, or a copy of the folder the current tab is in"), |ui| {
                let cur = (minos.minos_new_tab == StartIn::Here) as usize;
                if let Some(i) = ui::segmented(ui, pal, &["Home", "Copy of where I am"], cur) {
                    minos.minos_new_tab = [StartIn::Home, StartIn::Here][i];
                    minos_changed = true;
                }
            });
        });
        ui::group(ui, pal, Some("Minos"), |ui| {
            ui::row(ui, pal, true, "Search inside subfolders", Some("Searching in Minos looks through every folder below the one you are in; off searches only that folder"), |ui| {
                minos_changed |= ui::switch(ui, pal, &mut minos.search_subfolders).changed();
            });
            ui::row(ui, pal, false, "Show hidden items", Some("Files and folders whose names start with a dot"), |ui| {
                minos_changed |= ui::switch(ui, pal, &mut minos.show_hidden).changed();
            });
            ui::row(ui, pal, false, "Show Proton Drive file status", Some("A mark on each Proton Drive file: kept on this computer, or only in the cloud"), |ui| {
                minos_changed |= ui::switch(ui, pal, &mut minos.drive_status).changed();
            });
        });
        if minos_changed {
            // saved right away (a tiny file), so the slider and the sample follow the drag
            let _ = minos.save();
            ctx.request_repaint();
        }
    }

    fn page_apps_software(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(apps) = self.data.apps.clone() else { return loading(ui) };
        if apps.is_empty() {
            return;
        }
        let Some(cmd) = yggdrasil_conf::integrations::first("packages").and_then(|i| i.command("component")) else { return };
        let os = yggdrasil_conf::integrations::first("packages").and_then(|i| i.get("name").map(str::to_string)).unwrap_or_else(|| "The system".into());
        let run = |op: &str, id: &str| {
            let mut args: Vec<String> = vec!["-e".into()];
            args.extend(cmd.iter().cloned());
            args.extend([op.to_string(), id.to_string()]);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            sys::spawn("ghostty", &args);
        };
        ui::group(ui, pal, Some(&format!("{os}'s optional software")), |ui| {
            for (i, c) in apps.iter().enumerate() {
                let sub: String = c.description.chars().take(110).collect::<String>() + if c.description.chars().count() > 110 { "…" } else { "" };
                ui::row(ui, pal, i == 0, &c.name, Some(&sub), |ui| {
                    // Installing shows its progress (and asks for the password) in a terminal.
                    if c.installed {
                        if ui::quiet(ui, pal, "Remove").clicked() {
                            run("remove", &c.id);
                        }
                    } else if ui::action(ui, pal, "Install").clicked() {
                        run("install", &c.id);
                    }
                });
            }
        });
    }
}

/// One account in the Users list: a round initial, the name, and what kind of account it is. True when clicked.
fn account_row(ui: &mut egui::Ui, pal: &Palette, first: bool, a: &users::Account, me: bool, selected: bool) -> bool {
    if !first {
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.left()..=r.right(), r.top(), egui::Stroke::new(1.0, pal.border));
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    if selected {
        p.rect_filled(rect.shrink2(vec2(0.0, 3.0)), 8.0, pal.select);
    } else if resp.hovered() {
        p.rect_filled(rect.shrink2(vec2(0.0, 3.0)), 8.0, pal.hover);
    }
    let name = if a.full_name.is_empty() { a.login.clone() } else { a.full_name.clone() };
    let c = egui::pos2(rect.left() + 26.0, rect.center().y);
    p.circle_filled(c, 17.0, if a.protected { pal.dim } else { pal.accent });
    let initial = name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default();
    p.text(c, egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(15.0), egui::Color32::WHITE);
    let x = rect.left() + 54.0;
    p.text(egui::pos2(x, rect.center().y - 9.0), egui::Align2::LEFT_CENTER, if me { format!("{name}  (you)") } else { name }, egui::FontId::proportional(14.0), pal.text);
    let mut kind = vec![a.login.clone(), if a.admin { "Administrator".into() } else { "Standard user".into() }];
    if a.protected {
        kind.push("kept by the system".into());
    } else if !a.can_login {
        kind.push("cannot sign in".into());
    }
    p.text(egui::pos2(x, rect.center().y + 10.0), egui::Align2::LEFT_CENTER, kind.join(" · "), egui::FontId::proportional(12.0), pal.dim);
    resp.clicked()
}

/// The shortcuts of the running Hyprland session (Yggdrasil's own and the user's), grouped (yggdrasil_conf::shortcuts), read every
/// few seconds.
fn hypr_shortcuts() -> Vec<yggdrasil_conf::shortcuts::Shortcut> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use yggdrasil_conf::shortcuts::{self, Bind, Shortcut};
    static CACHE: Mutex<Option<(Instant, Vec<Shortcut>)>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, v)) = g.as_ref() {
        if t.elapsed() < Duration::from_secs(5) {
            return v.clone();
        }
    }
    let json: serde_json::Value = sys::run("hyprctl", &["-j", "binds"]).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let binds: Vec<Bind> = json
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .map(|b| Bind {
            modmask: b["modmask"].as_u64().unwrap_or(0) as u32,
            // a key given by its code (the number row: code:10 is 1) has no name, only the code
            key: b["key"].as_str().filter(|k| !k.is_empty()).map(str::to_string).unwrap_or_else(|| format!("code:{}", b["keycode"].as_u64().unwrap_or(0))),
            desc: b["description"].as_str().unwrap_or("").to_string(),
            release: b["release"].as_bool().unwrap_or(false),
        })
        .collect();
    let out = shortcuts::list(&binds);
    *g = Some((Instant::now(), out.clone()));
    out
}

/// Opens the user's own Hyprland additions (made with a short example the first time).
fn open_own_shortcuts() {
    let dir = sys::config_dir().join("yggdrasil");
    let file = dir.join("hyprland.lua");
    if !file.exists() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&file, "-- Your own additions to Yggdrasil's session (read last, so they win). After a change: hyprctl reload\n--\n-- A shortcut that starts a program:\n--   hl.bind(\"SUPER + B\", hl.dsp.exec_cmd(\"vivaldi\"), { desc = \"Browser\" })\n");
    }
    sys::spawn("xdg-open", &[file.to_string_lossy().as_ref()]);
}

/// Color Picker (packages/colorpicker, Win + Shift + C): the format a pick is copied in, and the last picks, each copied again by a
/// click. The format is `color_format` in ~/.config/yggdrasil/tools.conf; the history ~/.local/state/yggdrasil/colors.txt.
fn color_picker_section(ui: &mut egui::Ui, pal: &Palette) {
    use colorpicker::Format;
    let installed = std::path::Path::new("/usr/bin/yggdrasil-colorpicker").exists();
    let mut format = colorpicker::chosen_format();
    ui::group(ui, pal, Some("Color Picker"), |ui| {
        ui::row(ui, pal, true, "Color picker", Some("Win + Shift + C: the screen freezes with a loupe; a click copies the colour"), |ui| {
            if installed && ui.button("Pick a colour").clicked() {
                sys::spawn("yggdrasil-colorpicker", &[]);
            }
        });
        ui::row(ui, pal, false, "Copy as", Some(&format.text((0x7c, 0x54, 0xd6))), |ui| {
            egui::ComboBox::from_id_salt("color-format").selected_text(format.label()).show_ui(ui, |ui| {
                for f in Format::ALL {
                    if ui.selectable_value(&mut format, f, f.label()).changed() {
                        let _ = colorpicker::save_format(f);
                    }
                }
            });
        });
        let history = colorpicker::load_history();
        ui::row(ui, pal, false, "Recent colours", Some(if history.is_empty() { "None picked yet" } else { "Click one to copy it again" }), |_| {});
        if !history.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for c in history {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
                    ui.painter().rect_filled(rect, CornerRadius::same(6), egui::Color32::from_rgb(c.0, c.1, c.2));
                    if resp.hovered() {
                        ui.painter().rect_stroke(rect, CornerRadius::same(6), egui::Stroke::new(2.0, pal.text), egui::StrokeKind::Outside);
                    }
                    let tip = Format::ALL.iter().map(|f| f.text(c)).collect::<Vec<_>>().join("\n");
                    if resp.on_hover_text(tip).clicked() {
                        ui.ctx().copy_text(format.text(c));
                    }
                }
            });
        }
    });
}

fn page_shortcuts(ui: &mut egui::Ui, pal: &Palette) {
    // Find my mouse (the panel; ~/.config/yggdrasil/tools.conf)
    let mut tools = yggdrasil_conf::tools::ToolsConf::load();
    ui::group(ui, pal, Some("Tools"), |ui| {
        ui::row(ui, pal, true, "Find my mouse", Some("Press Ctrl twice: a spotlight shows where the pointer is"), |ui| {
            if ui::switch(ui, pal, &mut tools.find_mouse).changed() {
                let _ = tools.save();
            }
        });
    });
    color_picker_section(ui, pal);
    // Yggdrasil's session: the shortcuts as Hyprland has them right now, with their own descriptions, grouped, with a search
    if sys::hyprland() {
        let list = hypr_shortcuts();
        let qid = egui::Id::new("shortcut-search");
        let mut query = ui.data_mut(|d| d.get_temp::<String>(qid)).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search shortcuts").desired_width(320.0));
            ui.label(RichText::new("Win + / shows them all on a card").color(pal.dim));
        });
        ui.data_mut(|d| d.insert_temp(qid, query.clone()));
        let q = query.trim().to_lowercase();
        let shown: Vec<&yggdrasil_conf::shortcuts::Shortcut> = list.iter().filter(|s| q.is_empty() || s.what.to_lowercase().contains(&q) || s.keys.to_lowercase().contains(&q)).collect();
        for g in yggdrasil_conf::shortcuts::GROUPS {
            let rows: Vec<_> = shown.iter().filter(|s| s.group == g).collect();
            if rows.is_empty() {
                continue;
            }
            ui::group(ui, pal, Some(g), |ui| {
                for (i, s) in rows.iter().enumerate() {
                    ui::row(ui, pal, i == 0, &s.what, None, |ui| {
                        egui::Frame::new().fill(pal.hover).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
                            ui.label(RichText::new(&s.keys).monospace());
                        });
                    });
                }
            });
        }
        if list.is_empty() {
            ui.label(RichText::new("Hyprland did not answer").color(pal.dim));
        } else if shown.is_empty() {
            ui.label(RichText::new("No shortcut matches").color(pal.dim));
        }
        ui.add_space(8.0);
        if ui::quiet(ui, pal, "Add your own shortcuts…").on_hover_text("Opens your own settings file for the session (~/.config/yggdrasil/hyprland.lua)").clicked() {
            open_own_shortcuts();
        }
        return;
    }
    let groups: [(&str, &[(&str, &str)]); 3] = [
        ("Everywhere", &[
            ("Win", "Launcher"), ("Win + E", "Minos"), ("Win + R", "Run"), ("Win + I", "Settings"), ("Win + L", "Lock"),
            ("Win + V", "Clipboard history"), ("Win + Shift + S", "Screenshot of a region"), ("Ctrl + Shift + Esc", "Task manager"), ("Win + Ctrl + Shift + W", "Remove the current desktop"),
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
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1060.0, 720.0])
            .with_min_inner_size([560.0, 400.0])
            .with_title("Settings")
            .with_app_id("yggdrasil-settings")
            // Draws its own title bar like Minos (yggdrasil-ui chrome).
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

/// A time of day (minutes into it) shown as HH:MM: typed, or dragged in steps of 15 minutes.
fn time_field(ui: &mut egui::Ui, t: &mut u32) {
    let mut v = *t as f64;
    let r = ui.add(
        egui::DragValue::new(&mut v)
            .range(0.0..=1439.0)
            .speed(0.5)
            .custom_formatter(|v, _| nightlight::fmt_time(v as u32))
            .custom_parser(|s| nightlight::parse_time(s.trim()).map(f64::from)),
    );
    if r.changed() {
        *t = if r.dragged() { ((v / 15.0).round() * 15.0).min(1425.0) as u32 } else { v as u32 };
    }
}

/// A bar painted from `from` K to `to` K, as a fade goes.
fn ramp_bar(ui: &mut egui::Ui, pal: &Palette, from: u32, to: u32) {
    let (rect, _) = ui.allocate_exact_size(vec2(200.0, 10.0), Sense::hover());
    let slices = 64;
    let w = rect.width() / slices as f32;
    for k in 0..slices {
        let (r, g, b) = nightlight::kelvin_rgb(nightlight::kelvin_between(from, to, k as f32 / (slices - 1) as f32));
        let x = rect.left() + w * k as f32;
        ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + w + 0.5, rect.bottom())), 0.0, egui::Color32::from_rgb(r, g, b));
    }
    ui.painter().rect_stroke(rect, CornerRadius::same(2), egui::Stroke::new(1.0, pal.border), egui::StrokeKind::Outside);
}
