//! Saga MD, Yggdrasil's Markdown reader: one Markdown or text file in one page, either **formatted** (headings, lists, tables, pictures, code) or as its
//! **source**, the text itself, which can also be edited and saved. A switch in the title bar (or Ctrl+E) changes between the two;
//! there is never a second pane.
//!
//! Markdown files open formatted, other text files as their source. Links to web pages open in the browser, links to other Markdown
//! files open here (Back, or Alt+Left, returns), `#anchors` scroll to their heading. Pictures in the folder show; pictures from the web
//! are not loaded (nothing is fetched): they show as a link. HTML in a file is never run. When the file changes on disk (another
//! program wrote it) the page follows, unless you have unsaved changes, which are never thrown away without asking.

mod md;

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Key, Label, Layout, Margin, Modifiers, Rect, RichText, Sense, Stroke, ViewportCommand};
use md::{Alert, Block, Inline, Item, Style};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use yggdrasil_ui::theme::{self, Palette};
use yggdrasil_ui::{chrome, sysicon};

const APP_ID: &str = "saga-md";
const APP_NAME: &str = "Saga MD";
/// The widest the text runs (at 100 %), so lines stay readable in a wide window.
const MAX_WIDTH: f32 = 860.0;
const BASE_SIZE: f32 = 15.5;
/// Larger files are not opened (a text file that size is not meant to be read in a window).
const MAX_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Formatted,
    Source,
}

/// What waits for an answer about unsaved changes.
#[derive(Clone, Debug, PartialEq)]
enum Pending {
    Open(PathBuf, Option<String>),
    Back,
    Pick,
    Reload,
    Close,
}

struct Doc {
    path: PathBuf,
    /// The text shown (edited in the source view).
    text: String,
    /// The text as it is on disk.
    saved: String,
    /// Why it cannot be saved, when it cannot.
    readonly: Option<String>,
    blocks: Vec<Block>,
    gen: u64,
    parsed: u64,
}

impl Doc {
    fn dirty(&self) -> bool {
        self.text != self.saved
    }
    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.path.display().to_string())
    }
}

/// The file being shown, looked at once a second by a thread of its own (so an idle window does no work).
#[derive(Default)]
struct Watch {
    path: Mutex<Option<PathBuf>>,
    stamp: Mutex<Option<(SystemTime, u64)>>,
    changed: AtomicBool,
}

