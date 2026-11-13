//! Settings > Peripherals: the Mouse and Keyboard pages.
//!
//! - **Mouse**: pointer speed, acceleration (adaptive or flat), scroll speed, natural scrolling and the main button's side, for
//!   every mouse and, where the compositor lists its mice, per mouse; Find my mouse.
//! - **Keyboard**: how soon and how fast a held key repeats, the layout (the system's, for every account: `localectl`), and
//!   "Keyboard and mouse lights off while dimmed" (idle.conf's `lights_off`, moved here from Power).
//! - **Lights, DPI and polling rate**, only when there is something to show: Razer devices through OpenRazer's daemon
//!   (razer.rs), each with its lighting zones (brightness, effect, colour) and, for a mouse, its DPI, polling rate and battery;
//!   keyboards on the Keyboard page, mice and mouse mats on the Mouse page. Logitech (Solaar), Corsair (ckb-next) and the other
//!   tools Agora offers get a row that opens them when installed. With none of it installed, a line points to Agora.
//!
//! The mouse and keyboard settings go to ~/.config/yggdrasil/input.conf (yggdrasil-wm's `input`), which the session reads at
//! every start, and to the running compositor at once (`Desk::apply_input`). Everything that asks something of the system runs
//! on a background thread; the page never waits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;
use yggdrasil_wm::input::{self, Accel, InputConf, Pointer};

use crate::razer::{self, Change, Effect, ZoneKind};
use crate::{sys, ui};

/// Programs that set up devices of other makers: (program, title, what it is). A row opens each one installed.
const TOOLS: [(&str, &str, &str); 5] = [
    ("polychromatic-controller", "More Razer settings", "Polychromatic: effects, profiles and the rest of OpenRazer"),
    ("solaar", "Logitech devices", "Solaar: receivers, battery and the devices' own settings"),
    ("ckb-next", "Corsair devices", "ckb-next: lighting, macros and keys"),
    ("openrgb", "Lights of other makers", "OpenRGB: one app for many makers' lighting"),
    ("piper", "Gaming mouse buttons and lights", "Piper: Logitech, SteelSeries, Roccat and others"),
];
const OPENRAZER: &str = "openrazer-daemon";
/// How long a colour must stay put in the picker before it is sent (it changes on every pixel of a drag).
const COLOUR_SETTLE: Duration = Duration::from_millis(400);
/// The Razer devices are read again this often while their page is open (battery, changes made elsewhere).
const REREAD: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sub {
    Main,
    Mouse,
    Keyboard,
}

/// OpenRazer's state.
#[derive(Clone, Debug)]
enum Razer {
    Loading(Instant),
    /// openrazer-daemon is not installed
    Missing,
    /// installed, not running
    Stopped,
    Devices(Vec<razer::Device>),
    Failed(String),
}

/// The system's keyboard layout (localectl's X11 settings).
#[derive(Clone, Debug, Default, PartialEq)]
struct Layout {
    layout: String,
    variant: String,
    model: String,
    options: String,
}

enum Msg {
    /// The compositor's mice (None: it cannot list them).
    Mice(Option<Vec<String>>),
    Razer(Razer),
    Layout(Option<Layout>),
    /// What a change said: an error, or a note.
    Note(String, bool),
}

/// The compositor the session runs on, as far as these pages care.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Comp {
    Hyprland,
    Own,
    Other,
}

fn compositor() -> Comp {
    if crate::fixture::get().is_some() {
        return Comp::Hyprland;
    }
    match sys::desk() {
        Some(yggdrasil_wm::Desk::Hyprland(_)) => Comp::Hyprland,
        Some(yggdrasil_wm::Desk::Own(_)) => Comp::Own,
        None => Comp::Other,
    }
}

/// A program is installed (the fixture's list in a made-up system).
fn installed(program: &str) -> bool {
    if let Some(f) = crate::fixture::get() {
        return f.tools.iter().any(|t| t == program);
    }
    ["/usr/bin", "/usr/local/bin"].iter().any(|d| std::path::Path::new(d).join(program).is_file())
}

