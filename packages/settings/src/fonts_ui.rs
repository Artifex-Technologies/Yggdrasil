//! Settings > Appearance > Fonts: the system font for text and for code (yggdrasil_conf::fonts), each choice with a line of
//! text in it, the text size (the same setting as Accessibility's), and the login screen's font; then, like Windows 11's
//! Fonts page, adding fonts (dropped on the page, or chosen with the file chooser) and the installed fonts as cards with a
//! sample, searchable, those you added removable (fontlib.rs: every file goes through Heimdall's file guard first).
//!
//! Your own choice is your fontconfig file, written here at once (no password): Yggdrasil's apps change within a second, the
//! panel at once (it is told through appearance.conf), other programs as they start. The login screen reads the computer's
//! file, written through pkexec by /usr/lib/yggdrasil/fontconf.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};

use eframe::egui::{self, vec2, CornerRadius, RichText, Sense, Stroke};
use yggdrasil_conf::fonts::{self, Choice, Prefs};
use yggdrasil_ui::fonts::{has_family, preview_name, Found};
use yggdrasil_ui::theme::Palette;

use crate::fontlib::{self, Family, Incoming};
use crate::{ui, App};

/// The cards' sample line.
pub const CARD_SAMPLE: &str = "The quick brown fox jumps";

/// Adding fonts, done on a thread of its own: the file guard and ClamAV can take a while.
#[derive(Default)]
pub struct Import {
    pub busy: bool,
    /// Files that were not installed at once: warned about (they can be installed by choice) or not taken (and why).
    pub items: Vec<Incoming>,
    pub note: Option<String>,
    /// Something was installed or removed: read the list again.
    pub changed: bool,
}

/// The cards' samples, drawn on a thread of their own (a font file can be many megabytes): asked for by family, a coverage
/// mask when done (None: nothing to draw).
#[derive(Default)]
pub struct Samples {
    pub done: HashMap<String, Option<(usize, usize, Vec<u8>)>>,
    asked: std::collections::HashSet<String>,
    tx: Option<mpsc::Sender<(String, PathBuf, u32, f32)>>,
}

/// The page's state that outlives a frame.
#[derive(Default)]
pub struct Library {
    pub list: Option<Vec<Family>>,
    pub search: String,
    pub import: Arc<Mutex<Import>>,
    pub samples: Arc<Mutex<Samples>>,
    pub textures: HashMap<String, Option<egui::TextureHandle>>,
    /// The file guard can check new files (None: not looked yet).
    pub guard: Option<Result<(), fontlib::GuardMissing>>,
    /// Files are being dragged over the window.
    pub hover: bool,
    /// Files dropped on the window, for this page.
    pub dropped: Vec<PathBuf>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Checks and installs `paths` (fontlib::bring_in, then install for those with nothing against them).
fn start_import(shared: Arc<Mutex<Import>>, paths: Vec<PathBuf>, ctx: egui::Context) {
    {
        let mut st = lock(&shared);
        if st.busy || paths.is_empty() {
            return;
        }
        st.busy = true;
        st.note = None;
        st.items.clear();
    }
    std::thread::spawn(move || {
        let dir = fontlib::user_dir();
        fontlib::clean_staging();
        let result = fontlib::staging_dir().map_err(|e| e.to_string()).and_then(|stage| fontlib::bring_in(fontlib::GUARD, &paths, &stage));
        let mut st = lock(&shared);
        match result {
            Err(e) => st.note = Some(e),
            Ok(items) => {
                let mut done = vec![];
                for inc in items {
                    if inc.clean() {
                        match fontlib::install(&inc, &dir) {
                            Ok(_) => done.push(inc.family.clone().unwrap_or(inc.name.clone())),
                            Err(e) => st.items.push(Incoming { problem: Some(e), ..inc }),
                        }
                    } else {
                        st.items.push(inc);
                    }
                }
                if !done.is_empty() {
                    fontlib::refresh_cache(&dir);
                    done.dedup();
                    st.note = Some(format!("Installed: {}.", done.join(", ")));
                    st.changed = true;
                } else if st.items.is_empty() {
                    st.note = Some("Nothing was installed.".into());
                }
            }
        }
        st.busy = false;
        ctx.request_repaint();
    });
}

/// A file the guard warned about, installed because you chose to.
fn install_anyway(shared: Arc<Mutex<Import>>, index: usize, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut st = lock(&shared);
        if index >= st.items.len() {
            return;
        }
        let inc = st.items.remove(index);
        let dir = fontlib::user_dir();
        match fontlib::install(&inc, &dir) {
            Ok(_) => {
                fontlib::refresh_cache(&dir);
                st.note = Some(format!("Installed: {}.", inc.family.as_deref().unwrap_or(&inc.name)));
                st.changed = true;
            }
            Err(e) => st.note = Some(format!("{}: {e}", inc.name)),
        }
        ctx.request_repaint();
    });
}

