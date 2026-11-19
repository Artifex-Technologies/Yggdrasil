//! Settings > Appearance > Fonts, like Windows 11's Fonts page: adding fonts first (dropped on the page, or chosen with the file
//! chooser; fontlib.rs: every file goes through Heimdall's file guard first), then the system font as two rows, Text and Code
//! and terminals, each showing the font in use in its own letters and opening a page of its own (Fonts > Text) with the
//! choices (yggdrasil_conf::fonts), each with a line of text in it; then the installed fonts as cards with a sample,
//! searchable, those you added removable.
//!
//! The font chosen is the system font: Use writes your own fontconfig file at once (no password; Yggdrasil's apps change
//! within a second, the panel at once through appearance.conf, other programs as they start) and the computer's file, which
//! the login screen reads, through pkexec and /usr/lib/yggdrasil/fontconf (an administrator's password, kept a few minutes).
//! Without the password your own choice still applies, and a note says the login screen keeps its font until it is set again.

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
    pub tools: Tools,
    pub login: Arc<Mutex<Login>>,
    pub heal: Arc<Mutex<Heal>>,
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

/// The two rows' titles, which are also their pages' names in the path above the page.
pub const TEXT_TITLE: &str = "Text";
pub const CODE_TITLE: &str = "Code and terminals";

/// The computer's half of a choice: the helper run through pkexec.
pub const HELPER: &str = "/usr/lib/yggdrasil/fontconf";

/// The two kinds of system font, each a row on the Fonts page and a page of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Code,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::Text => TEXT_TITLE,
            Kind::Code => CODE_TITLE,
        }
    }

    /// What it is used for (the line at the top of its page).
    pub fn what(self) -> &'static str {
        match self {
            Kind::Text => "The font of menus, windows, documents and web pages, here and on the login screen.",
            Kind::Code => "The font of terminals and code editors, where every letter is as wide as the next.",
        }
    }

    pub fn choices(self) -> &'static [Choice] {
        match self {
            Kind::Text => fonts::SANS,
            Kind::Code => fonts::MONO,
        }
    }

    pub fn sample(self) -> &'static str {
        match self {
            Kind::Text => SANS_SAMPLE,
            Kind::Code => MONO_SAMPLE,
        }
    }

    /// The choice in effect for this kind.
    pub fn in_use(self, eff: &Prefs) -> &'static Choice {
        match self {
            Kind::Text => eff.sans_choice(),
            Kind::Code => eff.mono_choice(),
        }
    }
}

/// The programs and files the page uses: the real ones, or stand-ins in the tests.
#[derive(Clone, Debug)]
pub struct Tools {
    /// Your own fontconfig file and the computer's.
    pub mine: PathBuf,
    pub computer: PathBuf,
    /// pkexec, for the computer's file (None: not asked, as on a made-up system).
    pub pkexec: Option<String>,
    pub fc_list: String,
    pub fc_cache: String,
    /// Tell the panel through appearance.conf.
    pub tell_panel: bool,
}

impl Default for Tools {
    fn default() -> Tools {
        Tools {
            mine: fonts::user_path(),
            computer: PathBuf::from(fonts::SYSTEM_PATH),
            pkexec: crate::fixture::get().is_none().then(|| "pkexec".to_string()),
            fc_list: "fc-list".into(),
            fc_cache: "fc-cache".into(),
            tell_panel: true,
        }
    }
}

/// Setting the font on the login screen, on a thread of its own (the password box waits for an answer).
#[derive(Default)]
pub struct Login {
    pub busy: bool,
    /// The helper failed (not a cancelled password: that shows as the login screen's note).
    pub error: Option<String>,
}

/// Looking for a stale font cache, once per run of Settings (fontlib::heal).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Heal {
    #[default]
    NotYet,
    Checking,
    Done,
    /// Done, and the cache was made again: read the fonts again.
    Refreshed,
}

/// What the two files say, and what is in effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Now {
    pub mine: Prefs,
    pub computer: Prefs,
}