pub struct State {
    pub sub: Sub,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    conf: InputConf,
    /// The last settings given to the compositor, and which change is the newest (an older one still on its way is skipped).
    applied: Arc<Mutex<InputConf>>,
    generation: Arc<AtomicU64>,
    mice: Option<Vec<String>>,
    comp: Comp,
    razer: Razer,
    razer_read: Instant,
    /// A worker that makes the Razer changes in the order asked (started at the first one).
    razer_tx: Option<Sender<(String, Change)>>,
    pending_colour: Option<(String, ZoneKind, Effect, [u8; 3], Instant)>,
    layout: Option<Layout>,
    lights_off: bool,
    tools: Vec<usize>,
    openrazer: bool,
    note: Option<(String, bool)>,
    try_text: String,
    /// The star key's group (read when the Keyboard page is first shown)
    star: Option<crate::starkey::State>,
}

impl Default for State {
    fn default() -> Self {
        let (tx, rx) = channel();
        State {
            sub: Sub::Main,
            tx,
            rx,
            conf: InputConf::default(),
            applied: Arc::default(),
            generation: Arc::default(),
            mice: None,
            comp: Comp::Other,
            razer: Razer::Loading(Instant::now()),
            razer_read: Instant::now(),
            razer_tx: None,
            pending_colour: None,
            layout: None,
            lights_off: true,
            tools: vec![],
            openrazer: false,
            note: None,
            try_text: String::new(),
            star: None,
        }
    }
}

impl State {
    /// Opening on one of its pages.
    pub fn on(sub: Sub) -> State {
        State { sub, ..Default::default() }
    }