fn remove_family(shared: Arc<Mutex<Import>>, family: Family, ctx: egui::Context) {
    std::thread::spawn(move || {
        let r = fontlib::remove(&family, &fontlib::user_dir());
        let mut st = lock(&shared);
        st.note = Some(match r {
            Ok(()) => format!("Removed {}.", family.name),
            Err(e) => format!("{} was not removed: {e}", family.name),
        });
        st.changed = true;
        ctx.request_repaint();
    });
}

/// The desktop's file chooser (the FileChooser portal), for font files and archives; then they are added.
fn browse(shared: Arc<Mutex<Import>>, ctx: egui::Context) {
    std::thread::spawn(move || {
        if let Some(paths) = pick_fonts() {
            start_import(shared, paths, ctx);
        }
    });
}

fn pick_fonts() -> Option<Vec<PathBuf>> {
    use zbus::zvariant::{OwnedValue, Value};
    let conn = zbus::blocking::Connection::session().ok()?;
    let token = format!("settings_fonts_{}", std::process::id());
    let sender = conn.unique_name()?.trim_start_matches(':').replace('.', "_");
    let request = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let proxy = zbus::blocking::Proxy::new(&conn, "org.freedesktop.portal.Desktop", request, "org.freedesktop.portal.Request").ok()?;
    let mut answers = proxy.receive_signal("Response").ok()?;
    let mut globs: Vec<(u32, String)> = vec![];
    for k in fontlib::KINDS.iter().chain(&["zip"]) {
        globs.push((0, format!("*.{k}")));
        globs.push((0, format!("*.{}", k.to_uppercase())));
    }
    let globs: Vec<(u32, &str)> = globs.iter().map(|(n, g)| (*n, g.as_str())).collect();
    let filters: Vec<(&str, Vec<(u32, &str)>)> = vec![("Fonts and .zip archives of fonts", globs)];
    let mut opts: HashMap<&str, Value> = HashMap::new();
    opts.insert("handle_token", Value::from(token.as_str()));
    opts.insert("multiple", Value::from(true));
    opts.insert("filters", Value::from(filters));
    conn.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.FileChooser"),
        "OpenFile",
        &("", "Install fonts", opts),
    )
    .ok()?;
    let msg = answers.next()?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize().ok()?;
    if code != 0 {
        return None;
    }
    let uris: Vec<String> = results.get("uris").and_then(|v| Vec::<String>::try_from(v.clone()).ok())?;
    Some(yggdrasil_ui::dnd::parse_uri_list(&uris.join("\n")))
}

/// Asks for a family's sample (once); the worker draws it at `px` pixels.
fn want_sample(samples: &Arc<Mutex<Samples>>, f: &Family, px: f32, ctx: &egui::Context) {
    let mut s = lock(samples);
    if s.asked.contains(&f.name) {
        return;
    }
    s.asked.insert(f.name.clone());
    let Some((file, index)) = f.sample.clone() else {
        s.done.insert(f.name.clone(), None);
        return;
    };
    if s.tx.is_none() {
        let (tx, rx) = mpsc::channel::<(String, PathBuf, u32, f32)>();
        let (shared, ctx) = (samples.clone(), ctx.clone());
        std::thread::spawn(move || {
            for (name, file, index, px) in rx {
                // a font file above the size the library takes is not read for a sample either
                let small = std::fs::metadata(&file).is_ok_and(|m| m.len() <= fontlib::MAX_BYTES);
                let mask =
                    small.then(|| std::fs::read(&file).ok()).flatten().and_then(|b| fontlib::render_sample(&b, index, CARD_SAMPLE, px));
                lock(&shared).done.insert(name, mask);
                ctx.request_repaint();
            }
        });
        s.tx = Some(tx);
    }
    if let Some(tx) = &s.tx {
        let _ = tx.send((f.name.clone(), file, index, px));
    }
}