fn stamp(p: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

struct App {
    doc: Option<Doc>,
    view: View,
    zoom: f32,
    history: Vec<PathBuf>,
    anchor: Option<String>,
    /// Until when the page keeps scrolling to the anchor (pictures load a moment after the page and push it down).
    anchor_until: Option<Instant>,
    pending: Option<Pending>,
    note: Option<(String, Instant)>,
    watch: Arc<Watch>,
    disk_changed: bool,
    fonts: bool,
    picked: Arc<Mutex<Option<Option<PathBuf>>>>,
    close_ok: bool,
    scroll_top: bool,
    title: String,
    right_w: f32,
}

impl App {
    fn new(cc: &eframe::CreationContext, start: Start) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        // Ctrl+plus and minus size the page, not the whole window
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let fonts = install_fonts(&cc.egui_ctx);
        let watch = Arc::new(Watch::default());
        {
            let (w, ctx) = (watch.clone(), cc.egui_ctx.clone());
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(1));
                let Some(p) = w.path.lock().unwrap().clone() else { continue };
                let now = stamp(&p);
                let mut last = w.stamp.lock().unwrap();
                if *last != now {
                    *last = now;
                    w.changed.store(true, Ordering::Relaxed);
                    ctx.request_repaint();
                }
            });
        }
        let mut app = App {
            doc: None,
            view: View::Formatted,
            zoom: start.zoom,
            history: vec![],
            anchor: None,
            anchor_until: None,
            pending: None,
            note: None,
            watch,
            disk_changed: false,
            fonts,
            picked: Arc::new(Mutex::new(None)),
            close_ok: false,
            scroll_top: false,
            title: String::new(),
            right_w: 320.0,
        };
        if let Some(p) = start.path {
            app.open(p, start.anchor, false);
            if start.source {
                app.view = View::Source;
            }
        }
        app
    }

    fn dirty(&self) -> bool {
        self.doc.as_ref().map(|d| d.dirty()).unwrap_or(false)
    }

    fn say(&mut self, text: impl Into<String>) {
        self.note = Some((text.into(), Instant::now()));
    }

    /// Show a file (keeping the one before for Back when `remember`).
    fn open(&mut self, path: PathBuf, anchor: Option<String>, remember: bool) {
        match load(&path) {
            Ok(doc) => {
                if remember {
                    if let Some(old) = &self.doc {
                        if old.path != doc.path {
                            self.history.push(old.path.clone());
                        }
                    }
                }
                self.view = default_view(&doc.path);
                *self.watch.path.lock().unwrap() = Some(doc.path.clone());
                *self.watch.stamp.lock().unwrap() = stamp(&doc.path);
                self.watch.changed.store(false, Ordering::Relaxed);
                self.disk_changed = false;
                self.anchor = anchor;
                self.anchor_until = None;
                self.scroll_top = self.anchor.is_none();
                self.doc = Some(doc);
            }
            Err(e) => self.say(e),
        }
    }

    /// Read the file again from disk (unsaved changes are dropped: callers ask first).
    fn reload(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        match load(&doc.path) {
            Ok(fresh) => {
                if fresh.saved != doc.saved || doc.dirty() {
                    doc.text = fresh.text;
                    doc.saved = fresh.saved;
                    doc.readonly = fresh.readonly;
                    doc.gen += 1;
                }
                *self.watch.stamp.lock().unwrap() = stamp(&doc.path);
                self.disk_changed = false;
            }
            Err(e) => self.say(e),
        }
    }

    fn save(&mut self) -> bool {
        let Some(doc) = &mut self.doc else { return false };
        if let Some(why) = &doc.readonly {
            let why = why.clone();
            self.say(format!("Not saved: {why}"));
            return false;
        }
        match write_atomic(&doc.path, &doc.text) {
            Ok(()) => {
                doc.saved = doc.text.clone();
                *self.watch.stamp.lock().unwrap() = stamp(&doc.path);
                self.disk_changed = false;
                self.say("Saved");
                true
            }
            Err(e) => {
                self.say(format!("Not saved: {e}"));
                false
            }
        }
    }

    /// Do it now, or first ask about unsaved changes.
    fn request(&mut self, ctx: &egui::Context, p: Pending) {
        if self.dirty() {
            self.pending = Some(p);
        } else {
            self.perform(ctx, p);
        }
    }

    fn perform(&mut self, ctx: &egui::Context, p: Pending) {
        match p {
            Pending::Open(path, anchor) => self.open(path, anchor, true),
            Pending::Back => {
                if let Some(prev) = self.history.pop() {
                    self.open(prev, None, false);
                }
            }
            Pending::Pick => self.pick(ctx),
            Pending::Reload => self.reload(),
            Pending::Close => {
                self.close_ok = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    /// The desktop's file chooser (Minos), in a thread so the window keeps drawing.
    fn pick(&mut self, ctx: &egui::Context) {
        let (slot, ctx) = (self.picked.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = pick_file();
            *slot.lock().unwrap() = Some(r);
            ctx.request_repaint();
        });
    }

    fn follow(&mut self, ctx: &egui::Context, url: &str) {
        let dir = self.doc.as_ref().and_then(|d| d.path.parent().map(Path::to_path_buf));
        match resolve(dir.as_deref(), url) {
            Target::Anchor(a) => self.anchor = Some(a),
            Target::Web(u) => open_external(&u),
            Target::File(p, frag) if p.is_file() && readable_here(&p) => self.request(ctx, Pending::Open(p, frag)),
            Target::File(p, _) => open_external(&p.to_string_lossy()),
            Target::Missing(p) => self.say(format!("Not found: {p}")),
        }
    }
}

// ---------------------------------------------------------------- files

/// What a link points at.
#[derive(Debug, PartialEq)]
enum Target {
    Anchor(String),
    Web(String),
    File(PathBuf, Option<String>),
    Missing(String),
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A link's target, relative links taken from the file's own folder.
fn resolve(dir: Option<&Path>, url: &str) -> Target {
    let url = url.trim();
    if let Some(a) = url.strip_prefix('#') {
        return Target::Anchor(percent_decode(a).to_lowercase());
    }
    let lower = url.to_ascii_lowercase();
    let local = lower.starts_with("file://");
    let scheme = url.find(':').filter(|&i| i > 1 && url[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)));
    if scheme.is_some() && !local {
        return Target::Web(url.to_string());
    }
    let rest = if local { &url[7..] } else { url };
    let (p, frag) = match rest.split_once('#') {
        Some((p, f)) => (p, Some(percent_decode(f).to_lowercase()).filter(|f| !f.is_empty())),
        None => (rest, None),
    };
    let p = percent_decode(p.split('?').next().unwrap_or(""));
    let path = if p.starts_with('/') {
        PathBuf::from(&p)
    } else if let Some(r) = p.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(r)
    } else {
        dir.map(|d| d.join(&p)).unwrap_or_else(|| PathBuf::from(&p))
    };
    if path.exists() {
        Target::File(path, frag)
    } else {
        Target::Missing(path.display().to_string())
    }
}

fn ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn is_markdown(p: &Path) -> bool {
    let e = ext(p);
    matches!(e.as_str(), "md" | "markdown" | "mdown" | "mkd" | "mkdn" | "mdwn" | "mdtxt" | "mdtext" | "rmd")
        || (e.is_empty() && p.file_name().map(|n| n.to_string_lossy().to_uppercase().starts_with("README")).unwrap_or(false))
}

/// Files a link opens here rather than in another program.
fn readable_here(p: &Path) -> bool {
    is_markdown(p) || matches!(ext(p).as_str(), "txt" | "text" | "log")
}

fn default_view(p: &Path) -> View {
    if is_markdown(p) {
        View::Formatted
    } else {
        View::Source
    }
}

fn load(path: &Path) -> Result<Doc, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    if meta.is_dir() {
        return Err(format!("{} is a folder", path.display()));
    }
    if meta.len() > MAX_BYTES {
        return Err(format!("{} is too big to read here ({} MB)", path.display(), meta.len() / 1024 / 1024));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    let (text, mut readonly) = match String::from_utf8(bytes) {
        Ok(t) => (t, None),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), Some("it is not plain text (UTF-8), so it is shown as well as it can be".to_string())),
    };
    if readonly.is_none() && std::fs::OpenOptions::new().append(true).open(path).is_err() {
        readonly = Some("you cannot write to this file".to_string());
    }
    Ok(Doc { path: path.to_path_buf(), saved: text.clone(), text, readonly, blocks: vec![], gen: 1, parsed: 0 })
}