    /// Reads everything again: the settings file and the installed tools at once (small), the rest in the background.
    pub fn open(&mut self, ctx: &egui::Context) {
        self.conf = InputConf::load();
        *self.applied.lock().unwrap_or_else(|e| e.into_inner()) = self.conf.clone();
        self.lights_off = sys::lights_off();
        self.tools = (0..TOOLS.len()).filter(|&i| installed(TOOLS[i].0)).collect();
        self.openrazer = installed(OPENRAZER);
        self.comp = compositor();
        self.note = None;
        self.read_razer(ctx);
        let (tx, c) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Mice(read_mice()));
            let _ = tx.send(Msg::Layout(read_layout()));
            c.request_repaint();
        });
    }

    fn read_razer(&mut self, ctx: &egui::Context) {
        if !matches!(self.razer, Razer::Devices(_)) {
            self.razer = Razer::Loading(Instant::now());
        }
        self.razer_read = Instant::now();
        let (tx, c, installed) = (self.tx.clone(), ctx.clone(), self.openrazer);
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Razer(read_razer(installed)));
            c.request_repaint();
        });
    }

    /// Takes what the background threads found, and sends a colour that has settled.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Mice(m) => self.mice = m,
                Msg::Razer(r) => self.razer = r,
                Msg::Layout(l) => self.layout = l,
                Msg::Note(n, error) => self.note = (!n.is_empty()).then_some((n, error)),
            }
        }
        if let Some((id, z, e, c, at)) = self.pending_colour.clone() {
            if at.elapsed() >= COLOUR_SETTLE {
                self.pending_colour = None;
                self.razer_change(ctx, &id, Change::Effect(z, e, c));
            } else {
                ctx.request_repaint_after(COLOUR_SETTLE);
            }
        }
        // the Razer devices are read again now and then while a page that shows them is open (not while a colour is picked)
        let showing = matches!(self.sub, Sub::Mouse | Sub::Keyboard);
        if showing && self.pending_colour.is_none() && self.razer_read.elapsed() >= REREAD && matches!(self.razer, Razer::Devices(_)) {
            let idle = !ctx.input(|i| i.pointer.any_down()) && ctx.input(|i| i.viewport().focused != Some(false));
            if idle {
                self.read_razer(ctx);
            }
        }
        if showing {
            ctx.request_repaint_after(REREAD);
        }
    }

    /// Saves the settings and gives them to the compositor (in the background, newest last).
    fn apply_input(&mut self, now: InputConf, ctx: &egui::Context) {
        self.conf = now.clone();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (latest, applied, tx, c) = (self.generation.clone(), self.applied.clone(), self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut before = applied.lock().unwrap_or_else(|e| e.into_inner());
            if latest.load(Ordering::SeqCst) != generation {
                return; // a newer change follows with everything in it
            }
            let r = if crate::fixture::get().is_some() {
                Err("A made-up system (a fixture): nothing is changed.".to_string())
            } else {
                now.save().and_then(|_| match sys::desk() {
                    Some(d) => d.apply_input(&before, &now),
                    // no compositor of ours: kept for the next sign-in
                    None => Ok(()),
                })
            };
            *before = now;
            let _ = tx.send(match r {
                Ok(()) => Msg::Note(String::new(), false),
                Err(e) if e == input::NOT_YET => Msg::Note(e, false),
                Err(e) => Msg::Note(e, true),
            });
            c.request_repaint();
        });
    }

    fn razer_change(&mut self, ctx: &egui::Context, id: &str, change: Change) {
        // shown at once; read again if the device refused it
        if let Razer::Devices(list) = &mut self.razer {
            if let Some(d) = list.iter_mut().find(|d| d.id == id) {
                match &change {
                    Change::Brightness(z, b) => d.zones.iter_mut().filter(|x| x.kind == *z).for_each(|x| x.brightness = Some(*b)),
                    Change::Effect(z, e, c) => d.zones.iter_mut().filter(|x| x.kind == *z).for_each(|x| {
                        x.effect = Some(*e);
                        x.color = *c;
                    }),
                    Change::Dpi(v) => {
                        if let Some(dpi) = &mut d.dpi {
                            (dpi.x, dpi.y) = (*v, *v);
                        }
                    }
                    Change::Poll(r) => {
                        if let Some(p) = &mut d.poll {
                            p.0 = *r;
                        }
                    }
                }
            }
        }
        let tx = self.razer_tx.get_or_insert_with(|| {
            let (tx, rx) = channel::<(String, Change)>();
            let (back, c) = (self.tx.clone(), ctx.clone());
            let installed = self.openrazer;
            std::thread::spawn(move || {
                let mut bus = None;
                // ends when the page's state goes (its sender with it)
                while let Ok((id, change)) = rx.recv() {
                    let r = if crate::fixture::get().is_some() {
                        Err("A made-up system (a fixture): nothing is changed.".to_string())
                    } else {
                        if bus.is_none() {
                            bus = razer::session().ok();
                        }
                        match &bus {
                            Some(b) => razer::apply_on(b, &id, &change),
                            None => Err("The session bus could not be reached.".into()),
                        }
                    };
                    if let Err(e) = r {
                        let _ = back.send(Msg::Note(e, true));
                        let _ = back.send(Msg::Razer(read_razer(installed)));
                        c.request_repaint();
                    }
                }
            });
            tx
        });
        let _ = tx.send((id.to_string(), change));
    }

    /// The Peripherals page itself: the way to Mouse and Keyboard.
    pub fn main_page(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let razer_mice = self.razer_devices(|d| d.is_mouse_side()).next().is_some();
        let razer_keys = self.razer_devices(|d| d.is_keyboard_side()).next().is_some();
        let mut go = None;
        ui::group(ui, pal, None, |ui| {
            let mouse = if razer_mice {
                "Pointer speed, acceleration, scrolling and buttons; lights, DPI and polling rate"
            } else {
                "Pointer speed, acceleration, scrolling and buttons"
            };
            if ui::nav_row(ui, pal, true, "Mouse", Some(mouse), |_| {}) {
                go = Some(Sub::Mouse);
            }
            let keys = if razer_keys {
                "Key repeat, layout and lights; shortcuts, spelling and languages"
            } else {
                "Key repeat, layout, lights off while dimmed; shortcuts, spelling and languages"
            };
            if ui::nav_row(ui, pal, false, "Keyboard", Some(keys), |_| {}) {
                go = Some(Sub::Keyboard);
            }
        });
        // Razer devices that are neither (a headset, a dock, a chroma stand): their lights here
        let others: Vec<razer::Device> = self.razer_devices(|d| !d.is_mouse_side() && !d.is_keyboard_side()).cloned().collect();
        let ctx = ui.ctx().clone();
        for d in &others {
            self.razer_device(ui, pal, &ctx, d);
        }
        if let Some(s) = go {
            self.sub = s;
        }
    }

    fn razer_devices(&self, keep: impl Fn(&razer::Device) -> bool) -> impl Iterator<Item = &razer::Device> {
        let list: &[razer::Device] = match &self.razer {
            Razer::Devices(l) => l,
            _ => &[],
        };
        list.iter().filter(move |d| keep(d))
    }

    fn note(&self, ui: &mut egui::Ui, pal: &Palette) {
        if let Some((n, error)) = &self.note {
            ui.add_space(6.0);
            ui.label(RichText::new(n).size(12.5).color(if *error { pal.red } else { pal.dim }));
        }
    }

    /// Settings > Peripherals > Mouse.
    pub fn mouse_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        self.note(ui, pal);
        let mut conf = self.conf.clone();
        let mut commit = false;
        ui::group(ui, pal, Some("Every mouse"), |ui| {
            let base = Pointer::default();
            commit |= pointer_rows(ui, pal, &mut conf.all, &base, true);
            if !conf.all.is_empty() {
                ui::row(
                    ui,
                    pal,
                    false,
                    "Back to the defaults",
                    Some("The speed and scrolling of the mouse itself, with acceleration"),
                    |ui| {
                        if ui::quiet(ui, pal, "Reset").clicked() {
                            conf.all = Pointer::default();
                            commit = true;
                        }
                    },
                );
            }
        });
        // each mouse the compositor has, and those with settings of their own that are not plugged in now
        let mut names: Vec<String> = self.mice.clone().unwrap_or_default();
        names.extend(conf.devices.keys().cloned());
        names.sort();
        names.dedup();
        if !names.is_empty() && self.comp != Comp::Own {
            ui::group(ui, pal, Some("Each mouse"), |ui| {
                for (i, name) in names.iter().enumerate() {
                    let connected = self.mice.as_ref().is_some_and(|m| m.contains(name));
                    let mut own = conf.devices.contains_key(name);
                    let sub = match (own, connected) {
                        (true, true) => "Its own settings",
                        (true, false) => "Its own settings · not connected",
                        (false, _) => "As every mouse",
                    };
                    ui::row(ui, pal, i == 0, &input::pretty(name), Some(sub), |ui| {
                        if ui::switch(ui, pal, &mut own).on_hover_text("Settings of its own").changed() {
                            if own {
                                // it starts from what every mouse does now
                                let all = &conf.all;
                                let start = Pointer {
                                    sensitivity: Some(all.sensitivity.unwrap_or(0.0)),
                                    accel: Some(all.accel.unwrap_or(Accel::Adaptive)),
                                    scroll_factor: Some(all.scroll_factor.unwrap_or(1.0)),
                                    natural_scroll: Some(all.natural_scroll.unwrap_or(false)),
                                    left_handed: Some(all.left_handed.unwrap_or(false)),
                                };
                                conf.devices.insert(name.clone(), start);
                            } else {
                                conf.devices.remove(name);
                            }
                            commit = true;
                        }
                    });
                    if let Some(p) = conf.devices.get_mut(name) {
                        let all = conf.all.clone();
                        ui.indent(("mouse", name.as_str()), |ui| {
                            commit |= pointer_rows(ui, pal, p, &all, false);
                        });
                    }
                }
            });
        }
        match self.comp {
            Comp::Own => {
                ui.add_space(6.0);
                ui.label(RichText::new(input::NOT_YET).size(12.0).color(pal.dim));
            }
            Comp::Other => {
                ui.add_space(6.0);
                ui.label(RichText::new("Kept, and used from your next sign-in to Yggdrasil.").size(12.0).color(pal.dim));
            }
            Comp::Hyprland => {}
        }
        if commit {
            self.apply_input(conf, ctx);
        }
        let mice: Vec<razer::Device> = self.razer_devices(|d| d.is_mouse_side()).cloned().collect();
        for d in &mice {
            self.razer_device(ui, pal, ctx, d);
        }
        self.razer_status(ui, pal, ctx);
        // Find my mouse (the panel; ~/.config/yggdrasil/tools.conf)
        let mut tools = yggdrasil_conf::tools::ToolsConf::load();
        ui::group(ui, pal, Some("Tools"), |ui| {
            ui::row(ui, pal, true, "Find my mouse", Some("Press Ctrl twice: a spotlight shows where the pointer is"), |ui| {
                if ui::switch(ui, pal, &mut tools.find_mouse).changed() {
                    let _ = tools.save();
                }
            });
        });
        self.tool_rows(ui, pal);
    }

    /// Settings > Peripherals > Keyboard, its own part: typing, the layout, the lights, Razer keyboards. (Shortcuts and spelling
    /// follow from main.rs.)
    pub fn keyboard_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        self.note(ui, pal);
        let mut conf = self.conf.clone();
        let mut commit = false;
        ui::group(ui, pal, Some("Typing"), |ui| {
            let mut delay = conf.repeat_delay.unwrap_or(input::DEFAULT_REPEAT_DELAY) as f32;
            ui::row(ui, pal, true, "Repeat delay", Some("How long a key is held before it repeats"), |ui| {
                let r = ui::slider_range(ui, pal, &mut delay, 150.0..=1000.0, " ms");
                if released(&r) {
                    conf.repeat_delay = Some(delay.round() as u32);
                    commit = true;
                }
            });
            let mut rate = conf.repeat_rate.unwrap_or(input::DEFAULT_REPEAT_RATE) as f32;
            ui::row(ui, pal, false, "Repeat rate", Some("Characters a second while it repeats"), |ui| {
                let r = ui::slider_range(ui, pal, &mut rate, 5.0..=60.0, " a second");
                if released(&r) {
                    conf.repeat_rate = Some(rate.round() as u32);
                    commit = true;
                }
            });
            ui::row(ui, pal, false, "Try it", Some("Hold a key down in the box"), |ui| {
                ui::field(ui, pal, &mut self.try_text, "Type here", 220.0, false);
            });
            if conf.repeat_delay.is_some() || conf.repeat_rate.is_some() {
                ui::row(ui, pal, false, "Back to the defaults", Some("600 ms, then 25 a second"), |ui| {
                    if ui::quiet(ui, pal, "Reset").clicked() {
                        conf.repeat_delay = None;
                        conf.repeat_rate = None;
                        commit = true;
                    }
                });
            }
        });
        if commit {
            self.apply_input(conf, ctx);
        }
        // a keyboard with a star key (a ThinkPad's F12): what it does
        crate::starkey::group(ui, pal, self.star.get_or_insert_with(crate::starkey::State::read));
        self.layout_group(ui, pal, ctx);
        let mut lights = self.lights_off;
        ui::group(ui, pal, Some("Lights"), |ui| {
            ui::row(
                ui,
                pal,
                true,
                "Keyboard and mouse lights off while dimmed",
                Some("Also while the screen is off or shows the screensaver; they come back with you. Razer keyboards, mice and mouse mats (OpenRazer) and laptop keyboard backlights"),
                |ui| {
                    if ui::switch(ui, pal, &mut lights).changed() {
                        let (tx, c) = (self.tx.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            let r = sys::set_lights_off(lights);
                            let _ = tx.send(Msg::Note(r.err().unwrap_or_default(), true));
                            c.request_repaint();
                        });
                    }
                },
            );
        });
        self.lights_off = lights;
        let keyboards: Vec<razer::Device> = self.razer_devices(|d| d.is_keyboard_side()).cloned().collect();
        for d in &keyboards {
            self.razer_device(ui, pal, ctx, d);
        }
        self.razer_status(ui, pal, ctx);
    }

    fn layout_group(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(now) = self.layout.clone() else { return };
        let (layouts, variants) = xkb();
        let mut chosen: Option<(String, String)> = None;
        ui::group(ui, pal, Some("Layout"), |ui| {
            let name = |code: &str| layouts.iter().find(|(c, _)| c == code).map(|(_, d)| d.clone()).unwrap_or_else(|| code.to_string());
            ui::row(
                ui,
                pal,
                true,
                "Layout",
                Some("For every account on this computer, and the sign-in screen; asks for your password"),
                |ui| {
                    egui::ComboBox::from_id_salt("kb-layout").width(240.0).selected_text(name(&now.layout)).show_ui(ui, |ui| {
                        for (code, desc) in layouts.iter() {
                            if ui.selectable_label(*code == now.layout, desc).clicked() && *code != now.layout {
                                chosen = Some((code.clone(), String::new()));
                            }
                        }
                    });
                },
            );
            let mine: Vec<&(String, String, String)> = variants.iter().filter(|(_, l, _)| *l == now.layout).collect();
            if !mine.is_empty() {
                let vname = |v: &str| {
                    if v.is_empty() {
                        "Standard".to_string()
                    } else {
                        mine.iter().find(|(c, _, _)| c == v).map(|(_, _, d)| d.clone()).unwrap_or_else(|| v.to_string())
                    }
                };
                ui::row(ui, pal, false, "Variant", Some("Dvorak, Colemak, international and the like"), |ui| {
                    egui::ComboBox::from_id_salt("kb-variant").width(240.0).selected_text(vname(&now.variant)).show_ui(ui, |ui| {
                        if ui.selectable_label(now.variant.is_empty(), "Standard").clicked() && !now.variant.is_empty() {
                            chosen = Some((now.layout.clone(), String::new()));
                        }
                        for (code, _, desc) in &mine {
                            if ui.selectable_label(*code == now.variant, desc).clicked() && *code != now.variant {
                                chosen = Some((now.layout.clone(), code.clone()));
                            }
                        }
                    });
                });
            }
        });
        if let Some((layout, variant)) = chosen {
            let (tx, c) = (self.tx.clone(), ctx.clone());
            let (model, options) = (now.model.clone(), now.options.clone());
            std::thread::spawn(move || {
                let r = sys::act("localectl", &["set-x11-keymap", &layout, &model, &variant, &options]);
                if r.is_ok() {
                    // the running session too, at once (the session's config reads the system's file at its next start)
                    if let Some(d) = sys::desk() {
                        d.set_keyboard(&layout, &variant);
                    }
                }
                let _ = tx.send(Msg::Note(r.err().unwrap_or_default(), true));
                let _ = tx.send(Msg::Layout(read_layout()));
                c.request_repaint();
            });
        }
    }

    /// One Razer device: its battery, a mouse's DPI and polling rate, and each lighting zone.
    fn razer_device(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, d: &razer::Device) {
        let mut todo: Vec<Change> = vec![];
        let mut colour: Option<(ZoneKind, Effect, [u8; 3])> = None;
        ui::group(ui, pal, Some(d.name.as_str()), |ui| {
            let mut first = true;
            let mut next_first = || std::mem::replace(&mut first, false);
            if let Some((pct, charging)) = d.battery {
                let text = if charging { format!("{pct:.0} %, charging") } else { format!("{pct:.0} %") };
                ui::row(ui, pal, next_first(), "Battery", None, |ui| {
                    ui.label(text);
                });
            }
            if let Some(dpi) = &d.dpi {
                let mut v = dpi.x as f32;
                ui::row(
                    ui,
                    pal,
                    next_first(),
                    "DPI",
                    Some("How far the pointer goes for an inch of the mouse; Pointer speed comes on top"),
                    |ui| {
                        let r = ui::slider_range(ui, pal, &mut v, 100.0..=dpi.max as f32, "");
                        if released(&r) {
                            // in steps of 50, as the mice take it
                            todo.push(Change::Dpi(((v / 50.0).round() * 50.0).max(100.0) as u32));
                        }
                    },
                );
            }
            if let Some((now, rates)) = &d.poll {
                let labels: Vec<String> = rates.iter().map(|r| format!("{r} Hz")).collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                let sel = rates.iter().position(|r| r == now).unwrap_or(0);
                ui::row(
                    ui,
                    pal,
                    next_first(),
                    "Polling rate",
                    Some("How often a second the mouse reports; higher is smoother and uses more battery"),
                    |ui| {
                        if let Some(i) = ui::segmented(ui, pal, &refs, sel) {
                            todo.push(Change::Poll(rates[i]));
                        }
                    },
                );
            }
            for z in &d.zones {
                let sub = match z.effect {
                    Some(e) => e.label(),
                    None if z.effects.is_empty() => "Brightness",
                    None => "Another effect (set in Polychromatic or a game)",
                };
                ui::row(ui, pal, next_first(), z.kind.label(), Some(sub), |ui| {
                    // right to left: the colour, the effect, the brightness
                    if let Some(e) = z.effect.filter(|e| e.coloured()) {
                        let mut c = self.pending_colour.as_ref().filter(|p| p.0 == d.id && p.1 == z.kind).map(|p| p.3).unwrap_or(z.color);
                        if egui::color_picker::color_edit_button_srgb(ui, &mut c).on_hover_text("Colour").changed() {
                            colour = Some((z.kind, e, c));
                        }
                    }
                    if !z.effects.is_empty() {
                        let text = z.effect.map(|e| e.label()).unwrap_or("Other");
                        egui::ComboBox::from_id_salt(("razer-fx", d.id.as_str(), z.kind)).width(120.0).selected_text(text).show_ui(
                            ui,
                            |ui| {
                                for e in &z.effects {
                                    if ui.selectable_label(z.effect == Some(*e), e.label()).clicked() && z.effect != Some(*e) {
                                        todo.push(Change::Effect(z.kind, *e, z.color));
                                    }
                                }
                            },
                        );
                    }
                    if let Some(b) = z.brightness {
                        let mut v = b as f32;
                        let r = ui::slider_range(ui, pal, &mut v, 0.0..=100.0, " %");
                        if released(&r) {
                            todo.push(Change::Brightness(z.kind, v.round() as f64));
                        }
                    }
                });
            }
        });
        if let Some((z, e, c)) = colour {
            self.pending_colour = Some((d.id.clone(), z, e, c, Instant::now()));
            ctx.request_repaint_after(COLOUR_SETTLE);
        }
        for t in todo {
            self.razer_change(ctx, &d.id, t);
        }
    }

    /// What there is to say about OpenRazer when it shows no device.
    fn razer_status(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let (title, sub, start) = match &self.razer {
            Razer::Missing | Razer::Devices(_) => return,
            Razer::Loading(at) if at.elapsed() < Duration::from_secs(6) => return,
            Razer::Loading(_) => ("Razer devices", "OpenRazer's daemon did not answer".to_string(), false),
            Razer::Stopped => ("Razer devices", "OpenRazer's daemon is not running".to_string(), true),
            Razer::Failed(e) => ("Razer devices", e.clone(), false),
        };
        let mut again = false;
        let mut run = false;
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, title, Some(&sub), |ui| {
                if start && ui::quiet(ui, pal, "Start it").clicked() {
                    run = true;
                }
                if ui::quiet(ui, pal, "Look again").clicked() {
                    again = true;
                }
            });
        });
        if run {
            let (tx, c, installed) = (self.tx.clone(), ctx.clone(), self.openrazer);
            std::thread::spawn(move || {
                let r = sys::act("systemctl", &["--user", "start", "openrazer-daemon.service"]);
                let _ = tx.send(Msg::Note(r.err().unwrap_or_default(), true));
                std::thread::sleep(Duration::from_secs(2));
                let _ = tx.send(Msg::Razer(read_razer(installed)));
                c.request_repaint();
            });
        }
        if again {
            self.read_razer(ctx);
        }
    }

    /// The other makers' tools that are installed, each opened by its row; with nothing for lights installed, where to get it.
    pub fn tool_rows(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        if self.tools.is_empty() && !self.openrazer {
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Lights, DPI and polling rate: install support for your devices from Agora > Devices.")
                        .size(12.5)
                        .color(pal.dim),
                );
                if ui::quiet(ui, pal, "Open Agora").clicked() {
                    sys::spawn("agora", &[]);
                }
            });
            return;
        }
        if self.tools.is_empty() {
            return;
        }
        ui::group(ui, pal, Some("Other devices"), |ui| {
            for (n, &i) in self.tools.iter().enumerate() {
                let (program, title, sub) = TOOLS[i];
                ui::row(ui, pal, n == 0, title, Some(sub), |ui| {
                    if ui::quiet(ui, pal, "Open").clicked() {
                        sys::spawn(program, &[]);
                    }
                });
            }
        });
    }
}