impl Now {
    pub fn read(t: &Tools) -> Now {
        Now { mine: fonts::load_from(&t.mine), computer: fonts::load_from(&t.computer) }
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

/// What Use does: your own file as it will be, and the helper's arguments for the computer's (None: it already shows that).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub mine: Prefs,
    pub login: Option<[&'static str; 2]>,
}

pub fn plan(now: &Now, id: &'static str, kind: Kind) -> Plan {
    let mine = choose(now.mine, id, kind == Kind::Code);
    let after = mine.over(now.computer);
    Plan { mine, login: (!login_matches(&now.computer, &after)).then(|| login_args(&after)) }
}

/// The note under the system font while the login screen shows something else (its password was not given): what it keeps.
pub fn login_note(now: &Now) -> Option<String> {
    let eff = now.effective();
    let mut kept = vec![];
    if now.computer.sans_choice() != eff.sans_choice() {
        kept.push(now.computer.sans_choice().name);
    }
    if now.computer.mono_choice() != eff.mono_choice() {
        kept.push(now.computer.mono_choice().name);
    }
    (!kept.is_empty()).then(|| format!("The login screen keeps {} until the font is set again.", kept.join(" and ")))
}

/// How the computer's half went.
#[derive(Clone, Debug, PartialEq)]
pub enum LoginResult {
    Done,
    /// The password was not given (pkexec's 126: dismissed, 127: not authorised).
    Cancelled,
    Failed(String),
}

/// The computer's file through pkexec and the helper.
pub fn set_login(pkexec: &str, args: [&str; 2]) -> LoginResult {
    let out = std::process::Command::new(pkexec)
        .args([HELPER, "set", args[0], args[1]])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();
    match out {
        Ok(o) if o.status.success() => LoginResult::Done,
        Ok(o) if matches!(o.status.code(), Some(126) | Some(127)) => LoginResult::Cancelled,
        Ok(o) => {
            let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
            LoginResult::Failed(if e.is_empty() { "The login screen's font could not be set.".into() } else { e })
        }
        Err(e) => LoginResult::Failed(format!("{pkexec}: {e}")),
    }
}

/// What was clicked on the page.
enum Click {
    Use(&'static str, Kind),
    SetAgain,
    Open(Kind),
}

/// The choices of one kind (its own page): name, a line in that font, what it was made for, and Use or In use.
fn list(ui: &mut egui::Ui, pal: &Palette, found: &[Found], kind: Kind, current: &Choice, looking: bool) -> Option<Click> {
    let mut click = None;
    for (i, c) in kind.choices().iter().enumerate() {
        let here = found.iter().find(|f| f.choice == c).is_some_and(|f| f.file.is_some());
        let note = if here {
            c.note.to_string()
        } else if looking && std::path::Path::new(c.file).exists() {
            format!("{}. Looking for it…", c.note.trim_end_matches('.'))
        } else {
            format!("{}. Not installed.", c.note.trim_end_matches('.'))
        };
        ui::row_with(
            ui,
            pal,
            i == 0,
            |ui| {
                ui.label(RichText::new(c.name).size(14.0));
                let family = preview_name(c.id);
                if here && has_family(ui.ctx(), &family) {
                    ui.add(egui::Label::new(RichText::new(kind.sample()).size(17.0).family(egui::FontFamily::Name(family.into()))).wrap());
                }
            },
            Some(&note),
            |ui| {
                if c == current {
                    ui.label(RichText::new("In use").color(pal.accent));
                } else if here && ui::action(ui, pal, "Use").clicked() {
                    click = Some(Click::Use(c.id, kind));
                }
            },
        );
    }
    click
}

/// A row on the Fonts page: the kind, and the font in use drawn in its own letters.
fn kind_row(ui: &mut egui::Ui, pal: &Palette, first: bool, kind: Kind, current: &Choice) -> bool {
    ui::nav_row_with(
        ui,
        pal,
        first,
        |ui| {
            ui.label(RichText::new(kind.title()).size(14.0));
            let family = preview_name(current.id);
            let name = RichText::new(current.name).size(17.0);
            if has_family(ui.ctx(), &family) {
                ui.label(name.family(egui::FontFamily::Name(family.into())));
            } else {
                ui.label(name.color(pal.dim));
            }
        },
        None,
        |_| {},
    )
}

impl App {
    pub(crate) fn font_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let heal = *lock(&self.font_lib.heal);
        if heal == Heal::Refreshed {
            // fontconfig's cache was made again: the choices and the cards both read anew
            *lock(&self.font_lib.heal) = Heal::Done;
            self.font_found = None;
            self.font_lib.list = None;
            self.font_lib.textures.clear();
        }
        if heal == Heal::NotYet {
            self.start_heal(ctx);
        }
        // which fonts are here, and a family for each one's line of text: once per visit (one fc-match each, side by side)
        if self.font_found.is_none() {
            let found = yggdrasil_ui::fonts::find_choices();
            yggdrasil_ui::fonts::show_previews(ctx, &found);
            self.font_found = Some(found);
        }
        let found = self.font_found.clone().unwrap_or_default();
        let tools = self.font_lib.tools.clone();
        let now = Now::read(&tools);
        let eff = now.effective();
        let mut click = None;
        match self.app_sub {
            crate::AppSub::FontText | crate::AppSub::FontCode => {
                let kind = if self.app_sub == crate::AppSub::FontText { Kind::Text } else { Kind::Code };
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(kind.what()).color(pal.dim)).wrap());
                let looking = matches!(*lock(&self.font_lib.heal), Heal::NotYet | Heal::Checking);
                ui::group(ui, pal, None, |ui| {
                    click = list(ui, pal, &found, kind, kind.in_use(&eff), looking);
                });
                click = self.login_lines(ui, pal, &now).or(click);
            }
            _ => {
                self.library_state(ctx);
                self.add_fonts(ui, pal, ctx);
                ui::group(ui, pal, Some("System font"), |ui| {
                    for (i, kind) in [Kind::Text, Kind::Code].into_iter().enumerate() {
                        if kind_row(ui, pal, i == 0, kind, kind.in_use(&eff)) {
                            click = Some(Click::Open(kind));
                        }
                    }
                });
                click = self.login_lines(ui, pal, &now).or(click);
                ui.add_space(14.0);
                self.installed_fonts(ui, pal, ctx);
            }
        }
        match click {
            Some(Click::Use(id, kind)) => self.use_font(ctx, &now, id, kind),
            Some(Click::SetAgain) => {
                let args = login_args(&eff);
                self.start_login(ctx, args);
            }
            Some(Click::Open(Kind::Text)) => self.app_sub = crate::AppSub::FontText,
            Some(Click::Open(Kind::Code)) => self.app_sub = crate::AppSub::FontCode,
            None => {}
        }
    }