/// Saves by writing a new copy beside the file and putting it in place, so a crash halfway never leaves half a file. Where the
/// folder cannot take a new file, the file itself is written.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.saga-{}", std::process::id()));
    match std::fs::write(&tmp, text) {
        Ok(()) => {
            if let Ok(m) = std::fs::metadata(&target) {
                let _ = std::fs::set_permissions(&tmp, m.permissions());
            }
            std::fs::rename(&tmp, &target).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => std::fs::write(&target, text),
        Err(e) => Err(e),
    }
}

fn open_external(target: &str) {
    if let Ok(mut child) = std::process::Command::new("xdg-open").arg(target).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn path_from_uri(u: &str) -> Option<PathBuf> {
    u.strip_prefix("file://").map(|p| PathBuf::from(percent_decode(p)))
}

/// The desktop's file chooser (the FileChooser portal; in Yggdrasil it is Minos), for one Markdown or text file.
fn pick_file() -> Option<PathBuf> {
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedValue, Value};
    let conn = zbus::blocking::Connection::session().ok()?;
    let token = format!("saga_md_{}", std::process::id());
    let sender = conn.unique_name()?.trim_start_matches(':').replace('.', "_");
    let request = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let proxy = zbus::blocking::Proxy::new(&conn, "org.freedesktop.portal.Desktop", request, "org.freedesktop.portal.Request").ok()?;
    let mut answers = proxy.receive_signal("Response").ok()?;
    let filters: Vec<(&str, Vec<(u32, &str)>)> = vec![
        ("Markdown and text", vec![(0, "*.md"), (0, "*.markdown"), (0, "*.txt"), (1, "text/markdown"), (1, "text/plain")]),
        ("All files", vec![(0, "*")]),
    ];
    let mut opts: HashMap<&str, Value> = HashMap::new();
    opts.insert("handle_token", Value::from(token.as_str()));
    opts.insert("filters", Value::from(filters));
    conn.call_method(Some("org.freedesktop.portal.Desktop"), "/org/freedesktop/portal/desktop", Some("org.freedesktop.portal.FileChooser"), "OpenFile", &("", "Open a Markdown or text file", opts)).ok()?;
    let msg = answers.next()?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize().ok()?;
    if code != 0 {
        return None;
    }
    let uris: Vec<String> = results.get("uris").and_then(|v| Vec::<String>::try_from(v.clone()).ok())?;
    uris.first().and_then(|u| path_from_uri(u))
}

/// The page's own fonts (the system's sans and mono, with real bold and italic), as named families: the window's own text keeps
/// egui's like every Yggdrasil app. False when fontconfig finds none (the page then shows bold as brighter text).
fn install_fonts(ctx: &egui::Context) -> bool {
    fn fc_file(pattern: &str) -> Option<PathBuf> {
        let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
        let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        (p.is_file() && matches!(ext(&p).as_str(), "ttf" | "otf")).then_some(p)
    }
    let want = [("doc", "sans:regular"), ("doc-b", "sans:bold"), ("doc-i", "sans:italic"), ("doc-bi", "sans:bold:italic"), ("doc-mono", "monospace:regular")];
    let mut defs = egui::FontDefinitions::default();
    let prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    for (name, pattern) in want {
        let Some(bytes) = fc_file(pattern).and_then(|f| std::fs::read(f).ok()) else { return false };
        defs.font_data.insert(name.to_string(), Arc::new(egui::FontData::from_owned(bytes)));
        let mut list = vec![name.to_string()];
        list.extend(if name == "doc-mono" { mono.clone() } else { prop.clone() });
        defs.families.insert(FontFamily::Name(name.into()), list);
    }
    ctx.set_fonts(defs);
    true
}

// ---------------------------------------------------------------- drawing the page

struct Page<'a> {
    pal: &'a Palette,
    fonts: bool,
    size: f32,
    width: f32,
    dir: Option<&'a Path>,
    text: Color32,
    depth: usize,
    n: usize,
    anchor: &'a mut Option<String>,
    clicked: &'a mut Option<String>,
    copied: &'a mut bool,
}

impl Page<'_> {
    fn family(&self, s: Style) -> FontFamily {
        if !self.fonts {
            return if s.code { FontFamily::Monospace } else { FontFamily::Proportional };
        }
        FontFamily::Name(
            match (s.code, s.bold, s.italic) {
                (true, ..) => "doc-mono",
                (_, true, true) => "doc-bi",
                (_, true, false) => "doc-b",
                (_, false, true) => "doc-i",
                _ => "doc",
            }
            .into(),
        )
    }
    fn link(&self) -> Color32 {
        theme::readable_on_dark(self.pal.accent)
    }
    fn code_bg(&self) -> Color32 {
        self.pal.field
    }
}

fn blocks(ui: &mut egui::Ui, c: &mut Page, bs: &[Block]) {
    for (i, b) in bs.iter().enumerate() {
        if i > 0 {
            ui.add_space(if matches!(b, Block::Heading { .. }) { c.size * 1.25 } else { c.size * 0.75 });
        }
        block(ui, c, b);
    }
}