/// A slider let go, or moved by a click or a key: the moment to apply it (not on every pixel of a drag).
fn released(r: &egui::Response) -> bool {
    r.drag_stopped() || (r.changed() && !r.dragged())
}

/// The pointer rows for `p` over `base` (every mouse's settings: `base` is the defaults). True when something changed.
fn pointer_rows(ui: &mut egui::Ui, pal: &Palette, p: &mut Pointer, base: &Pointer, first: bool) -> bool {
    let now = p.over(base);
    let mut changed = false;
    let mut speed = (now.sensitivity.unwrap_or(0.0) * 100.0) as f32;
    ui::row(ui, pal, first, "Pointer speed", Some("0 % is the mouse's own speed"), |ui| {
        let r = ui::slider_range(ui, pal, &mut speed, -100.0..=100.0, " %");
        if released(&r) {
            p.sensitivity = Some((speed.round() / 100.0) as f64);
            changed = true;
        }
    });
    let mut accel = now.accel != Some(Accel::Flat);
    ui::row(
        ui,
        pal,
        false,
        "Mouse acceleration",
        Some("Faster movements take the pointer further. Off: it goes exactly as far as the mouse, as most games want"),
        |ui| {
            if ui::switch(ui, pal, &mut accel).changed() {
                p.accel = Some(if accel { Accel::Adaptive } else { Accel::Flat });
                changed = true;
            }
        },
    );
    let mut scroll = (now.scroll_factor.unwrap_or(1.0) * 100.0) as f32;
    ui::row(ui, pal, false, "Scroll speed", Some("100 % is the mouse's own"), |ui| {
        let r = ui::slider_range(ui, pal, &mut scroll, 10.0..=500.0, " %");
        if released(&r) {
            p.scroll_factor = Some((scroll.round() / 100.0) as f64);
            changed = true;
        }
    });
    let mut natural = now.natural_scroll.unwrap_or(false);
    ui::row(ui, pal, false, "Natural scrolling", Some("The page follows the wheel, as on a touchpad"), |ui| {
        if ui::switch(ui, pal, &mut natural).changed() {
            p.natural_scroll = Some(natural);
            changed = true;
        }
    });
    let left = now.left_handed.unwrap_or(false);
    ui::row(ui, pal, false, "Main button", Some("Right for the left hand"), |ui| {
        if let Some(i) = ui::segmented(ui, pal, &["Left", "Right"], left as usize) {
            p.left_handed = Some(i == 1);
            changed = true;
        }
    });
    changed
}