    /// Under the system font: who follows the choice, and the login screen's state (being set, or keeping its old font).
    fn login_lines(&mut self, ui: &mut egui::Ui, pal: &Palette, now: &Now) -> Option<Click> {
        let (busy, error) = {
            let l = lock(&self.font_lib.login);
            (l.busy, l.error.clone())
        };
        ui::footnote(
            ui,
            pal,
            "The font you choose is the system font: Yggdrasil's apps and the top bar change at once, other programs when they \
             start again, the login screen too (it asks for an administrator's password).",
        );
        let mut click = None;
        if busy {
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.spinner();
                ui.label(RichText::new("Setting it on the login screen too…").size(12.5).color(pal.dim));
            });
        } else if let Some(note) = login_note(now) {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new(note).size(12.5).color(pal.amber));
                if ui::quiet(ui, pal, "Set again").clicked() {
                    click = Some(Click::SetAgain);
                }
            });
        }
        if let Some(e) = error {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new(e).size(12.5).color(pal.red));
            });
        }
        click
    }

    /// Use: your own file at once (and the panel told), then the computer's through pkexec when it shows something else.
    fn use_font(&mut self, ctx: &egui::Context, now: &Now, id: &'static str, kind: Kind) {
        let tools = self.font_lib.tools.clone();
        let p = plan(now, id, kind);
        let after = p.mine.over(now.computer);
        let saved = fonts::save_to(&tools.mine, &p.mine).and_then(|_| {
            if tools.tell_panel {
                yggdrasil_conf::appearance::note_fonts(after.sans_choice().id, after.mono_choice().id)
            } else {
                Ok(())
            }
        });
        if let Err(e) = saved {
            self.status = format!("The font could not be saved: {e}");
            return;
        }
        self.status.clear();
        if let Some(args) = p.login {
            self.start_login(ctx, args);
        }
        ctx.request_repaint();
    }

    fn start_login(&mut self, ctx: &egui::Context, args: [&'static str; 2]) {
        let Some(pkexec) = self.font_lib.tools.pkexec.clone() else { return };
        let shared = self.font_lib.login.clone();
        {
            let mut l = lock(&shared);
            if l.busy {
                return;
            }
            l.busy = true;
            l.error = None;
        }
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = set_login(&pkexec, args);
            let mut l = lock(&shared);
            l.busy = false;
            // a cancelled password shows as the note (the computer's file still differs from yours)
            if let LoginResult::Failed(e) = r {
                l.error = Some(e);
            }
            ctx.request_repaint();
        });
    }

    /// Once per run: a font whose files are on disk but which fontconfig does not list means a stale cache (fontlib::heal).
    fn start_heal(&mut self, ctx: &egui::Context) {
        let shared = self.font_lib.heal.clone();
        *lock(&shared) = Heal::Checking;
        let tools = self.font_lib.tools.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = fontlib::heal(&tools.fc_list, &tools.fc_cache, |p| p.exists());
            *lock(&shared) = if matches!(r, fontlib::Healed::Refreshed(_)) { Heal::Refreshed } else { Heal::Done };
            ctx.request_repaint();
        });
    }
}