fn block(ui: &mut egui::Ui, c: &mut Page, b: &Block) {
    match b {
        Block::Heading { level, text, slug } => {
            let scale = [2.0, 1.55, 1.28, 1.12, 1.0, 0.94][(*level as usize).clamp(1, 6) - 1];
            let resp = ui.scope(|ui| inlines(ui, c, text, c.size * scale, true, c.text)).response;
            if *level <= 2 {
                ui.add_space(c.size * 0.3);
                rule(ui, c);
            }
            if c.anchor.as_deref() == Some(slug.as_str()) {
                ui.scroll_to_rect(resp.rect, Some(Align::TOP));
            }
        }
        Block::Para(inl) => {
            let color = c.text;
            inlines(ui, c, inl, c.size, false, color);
        }
        Block::Center(inl) => {
            let color = c.text;
            ui.with_layout(Layout::top_down(Align::Center), |ui| inlines(ui, c, inl, c.size, false, color));
        }
        Block::Code { lang, text } => code_block(ui, c, lang, text),
        Block::Quote { alert, blocks: inner } => quote(ui, c, *alert, inner),
        Block::List { start, items } => list(ui, c, *start, items),
        Block::Table { head, rows, .. } => table(ui, c, head, rows),
        Block::Footnote { label, blocks: inner } => {
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(format!("[{label}]")).size(c.size * 0.85).color(c.pal.dim));
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    let old = c.size;
                    c.size *= 0.92;
                    blocks(ui, c, inner);
                    c.size = old;
                });
            });
        }
        Block::Rule => {
            ui.add_space(c.size * 0.2);
            rule(ui, c);
            ui.add_space(c.size * 0.2);
        }
    }
}

fn rule(ui: &mut egui::Ui, c: &Page) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, c.pal.border));
}

/// Words, styles, links and pictures, wrapping at the page's width.
fn inlines(ui: &mut egui::Ui, c: &mut Page, inl: &[Inline], size: f32, heading: bool, color: Color32) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, size * 0.32);
        for i in inl {
            match i {
                Inline::Text { text, style, link } => {
                    let mut st = *style;
                    st.bold |= heading;
                    let mut rt = RichText::new(text).size(if st.code { size * 0.9 } else { size }).family(c.family(st)).color(color);
                    if st.code {
                        rt = rt.background_color(c.code_bg());
                    }
                    if st.strike {
                        rt = rt.strikethrough();
                    }
                    if !c.fonts {
                        if st.bold {
                            rt = rt.strong();
                        }
                        if st.italic {
                            rt = rt.italics();
                        }
                    }
                    match link {
                        Some(url) => {
                            let r = ui.add(Label::new(rt.color(c.link()).underline()).sense(Sense::click())).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(url.as_str());
                            if r.clicked() {
                                *c.clicked = Some(url.clone());
                            }
                        }
                        None => {
                            ui.add(Label::new(rt));
                        }
                    }
                }
                Inline::Image { url, alt, link } => picture(ui, c, url, alt, link.as_deref(), size),
                Inline::Break => ui.end_row(),
            }
        }
    });
}

/// A picture from the file's folder, drawn; one from the web is not fetched, only offered as a link.
fn picture(ui: &mut egui::Ui, c: &mut Page, url: &str, alt: &str, link: Option<&str>, size: f32) {
    let local = match resolve(c.dir, url) {
        Target::File(p, _) if p.is_file() => Some(p),
        _ => None,
    };
    match local {
        Some(p) => {
            let img = egui::Image::new(format!("file://{}", p.display())).fit_to_original_size(1.0).max_width(c.width).max_height(720.0).corner_radius(6);
            let r = ui.add(img.sense(if link.is_some() { Sense::click() } else { Sense::hover() }));
            let r = if alt.is_empty() { r } else { r.on_hover_text(alt) };
            if let (Some(l), true) = (link, r.clicked()) {
                *c.clicked = Some(l.to_string());
            }
        }
        None => {
            let web = matches!(resolve(None, url), Target::Web(_));
            let label = if alt.trim().is_empty() { "picture" } else { alt.trim() };
            let rt = RichText::new(format!("[{label}]")).size(size * 0.9).color(if web { c.link() } else { c.pal.dim });
            if web {
                let r = ui.add(Label::new(rt.underline()).sense(Sense::click())).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(format!("A picture on the web (not loaded): {url}"));
                if r.clicked() {
                    *c.clicked = Some(link.unwrap_or(url).to_string());
                }
            } else {
                ui.add(Label::new(rt)).on_hover_text(format!("Picture not found: {url}"));
            }
        }
    }
}

fn code_block(ui: &mut egui::Ui, c: &mut Page, lang: &str, text: &str) {
    c.n += 1;
    let id = c.n;
    egui::Frame::new().fill(c.code_bg()).corner_radius(8).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(lang).size(c.size * 0.72).color(c.pal.dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Copy").size(c.size * 0.72).color(c.pal.dim)).frame(false)).on_hover_text("Copy this code").clicked() {
                    ui.ctx().copy_text(text.to_string());
                    *c.copied = true;
                }
            });
        });
        egui::ScrollArea::horizontal().id_salt(("code", id)).show(ui, |ui| {
            let mono = c.family(Style { code: true, ..Default::default() });
            ui.add(Label::new(RichText::new(text).family(mono).size(c.size * 0.86).color(c.text)).extend());
        });
    });
}