/// One installed family as a card: its sample, its name, how many styles, and "..." (Remove, for one you added).
fn card(ui: &mut egui::Ui, pal: &Palette, lib: &mut Library, f: &Family, width: f32) -> Option<Family> {
    let mut remove = None;
    let (rect, _) = ui.allocate_exact_size(vec2(width, 104.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    ui.painter().rect_filled(rect, CornerRadius::same(10), pal.field);
    if !lib.textures.contains_key(&f.name) {
        want_sample(&lib.samples, f, 22.0 * ppp, &ctx);
        if let Some(done) = lock(&lib.samples).done.remove(&f.name) {
            let tex = done.map(|(w, h, mask)| {
                let img = egui::ColorImage::new([w, h], mask.into_iter().map(egui::Color32::from_white_alpha).collect());
                ctx.load_texture(format!("font-sample-{}", f.name), img, egui::TextureOptions::LINEAR)
            });
            lib.textures.insert(f.name.clone(), tex);
        }
    }
    let inner = rect.shrink2(vec2(14.0, 10.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    match lib.textures.get(&f.name) {
        Some(Some(tex)) => {
            // as much of the line as the card has room for (the rest cut off, not squeezed)
            let size = tex.size_vec2() / ppp;
            let w = size.x.min(inner.width());
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(w / size.x.max(1.0), 1.0));
            child.add(egui::Image::new(tex).uv(uv).maintain_aspect_ratio(false).fit_to_exact_size(vec2(w, size.y)).tint(pal.text));
        }
        Some(None) => {
            child.label(RichText::new("No preview").size(16.0).color(pal.dim));
        }
        None => {
            child.label(RichText::new("…").size(16.0).color(pal.dim));
        }
    }
    child.add_space(4.0);
    child.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(&f.name).size(14.0));
            let faces = if f.faces == 1 { "1 style".to_string() } else { format!("{} styles", f.faces) };
            ui.label(RichText::new(if f.yours { format!("{faces} · added by you") } else { faces }).size(12.0).color(pal.dim));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(RichText::new("···").size(16.0), |ui| {
                if f.yours {
                    if ui.button("Remove").clicked() {
                        remove = Some(f.clone());
                        ui.close();
                    }
                } else {
                    ui.add_enabled(false, egui::Button::new("Remove"))
                        .on_disabled_hover_text("Comes with the system: removed with its package, not here");
                }
            });
        });
    });
    remove
}

pub const SANS_SAMPLE: &str = "Reading is easier when Il1 and O0 look different.";
pub const MONO_SAMPLE: &str = "let total = items.len() + 10; // Il1 O0";

/// What the two files say, and what is in effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Now {
    pub mine: Prefs,
    pub computer: Prefs,
}

impl Now {
    pub fn read() -> Now {
        Now { mine: fonts::load_from(&fonts::user_path()), computer: fonts::load_from(std::path::Path::new(fonts::SYSTEM_PATH)) }
    }

    pub fn effective(&self) -> Prefs {
        self.mine.over(self.computer)
    }
}

/// Your file after choosing `id` (a text font when `mono` is false): the other family as it was.
pub fn choose(mine: Prefs, id: &'static str, mono: bool) -> Prefs {
    if mono {
        Prefs { mono: Some(id), ..mine }
    } else {
        Prefs { sans: Some(id), ..mine }
    }
}