impl App {
    /// The library's state for this frame: the file guard looked for, the list read (again after a change), dropped files taken.
    fn library_state(&mut self, ctx: &egui::Context) {
        let lib = &mut self.font_lib;
        if lib.guard.is_none() {
            lib.guard = Some(fontlib::guard_ready(fontlib::GUARD));
        }
        let changed = std::mem::take(&mut lock(&lib.import).changed);
        if changed || lib.list.is_none() {
            lib.list = Some(fontlib::installed());
            if changed {
                // the choices may have gained or lost a font
                self.font_found = None;
            }
        }
        let ready = matches!(lib.guard, Some(Ok(())));
        let dropped = std::mem::take(&mut lib.dropped);
        if !dropped.is_empty() && ready {
            start_import(lib.import.clone(), dropped, ctx.clone());
        }
    }

    /// Adding fonts, at the top of the page: the drop area with "Browse and install fonts", then what was not installed.
    fn add_fonts(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let lib = &mut self.font_lib;
        let ready = matches!(lib.guard, Some(Ok(())));
        let (busy, note, items) = {
            let st = lock(&lib.import);
            (st.busy, st.note.clone(), st.items.clone())
        };
        let mut browse_now = false;
        let mut anyway = None;
        let mut clear = false;
        ui.add_space(14.0);
        ui.label(RichText::new("Add fonts").strong().color(pal.dim));
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
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// The installed fonts as cards, with a search box, at the bottom of the page.
    fn installed_fonts(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let lib = &mut self.font_lib;
        let list = lib.list.clone().unwrap_or_default();
        let mut remove = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Installed fonts ({})", list.len())).strong().color(pal.dim));
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
        if let Some(f) = remove {
            remove_family(lib.import.clone(), f, ctx.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

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

    #[test]
    fn use_writes_your_file_and_the_computers() {
        // nothing chosen anywhere: Lexend for text is yours and the login screen's
        let now = Now { mine: Prefs::default(), computer: Prefs::default() };
        let p = plan(&now, "lexend", Kind::Text);
        assert_eq!(p, Plan { mine: Prefs { sans: Some("lexend"), mono: None }, login: Some(["lexend", "-"]) });
        // a code font keeps the text font you have, and the computer gets both
        let now = Now { mine: Prefs { sans: Some("lexend"), mono: None }, computer: Prefs { sans: Some("lexend"), mono: None } };
        let p = plan(&now, "opendyslexic-mono", Kind::Code);
        assert_eq!(p.mine, Prefs { sans: Some("lexend"), mono: Some("opendyslexic-mono") });
        assert_eq!(p.login, Some(["lexend", "opendyslexic-mono"]));
        // the login screen already shows it (the installer wrote it): no password asked
        let now = Now { mine: Prefs::default(), computer: Prefs { sans: Some("atkinson-hyperlegible-next"), mono: None } };
        let p = plan(&now, "atkinson-hyperlegible-next", Kind::Text);
        assert_eq!(p.login, None);
        // back to Noto Sans: the computer's file goes (both "-")
        let now = Now { mine: Prefs { sans: Some("lexend"), mono: None }, computer: Prefs { sans: Some("lexend"), mono: None } };
        assert_eq!(plan(&now, "noto-sans", Kind::Text).login, Some(["-", "-"]));
    }

    #[test]
    fn the_note_says_what_the_login_screen_keeps() {
        let same = Now { mine: Prefs { sans: Some("lexend"), mono: None }, computer: Prefs { sans: Some("lexend"), mono: None } };
        assert_eq!(login_note(&same), None);
        let text = Now { mine: Prefs { sans: Some("lexend"), mono: None }, computer: Prefs::default() };
        assert_eq!(login_note(&text).as_deref(), Some("The login screen keeps Noto Sans until the font is set again."));
        let both = Now { mine: Prefs { sans: Some("lexend"), mono: Some("opendyslexic-mono") }, computer: Prefs::default() };
        assert_eq!(login_note(&both).as_deref(), Some("The login screen keeps Noto Sans and Noto Sans Mono until the font is set again."));
    }

    /// A scratch folder with a stand-in pkexec that logs its arguments and answers `code` (0 also copies your file over the
    /// computer's, as the helper would write the same choice).
    fn scratch(tag: &str, code: i32) -> (PathBuf, Tools) {
        let dir = std::env::temp_dir().join(format!("ygg-fonts-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (mine, computer, log) = (dir.join("mine.conf"), dir.join("computer.conf"), dir.join("pkexec.log"));
        let copy = if code == 0 { format!("cp '{}' '{}'\n", mine.display(), computer.display()) } else { String::new() };
        let script = dir.join("pkexec");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> '{}'\n{copy}echo refused >&2\nexit {code}\n", log.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tools = Tools {
            mine,
            computer,
            pkexec: Some(script.display().to_string()),
            fc_list: "false".into(),
            fc_cache: "false".into(),
            tell_panel: false,
        };
        (dir, tools)
    }

    #[test]
    fn pkexecs_answers() {
        for (code, want) in [
            (0, LoginResult::Done),
            (126, LoginResult::Cancelled),
            (127, LoginResult::Cancelled),
            (1, LoginResult::Failed("refused".into())),
        ] {
            let (dir, t) = scratch(&format!("answer-{code}"), code);
            assert_eq!(set_login(t.pkexec.as_deref().unwrap(), ["lexend", "-"]), want, "{code}");
            assert_eq!(std::fs::read_to_string(dir.join("pkexec.log")).unwrap(), format!("{HELPER} set lexend -\n"));
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    // ------------------------------------------------------------------ the page itself, driven headless

    struct Page {
        ctx: egui::Context,
        app: App,
    }

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 2400.0))),
            events,
            ..Default::default()
        }
    }

    impl Page {
        fn new(tools: Tools) -> Page {
            let mut app = App::bare();
            app.page = crate::Page::Appearance;
            app.app_sub = crate::AppSub::Font;
            // every font here (no previews drawn: their names show in the plain font), no cards, the guard ready, the cache fine
            let all = fonts::SANS.iter().chain(fonts::MONO);
            app.font_found = Some(all.map(|c| Found { choice: c, file: Some(PathBuf::from(c.file)) }).collect());
            app.font_lib.list = Some(vec![]);
            app.font_lib.guard = Some(Ok(()));
            *lock(&app.font_lib.heal) = Heal::Done;
            app.font_lib.tools = tools;
            Page { ctx: egui::Context::default(), app }
        }

        /// One frame: the texts drawn, top to bottom, with where they are.
        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
            let pal = yggdrasil_ui::theme::palette(true);
            let app = &mut self.app;
            let out = self.ctx.run_ui(input(events), |ui| {
                let ctx = ui.ctx().clone();
                app.font_page(ui, &pal, &ctx);
            });
            fn walk(s: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
                match s {
                    egui::Shape::Text(t) => out.push((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()))),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                    _ => {}
                }
            }
            let mut texts = vec![];
            for s in &out.shapes {
                walk(&s.shape, &mut texts);
            }
            out.drop_without_applying_deltas();
            texts.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
            texts
        }

        /// A click on the `nth` text that reads `label`, top to bottom.
        fn click(&mut self, label: &str, nth: usize) {
            let texts = self.frame(vec![]);
            let at = texts.iter().filter(|(t, _)| t == label).nth(nth).unwrap_or_else(|| panic!("no {label:?} #{nth}")).1.center();
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![egui::Event::PointerMoved(at)]);
            self.frame(vec![button(true)]);
            self.frame(vec![button(false)]);
        }