fn quote(ui: &mut egui::Ui, c: &mut Page, alert: Option<Alert>, inner: &[Block]) {
    let (bar, name) = match alert {
        None => (c.pal.border, ""),
        Some(Alert::Note) => (c.pal.accent, "Note"),
        Some(Alert::Tip) => (c.pal.green, "Tip"),
        Some(Alert::Important) => (c.pal.accent, "Important"),
        Some(Alert::Warning) => (c.pal.amber, "Warning"),
        Some(Alert::Caution) => (c.pal.red, "Caution"),
    };
    let resp = egui::Frame::new().inner_margin(Margin { left: 16, right: 0, top: 2, bottom: 2 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if !name.is_empty() {
            let f = c.family(Style { bold: true, ..Default::default() });
            ui.label(RichText::new(name).size(c.size).family(f).color(theme::readable_on_dark(bar)));
            ui.add_space(2.0);
        }
        let old = c.text;
        if alert.is_none() {
            c.text = c.pal.dim;
        }
        blocks(ui, c, inner);
        c.text = old;
    });
    let r = resp.response.rect;
    ui.painter().rect_filled(Rect::from_min_max(r.left_top(), pos2(r.left() + 3.0, r.bottom())), CornerRadius::same(2), bar);
}

fn list(ui: &mut egui::Ui, c: &mut Page, start: Option<u64>, items: &[Item]) {
    c.depth += 1;
    let bullet = (c.depth - 1).min(2);
    for (k, item) in items.iter().enumerate() {
        ui.horizontal_top(|ui| {
            let w = c.size * 1.7;
            let (rect, _) = ui.allocate_exact_size(vec2(w, c.size * 1.35), Sense::hover());
            let p = ui.painter();
            match item.task {
                Some(done) => {
                    let b = Rect::from_center_size(pos2(rect.left() + c.size * 0.6, rect.center().y), vec2(c.size * 0.82, c.size * 0.82));
                    if done {
                        p.rect_filled(b, CornerRadius::same(3), c.pal.accent);
                        let ink = Stroke::new(1.8, c.pal.on_accent());
                        let (l, m, r) = (b.left_center() + vec2(b.width() * 0.2, 0.0), b.center_bottom() + vec2(-b.width() * 0.08, -b.height() * 0.25), b.right_top() + vec2(-b.width() * 0.2, b.height() * 0.25));
                        p.line_segment([l, m], ink);
                        p.line_segment([m, r], ink);
                    } else {
                        p.rect_stroke(b, CornerRadius::same(3), Stroke::new(1.4, c.pal.dim), egui::StrokeKind::Inside);
                    }
                }
                None => match start {
                    Some(s) => {
                        let f = FontId::new(c.size, c.family(Style::default()));
                        p.text(pos2(rect.right() - 6.0, rect.center().y), Align2::RIGHT_CENTER, format!("{}.", s + k as u64), f, c.pal.dim);
                    }
                    None => {
                        // drawn, not typed: a disc, a ring, a square (fonts often lack the ring)
                        let at = pos2(rect.left() + c.size * 0.6, rect.center().y + c.size * 0.04);
                        let r = c.size * 0.17;
                        match bullet {
                            0 => {
                                p.circle_filled(at, r, c.pal.dim);
                            }
                            1 => {
                                p.circle_stroke(at, r, Stroke::new(1.2, c.pal.dim));
                            }
                            _ => {
                                p.rect_filled(Rect::from_center_size(at, vec2(r * 1.7, r * 1.7)), CornerRadius::ZERO, c.pal.dim);
                            }
                        }
                    }
                },
            }
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width());
                blocks(ui, c, &item.blocks);
            });
        });
        if k + 1 < items.len() {
            ui.add_space(c.size * 0.2);
        }
    }
    c.depth -= 1;
}

fn table(ui: &mut egui::Ui, c: &mut Page, head: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>]) {
    c.n += 1;
    let id = c.n;
    let cols = head.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(0)).max(1);
    let cell_w = ((c.width - 40.0) / cols as f32 - 18.0).max(110.0);
    egui::ScrollArea::horizontal().id_salt(("table", id)).show(ui, |ui| {
        egui::Frame::new().stroke(Stroke::new(1.0, c.pal.border)).corner_radius(6).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
            egui::Grid::new(("grid", id)).striped(true).spacing([18.0, 8.0]).show(ui, |ui| {
                for cell in head {
                    ui.vertical(|ui| {
                        ui.set_max_width(cell_w);
                        inlines(ui, c, cell, c.size * 0.95, true, c.text);
                    });
                }
                ui.end_row();
                for row in rows {
                    for cell in row {
                        ui.vertical(|ui| {
                            ui.set_max_width(cell_w);
                            let color = c.text;
                            inlines(ui, c, cell, c.size * 0.95, false, color);
                        });
                    }
                    ui.end_row();
                }
            });
        });
    });
}