/// The helper's arguments for the login screen to match `eff`: a default is "-" (no file is needed for it).
pub fn login_args(eff: &Prefs) -> [&'static str; 2] {
    let s = eff.sans_choice();
    let m = eff.mono_choice();
    [if *s == fonts::SANS[0] { "-" } else { s.id }, if *m == fonts::MONO[0] { "-" } else { m.id }]
}

/// Whether the login screen already shows `eff`.
pub fn login_matches(computer: &Prefs, eff: &Prefs) -> bool {
    computer.sans_choice() == eff.sans_choice() && computer.mono_choice() == eff.mono_choice()
}

/// What was clicked on the page.
enum Click {
    Use(&'static str, bool),
    Login,
}

fn list(ui: &mut egui::Ui, pal: &Palette, found: &[Found], choices: &[Choice], current: &Choice, mono: bool) -> Option<Click> {
    let mut click = None;
    for (i, c) in choices.iter().enumerate() {
        let here = found.iter().find(|f| f.choice == c).is_some_and(|f| f.file.is_some());
        let note = if here { c.note.to_string() } else { format!("{} Not installed.", c.note) };
        ui::row_with(
            ui,
            pal,
            i == 0,
            |ui| {
                ui.label(RichText::new(c.name).size(14.0));
                let family = preview_name(c.id);
                if here && has_family(ui.ctx(), &family) {
                    let sample = if mono { MONO_SAMPLE } else { SANS_SAMPLE };
                    ui.add(egui::Label::new(RichText::new(sample).size(17.0).family(egui::FontFamily::Name(family.into()))).wrap());
                }
            },
            Some(&note),
            |ui| {
                if c == current {
                    ui.label(RichText::new("In use").color(pal.accent));
                } else if here && ui::action(ui, pal, "Use").clicked() {
                    click = Some(Click::Use(c.id, mono));
                }
            },
        );
    }
    click
}

impl App {
    pub(crate) fn font_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        // which fonts are here, and a family for each one's line of text: once per visit (one fc-match each, side by side)
        if self.font_found.is_none() {
            let found = yggdrasil_ui::fonts::find_choices();
            yggdrasil_ui::fonts::show_previews(ctx, &found);
            self.font_found = Some(found);
        }
        let found = self.font_found.clone().unwrap_or_default();
        let now = Now::read();
        let eff = now.effective();
        let mut click = None;
        ui::group(ui, pal, Some("Text"), |ui| {
            click = list(ui, pal, &found, fonts::SANS, eff.sans_choice(), false).or(click.take());
        });
        ui::group(ui, pal, Some("Code and terminals"), |ui| {
            click = list(ui, pal, &found, fonts::MONO, eff.mono_choice(), true).or(click.take());
        });
        let mut conf = yggdrasil_conf::PanelConf::load();
        ui::group(ui, pal, Some("Size"), |ui| {
            let sizes = ["100%", "125%", "150%", "200%"];
            let cur = match conf.text_scale {
                0..=112 => 0,
                113..=137 => 1,
                138..=174 => 2,
                _ => 3,
            };
            ui::row(
                ui,
                pal,
                true,
                "Text and control size",
                Some("Yggdrasil's own apps. The same setting as Accessibility > Text and control size"),
                |ui| {
                    if let Some(i) = ui::segmented(ui, pal, &sizes, cur) {
                        conf.text_scale = [100, 125, 150, 200][i];
                        let _ = conf.save();
                    }
                },
            );
        });
        let same = login_matches(&now.computer, &eff);
        ui::group(ui, pal, Some("Login screen"), |ui| {
            let shows = format!(
                "It shows {} and {}.{}",
                now.computer.sans_choice().name,
                now.computer.mono_choice().name,
                if same { "" } else { " Use asks for an administrator's password" }
            );
            ui::row(ui, pal, true, "Use these fonts on the login screen", Some(&shows), |ui| {
                if same {
                    ui.label(RichText::new("In use").color(pal.accent));
                } else if ui::action(ui, pal, "Use").clicked() {
                    click = Some(Click::Login);
                }
            });
        });
        ui.label(
            RichText::new("Yggdrasil's apps and the top bar change at once; other programs when they start again.")
                .size(12.5)
                .color(pal.dim),
        );
        ui.add_space(14.0);
        self.font_library(ui, pal, ctx);
        match click {
            Some(Click::Use(id, mono)) => {
                let mine = choose(now.mine, id, mono);
                let after = mine.over(now.computer);
                // a tiny file, written at once; the note in appearance.conf tells the panel
                self.status = match fonts::save_user(&mine)
                    .and_then(|_| yggdrasil_conf::appearance::note_fonts(after.sans_choice().id, after.mono_choice().id))
                {
                    Ok(()) => String::new(),
                    Err(e) => format!("The font could not be saved: {e}"),
                };
                ctx.request_repaint();
            }
            Some(Click::Login) => {
                let [s, m] = login_args(&eff);
                self.change(ctx, move || crate::sys::act("pkexec", &["/usr/lib/yggdrasil/fontconf", "set", s, m]));
            }
            None => {}
        }
    }
}