/// The compositor's mice, one per mouse.
fn read_mice() -> Option<Vec<String>> {
    let devices = match crate::fixture::get() {
        Some(f) => yggdrasil_wm::hyprland::parse_devices(&f.answer("hyprctl", &["-j", "devices"])?)?,
        None => sys::desk()?.input_devices()?,
    };
    Some(input::mice(&devices))
}

fn read_razer(installed: bool) -> Razer {
    if let Some(f) = crate::fixture::get() {
        return f.razer.clone().map(Razer::Devices).unwrap_or(Razer::Missing);
    }
    if !installed {
        return Razer::Missing;
    }
    let bus = match razer::session() {
        Ok(b) => b,
        Err(e) => return Razer::Failed(e),
    };
    if !razer::running(&bus) {
        return Razer::Stopped;
    }
    match razer::list_on(&bus) {
        Ok(l) => Razer::Devices(l),
        Err(e) => Razer::Failed(format!("OpenRazer's daemon did not answer: {e}")),
    }
}

/// localectl's X11 keyboard settings (None: localectl did not answer).
fn read_layout() -> Option<Layout> {
    Some(parse_localectl(&sys::run("localectl", &["status"])?))
}

fn parse_localectl(text: &str) -> Layout {
    let mut l = Layout::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim().to_string();
        match k.trim() {
            "X11 Layout" => l.layout = v,
            "X11 Variant" => l.variant = v,
            "X11 Model" => l.model = v,
            "X11 Options" => l.options = v,
            _ => {}
        }
    }
    if l.layout.is_empty() {
        l.layout = "us".into();
    }
    l
}