// ---------------------------------------------------------------- the window

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        let dark = theme::system_dark();
        theme::apply(&ctx, dark, accent);
        let pal = theme::palette_for(dark, accent);
        sysicon::refresh(&ctx);

        // a file chosen in the file chooser
        let picked = self.picked.lock().unwrap().take();
        if let Some(Some(p)) = picked {
            self.open(p, None, true);
        }
        // the file changed on disk
        if self.watch.changed.swap(false, Ordering::Relaxed) && self.doc.is_some() {
            if self.dirty() {
                self.disk_changed = true;
            } else {
                self.reload();
            }
        }
        // a file dropped on the window
        let dropped: Option<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty()));
        if let Some(p) = dropped {
            self.request(&ctx, Pending::Open(p, None));
        }
        // closing with unsaved changes asks first
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() && !self.close_ok {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }

        let (k_open, k_save, k_view, k_back, k_reload, k_in, k_out, k_reset, k_close) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::O),
                i.consume_key(Modifiers::COMMAND, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::E),
                i.consume_key(Modifiers::ALT, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::F5),
                i.consume_key(Modifiers::COMMAND, Key::Plus) || i.consume_key(Modifiers::COMMAND, Key::Equals),
                i.consume_key(Modifiers::COMMAND, Key::Minus),
                i.consume_key(Modifiers::COMMAND, Key::Num0),
                i.consume_key(Modifiers::COMMAND, Key::W) || i.consume_key(Modifiers::COMMAND, Key::Q),
            )
        });
        let pinch = ctx.input(|i| i.zoom_delta());
        if pinch != 1.0 {
            self.zoom *= pinch;
        }
        if k_in {
            self.zoom *= 1.1;
        }
        if k_out {
            self.zoom /= 1.1;
        }
        if k_reset {
            self.zoom = 1.0;
        }
        self.zoom = self.zoom.clamp(0.5, 3.0);
        if k_open {
            self.request(&ctx, Pending::Pick);
        }
        if k_save && self.dirty() {
            self.save();
        }
        if k_view && self.doc.is_some() {
            self.view = if self.view == View::Formatted { View::Source } else { View::Formatted };
        }
        if k_back && !self.history.is_empty() {
            self.request(&ctx, Pending::Back);
        }
        if k_reload && self.doc.is_some() {
            self.request(&ctx, Pending::Reload);
        }
        if k_close {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        // the window's title
        let title = match &self.doc {
            Some(d) => format!("{}{} – {APP_NAME}", if d.dirty() { "• " } else { "" }, d.name()),
            None => APP_NAME.to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        let mut acts: Vec<Pending> = vec![];
        let mut want_save = false;

        // the title bar: the file, the switch, Open and Save, the window buttons
        egui::Panel::top("bar").frame(egui::Frame::new().fill(pal.strip).inner_margin(Margin::symmetric(10, 5))).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if !self.history.is_empty() && ui.button("‹ Back").on_hover_text("The page before (Alt+Left)").clicked() {
                    acts.push(Pending::Back);
                }
                let (icon, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                if !sysicon::paint(ui.painter(), icon, &["text-markdown", "text-x-markdown", "text-x-generic"]) {
                    ui.painter().rect_stroke(icon.shrink(3.0), CornerRadius::same(3), Stroke::new(1.2, pal.dim), egui::StrokeKind::Inside);
                }
                match &self.doc {
                    Some(d) => {
                        ui.label(RichText::new(d.name()).size(14.5).strong());
                        if d.dirty() {
                            ui.label(RichText::new("●").size(10.0).color(pal.accent)).on_hover_text("Changed, not saved yet (Ctrl+S saves)");
                        }
                        if let Some(why) = &d.readonly {
                            ui.label(RichText::new("read only").size(12.0).color(pal.dim)).on_hover_text(format!("Cannot be saved: {why}"));
                        }
                        let folder = d.path.parent().map(tidy_path).unwrap_or_default();
                        let room = (ui.available_width() - self.right_w - chrome::controls_width() - 40.0).clamp(0.0, 360.0);
                        if room > 40.0 {
                            ui.allocate_ui(vec2(room, 20.0), |ui| ui.add(Label::new(RichText::new(folder).size(12.0).color(pal.dim)).truncate()));
                        }
                    }
                    None => {
                        ui.label(RichText::new(APP_NAME).size(14.5).strong());
                    }
                }
                chrome::drag_area(ui, ui.available_width() - self.right_w - chrome::controls_width());
                let from = ui.cursor().left();
                if self.doc.is_some() {
                    egui::Frame::new().fill(pal.field).corner_radius(7).inner_margin(Margin::same(2)).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.selectable_value(&mut self.view, View::Formatted, "Formatted").on_hover_text("The page as it is meant to look (Ctrl+E switches)");
                        ui.selectable_value(&mut self.view, View::Source, "Source").on_hover_text("The text itself, which you can also edit (Ctrl+E switches)");
                    });
                    if (self.zoom - 1.0).abs() > 0.01 && ui.button(format!("{:.0} %", self.zoom * 100.0)).on_hover_text("Back to 100 % (Ctrl+0)").clicked() {
                        self.zoom = 1.0;
                    }
                }
                if ui.button("Open…").on_hover_text("Open another file (Ctrl+O)").clicked() {
                    acts.push(Pending::Pick);
                }
                if self.dirty() && ui.add(egui::Button::new(RichText::new("Save").color(pal.on_accent())).fill(pal.accent)).on_hover_text("Ctrl+S").clicked() {
                    want_save = true;
                }
                self.right_w = (ui.cursor().left() - from).max(60.0);
                chrome::window_controls(ui, &pal);
            });
        });

        // changed on disk while there are unsaved changes here
        if self.disk_changed {
            egui::Panel::top("changed").frame(egui::Frame::new().fill(pal.sidebar).inner_margin(Margin::symmetric(14, 8))).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Another program changed this file on disk.").color(pal.amber));
                    if ui.button("Load that version").on_hover_text("Your changes here are lost").clicked() {
                        acts.push(Pending::Reload);
                        self.disk_changed = false;
                        if let Some(d) = &mut self.doc {
                            d.text = d.saved.clone();
                        }
                    }
                    if ui.button("Keep mine").on_hover_text("Saving then writes your version over it").clicked() {
                        self.disk_changed = false;
                    }
                });
            });
        }

        let mut clicked: Option<String> = None;
        let mut copied = false;
        let zoom = self.zoom;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg)).show(ui, |ui| {
            let Some(doc) = &mut self.doc else {
                empty_page(ui, &pal, &mut acts);
                return;
            };
            match self.view {
                View::Formatted => {
                    if doc.parsed != doc.gen {
                        doc.blocks = md::parse(&doc.text);
                        doc.parsed = doc.gen;
                    }
                    let mut sa = egui::ScrollArea::vertical().id_salt(("page", doc.path.clone())).auto_shrink([false, false]);
                    if self.scroll_top {
                        sa = sa.vertical_scroll_offset(0.0);
                    }
                    sa.show(ui, |ui| {
                        let avail = ui.available_width();
                        let width = (avail - 48.0).min(MAX_WIDTH * zoom).max(160.0);
                        let pad = ((avail - width) / 2.0).max(0.0);
                        ui.add_space(26.0);
                        ui.horizontal_top(|ui| {
                            ui.add_space(pad);
                            ui.vertical(|ui| {
                                ui.set_width(width);
                                let mut page = Page {
                                    pal: &pal,
                                    fonts: self.fonts,
                                    size: BASE_SIZE * zoom,
                                    width,
                                    dir: doc.path.parent(),
                                    text: pal.text,
                                    depth: 0,
                                    n: 0,
                                    anchor: &mut self.anchor,
                                    clicked: &mut clicked,
                                    copied: &mut copied,
                                };
                                if doc.blocks.is_empty() {
                                    ui.label(RichText::new("This file is empty.").color(pal.dim));
                                } else {
                                    blocks(ui, &mut page, &doc.blocks);
                                }
                            });
                        });
                        ui.add_space(60.0);
                    });
                }
                View::Source => {
                    let mut sa = egui::ScrollArea::vertical().id_salt(("source", doc.path.clone())).auto_shrink([false, false]);
                    if self.scroll_top {
                        sa = sa.vertical_scroll_offset(0.0);
                    }
                    sa.show(ui, |ui| {
                        let font = FontId::new(13.5 * zoom, if self.fonts { FontFamily::Name("doc-mono".into()) } else { FontFamily::Monospace });
                        let size = vec2(ui.available_width(), ui.available_height());
                        egui::Frame::new().inner_margin(Margin::symmetric(18, 14)).show(ui, |ui| {
                            if doc.readonly.is_some() {
                                let mut view: &str = doc.text.as_str();
                                ui.add(egui::TextEdit::multiline(&mut view).font(font).frame(egui::Frame::NONE).desired_width(f32::INFINITY).min_size(size - vec2(36.0, 28.0)));
                            } else {
                                let r = ui.add(egui::TextEdit::multiline(&mut doc.text).font(font).frame(egui::Frame::NONE).code_editor().lock_focus(true).desired_width(f32::INFINITY).min_size(size - vec2(36.0, 28.0)));
                                if r.changed() {
                                    doc.gen += 1;
                                }
                            }
                        });
                    });
                }
            }
        });
        self.scroll_top = false;
        if self.anchor.is_some() {
            let until = *self.anchor_until.get_or_insert_with(|| Instant::now() + Duration::from_millis(1500));
            if Instant::now() >= until || ctx.input(|i| i.smooth_scroll_delta != egui::Vec2::ZERO || i.pointer.any_pressed()) {
                self.anchor = None;
                self.anchor_until = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        if copied {
            self.say("Copied");
        }
        if let Some(url) = clicked {
            self.follow(&ctx, &url);
        }
        if want_save {
            self.save();
        }
        for a in acts {
            self.request(&ctx, a);
        }

        // unsaved changes: save them, drop them, or stay
        if let Some(p) = self.pending.clone() {
            let name = self.doc.as_ref().map(|d| d.name()).unwrap_or_default();
            let mut choice: Option<u8> = None;
            let resp = egui::Modal::new(egui::Id::new("unsaved")).show(&ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new(format!("Save the changes to {name}?")).size(17.0).strong());
                ui.add_space(8.0);
                ui.label("If you don't, the changes you made here are lost.");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Save").color(pal.on_accent())).fill(pal.accent)).clicked() {
                        choice = Some(0);
                    }
                    if ui.button("Don't save").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(2);
                    }
                });
            });
            if resp.should_close() && choice.is_none() {
                choice = Some(2);
            }
            match choice {
                Some(0) => {
                    self.pending = None;
                    if self.save() {
                        self.perform(&ctx, p);
                    }
                }
                Some(1) => {
                    self.pending = None;
                    if let Some(d) = &mut self.doc {
                        d.text = d.saved.clone();
                        d.gen += 1;
                    }
                    self.perform(&ctx, p);
                }
                Some(_) => self.pending = None,
                None => {}
            }
        }

        // a short note at the bottom right (Saved, Copied, Not found ...)
        if let Some((text, at)) = &self.note {
            let age = at.elapsed();
            if age > Duration::from_millis(3200) {
                self.note = None;
            } else {
                let screen = ctx.viewport_rect();
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("note")));
                let galley = ctx.fonts_mut(|f| f.layout_no_wrap(text.clone(), FontId::proportional(13.0), pal.text));
                let r = Rect::from_min_size(screen.right_bottom() - galley.size() - vec2(36.0, 30.0), galley.size() + vec2(20.0, 12.0));
                painter.rect_filled(r, CornerRadius::same(8), pal.sidebar);
                painter.rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
                painter.galley(r.min + vec2(10.0, 6.0), galley, pal.text);
                ctx.request_repaint_after(Duration::from_millis(3300) - age.min(Duration::from_millis(3300)));
            }
        }
        chrome::outline(&ctx, &pal);
    }
}