impl App {
    /// Adding fonts and the installed ones (the second half of the page).
    fn font_library(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let lib = &mut self.font_lib;
        if lib.guard.is_none() {
            lib.guard = Some(fontlib::guard_ready(fontlib::GUARD));
        }
        let changed = std::mem::take(&mut lock(&lib.import).changed);
        if changed || lib.list.is_none() {
            lib.list = Some(fontlib::installed());
            if changed {
                // the choices at the top may have gained or lost a font
                self.font_found = None;
            }
        }
        let ready = matches!(lib.guard, Some(Ok(())));
        let dropped = std::mem::take(&mut lib.dropped);
        if !dropped.is_empty() && ready {
            start_import(lib.import.clone(), dropped, ctx.clone());
        }
        let (busy, note, items) = {
            let st = lock(&lib.import);
            (st.busy, st.note.clone(), st.items.clone())
        };
        let mut browse_now = false;
        let mut anyway = None;
        let mut clear = false;
        ui.label(RichText::new("Add fonts").size(13.0).color(pal.dim));
        ui.add_space(4.0);
        let (zone, _) = ui.allocate_exact_size(vec2(ui.available_width(), 116.0), Sense::hover());
        let hot = lib.hover && ready;
        ui.painter().rect(
            zone,
            CornerRadius::same(12),
            if hot { pal.select } else { pal.field },
            Stroke::new(if hot { 2.0 } else { 1.0 }, if hot { pal.accent } else { pal.border }),
            egui::StrokeKind::Inside,
        );
        let mut z = ui.new_child(egui::UiBuilder::new().max_rect(zone.shrink(14.0)).layout(egui::Layout::top_down(egui::Align::Center)));
        if ready {
            z.label(RichText::new("Drag and drop to install").size(17.0));
            z.label(
                RichText::new(
                    "TrueType, OpenType, collections and WOFF2 fonts, or a .zip of them. Heimdall's file guard checks each file first.",
                )
                .size(12.0)
                .color(pal.dim),
            );
            z.add_space(6.0);
            z.add_enabled_ui(!busy, |ui| {
                if ui::action(ui, pal, "Browse and install fonts").clicked() {
                    browse_now = true;
                }
            });
        } else {
            let why = match lib.guard {
                Some(Err(fontlib::GuardMissing::TooOld)) => {
                    "Installing new fonts needs a newer Heimdall privacy package (its file guard cannot check fonts yet)."
                }
                _ => "Installing new fonts needs Heimdall's file guard (heimdall-fileguard), which checks every file first.",
            };
            z.add_space(24.0);
            z.add(egui::Label::new(RichText::new(why).color(pal.amber)).wrap());
        }
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Checking the files…").color(pal.dim));
            });
        }
        if let Some(n) = &note {
            ui.add(egui::Label::new(RichText::new(n).size(12.5)).wrap());
        }
        if !items.is_empty() {
            ui::group(ui, pal, None, |ui| {
                for (i, inc) in items.iter().enumerate() {
                    let (what, color) = match (&inc.problem, &inc.verdict) {
                        (Some(p), _) => (format!("Not installed: {p}"), pal.red),
                        (None, Some(v)) => (format!("Heimdall warns: {}", v.words()), pal.amber),
                        _ => (String::new(), pal.dim),
                    };
                    let title = match &inc.family {
                        Some(f) => format!("{} ({f})", inc.name),
                        None => inc.name.clone(),
                    };
                    ui::row_with(
                        ui,
                        pal,
                        i == 0,
                        |ui| {
                            ui.add(egui::Label::new(RichText::new(&title).size(14.0)).wrap());
                            ui.add(egui::Label::new(RichText::new(&what).size(12.0).color(color)).wrap());
                        },
                        None,
                        |ui| {
                            if inc.warned() && ui::quiet(ui, pal, "Install anyway").clicked() {
                                anyway = Some(i);
                            }
                        },
                    );
                }
                if ui::quiet(ui, pal, "Done").clicked() {
                    clear = true;
                }
            });
        }
        ui.add_space(14.0);
        let list = lib.list.clone().unwrap_or_default();
        let mut remove = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Installed fonts ({})", list.len())).size(13.0).color(pal.dim));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::TextEdit::singleline(&mut lib.search).hint_text("Search fonts").desired_width(220.0));
            });
        });
        ui.add_space(6.0);
        let shown = fontlib::search(&list, &lib.search);
        let gap = 10.0;
        let cols = ((ui.available_width() + gap) / (240.0 + gap)).floor().max(1.0) as usize;
        let width = (ui.available_width() - gap * (cols - 1) as f32) / cols as f32;
        for row in shown.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for f in row {
                    if let Some(r) = card(ui, pal, lib, f, width) {
                        remove = Some(r);
                    }
                }
            });
            ui.add_space(4.0);
        }
        if shown.is_empty() {
            ui.label(RichText::new("No font by that name.").color(pal.dim));
        }
        if browse_now {
            browse(lib.import.clone(), ctx.clone());
        }
        if let Some(i) = anyway {
            install_anyway(lib.import.clone(), i, ctx.clone());
        }
        if clear {
            let mut st = lock(&lib.import);
            st.items.clear();
            st.note = None;
        }
        if let Some(f) = remove {
            remove_family(lib.import.clone(), f, ctx.clone());
        }
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choosing_keeps_the_other_family() {
        let mine = Prefs { sans: None, mono: Some("opendyslexic-mono") };
        assert_eq!(choose(mine, "lexend", false), Prefs { sans: Some("lexend"), mono: Some("opendyslexic-mono") });
        assert_eq!(choose(mine, "noto-sans-mono", true), Prefs { sans: None, mono: Some("noto-sans-mono") });
    }

    #[test]
    fn the_login_screen_gets_what_is_in_effect() {
        let now = Now { mine: Prefs { sans: Some("atkinson-hyperlegible"), mono: None }, computer: Prefs::default() };
        let eff = now.effective();
        assert_eq!(login_args(&eff), ["atkinson-hyperlegible", "-"]);
        assert!(!login_matches(&now.computer, &eff));
        // your own Noto Sans over the computer's OpenDyslexic: the login screen goes back to the default (no file)
        let now = Now { mine: Prefs { sans: Some("noto-sans"), mono: None }, computer: Prefs { sans: Some("opendyslexic"), mono: None } };
        assert_eq!(login_args(&now.effective()), ["-", "-"]);
        assert!(!login_matches(&now.computer, &now.effective()));
        // the computer's choice with nothing of your own: already the same
        let now = Now { mine: Prefs::default(), computer: Prefs { sans: Some("lexend"), mono: Some("atkinson-hyperlegible-mono") } };
        assert!(login_matches(&now.computer, &now.effective()));
        assert_eq!(login_args(&now.effective()), ["lexend", "atkinson-hyperlegible-mono"]);
    }

    #[test]
    fn the_helper_takes_what_the_page_sends() {
        for s in fonts::SANS {
            for m in fonts::MONO {
                let [a, b] = login_args(&Prefs { sans: Some(s.id), mono: Some(m.id) });
                assert!(Prefs::from_ids(a, b).is_some(), "{a} {b}");
            }
        }
    }
}