        fn wait_for_login(&self) {
            for _ in 0..200 {
                if !lock(&self.app.font_lib.login).busy {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("the login screen's thread did not finish");
        }
    }

    fn order(texts: &[(String, egui::Rect)], want: &[&str]) {
        let names: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        let at: Vec<usize> = want.iter().map(|w| names.iter().position(|n| n == w).unwrap_or_else(|| panic!("{w:?} not drawn"))).collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{want:?} not in that order: {names:?}");
    }

    #[test]
    fn the_fonts_page_adds_first_then_the_two_rows_then_the_cards() {
        let (dir, t) = scratch("page", 0);
        let mut p = Page::new(t);
        let texts = p.frame(vec![]);
        order(
            &texts,
            &[
                "Add fonts",
                "Drag and drop to install",
                "Browse and install fonts",
                "System font",
                TEXT_TITLE,
                "Noto Sans",
                CODE_TITLE,
                "Noto Sans Mono",
                "Installed fonts (0)",
            ],
        );
        for gone in ["Size", "Text and control size", "Login screen", "Use these fonts on the login screen", "Use"] {
            assert!(!texts.iter().any(|(t, _)| t == gone), "{gone:?} is still on the page");
        }
        // the rows open their pages
        p.click(TEXT_TITLE, 0);
        assert!(p.app.app_sub == crate::AppSub::FontText);
        assert_eq!(p.app.path(), ["Appearance", "Fonts", "Text"]);
        p.app.back();
        p.click(CODE_TITLE, 0);
        assert!(p.app.app_sub == crate::AppSub::FontCode);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_kinds_page_lists_its_choices_noto_first() {
        let (dir, t) = scratch("kinds", 0);
        let mut p = Page::new(t);
        for (sub, kind) in [(crate::AppSub::FontText, Kind::Text), (crate::AppSub::FontCode, Kind::Code)] {
            p.app.app_sub = sub;
            let texts = p.frame(vec![]);
            let names: Vec<&str> = kind.choices().iter().map(|c| c.name).collect();
            order(&texts, &names);
            assert_eq!(names[0], if kind == Kind::Text { "Noto Sans" } else { "Noto Sans Mono" });
            assert_eq!(texts.iter().filter(|(t, _)| t == "In use").count(), 1);
            assert_eq!(texts.iter().filter(|(t, _)| t == "Use").count(), names.len() - 1);
            assert!(texts.iter().any(|(t, _)| t == kind.what()));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn use_sets_your_font_and_the_login_screens() {
        let (dir, t) = scratch("use", 0);
        let mut p = Page::new(t.clone());
        p.app.app_sub = crate::AppSub::FontText;
        // the Use buttons: Atkinson Hyperlegible, Next, Lexend, OpenDyslexic
        p.click("Use", 2);
        assert_eq!(fonts::load_from(&t.mine), Prefs { sans: Some("lexend"), mono: None }, "your file at once");
        p.wait_for_login();
        assert_eq!(std::fs::read_to_string(dir.join("pkexec.log")).unwrap(), format!("{HELPER} set lexend -\n"));
        assert_eq!(fonts::load_from(&t.computer), Prefs { sans: Some("lexend"), mono: None }, "and the computer's");
        let texts = p.frame(vec![]);
        assert!(!texts.iter().any(|(t, _)| t.starts_with("The login screen keeps")));
        assert_eq!(texts.iter().filter(|(t, _)| t == "In use").count(), 1);
        order(&texts, &["Lexend", "In use", "OpenDyslexic"]);
        // a code font: the login screen gets both, the text font as it was
        p.app.app_sub = crate::AppSub::FontCode;
        p.click("Use", 0);
        p.wait_for_login();
        assert_eq!(fonts::load_from(&t.mine), Prefs { sans: Some("lexend"), mono: Some("atkinson-hyperlegible-mono") });
        let log = std::fs::read_to_string(dir.join("pkexec.log")).unwrap();
        assert_eq!(log.lines().nth(1), Some(format!("{HELPER} set lexend atkinson-hyperlegible-mono").as_str()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_the_password_your_font_still_applies_and_a_note_says_so() {
        let (dir, t) = scratch("cancel", 126);
        let mut p = Page::new(t.clone());
        p.app.app_sub = crate::AppSub::FontText;
        p.click("Use", 0);
        p.wait_for_login();
        assert_eq!(fonts::load_from(&t.mine), Prefs { sans: Some("atkinson-hyperlegible"), mono: None });
        assert!(!t.computer.exists(), "the computer's file is not written");
        let note = "The login screen keeps Noto Sans until the font is set again.";
        let texts = p.frame(vec![]);
        assert!(texts.iter().any(|(t, _)| t == note), "{texts:?}");
        assert!(p.app.status.is_empty(), "a cancelled password is no error");
        assert!(lock(&p.app.font_lib.login).error.is_none());
        // on the Fonts page too, and "Set again" asks once more
        p.app.app_sub = crate::AppSub::Font;
        assert!(p.frame(vec![]).iter().any(|(t, _)| t == note));
        p.click("Set again", 0);
        p.wait_for_login();
        assert_eq!(std::fs::read_to_string(dir.join("pkexec.log")).unwrap(), format!("{HELPER} set atkinson-hyperlegible -\n").repeat(2));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