/// A folder as a person writes it: the home folder as ~.
fn tidy_path(p: &Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

fn empty_page(ui: &mut egui::Ui, pal: &Palette, acts: &mut Vec<Pending>) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() / 2.0 - 110.0).max(20.0));
        let (icon, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
        sysicon::paint(ui.painter(), icon, &["saga-md", "text-markdown", "text-x-markdown", "text-x-generic"]);
        ui.add_space(14.0);
        ui.label(RichText::new("Open a Markdown or text file").size(19.0).strong());
        ui.add_space(6.0);
        ui.label(RichText::new("or drop one on this window").color(pal.dim));
        ui.add_space(16.0);
        if ui.add(egui::Button::new(RichText::new("Open…").color(pal.on_accent())).fill(pal.accent).min_size(vec2(110.0, 30.0))).clicked() {
            acts.push(Pending::Pick);
        }
    });
}

/// What the command line asks for: `saga-md [--source] [--zoom PERCENT] [FILE[#heading]]` (a file:// address works too).
#[derive(Debug, PartialEq)]
struct Start {
    path: Option<PathBuf>,
    anchor: Option<String>,
    source: bool,
    zoom: f32,
}

fn parse_args(args: &[String]) -> Start {
    let mut s = Start { path: None, anchor: None, source: false, zoom: 1.0 };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--source" => s.source = true,
            "--zoom" => s.zoom = it.next().and_then(|z| z.trim_end_matches('%').parse::<f32>().ok()).map(|z| (z / 100.0).clamp(0.5, 3.0)).unwrap_or(1.0),
            _ if s.path.is_none() => {
                let raw = path_from_uri(a).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| a.clone());
                let (p, anchor) = match raw.rsplit_once('#') {
                    Some((p, f)) if !Path::new(&raw).exists() => (p.to_string(), Some(f.to_lowercase())),
                    _ => (raw, None),
                };
                s.path = Some(std::fs::canonicalize(&p).unwrap_or_else(|_| PathBuf::from(p)));
                s.anchor = anchor;
            }
            _ => {}
        }
    }
    s
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let start = parse_args(&args);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 760.0])
            .with_min_inner_size([420.0, 320.0])
            .with_title(APP_NAME)
            .with_app_id(APP_ID)
            .with_decorations(false)
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(APP_NAME, options, Box::new(move |cc| Ok(Box::new(App::new(cc, start)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("saga-md-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn links_go_where_they_point() {
        let d = scratch("links");
        std::fs::write(d.join("other doc.md"), "# x").unwrap();
        assert_eq!(resolve(Some(&d), "#Some-Heading"), Target::Anchor("some-heading".into()));
        assert_eq!(resolve(Some(&d), "https://example.org/a"), Target::Web("https://example.org/a".into()));
        assert_eq!(resolve(Some(&d), "mailto:me@example.org"), Target::Web("mailto:me@example.org".into()));
        assert_eq!(resolve(Some(&d), "other%20doc.md#Part"), Target::File(d.join("other doc.md"), Some("part".into())));
        assert_eq!(resolve(Some(&d), &format!("file://{}/other%20doc.md", d.display())), Target::File(d.join("other doc.md"), None));
        assert!(matches!(resolve(Some(&d), "missing.md"), Target::Missing(_)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn markdown_opens_formatted_text_as_source() {
        assert_eq!(default_view(Path::new("/a/README.md")), View::Formatted);
        assert_eq!(default_view(Path::new("/a/notes.MARKDOWN")), View::Formatted);
        assert_eq!(default_view(Path::new("/a/README")), View::Formatted);
        assert_eq!(default_view(Path::new("/a/answers.txt")), View::Source);
        assert!(readable_here(Path::new("x.log")) && !readable_here(Path::new("x.png")));
    }

    #[test]
    fn saving_keeps_the_file_whole_and_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("save");
        let f = d.join("a.md");
        std::fs::write(&f, "old").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&f, "new text\n").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "new text\n");
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "no copy left behind");
        let doc = load(&f).unwrap();
        assert!(!doc.dirty() && doc.readonly.is_none());
        std::fs::write(d.join("bin.txt"), [0xff, 0xfe, b'a']).unwrap();
        assert!(load(&d.join("bin.txt")).unwrap().readonly.is_some(), "not UTF-8: shown, never saved");
        assert!(load(&d).is_err(), "a folder");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn command_line() {
        let s = parse_args(&["--source".into(), "--zoom".into(), "125".into(), "/nonexistent/a.md#Intro".into()]);
        assert_eq!(s, Start { path: Some(PathBuf::from("/nonexistent/a.md")), anchor: Some("intro".into()), source: true, zoom: 1.25 });
        assert_eq!(parse_args(&[]).path, None);
        assert_eq!(percent_decode("a%20b%2"), "a b%2");
    }
}