type Layouts = (Vec<(String, String)>, Vec<(String, String, String)>);

/// xkb's layouts (code, name), by name, and variants (code, layout, name), from its rules list; read once.
fn xkb() -> &'static Layouts {
    static L: OnceLock<Layouts> = OnceLock::new();
    L.get_or_init(|| parse_xkb(&std::fs::read_to_string("/usr/share/X11/xkb/rules/base.lst").unwrap_or_default()))
}

fn parse_xkb(text: &str) -> Layouts {
    let (mut layouts, mut variants) = (vec![], vec![]);
    let mut section = "";
    for line in text.lines() {
        if let Some(s) = line.strip_prefix("! ") {
            section = if s.trim() == "layout" {
                "layout"
            } else if s.trim() == "variant" {
                "variant"
            } else {
                ""
            };
            continue;
        }
        let line = line.trim();
        let Some((code, rest)) = line.split_once(char::is_whitespace) else { continue };
        let rest = rest.trim();
        match section {
            "layout" => layouts.push((code.to_string(), rest.to_string())),
            "variant" => {
                if let Some((layout, name)) = rest.split_once(':') {
                    variants.push((code.to_string(), layout.trim().to_string(), name.trim().to_string()));
                }
            }
            _ => {}
        }
    }
    layouts.sort_by_key(|a: &(String, String)| a.1.to_lowercase());
    (layouts, variants)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_layout_is_read() {
        let l = parse_localectl("   System Locale: LANG=de_DE.UTF-8\n       VC Keymap: de\n      X11 Layout: de\n     X11 Variant: nodeadkeys\n     X11 Options: caps:escape\n");
        assert_eq!(l, Layout { layout: "de".into(), variant: "nodeadkeys".into(), model: String::new(), options: "caps:escape".into() });
        assert_eq!(parse_localectl("").layout, "us", "nothing set: US, as the session takes it");
    }

    #[test]
    fn xkbs_lists_are_read() {
        let (l, v) = parse_xkb(
            "! model\n  pc105           Generic 105-key PC\n\n! layout\n  us              English (US)\n  de              German\n\n! variant\n  dvorak          us: English (Dvorak)\n  nodeadkeys      de: German (no dead keys)\n\n! option\n  grp             Switching\n",
        );
        assert_eq!(l, [("us".to_string(), "English (US)".to_string()), ("de".into(), "German".into())], "by name");
        assert_eq!(v[0], ("dvorak".to_string(), "us".to_string(), "English (Dvorak)".to_string()));
        assert_eq!(v.len(), 2, "options are not variants");
    }
}
