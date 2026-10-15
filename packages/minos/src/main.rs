use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

mod icons;

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, RichText, Sense, UiBuilder};
use icons::Icon;
use labyrinth_ui::sysicon;
use labyrinth_ui::{chrome, theme};
use theme::{palette, palette_for, Palette};
use egui_extras::{Column, TableBuilder};
use minos_core::archive::{self, Format};
use minos_core::cloud::{self, Badge, Link, Report};
use minos_core::listing::{self, Entry, Sort, SortKey};
use minos_core::places::{self, CloudPlace, CloudState};
use minos_core::preview::{self, Body, PreviewData};
use minos_core::search::{display_relative, Search};
use minos_core::state::{SavedTab, State, ViewMode, GALLERY_SIZES};
use minos_core::{expand_address, fileops, History, Tab};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Drag-and-drop payload: the files being dragged.
struct DragPaths(Vec<PathBuf>);

enum Action {
    Focus(usize),
    Up(usize),
    TogglePreview,
    ExtractHere(Vec<PathBuf>),
    ExtractTo(Vec<PathBuf>),
    Compress(Vec<PathBuf>, Format),
    NewFile(usize),
    EditAddress(usize),
    Go(usize, PathBuf),
    OpenTab(usize, PathBuf),
    CloseTab(usize, usize),
    SelectTab(usize, usize),
    PinTab(usize, usize),
    Reload,
    OpenFile(PathBuf),
    Cut(Vec<PathBuf>),
    Copy(Vec<PathBuf>),
    Paste(usize),
    Rename(PathBuf),
    Trash(Vec<PathBuf>),
    NewFolder(usize),
    ToggleTag(PathBuf, String),
    SetColor(PathBuf, Option<[u8; 3]>),
    ToggleFav(PathBuf),
    MoveFav(PathBuf, i32),
    ResetFavs,
    Move(Vec<PathBuf>, PathBuf),
    CopyPath(PathBuf),
    Verify(String),
    ToggleSplit,
    SetView(ViewMode),
    ToggleTheme,
    LinkDialog,
    ProtonConnect,
}

struct Watch {
    path: PathBuf,
    _watcher: RecommendedWatcher,
    rx: Receiver<()>,
}

struct TabView {
    tab: Tab,
    entries: Vec<Entry>,
    error: Option<String>,
    query: String,
    address: String,
    selected: BTreeSet<PathBuf>,
    anchor: Option<usize>,
    recursive: bool,
    search: Option<Search>,
    results: Option<Vec<Entry>>,
    /// The address bar is showing an editable path instead of clickable segments.
    editing: bool,
    edit_fresh: bool,
}

impl TabView {
    fn new(path: PathBuf, pinned: bool, sort: Sort, hidden: bool) -> Self {
        let mut t = Self {
            tab: Tab { history: History::new(path), pinned },
            entries: vec![],
            error: None,
            query: String::new(),
            address: String::new(),
            selected: BTreeSet::new(),
            anchor: None,
            recursive: false,
            search: None,
            results: None,
            editing: false,
            edit_fresh: false,
        };
        t.reload(sort, hidden);
        t
    }

    fn dir(&self) -> PathBuf {
        self.tab.history.current().to_path_buf()
    }

    fn reload(&mut self, sort: Sort, _hidden: bool) {
        let dir = self.dir();
        self.address = dir.to_string_lossy().into_owned();
        if places::is_this_pc(&dir) {
            // The physical disks; with no lsblk, the mounted drives as before.
            let disks = places::disks();
            self.entries = if disks.is_empty() { listing::drive_entries(&places::drives()) } else { listing::disk_entries(&disks) };
            self.error = None;
            self.selected.clear();
            return;
        }
        if let Some(name) = places::disk_name(&dir) {
            // One disk: its partitions.
            self.entries = places::disks().iter().find(|d| d.name == name).map(listing::partition_entries).unwrap_or_default();
            self.error = None;
            self.selected.clear();
            return;
        }
        if let Some((arc, inner)) = archive::split(&dir) {
            match archive::list_cached(&arc) {
                Ok(list) => {
                    let mut e = archive::children(&arc, &list, &inner);
                    listing::sort_entries(&mut e, sort);
                    self.entries = e;
                    self.error = None;
                }
                Err(msg) => {
                    self.entries.clear();
                    self.error = Some(format!("{}: {msg}", arc.display()));
                }
            }
            let still: BTreeSet<PathBuf> = self.entries.iter().map(|e| e.path.clone()).collect();
            self.selected.retain(|p| still.contains(p));
            return;
        }
        match listing::read_dir(&dir) {
            Ok(mut e) => {
                listing::sort_entries(&mut e, sort);
                self.entries = e;
                self.error = None;
            }
            Err(err) => {
                self.entries.clear();
                self.error = Some(format!("{}: {err}", dir.display()));
            }
        }
        let still: BTreeSet<PathBuf> = self.entries.iter().map(|e| e.path.clone()).collect();
        self.selected.retain(|p| still.contains(p));
    }

    fn navigate(&mut self, path: PathBuf, sort: Sort, hidden: bool) {
        self.tab.history.visit(path);
        self.query.clear();
        self.selected.clear();
        self.anchor = None;
        self.search = None;
        self.results = None;
        self.reload(sort, hidden);
    }

    fn restart_search(&mut self, ctx: &egui::Context, hidden: bool) {
        self.search = None;
        self.results = None;
        if self.recursive && !self.query.trim().is_empty() {
            let ctx = ctx.clone();
            self.results = Some(vec![]);
            self.search = Some(Search::start(self.dir(), self.query.clone(), hidden, 5000, move || ctx.request_repaint()));
        }
    }

    fn poll_search(&mut self) {
        let (Some(s), Some(res)) = (&self.search, &mut self.results) else { return };
        let root = self.tab.history.current().to_path_buf();
        while let Ok(p) = s.rx.try_recv() {
            if let Ok(m) = std::fs::metadata(&p) {
                res.push(Entry {
                    name: display_relative(&root, &p),
                    hidden: false,
                    is_dir: m.is_dir(),
                    size: if m.is_dir() { 0 } else { m.len() },
                    modified: m.modified().ok(),
                    path: p,
                    usage: None,
                });
            }
        }
    }
}

struct Pane {
    tabs: Vec<TabView>,
    active: usize,
    sort: Sort,
    watch: Option<Watch>,
}

impl Pane {
    fn from_saved(saved: &[SavedTab], active: usize, fallback: PathBuf, hidden: bool) -> Self {
        let sort = Sort::default();
        let mut tabs: Vec<TabView> = saved
            .iter()
            .filter(|t| t.path.is_dir() || places::is_virtual(&t.path))
            .map(|t| TabView::new(t.path.clone(), t.pinned, sort, hidden))
            .collect();
        if tabs.is_empty() {
            tabs.push(TabView::new(fallback, false, sort, hidden));
        }
        let active = active.min(tabs.len() - 1);
        Self { tabs, active, sort, watch: None }
    }

    fn cur(&mut self) -> &mut TabView {
        &mut self.tabs[self.active]
    }

    fn ensure_watch(&mut self, ctx: &egui::Context) {
        let dir = self.tabs[self.active].dir();
        if self.watch.as_ref().is_some_and(|w| w.path == dir) {
            return;
        }
        self.watch = None;
        let (tx, rx) = channel();
        let c = ctx.clone();
        let Ok(mut w) = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
            if r.is_ok() {
                let _ = tx.send(());
                c.request_repaint();
            }
        }) else {
            return;
        };
        if w.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
            self.watch = Some(Watch { path: dir, _watcher: w, rx });
        }
    }

    /// Reload when the filesystem changed under us.
    fn poll_watch(&mut self, hidden: bool) {
        let mut dirty = false;
        if let Some(w) = &self.watch {
            while w.rx.try_recv().is_ok() {
                dirty = true;
            }
        }
        let sort = self.sort;
        let t = &mut self.tabs[self.active];
        t.poll_search();
        if dirty && t.results.is_none() {
            t.reload(sort, hidden);
        }
    }

    fn saved(&self) -> Vec<SavedTab> {
        self.tabs.iter().map(|t| SavedTab { path: t.dir(), pinned: t.tab.pinned }).collect()
    }
}

fn is_image(p: &Path) -> bool {
    matches!(
        p.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
    )
}

fn file_uri(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

fn open_with_system(path: &Path) {
    #[cfg(target_os = "linux")]
    let opener = "xdg-open";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    let _ = std::process::Command::new(opener).arg(path).spawn();
}

fn fmt_time(t: std::time::SystemTime) -> String {
    // Civil-from-days conversion on UTC; avoids a date crate for one column.
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

struct Ctx<'a> {
    st: &'a State,
    links: &'a [Link],
    reports: &'a BTreeMap<String, Report>,
    clip_has: bool,
    pi: usize,
}

/// Selection, drag source, drop target and context menu shared by both views.
fn entry_interact(
    ctx: &egui::Context,
    r: &egui::Response,
    e: &Entry,
    pos: usize,
    order: &[usize],
    shown: &[Entry],
    selected: &mut BTreeSet<PathBuf>,
    anchor: &mut Option<usize>,
    cx: &Ctx,
    actions: &mut Vec<Action>,
) {
    if r.clicked() || r.secondary_clicked() {
        actions.push(Action::Focus(cx.pi));
        let m = ctx.input(|i| i.modifiers);
        // Two slow clicks on the same, already selected item rename it (a quick pair is a double click and opens it).
        let was_only_selected = selected.len() == 1 && selected.contains(&e.path);
        let now = ctx.input(|i| i.time);
        let last_id = egui::Id::new("minos-last-click");
        if r.clicked() && !m.shift && !m.command {
            let last: Option<(PathBuf, f64)> = ctx.data(|d| d.get_temp(last_id));
            let slow = matches!(&last, Some((p, at)) if *p == e.path && (0.5..=1.8).contains(&(now - at)));
            if was_only_selected && slow && e.usage.is_none() {
                actions.push(Action::Rename(e.path.clone()));
                ctx.data_mut(|d| d.remove::<(PathBuf, f64)>(last_id));
            } else {
                ctx.data_mut(|d| d.insert_temp(last_id, (e.path.clone(), now)));
            }
        }
        if r.secondary_clicked() && selected.contains(&e.path) {
            // keep the existing multi-selection for the context menu
        } else if m.shift && anchor.is_some() {
            let a = anchor.unwrap().min(order.len().saturating_sub(1));
            let (lo, hi) = (a.min(pos), a.max(pos));
            selected.clear();
            for &i in &order[lo..=hi] {
                selected.insert(shown[i].path.clone());
            }
        } else if m.command && r.clicked() {
            if !selected.remove(&e.path) {
                selected.insert(e.path.clone());
            }
            *anchor = Some(pos);
        } else {
            selected.clear();
            selected.insert(e.path.clone());
            *anchor = Some(pos);
        }
    }
    if r.double_clicked() {
        if e.is_dir || archive::is_archive_file(&e.path) {
            actions.push(Action::Go(cx.pi, e.path.clone()));
        } else {
            actions.push(Action::OpenFile(e.path.clone()));
        }
    }
    if r.middle_clicked() && e.is_dir {
        actions.push(Action::OpenTab(cx.pi, e.path.clone()));
    }
    if r.drag_started() && e.usage.is_none() {
        if !selected.contains(&e.path) {
            selected.clear();
            selected.insert(e.path.clone());
        }
        egui::DragAndDrop::set_payload(ctx, DragPaths(selected.iter().cloned().collect()));
    }
    if e.is_dir {
        if egui::DragAndDrop::has_payload_of_type::<DragPaths>(ctx) && r.contains_pointer() {
            ctx.layer_painter(r.layer_id).rect_stroke(r.interact_rect, 6, egui::Stroke::new(2.0, Color32::from_rgb(124, 84, 214)), egui::StrokeKind::Inside);
        }
        if let Some(p) = r.dnd_release_payload::<DragPaths>() {
            actions.push(Action::Move(p.0.clone(), e.path.clone()));
        }
    }

    let targets: Vec<PathBuf> = if selected.contains(&e.path) { selected.iter().cloned().collect() } else { vec![e.path.clone()] };
    r.context_menu(|ui| {
        if e.is_dir {
            if ui.button("Open").clicked() {
                actions.push(Action::Go(cx.pi, e.path.clone()));
                ui.close();
            }
            if ui.button("Open in new tab").clicked() {
                actions.push(Action::OpenTab(cx.pi, e.path.clone()));
                ui.close();
            }
        } else if ui.button("Open").clicked() {
            actions.push(Action::OpenFile(e.path.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button("Cut").clicked() {
            actions.push(Action::Cut(targets.clone()));
            ui.close();
        }
        if ui.button("Copy").clicked() {
            actions.push(Action::Copy(targets.clone()));
            ui.close();
        }
        if ui.add_enabled(cx.clip_has, egui::Button::new("Paste")).clicked() {
            actions.push(Action::Paste(cx.pi));
            ui.close();
        }
        if ui.add_enabled(targets.len() == 1, egui::Button::new("Rename")).clicked() {
            actions.push(Action::Rename(e.path.clone()));
            ui.close();
        }
        if ui.button("Move to trash").clicked() {
            actions.push(Action::Trash(targets.clone()));
            ui.close();
        }
        if ui.button("Copy path").clicked() {
            actions.push(Action::CopyPath(e.path.clone()));
            ui.close();
        }
        // Archives (7-Zip): extract from one, or compress the selection into one.
        if e.usage.is_none() && !places::is_this_pc(&e.path) {
            let inside = archive::split(&e.path).is_some();
            if archive::is_archive_file(&e.path) {
                ui.separator();
                if ui.button("Extract here").clicked() {
                    actions.push(Action::ExtractHere(targets.clone()));
                    ui.close();
                }
                if ui.button("Extract to a new folder").clicked() {
                    actions.push(Action::ExtractTo(targets.clone()));
                    ui.close();
                }
            } else if inside {
                ui.separator();
                if ui.button("Extract to a new folder").clicked() {
                    actions.push(Action::ExtractTo(targets.clone()));
                    ui.close();
                }
            } else {
                ui.separator();
                ui.menu_button("Compress to", |ui| {
                    if ui.button("ZIP").clicked() {
                        actions.push(Action::Compress(targets.clone(), Format::Zip));
                        ui.close();
                    }
                    if ui.button("7z").clicked() {
                        actions.push(Action::Compress(targets.clone(), Format::SevenZ));
                        ui.close();
                    }
                });
            }
        }
        ui.separator();
        ui.menu_button("Tags", |ui| {
            for t in &cx.st.tag_defs {
                let on = cx.st.tags.get(&e.path).is_some_and(|v| v.contains(&t.name));
                let label = RichText::new(format!("{} {}", if on { "\u{2714}" } else { "   " }, t.name)).color(rgb(t.color));
                if ui.button(label).clicked() {
                    for p in &targets {
                        actions.push(Action::ToggleTag(p.clone(), t.name.clone()));
                    }
                    ui.close();
                }
            }
        });
        if e.is_dir {
            ui.menu_button("Folder color", |ui| {
                for t in &cx.st.tag_defs {
                    if ui.button(RichText::new(&t.name).color(rgb(t.color))).clicked() {
                        actions.push(Action::SetColor(e.path.clone(), Some(t.color)));
                        ui.close();
                    }
                }
                if ui.button("Clear").clicked() {
                    actions.push(Action::SetColor(e.path.clone(), None));
                    ui.close();
                }
            });
            let fav = cx.st.favorites.contains(&e.path);
            if ui.button(if fav { "Remove from favorites" } else { "Add to favorites" }).clicked() {
                actions.push(Action::ToggleFav(e.path.clone()));
                ui.close();
            }
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Look and feel, after EdenExplorer: icon toolbar, breadcrumb address bar, purple tabs,
// a sidebar of Places / Favorites / Storage (with usage bars), and a "This PC" view.
// ---------------------------------------------------------------------------------------------

fn icon_button(ui: &mut egui::Ui, pal: &Palette, icon: Icon, tip: &str, enabled: bool, active: bool) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), sense);
    let r = CornerRadius::same(8);
    if active {
        ui.painter().rect_filled(rect, r, pal.select);
    } else if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, r, pal.hover);
    }
    let col = if enabled { pal.text } else { pal.dim.gamma_multiply(0.45) };
    icons::paint(ui.painter(), rect.shrink(7.0), icon, col);
    resp.on_hover_text(tip)
}

fn entry_icon(ui: &mut egui::Ui, pal: &Palette, st: &State, e: &Entry, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let tint = st.color_for(&e.path).map(rgb);
    let (icon, color) = if e.usage.is_some() {
        (Icon::Drive, pal.drive)
    } else if e.is_dir {
        (Icon::Folder, tint.unwrap_or(pal.folder))
    } else if archive::is_archive_name(&e.path) {
        (Icon::Archive, tint.unwrap_or(pal.amber))
    } else {
        (Icon::File, tint.unwrap_or(pal.file))
    };
    if tint.is_none() && sys_entry_icon(ui.painter(), rect, e) {
        return;
    }
    icons::paint(ui.painter(), rect, icon, color);
}

/// The system theme's icon for a listing entry (a tinted folder keeps the drawn one, so colour tags still show).
fn sys_entry_icon(p: &egui::Painter, rect: Rect, e: &Entry) -> bool {
    let name = e.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if e.usage.is_some() {
        false // drives keep the drawn drive icon
    } else if e.is_dir {
        sysicon::paint(p, rect, &sysicon::folder_names(&name))
    } else if archive::is_archive_name(&e.path) {
        sysicon::paint(p, rect, &["application-zip", "package-x-generic"])
    } else {
        sysicon::paint(p, rect, &sysicon::file_names(&name))
    }
}

/// Sync-status badge, drawn as shapes (no font needed).
fn paint_badge(p: &egui::Painter, center: egui::Pos2, b: Badge, pal: &Palette) {
    let (fill, kind) = match b {
        Badge::Synced => (pal.green, 0),
        Badge::SizeOnly => (pal.amber, 1),
        Badge::Problem => (pal.red, 2),
        Badge::Unverified => (pal.dim, 3),
        Badge::ContainsLink => (Color32::from_rgb(110, 150, 220), 3),
    };
    p.circle_filled(center, 6.5, fill);
    let w = egui::Stroke::new(1.6, Color32::WHITE);
    match kind {
        0 => {
            p.add(egui::Shape::line(vec![center + vec2(-3.0, 0.2), center + vec2(-0.8, 2.6), center + vec2(3.2, -2.4)], w));
        }
        1 => {
            p.line_segment([center + vec2(0.0, -3.4), center + vec2(0.0, 0.6)], w);
            p.circle_filled(center + vec2(0.0, 3.0), 1.0, Color32::WHITE);
        }
        2 => {
            p.line_segment([center + vec2(-2.6, -2.6), center + vec2(2.6, 2.6)], w);
            p.line_segment([center + vec2(2.6, -2.6), center + vec2(-2.6, 2.6)], w);
        }
        _ => {
            p.circle_filled(center, 1.6, Color32::WHITE);
        }
    }
}

fn badge_tip(b: Badge) -> &'static str {
    match b {
        Badge::Synced => "Synced with Proton Drive (verified by hash)",
        Badge::SizeOnly => "Same size as Proton Drive, but contents could not be hash-checked",
        Badge::Problem => "Differs from Proton Drive (see Verify now)",
        Badge::Unverified => "Linked to Proton Drive; not verified yet",
        Badge::ContainsLink => "Contains a folder linked to Proton Drive",
    }
}

fn name_text(e: &Entry, st: &State, pal: &Palette) -> RichText {
    let mut t = RichText::new(&e.name).color(pal.text);
    if let Some(c) = st.color_for(&e.path) {
        t = t.color(rgb(c));
    }
    t
}

fn tab_title(dir: &Path) -> String {
    if places::is_this_pc(dir) {
        return places::pc_name();
    }
    if let Some(name) = places::disk_name(dir) {
        return name;
    }
    // The dock's Trash opens this folder.
    if dir.ends_with("Trash/files") {
        return "Trash".into();
    }
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.to_string_lossy().into_owned())
}

fn tab_strip(ui: &mut egui::Ui, pane: &Pane, pi: usize, pal: &Palette, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 0.0);
        for i in 0..pane.tabs.len() {
            let t = &pane.tabs[i];
            let dir = t.dir();
            let title = tab_title(&dir);
            let galley = ui.painter().layout_no_wrap(title, FontId::proportional(13.5), pal.text);
            let pin_w = if t.tab.pinned { 14.0 } else { 0.0 };
            let w = (10.0 + 18.0 + 6.0 + pin_w + galley.size().x + 8.0 + 20.0 + 6.0).min(230.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
            let active = i == pane.active;
            let fill = if active {
                pal.tab_active
            } else if resp.hovered() {
                pal.hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, fill);
            let icon = if places::is_this_pc(&dir) { Icon::Computer } else if places::is_virtual(&dir) { Icon::Drive } else { Icon::Folder };
            let icon_col = if active { pal.text } else if places::is_virtual(&dir) { pal.dim } else { pal.folder };
            let tab_rect = Rect::from_min_size(rect.min + vec2(10.0, 7.0), vec2(18.0, 18.0));
            let dir_name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let sys_names = if places::is_virtual(&dir) { vec![] } else { sysicon::folder_names(&dir_name) };
            if sys_names.is_empty() || !sysicon::paint(ui.painter(), tab_rect, &sys_names) {
                icons::paint(ui.painter(), tab_rect, icon, icon_col);
            }
            let mut x = rect.left() + 10.0 + 18.0 + 6.0;
            if t.tab.pinned {
                icons::paint(ui.painter(), Rect::from_min_size(pos2(x - 2.0, rect.top() + 9.0), vec2(14.0, 14.0)), Icon::Pin, pal.accent);
                x += 14.0;
            }
            let clip = Rect::from_min_max(pos2(x, rect.top()), pos2(rect.right() - 26.0, rect.bottom()));
            ui.painter().with_clip_rect(clip).galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, pal.text);
            // close button
            let cr = Rect::from_center_size(pos2(rect.right() - 15.0, rect.center().y), vec2(18.0, 18.0));
            let cresp = ui.interact(cr, resp.id.with("close"), Sense::click());
            if cresp.hovered() {
                ui.painter().circle_filled(cr.center(), 9.0, pal.hover.gamma_multiply(1.6));
            }
            icons::paint(ui.painter(), cr.shrink(3.0), Icon::Close, if active { pal.text } else { pal.dim });

            if cresp.clicked() || resp.middle_clicked() {
                actions.push(Action::CloseTab(pi, i));
            } else if resp.clicked() {
                actions.push(Action::SelectTab(pi, i));
            }
            if let Some(p) = resp.dnd_release_payload::<DragPaths>() {
                actions.push(Action::Move(p.0.clone(), dir.clone()));
            }
            let pinned = t.tab.pinned;
            resp.context_menu(|ui| {
                if ui.button(if pinned { "Unpin tab" } else { "Pin tab" }).clicked() {
                    actions.push(Action::PinTab(pi, i));
                    ui.close();
                }
                if ui.button("Close tab").clicked() {
                    actions.push(Action::CloseTab(pi, i));
                    ui.close();
                }
            });
        }
        if icon_button(ui, pal, Icon::Plus, "New tab (Ctrl+T)", true, false).clicked() {
            actions.push(Action::OpenTab(pi, pane.tabs[pane.active].dir()));
        }
    });
}

fn crumbs(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v = vec![(places::pc_name(), places::this_pc())];
    if places::is_this_pc(dir) {
        return v;
    }
    if let Some(name) = places::disk_name(dir) {
        v.push((name, dir.to_path_buf()));
        return v;
    }
    let mut chain: Vec<PathBuf> = dir.ancestors().map(Path::to_path_buf).collect();
    chain.reverse();
    for p in chain {
        if p.as_os_str().is_empty() {
            continue;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned());
        v.push((name, p));
    }
    v
}

/// The address bar: clickable path segments (also drop targets); click the empty part to type a path.
fn breadcrumb(ui: &mut egui::Ui, pal: &Palette, t: &mut TabView, pi: usize, width: f32, actions: &mut Vec<Action>) {
    let (rect, bg) = ui.allocate_exact_size(vec2(width.max(120.0), 30.0), Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(9), pal.field);
    ui.painter().rect_stroke(rect, CornerRadius::same(9), egui::Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);

    if t.editing {
        let inner = rect.shrink2(vec2(8.0, 3.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
        let r = child.add(egui::TextEdit::singleline(&mut t.address).frame(egui::Frame::NONE).desired_width(inner.width()));
        if t.edit_fresh {
            r.request_focus();
            t.edit_fresh = false;
        }
        let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if r.lost_focus() || esc {
            if enter && !esc {
                let expanded = expand_address(&t.address, |k| std::env::var(k).ok());
                actions.push(Action::Go(pi, PathBuf::from(expanded)));
            }
            t.editing = false;
        }
        return;
    }

    let dir = t.dir();
    let segs = crumbs(&dir);
    let font = FontId::proportional(14.0);
    let galleys: Vec<_> = segs.iter().map(|(n, _)| ui.painter().layout_no_wrap(n.clone(), font.clone(), pal.text)).collect();
    let seg_w = |i: usize| galleys[i].size().x + 14.0 + if i == 0 { 22.0 } else { 0.0 };
    let avail = rect.width() - 20.0;
    let total = |from: usize| -> f32 { (from..segs.len()).map(|i| seg_w(i) + 14.0).sum::<f32>() + if from > 0 { 20.0 } else { 0.0 } };
    let mut start = 0;
    while start + 1 < segs.len() && total(start) > avail {
        start += 1;
    }

    let mut x = rect.left() + 10.0;
    let cy = rect.center().y;
    if start > 0 {
        ui.painter().text(pos2(x, cy), Align2::LEFT_CENTER, "...", font.clone(), pal.dim);
        x += 20.0;
    }
    for i in start..segs.len() {
        let w = seg_w(i);
        let srect = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(w, 24.0));
        let r = ui.interact(srect, bg.id.with(("crumb", i)), Sense::click());
        if r.hovered() {
            ui.painter().rect_filled(srect, CornerRadius::same(6), pal.hover);
        }
        let mut tx = srect.left() + 7.0;
        if i == 0 {
            icons::paint(ui.painter(), Rect::from_min_size(pos2(tx, cy - 8.0), vec2(16.0, 16.0)), Icon::Computer, pal.dim);
            tx += 22.0;
        }
        ui.painter().galley(pos2(tx, cy - galleys[i].size().y / 2.0), galleys[i].clone(), pal.text);
        if r.clicked() {
            actions.push(Action::Go(pi, segs[i].1.clone()));
        }
        if let Some(d) = r.dnd_release_payload::<DragPaths>() {
            actions.push(Action::Move(d.0.clone(), segs[i].1.clone()));
        }
        x += w;
        if i + 1 < segs.len() {
            icons::paint(ui.painter(), Rect::from_min_size(pos2(x - 2.0, cy - 7.0), vec2(14.0, 14.0)), Icon::Chevron, pal.dim);
            x += 14.0;
        }
    }
    if bg.clicked() {
        t.editing = true;
        t.edit_fresh = true;
        t.address = if places::is_this_pc(&dir) { String::new() } else { dir.to_string_lossy().into_owned() };
    }
}

fn toolbar(ui: &mut egui::Ui, pane: &mut Pane, pi: usize, pal: &Palette, st: &State, clip_has: bool, actions: &mut Vec<Action>) {
    let sort = pane.sort;
    let hidden = st.show_hidden;
    let active = pane.active;
    let t = &mut pane.tabs[active];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon_button(ui, pal, Icon::Back, "Back (Alt+Left)", t.tab.history.can_back(), false).clicked() && t.tab.history.back().is_some() {
            t.search = None;
            t.results = None;
            t.reload(sort, hidden);
        }
        if icon_button(ui, pal, Icon::Forward, "Forward (Alt+Right)", t.tab.history.can_forward(), false).clicked() && t.tab.history.forward().is_some() {
            t.search = None;
            t.results = None;
            t.reload(sort, hidden);
        }
        let can_up = !places::is_this_pc(&t.dir());
        if icon_button(ui, pal, Icon::Up, "Up (Alt+Up / Backspace)", can_up, false).clicked() {
            actions.push(Action::Up(pi));
        }
        if icon_button(ui, pal, Icon::Refresh, "Refresh (F5)", true, false).clicked() {
            actions.push(Action::Reload);
        }
        ui.add_space(6.0);
        let in_pc = places::is_virtual(&t.dir());
        if icon_button(ui, pal, Icon::NewFolder, "New folder (Ctrl+Shift+N)", !in_pc, false).clicked() {
            actions.push(Action::NewFolder(pi));
        }
        if icon_button(ui, pal, Icon::NewFile, "New file", !in_pc, false).clicked() {
            actions.push(Action::NewFile(pi));
        }
        let dir = t.dir();
        let fav = st.favorites.contains(&dir);
        if icon_button(ui, pal, if fav { Icon::StarFilled } else { Icon::Star }, if fav { "Remove from favorites" } else { "Add to favorites" }, !in_pc, fav).clicked() {
            actions.push(Action::ToggleFav(dir));
        }
        if clip_has && !in_pc && icon_button(ui, pal, Icon::NewFile, "Paste (Ctrl+V)", true, false).clicked() {
            actions.push(Action::Paste(pi));
        }
        ui.add_space(6.0);
        let search_w = 210.0;
        let w = ui.available_width() - search_w - 90.0 - 108.0;
        breadcrumb(ui, pal, t, pi, w, actions);
        let q = ui.add(egui::TextEdit::singleline(&mut t.query).hint_text("Search").desired_width(search_w));
        if q.changed() {
            t.restart_search(ui.ctx(), hidden);
        }
        if ui.checkbox(&mut t.recursive, "Subfolders").on_hover_text("Search inside subfolders too").changed() {
            t.restart_search(ui.ctx(), hidden);
        }
        if icon_button(ui, pal, Icon::List, "Details view", true, st.view == ViewMode::Details).clicked() {
            actions.push(Action::SetView(ViewMode::Details));
        }
        if icon_button(ui, pal, Icon::Grid, "Gallery view", true, st.view == ViewMode::Gallery).clicked() {
            actions.push(Action::SetView(ViewMode::Gallery));
        }
        if icon_button(ui, pal, Icon::Preview, "Preview pane (F3)", true, st.preview).clicked() {
            actions.push(Action::TogglePreview);
        }
    });
}

fn header_cell(ui: &mut egui::Ui, pal: &Palette, label: &str, key: Option<SortKey>, sort: Sort, new_sort: &mut Sort) {
    ui.horizontal(|ui| {
        let r = ui.add(egui::Label::new(RichText::new(label).strong().color(pal.text)).sense(if key.is_some() { Sense::click() } else { Sense::hover() }));
        if let Some(k) = key {
            if sort.key == k {
                let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                icons::paint(ui.painter(), rect, if sort.descending { Icon::SortDown } else { Icon::SortUp }, pal.text);
            }
            if r.clicked() {
                *new_sort = Sort { key: k, descending: sort.key == k && !sort.descending };
            }
        }
    });
}

fn usage_bar(ui: &mut egui::Ui, pal: &Palette, total: u64, free: u64, width: f32) {
    let frac = listing::used_fraction(total, free);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 16.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(3), pal.border.gamma_multiply(0.5));
    let mut fill = rect;
    fill.set_width(rect.width() * frac);
    ui.painter().rect_filled(fill, CornerRadius::same(3), theme::usage_color(pal, frac));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, format!("{:.0}%", frac * 100.0), FontId::proportional(11.0), Color32::WHITE);
}

fn draw_pane(ui: &mut egui::Ui, pane: &mut Pane, pi: usize, focused: bool, st: &State, cloud_ctx: (&[Link], &BTreeMap<String, Report>), clip_has: bool, actions: &mut Vec<Action>) {
    if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(ui.max_rect()) {
        actions.push(Action::Focus(pi));
    }
    let hidden = st.show_hidden;
    let pal = palette_for(st.dark, st.accent());
    let cx = Ctx { st, links: cloud_ctx.0, reports: cloud_ctx.1, clip_has, pi };

    tab_strip(ui, pane, pi, &pal, actions);
    ui.add_space(2.0);
    toolbar(ui, pane, pi, &pal, st, clip_has, actions);
    ui.add_space(4.0);

    let sort = pane.sort;
    let active = pane.active;
    let t = &mut pane.tabs[active];
    if let Some(err) = &t.error {
        ui.colored_label(pal.red, err);
        return;
    }

    let searching = t.results.is_some();
    let in_pc = places::is_virtual(&t.dir());
    let in_archive = archive::split(&t.dir()).is_some();
    let TabView { entries, results, query, selected, anchor, .. } = t;
    let shown: &[Entry] = results.as_deref().unwrap_or(entries);
    let order: Vec<usize> = if searching || in_pc { (0..shown.len()).collect() } else { listing::visible(shown, query, hidden) };
    let mut new_sort = sort;

    match st.view {
        ViewMode::Details => {
            // The row, not the name, takes the clicks and drags (see entry_interact below).
            let ectx = ui.ctx().clone();
            let name_cell = |ui: &mut egui::Ui, e: &Entry| {
                ui.horizontal(|ui| {
                    entry_icon(ui, &pal, st, e, 18.0);
                    if let Some(b) = cloud::badge_for(&e.path, cx.links, cx.reports) {
                        let (rect, r) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        paint_badge(ui.painter(), rect.center(), b, &pal);
                        r.on_hover_text(badge_tip(b));
                    }
                    ui.add(egui::Label::new(name_text(e, st, &pal)).selectable(false));
                });
            };
            if in_pc {
                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .sense(Sense::click_and_drag())
                    .column(Column::initial(340.0).at_least(160.0).clip(true))
                    .column(Column::initial(90.0).at_least(60.0))
                    .column(Column::initial(210.0).at_least(120.0))
                    .column(Column::remainder().at_least(110.0))
                    .header(26.0, |mut h| {
                        h.col(|ui| header_cell(ui, &pal, "Name", None, sort, &mut new_sort));
                        h.col(|ui| header_cell(ui, &pal, "Type", None, sort, &mut new_sort));
                        h.col(|ui| header_cell(ui, &pal, "Size", None, sort, &mut new_sort));
                        h.col(|ui| header_cell(ui, &pal, "Usage", None, sort, &mut new_sort));
                    })
                    .body(|body| {
                        body.rows(30.0, order.len(), |mut row| {
                            let pos = row.index();
                            let e = &shown[order[pos]];
                            row.set_selected(selected.contains(&e.path));
                            row.col(|ui| name_cell(ui, e));
                            row.col(|ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.label(RichText::new("Folder").color(pal.dim));
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    if let Some((total, free)) = e.usage {
                                        ui.add(egui::Label::new(RichText::new(format!("{} / {}", listing::human_size(free), listing::human_size(total))).color(pal.dim)).truncate());
                                    }
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    if let Some((total, free)) = e.usage {
                                        usage_bar(ui, &pal, total, free, (ui.available_width() - 14.0).clamp(80.0, 220.0));
                                    }
                                });
                            });
                            let r = row.response();
                            entry_interact(&ectx, &r, e, pos, &order, shown, selected, anchor, &cx, actions);
                        });
                    });
            } else {
                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .sense(Sense::click_and_drag())
                    .column(Column::initial(420.0).at_least(160.0).clip(true))
                    .column(Column::initial(160.0).at_least(80.0))
                    .column(Column::initial(120.0).at_least(60.0))
                    .column(Column::remainder().at_least(70.0))
                    .header(26.0, |mut h| {
                        h.col(|ui| header_cell(ui, &pal, "Name", Some(SortKey::Name), sort, &mut new_sort));
                        h.col(|ui| header_cell(ui, &pal, "Date modified", Some(SortKey::Modified), sort, &mut new_sort));
                        h.col(|ui| header_cell(ui, &pal, "Type", None, sort, &mut new_sort));
                        h.col(|ui| header_cell(ui, &pal, "Size", Some(SortKey::Size), sort, &mut new_sort));
                    })
                    .body(|body| {
                        body.rows(28.0, order.len(), |mut row| {
                            let pos = row.index();
                            let e = &shown[order[pos]];
                            row.set_selected(selected.contains(&e.path));
                            row.col(|ui| name_cell(ui, e));
                            row.col(|ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.label(RichText::new(e.modified.map(fmt_time).unwrap_or_default()).color(pal.dim));
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.label(RichText::new(listing::type_name(e)).color(pal.dim));
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.label(RichText::new(if e.is_dir { String::new() } else { listing::human_size(e.size) }).color(pal.dim));
                                });
                            });
                            let r = row.response();
                            entry_interact(&ectx, &r, e, pos, &order, shown, selected, anchor, &cx, actions);
                        });
                    });
            }
        }
        ViewMode::Gallery => {
            let size = GALLERY_SIZES[st.gallery_size.min(GALLERY_SIZES.len() - 1)];
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (pos, &i) in order.iter().enumerate() {
                        let e = &shown[i];
                        let cell = vec2(size + 12.0, size + 34.0);
                        let (rect, _) = ui.allocate_exact_size(cell, Sense::hover());
                        let r = ui.interact(rect, ui.id().with(("cell", pos)), Sense::click_and_drag());
                        if selected.contains(&e.path) {
                            ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
                        } else if r.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
                        }
                        let img_rect = Rect::from_min_size(rect.min + vec2(6.0, 4.0), vec2(size, size));
                        let tint = st.color_for(&e.path).map(rgb);
                        if !e.is_dir && !in_archive && is_image(&e.path) && ui.is_rect_visible(rect) {
                            egui::Image::new(file_uri(&e.path)).fit_to_exact_size(img_rect.size()).corner_radius(6).paint_at(ui, img_rect);
                        } else {
                            let (icon, color) = if e.usage.is_some() {
                                (Icon::Drive, pal.drive)
                            } else if e.is_dir {
                                (Icon::Folder, tint.unwrap_or(pal.folder))
                            } else {
                                (Icon::File, tint.unwrap_or(pal.file))
                            };
                            if !(tint.is_none() && sys_entry_icon(ui.painter(), img_rect, e)) {
                                icons::paint(ui.painter(), img_rect.shrink(size * 0.08), icon, color);
                            }
                        }
                        if let Some(b) = cloud::badge_for(&e.path, cx.links, cx.reports) {
                            paint_badge(ui.painter(), rect.right_top() + vec2(-10.0, 12.0), b, &pal);
                        }
                        let mut shown_name = e.name.clone();
                        let max_chars = ((size + 12.0) / 7.0) as usize;
                        if shown_name.chars().count() > max_chars {
                            shown_name = shown_name.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "...";
                        }
                        ui.painter().text(pos2(rect.center().x, rect.bottom() - 16.0), Align2::CENTER_CENTER, shown_name, FontId::proportional(13.0), tint.unwrap_or(pal.text));
                        let r = r.on_hover_text(&e.name);
                        entry_interact(ui.ctx(), &r, e, pos, &order, shown, selected, anchor, &cx, actions);
                    }
                });
            });
        }
    }
    if new_sort.key != sort.key || new_sort.descending != sort.descending {
        actions.push(Action::Reload);
        pane.sort = new_sort;
    }
    ui.add_space(4.0);
    let nsel = pane.tabs[pane.active].selected.len();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} items", order.len())).color(pal.dim));
        if nsel > 0 {
            ui.label(RichText::new(format!("{nsel} selected")).color(pal.dim));
        }
        if searching {
            ui.label(RichText::new("search results").color(pal.dim));
        }
        let _ = focused;
    });
}


/// What the preview pane is showing, and the worker that fills it in.
struct PreviewState {
    /// The selection (or folder) the pane is for.
    key: Option<Vec<PathBuf>>,
    data: Option<Arc<PreviewData>>,
    texture: Option<egui::TextureHandle>,
    loading: bool,
    tx: Sender<(Vec<PathBuf>, PreviewData)>,
    rx: Receiver<(Vec<PathBuf>, PreviewData)>,
    cache: HashMap<(PathBuf, u64), Arc<PreviewData>>,
}

impl PreviewState {
    fn new() -> Self {
        let (tx, rx) = channel();
        Self { key: None, data: None, texture: None, loading: false, tx, rx, cache: HashMap::new() }
    }

    fn set(&mut self, ctx: &egui::Context, d: Arc<PreviewData>) {
        self.texture = match &d.body {
            Body::Image { width, height, rgba } => {
                let img = egui::ColorImage::from_rgba_unmultiplied([*width as usize, *height as usize], rgba);
                Some(ctx.load_texture("minos-preview", img, egui::TextureOptions::LINEAR))
            }
            _ => None,
        };
        self.data = Some(d);
        self.loading = false;
    }

    fn mtime_key(p: &Path) -> u64 {
        std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
    }

    /// Start (or satisfy from cache) the preview for `want`.
    fn request(&mut self, ctx: &egui::Context, want: Vec<PathBuf>, drive: Option<PreviewData>) {
        if let Some(d) = drive {
            self.set(ctx, Arc::new(d));
            return;
        }
        if want.len() > 1 {
            self.set(ctx, Arc::new(preview::summarize(&want)));
            return;
        }
        let Some(path) = want.first().cloned() else { return };
        let ck = (path.clone(), Self::mtime_key(&path));
        if let Some(hit) = self.cache.get(&ck).cloned() {
            self.set(ctx, hit);
            return;
        }
        self.loading = true;
        let (tx, c) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let d = preview::build(&path, 640);
            let _ = tx.send((want, d));
            c.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((key, data)) = self.rx.try_recv() {
            if self.key.as_ref() == Some(&key) {
                let d = Arc::new(data);
                if let Some(p) = key.first() {
                    if self.cache.len() > 24 {
                        self.cache.clear();
                    }
                    self.cache.insert((p.clone(), Self::mtime_key(p)), d.clone());
                }
                self.set(ctx, d);
            }
        }
    }
}

fn draw_preview(ui: &mut egui::Ui, pal: &Palette, ps: &PreviewState) {
    ui.set_clip_rect(ui.max_rect());
    let Some(d) = &ps.data else {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| ui.label(RichText::new("Select a file to preview it").color(pal.dim)));
        return;
    };
    let w = ui.available_width();
    ui.add(egui::Label::new(RichText::new(&d.title).strong().size(15.0).color(pal.text)).truncate());
    ui.add(egui::Label::new(RichText::new(&d.subtitle).size(12.5).color(pal.dim)).truncate());
    if ps.loading {
        ui.label(RichText::new("Loading...").color(pal.dim));
    }
    ui.add_space(8.0);

    // The picture or excerpt: always fitted into the pane, never scrolled.
    let box_h = 280.0_f32;
    match (&d.body, &ps.texture) {
        (Body::Image { width, height, .. }, Some(tex)) => {
            let scale = (w / *width as f32).min(box_h / *height as f32).min(2.0);
            let size = vec2(*width as f32 * scale, *height as f32 * scale);
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new((tex.id(), size)).corner_radius(8));
            });
        }
        (Body::Text { text, truncated }, _) => {
            egui::Frame::new().fill(pal.field).corner_radius(8).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                let h = box_h - 16.0;
                ui.set_height(h);
                ui.set_width(w - 16.0);
                let clip = Rect::from_min_size(ui.cursor().min, vec2(w - 16.0, h));
                ui.set_clip_rect(clip.intersect(ui.clip_rect()));
                let mut shown = text.clone();
                if *truncated {
                    shown.push_str("\n...");
                }
                ui.add(egui::Label::new(RichText::new(shown).monospace().size(11.5).color(pal.text)).wrap_mode(egui::TextWrapMode::Wrap));
            });
        }
        (Body::Lines(lines), _) => {
            for l in lines {
                ui.add(egui::Label::new(RichText::new(l).size(13.0).color(pal.text)).truncate());
            }
        }
        (Body::Message(m), _) => {
            ui.label(RichText::new(m).italics().color(pal.dim));
        }
        (Body::Drive { total, free }, _) => {
            usage_bar(ui, pal, *total, *free, (w - 4.0).max(60.0));
            ui.label(RichText::new(format!("{} free of {}", listing::human_size(*free), listing::human_size(*total))).color(pal.dim));
        }
        _ => {}
    }

    if !d.details.is_empty() {
        ui.add_space(10.0);
        ui.label(RichText::new("Details").strong().size(12.5).color(pal.dim));
        ui.add_space(2.0);
        egui::Grid::new("preview-details").num_columns(2).spacing(vec2(10.0, 4.0)).show(ui, |ui| {
            for (k, v) in &d.details {
                ui.label(RichText::new(k).size(12.5).color(pal.dim));
                ui.add(egui::Label::new(RichText::new(v).size(12.5).color(pal.text)).wrap());
                ui.end_row();
            }
        });
    }
}

struct RenameDlg {
    path: PathBuf,
    text: String,
    /// True until the first frame has taken keyboard focus. Requesting focus every frame
    /// stops egui ever reporting the field as having lost focus, so Enter did nothing.
    fresh: bool,
}

struct App {
    panes: [Pane; 2],
    focus: usize,
    st: State,
    st_path: Option<PathBuf>,
    last_saved: Instant,
    dirty: bool,
    clipboard: Option<(Vec<PathBuf>, bool)>,
    rename: Option<RenameDlg>,
    status: String,
    drives: Vec<places::Drive>,
    disks: Vec<places::Disk>,
    cloud: Option<CloudPlace>,
    links: Vec<Link>,
    reports: BTreeMap<String, Report>,
    verifying: Vec<String>,
    verify_tx: Sender<(String, Result<Report, String>)>,
    verify_rx: Receiver<(String, Result<Report, String>)>,
    link_form: Option<(String, String, String)>,
    /// Proton Drive sign-in form: email, password, mailbox password, 2FA code.
    proton_form: Option<[String; 4]>,
    proton_busy: bool,
    proton_tx: Sender<Result<(), String>>,
    proton_rx: Receiver<Result<(), String>>,
    applied_theme: Option<(bool, Option<[u8; 3]>)>,
    accent_checked: Instant,
    pv: PreviewState,
}

impl App {
    fn new() -> Self {
        let st_path = State::config_path();
        let mut st = st_path.as_deref().map(State::load_from).unwrap_or_default();
        if st.favorites.is_empty() {
            st.favorites = places::favorites().into_iter().map(|p| p.path).collect();
        }
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let links = cloud::links_path().map(|p| cloud::load_links(&p)).unwrap_or_default();
        let reports = links.iter().filter_map(|l| cloud::last_report(&l.name).map(|r| (l.name.clone(), r))).collect();
        let (verify_tx, verify_rx) = channel();
        let (proton_tx, proton_rx) = channel();
        Self {
            panes: [
                Pane::from_saved(&st.tabs, st.active, home.clone(), st.show_hidden),
                Pane::from_saved(&st.right_tabs, st.right_active, home, st.show_hidden),
            ],
            focus: 0,
            st,
            st_path,
            last_saved: Instant::now(),
            dirty: false,
            clipboard: None,
            rename: None,
            status: String::new(),
            drives: places::drives(),
            disks: places::disks(),
            cloud: places::proton_drive(),
            links,
            reports,
            verifying: vec![],
            verify_tx,
            verify_rx,
            link_form: None,
            proton_form: None,
            proton_busy: false,
            proton_tx,
            proton_rx,
            applied_theme: None,
            accent_checked: Instant::now(),
            pv: PreviewState::new(),
        }
    }

    fn reload_all(&mut self) {
        self.drives = places::drives();
        self.disks = places::disks();
        let hidden = self.st.show_hidden;
        for p in &mut self.panes {
            let sort = p.sort;
            for t in &mut p.tabs {
                t.reload(sort, hidden);
            }
        }
    }

    fn sync_state(&mut self) {
        let (l, r) = (self.panes[0].saved(), self.panes[1].saved());
        if l != self.st.tabs || r != self.st.right_tabs || self.panes[0].active != self.st.active || self.panes[1].active != self.st.right_active {
            self.st.tabs = l;
            self.st.right_tabs = r;
            self.st.active = self.panes[0].active;
            self.st.right_active = self.panes[1].active;
            self.dirty = true;
        }
    }

    fn save(&mut self) {
        if let Some(p) = &self.st_path {
            if let Err(e) = self.st.save_to(p) {
                self.status = format!("Could not save settings: {e}");
            }
        }
        self.dirty = false;
        self.last_saved = Instant::now();
    }

    fn selected_in_focus(&self) -> Vec<PathBuf> {
        let p = &self.panes[self.focus];
        p.tabs[p.active].selected.iter().cloned().collect()
    }

    fn apply(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        let hidden = self.st.show_hidden;
        for a in actions {
            match a {
                Action::Focus(p) => self.focus = p,
                Action::TogglePreview => {
                    self.st.preview = !self.st.preview;
                    self.dirty = true;
                }
                Action::Up(p) => {
                    let cur = self.panes[p].tabs[self.panes[p].active].dir();
                    if !places::is_this_pc(&cur) {
                        let target = match cur.parent() {
                            Some(par) if !par.as_os_str().is_empty() => par.to_path_buf(),
                            _ => places::this_pc(), // a drive root: up is "This PC"
                        };
                        self.focus = p;
                        let sort = self.panes[p].sort;
                        self.panes[p].cur().navigate(target, sort, hidden);
                    }
                }
                Action::EditAddress(p) => {
                    let t = self.panes[p].cur();
                    t.editing = true;
                    t.edit_fresh = true;
                    t.address = if places::is_virtual(&t.dir()) { String::new() } else { t.dir().to_string_lossy().into_owned() };
                }
                Action::NewFile(p) => {
                    let dir = self.panes[p].tabs[self.panes[p].active].dir();
                    if places::is_virtual(&dir) || archive::split(&dir).is_some() {
                        self.status = "Open a normal folder first (archives are read-only here)".into();
                        continue;
                    }
                    match fileops::new_file(&dir) {
                        Ok(new) => {
                            self.status = "File created".into();
                            self.reload_all();
                            self.rename = Some(RenameDlg { text: new.file_name().unwrap().to_string_lossy().into_owned(), path: new, fresh: true });
                        }
                        Err(e) => self.status = format!("New file failed: {e}"),
                    }
                }
                Action::Go(p, path) => {
                    self.focus = p;
                    let sort = self.panes[p].sort;
                    let t = self.panes[p].cur();
                    if path.is_dir() || places::is_virtual(&path) || archive::split(&path).is_some() {
                        t.navigate(path, sort, hidden);
                    } else if path.to_string_lossy().starts_with(places::PART_PREFIX) {
                        t.error = Some("This partition is not mounted".into());
                    } else {
                        t.error = Some(format!("{}: not a folder", path.display()));
                    }
                }
                Action::OpenTab(p, path) => {
                    let sort = self.panes[p].sort;
                    self.panes[p].tabs.push(TabView::new(path, false, sort, hidden));
                    self.panes[p].active = self.panes[p].tabs.len() - 1;
                }
                Action::CloseTab(p, i) => {
                    let pane = &mut self.panes[p];
                    if pane.tabs.len() > 1 && !pane.tabs[i].tab.pinned {
                        pane.tabs.remove(i);
                        pane.active = pane.active.min(pane.tabs.len() - 1);
                    }
                }
                Action::SelectTab(p, i) => {
                    self.panes[p].active = i;
                    let sort = self.panes[p].sort;
                    self.panes[p].cur().reload(sort, hidden);
                }
                Action::PinTab(p, i) => self.panes[p].tabs[i].tab.pinned ^= true,
                Action::Reload => self.reload_all(),
                Action::OpenFile(p) => match archive::split(&p) {
                    None => open_with_system(&p),
                    Some((arc, inner)) => {
                        let tmp = std::env::temp_dir().join(format!("minos-open-{}", std::process::id()));
                        match archive::extract_file_flat(&arc, &inner, &tmp) {
                            Ok(file) => open_with_system(&file),
                            Err(e) => self.status = format!("Could not open {inner}: {e}"),
                        }
                    }
                },
                Action::ExtractHere(archives) => {
                    let mut msgs = vec![];
                    for a in &archives {
                        let dest = a.parent().map(Path::to_path_buf).unwrap_or_default();
                        match archive::extract_all(a, &dest) {
                            Ok(()) => msgs.push(format!("Extracted {}", a.file_name().unwrap_or_default().to_string_lossy())),
                            Err(e) => msgs.push(format!("{}: {e}", a.display())),
                        }
                    }
                    self.status = msgs.join("; ");
                    self.reload_all();
                }
                Action::ExtractTo(paths) => {
                    let mut msgs = vec![];
                    for p in &paths {
                        let (arc, inner) = match archive::split(p) {
                            Some((a, i)) => (a, i),
                            None => (p.clone(), String::new()),
                        };
                        let stem = arc.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "extracted".into());
                        let parent = arc.parent().map(Path::to_path_buf).unwrap_or_default();
                        // An archive that already has ONE top-level folder is extracted straight into this
                        // folder (no `name/name` doubling); anything else gets a folder named after it.
                        let single_top = inner.is_empty() && archive::list_cached(&arc).map(|l| archive::top_level_names(&l).len() == 1).unwrap_or(false);
                        let dest = if single_top { parent.clone() } else { fileops::unique_path(&parent, &stem) };
                        let r = if inner.is_empty() { archive::extract_all(&arc, &dest) } else { archive::extract_entry(&arc, &inner, &dest) };
                        msgs.push(match r {
                            Ok(()) => format!("Extracted to {}", dest.display()),
                            Err(e) => format!("{}: {e}", arc.display()),
                        });
                    }
                    self.status = msgs.join("; ");
                    self.reload_all();
                }
                Action::Compress(paths, format) => {
                    let Some(first) = paths.first() else { continue };
                    let parent = first.parent().map(Path::to_path_buf).unwrap_or_default();
                    let base = if paths.len() == 1 { first.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default() } else { "Archive".into() };
                    let dest = fileops::unique_path(&parent, &format!("{base}.{}", format.ext()));
                    self.status = match archive::create(&dest, &paths, format) {
                        Ok(()) => format!("Created {}", dest.file_name().unwrap_or_default().to_string_lossy()),
                        Err(e) => format!("Compress failed: {e}"),
                    };
                    self.reload_all();
                }
                Action::Cut(v) if v.iter().any(|p| archive::split(p).is_some()) => {
                    self.status = "Items inside an archive cannot be cut; copy them out instead".into();
                }
                Action::Cut(v) => {
                    self.status = format!("{} item(s) cut", v.len());
                    self.clipboard = Some((v, true));
                }
                Action::Copy(v) => {
                    self.status = format!("{} item(s) copied", v.len());
                    self.clipboard = Some((v, false));
                }
                Action::Paste(p) => {
                    let Some((paths, cut)) = self.clipboard.clone() else { continue };
                    let dest = self.panes[p].tabs[self.panes[p].active].dir();
                    if places::is_virtual(&dest) {
                        self.status = "Open a drive or folder to paste into".into();
                        continue;
                    }
                    if archive::split(&dest).is_some() {
                        self.status = "Archives are read-only here: extract first, then add files".into();
                        continue;
                    }
                    let mut errs = vec![];
                    for src in &paths {
                        if let Some((arc, inner)) = archive::split(src) {
                            // Copying out of an archive extracts. (Cut is blocked above.)
                            let is_dir = archive::list_cached(&arc).ok().and_then(|l| l.into_iter().find(|e| e.path == inner)).map(|e| e.is_dir).unwrap_or(false);
                            let r = if is_dir { archive::extract_entry(&arc, &inner, &dest) } else { archive::extract_file_flat(&arc, &inner, &dest).map(|_| ()) };
                            if let Err(e) = r {
                                errs.push(format!("{inner}: {e}"));
                            }
                            continue;
                        }
                        let r = if cut { fileops::move_into(src, &dest) } else { fileops::copy_into(src, &dest) };
                        match r {
                            Ok(new) if cut => self.st.rekey(src, &new),
                            Ok(_) => {}
                            Err(e) => errs.push(format!("{}: {e}", src.display())),
                        }
                    }
                    if cut {
                        self.clipboard = None;
                    }
                    self.status = if errs.is_empty() { format!("Pasted {} item(s)", paths.len()) } else { errs.join("; ") };
                    self.dirty = true;
                    self.reload_all();
                }
                Action::Move(paths, dest) => {
                    if places::is_virtual(&dest) {
                        self.status = "Drop on a drive or a folder".into();
                        continue;
                    }
                    if archive::split(&dest).is_some() || paths.iter().any(|p| archive::split(p).is_some()) {
                        self.status = "Archives are read-only here: use Extract, or copy and paste".into();
                        continue;
                    }
                    let mut errs = vec![];
                    for src in &paths {
                        match fileops::move_into(src, &dest) {
                            Ok(new) => self.st.rekey(src, &new),
                            Err(e) => errs.push(format!("{}: {e}", src.display())),
                        }
                    }
                    self.status = if errs.is_empty() { format!("Moved {} item(s) to {}", paths.len(), dest.display()) } else { errs.join("; ") };
                    self.dirty = true;
                    self.reload_all();
                }
                Action::Rename(path) if archive::split(&path).is_some() => {
                    self.status = "Items inside an archive cannot be renamed".into();
                }
                Action::Rename(path) => {
                    let text = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.rename = Some(RenameDlg { path, text, fresh: true });
                }
                Action::Trash(paths) if paths.iter().any(|p| archive::split(p).is_some()) => {
                    self.status = "Items inside an archive cannot be deleted from here".into();
                }
                Action::Trash(paths) => {
                    let mut errs = vec![];
                    for p in &paths {
                        if let Err(e) = fileops::trash(p) {
                            errs.push(format!("{}: {e}", p.display()));
                        }
                    }
                    self.status = if errs.is_empty() { format!("Moved {} item(s) to trash", paths.len()) } else { errs.join("; ") };
                    self.reload_all();
                }
                Action::NewFolder(p) => {
                    let dir = self.panes[p].tabs[self.panes[p].active].dir();
                    if places::is_virtual(&dir) || archive::split(&dir).is_some() {
                        self.status = "Open a normal folder first (archives are read-only here)".into();
                        continue;
                    }
                    match fileops::new_folder(&dir) {
                        Ok(new) => {
                            self.status = "Folder created".into();
                            self.reload_all();
                            self.rename = Some(RenameDlg { text: new.file_name().unwrap().to_string_lossy().into_owned(), path: new, fresh: true });
                        }
                        Err(e) => self.status = format!("New folder failed: {e}"),
                    }
                }
                Action::ToggleTag(p, t) => {
                    self.st.toggle_tag(&p, &t);
                    self.dirty = true;
                }
                Action::SetColor(p, c) => {
                    match c {
                        Some(c) => self.st.folder_colors.insert(p, c),
                        None => self.st.folder_colors.remove(&p),
                    };
                    self.dirty = true;
                }
                Action::ToggleFav(p) => {
                    self.st.toggle_favorite(&p);
                    self.dirty = true;
                }
                Action::MoveFav(p, d) => {
                    if let Some(i) = self.st.favorites.iter().position(|f| *f == p) {
                        let j = (i as i32 + d).clamp(0, self.st.favorites.len() as i32 - 1) as usize;
                        self.st.favorites.swap(i, j);
                        self.dirty = true;
                    }
                }
                Action::ResetFavs => {
                    self.st.favorites = places::favorites().into_iter().map(|p| p.path).collect();
                    self.dirty = true;
                }
                Action::CopyPath(p) => ctx.copy_text(p.to_string_lossy().into_owned()),
                Action::Verify(name) => {
                    if let Some(link) = self.links.iter().find(|l| l.name == name).cloned() {
                        if !self.verifying.contains(&name) {
                            self.verifying.push(name.clone());
                            let (tx, c) = (self.verify_tx.clone(), ctx.clone());
                            std::thread::spawn(move || {
                                let r = cloud::verify_link(&link);
                                let _ = tx.send((link.name, r));
                                c.request_repaint();
                            });
                        }
                    }
                }
                Action::ToggleSplit => {
                    self.st.split = !self.st.split;
                    self.focus = 0;
                    self.dirty = true;
                }
                Action::SetView(v) => {
                    self.st.view = v;
                    self.dirty = true;
                }
                Action::ToggleTheme => {
                    self.st.dark = !self.st.dark;
                    self.dirty = true;
                }
                Action::ProtonConnect => {
                    if cloud::proton_configured() {
                        // signed in already: the mount is just not running
                        self.proton_busy = true;
                        let (tx, c) = (self.proton_tx.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            let _ = tx.send(cloud::start_proton_mount());
                            c.request_repaint();
                        });
                    } else {
                        self.proton_form = Some(Default::default());
                    }
                }
                Action::LinkDialog => {
                    let local = self.panes[self.focus].tabs[self.panes[self.focus].active].dir().to_string_lossy().into_owned();
                    self.link_form = Some((String::new(), local, "proton:".into()));
                }
            }
        }
    }
}

fn section_heading(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).strong().size(12.5).color(pal.dim));
    ui.add_space(2.0);
}

/// One row of the sidebar: icon, label, and optionally a usage bar underneath (drives).
fn side_row(ui: &mut egui::Ui, pal: &Palette, icon: Icon, icon_color: Color32, sys: &[&str], label: &str, selected: bool, bar: Option<(f32, Color32)>) -> egui::Response {
    let h = if bar.is_some() { 40.0 } else { 28.0 };
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let row = Rect::from_min_size(rect.min, vec2(rect.width(), 28.0));
    if selected {
        ui.painter().rect_filled(row, CornerRadius::same(8), pal.select);
    } else if r.hovered() {
        ui.painter().rect_filled(row, CornerRadius::same(8), pal.hover);
    }
    let icon_rect = Rect::from_min_size(row.min + vec2(8.0, 4.0), vec2(20.0, 20.0));
    if sys.is_empty() || !sysicon::paint(ui.painter(), icon_rect, sys) {
        icons::paint(ui.painter(), Rect::from_min_size(row.min + vec2(8.0, 5.0), vec2(18.0, 18.0)), icon, icon_color);
    }
    ui.painter().with_clip_rect(row.shrink2(vec2(4.0, 0.0))).text(pos2(row.left() + 34.0, row.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.5), pal.text);
    if let Some((frac, color)) = bar {
        let track = Rect::from_min_size(pos2(row.left() + 8.0, row.bottom() + 1.0), vec2(rect.width() - 16.0, 5.0));
        ui.painter().rect_filled(track, CornerRadius::same(3), pal.border.gamma_multiply(0.7));
        let mut fill = track;
        fill.set_width(track.width() * frac);
        ui.painter().rect_filled(fill, CornerRadius::same(3), color);
    }
    r
}

impl App {
    #[allow(clippy::too_many_arguments)]
    fn nav_row(&self, ui: &mut egui::Ui, pal: &Palette, icon: Icon, icon_color: Color32, label: &str, path: &Path, bar: Option<(f32, Color32)>, fav: bool, actions: &mut Vec<Action>) {
        let selected = {
            let p = &self.panes[self.focus];
            p.tabs[p.active].dir() == path
        };
        // A folder that has a colour tag keeps its drawn, tinted icon; the rest use the system theme.
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let sys: Vec<&str> = match icon {
            Icon::Cloud => vec!["folder-cloud", "folder"],
            Icon::Folder if icon_color == pal.folder => sysicon::folder_names(&name),
            _ => vec![],
        };
        let r = side_row(ui, pal, icon, icon_color, &sys, label, selected, bar);
        if r.clicked() {
            actions.push(Action::Go(self.focus, path.to_path_buf()));
        }
        if r.middle_clicked() {
            actions.push(Action::OpenTab(self.focus, path.to_path_buf()));
        }
        if !places::is_virtual(path) {
            if let Some(d) = r.dnd_release_payload::<DragPaths>() {
                actions.push(Action::Move(d.0.clone(), path.to_path_buf()));
            }
        }
        if fav {
            r.context_menu(|ui| {
                if ui.button("Move up").clicked() {
                    actions.push(Action::MoveFav(path.to_path_buf(), -1));
                    ui.close();
                }
                if ui.button("Move down").clicked() {
                    actions.push(Action::MoveFav(path.to_path_buf(), 1));
                    ui.close();
                }
                if ui.button("Remove").clicked() {
                    actions.push(Action::ToggleFav(path.to_path_buf()));
                    ui.close();
                }
            });
        }
    }

    fn sidebar(&self, ui: &mut egui::Ui, pal: &Palette, actions: &mut Vec<Action>) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section_heading(ui, pal, "Places");
            self.nav_row(ui, pal, Icon::Computer, pal.drive, &places::pc_name(), &places::this_pc(), None, false, actions);
            if let Some(home) = dirs::home_dir() {
                self.nav_row(ui, pal, Icon::Home, pal.drive, "My User Home", &home, None, false, actions);
            }

            section_heading(ui, pal, "Favorites");
            for path in &self.st.favorites {
                let tint = self.st.color_for(path).map(rgb).unwrap_or(pal.folder);
                self.nav_row(ui, pal, Icon::Folder, tint, &places::label_for(path), path, None, true, actions);
            }
            if ui.small_button("Reset favorites").clicked() {
                actions.push(Action::ResetFavs);
            }

            section_heading(ui, pal, "Storage");
            if self.disks.is_empty() {
                for d in &self.drives {
                    let frac = listing::used_fraction(d.total, d.free);
                    self.nav_row(ui, pal, Icon::Drive, pal.drive, &d.label, &d.path, Some((frac, theme::usage_color(pal, frac))), false, actions);
                }
            }
            for d in &self.disks {
                let (total, free) = d.usage();
                let frac = listing::used_fraction(total, free);
                self.nav_row(ui, pal, Icon::Drive, pal.drive, &d.label, &places::disk_path(&d.name), Some((frac, theme::usage_color(pal, frac))), false, actions);
            }

            if self.cloud.is_some() || !self.links.is_empty() {
                section_heading(ui, pal, "Cloud");
            }
            if let Some(c) = &self.cloud {
                match c.state {
                    CloudState::Ready => self.nav_row(ui, pal, Icon::Cloud, pal.dim, &c.label, &c.path, None, false, actions),
                    CloudState::NotMounted => {
                        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                        if r.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
                        }
                        icons::paint(ui.painter(), Rect::from_min_size(rect.min + vec2(8.0, 5.0), vec2(18.0, 18.0)), Icon::Cloud, pal.dim.gamma_multiply(0.5));
                        let label = if self.proton_busy { format!("{} (connecting...)", c.label) } else { format!("{} (click to connect)", c.label) };
                        ui.painter().text(pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.5), pal.dim.gamma_multiply(0.7));
                        if r.clicked() && !self.proton_busy {
                            actions.push(Action::ProtonConnect);
                        }
                        r.on_hover_text(format!("Not connected. Click to sign in; it will appear at {}", c.path.display()));
                    }
                }
            }
            for l in &self.links {
                let badge = if self.verifying.contains(&l.name) {
                    None
                } else {
                    cloud::badge_for(&l.local, std::slice::from_ref(l), &self.reports)
                };
                let tip = if self.verifying.contains(&l.name) {
                    "Verifying...".to_string()
                } else {
                    badge.map(badge_tip).unwrap_or("Not verified yet").to_string()
                };
                let selected = {
                    let p = &self.panes[self.focus];
                    p.tabs[p.active].dir() == l.local
                };
                let r = side_row(ui, pal, Icon::Folder, pal.folder, &["folder-cloud", "folder"], &l.name, selected, None);
                if let Some(b) = badge {
                    paint_badge(ui.painter(), pos2(r.rect.right() - 16.0, r.rect.top() + 14.0), b, pal);
                }
                let r = r.on_hover_text(format!("{} <-> {}\n{}", l.local.display(), l.remote, tip));
                if r.clicked() {
                    actions.push(Action::Go(self.focus, l.local.clone()));
                }
                r.context_menu(|ui| {
                    if ui.button("Verify now").clicked() {
                        actions.push(Action::Verify(l.name.clone()));
                        ui.close();
                    }
                });
            }
            if self.cloud.is_some() || !self.links.is_empty() {
                if ui.small_button("Link a folder...").clicked() {
                    actions.push(Action::LinkDialog);
                }
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Follow the system accent: read it now and then, so a change in the system settings
        // shows up without restarting Minos.
        if self.st.follow_accent && self.accent_checked.elapsed() >= Duration::from_secs(2) {
            self.st.system_accent = theme::system_accent();
            self.accent_checked = Instant::now();
        } else if !self.st.follow_accent {
            self.st.system_accent = None;
        }
        if self.st.follow_accent {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        let want = (self.st.dark, self.st.accent());
        if self.applied_theme != Some(want) {
            theme::apply(&ctx, want.0, want.1);
            self.applied_theme = Some(want);
        }
        let pal = palette_for(self.st.dark, self.st.accent());
        let hidden = self.st.show_hidden;
        let split = self.st.split;
        for (i, p) in self.panes.iter_mut().enumerate() {
            if i == 0 || split {
                p.ensure_watch(&ctx);
                p.poll_watch(hidden);
            }
        }
        while let Ok(res) = self.proton_rx.try_recv() {
            self.proton_busy = false;
            match res {
                Ok(()) => {
                    self.status = "Proton Drive is connecting; its files appear in a few seconds".into();
                    self.proton_form = None;
                }
                Err(e) => self.status = e,
            }
            self.cloud = places::proton_drive();
        }
        if self.proton_busy || self.cloud.as_ref().map(|c| c.state == CloudState::NotMounted).unwrap_or(false) && cloud::proton_configured() {
            // keep looking while the mount comes up
            self.cloud = places::proton_drive();
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        while let Ok((name, res)) = self.verify_rx.try_recv() {
            self.verifying.retain(|n| *n != name);
            match res {
                Ok(r) => {
                    self.status = format!("{name}: {}", r.summary());
                    self.reports.insert(name, r);
                }
                Err(e) => self.status = format!("{name}: verification failed: {e}"),
            }
        }

        let mut actions: Vec<Action> = vec![];
        chrome::resize_edges(&ctx);

        // Keyboard shortcuts, only when no text field has focus.
        if !ctx.egui_wants_keyboard_input() {
            let sel = self.selected_in_focus();
            let f = self.focus;
            ctx.input(|i| {
                let cmd = i.modifiers.command;
                // egui turns Ctrl+C / Ctrl+X / Ctrl+V into dedicated events and does NOT also
                // deliver them as key presses, so key_pressed(C) alone never fires.
                let copy_ev = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                let cut_ev = i.events.iter().any(|e| matches!(e, egui::Event::Cut));
                let paste_ev = i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
                if (i.modifiers.alt && i.key_pressed(egui::Key::ArrowUp)) || i.key_pressed(egui::Key::Backspace) {
                    actions.push(Action::Up(f));
                }
                if i.key_pressed(egui::Key::F3) {
                    actions.push(Action::TogglePreview);
                }
                if cmd && i.key_pressed(egui::Key::L) {
                    actions.push(Action::EditAddress(f));
                }
                if cmd && i.key_pressed(egui::Key::T) {
                    actions.push(Action::OpenTab(f, self.panes[f].tabs[self.panes[f].active].dir()));
                }
                if cmd && i.key_pressed(egui::Key::W) {
                    actions.push(Action::CloseTab(f, self.panes[f].active));
                }
                if i.key_pressed(egui::Key::F5) {
                    actions.push(Action::Reload);
                }
                if (cut_ev || (cmd && i.key_pressed(egui::Key::X))) && !sel.is_empty() {
                    actions.push(Action::Cut(sel.clone()));
                }
                if (copy_ev || (cmd && i.key_pressed(egui::Key::C))) && !sel.is_empty() {
                    actions.push(Action::Copy(sel.clone()));
                }
                if paste_ev || (cmd && i.key_pressed(egui::Key::V)) {
                    actions.push(Action::Paste(f));
                }
                if i.key_pressed(egui::Key::F2) && sel.len() == 1 {
                    actions.push(Action::Rename(sel[0].clone()));
                }
                if i.key_pressed(egui::Key::Delete) && !sel.is_empty() {
                    actions.push(Action::Trash(sel.clone()));
                }
                if cmd && i.modifiers.shift && i.key_pressed(egui::Key::N) {
                    actions.push(Action::NewFolder(f));
                }
            });
            // Ctrl+A selects everything visible in the focused tab.
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A)) {
                let t = self.panes[self.focus].cur();
                let all: Vec<PathBuf> = match &t.results {
                    Some(r) => r.iter().map(|e| e.path.clone()).collect(),
                    None => listing::visible(&t.entries, &t.query, hidden).into_iter().map(|i| t.entries[i].path.clone()).collect(),
                };
                t.selected = all.into_iter().collect();
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
        }

        // Above the sidebar too, so it runs the full width like a title bar.
        egui::Panel::top("strip")
            .frame(egui::Frame::new().fill(pal.strip).inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let menu = icon_button(ui, &pal, Icon::Menu, "Menu", true, false);
                    egui::Popup::menu(&menu).show(|ui| {
                        let mut h = self.st.show_hidden;
                        if ui.checkbox(&mut h, "Show hidden items").changed() {
                            self.st.show_hidden = h;
                            self.dirty = true;
                            actions.push(Action::Reload);
                        }
                        let mut f = self.st.follow_accent;
                        if ui.checkbox(&mut f, "Follow system accent color").changed() {
                            self.st.follow_accent = f;
                            self.dirty = true;
                        }
                        ui.separator();
                        if ui.selectable_label(self.st.view == ViewMode::Details, "Details view").clicked() {
                            actions.push(Action::SetView(ViewMode::Details));
                        }
                        if ui.selectable_label(self.st.view == ViewMode::Gallery, "Gallery view").clicked() {
                            actions.push(Action::SetView(ViewMode::Gallery));
                        }
                        if self.st.view == ViewMode::Gallery {
                            let mut s = self.st.gallery_size as u8;
                            if ui.add(egui::Slider::new(&mut s, 0..=3).text("Thumbnail size")).changed() {
                                self.st.gallery_size = s as usize;
                                self.dirty = true;
                            }
                        }
                        ui.separator();
                        let mut pv = self.st.preview;
                        if ui.checkbox(&mut pv, "Preview pane (F3)").changed() {
                            actions.push(Action::TogglePreview);
                        }
                        let mut sp = self.st.split;
                        if ui.checkbox(&mut sp, "Split pane").changed() {
                            actions.push(Action::ToggleSplit);
                        }
                        if ui.button("Link a folder to the cloud...").clicked() {
                            actions.push(Action::LinkDialog);
                            ui.close();
                        }
                    });
                    let (ticon, ttip) = if self.st.dark { (Icon::Sun, "Switch to light theme") } else { (Icon::Moon, "Switch to dark theme") };
                    if icon_button(ui, &pal, ticon, ttip, true, false).clicked() {
                        actions.push(Action::ToggleTheme);
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new(&self.status).color(pal.dim));
                    // The strip is the title bar: empty space moves the window, buttons at the end.
                    chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                    chrome::window_controls(ui, &pal);
                });
            });

        egui::Panel::left("sidebar")
            .default_size(230.0)
            .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(egui::Margin::symmetric(8, 6)))
            .show(ui, |ui| {
                self.sidebar(ui, &pal, &mut actions);
            });

        if self.st.preview {
            let want: Vec<PathBuf> = {
                let p = &self.panes[self.focus];
                let t = &p.tabs[p.active];
                if t.selected.is_empty() { vec![t.dir()] } else { t.selected.iter().cloned().collect() }
            };
            if self.pv.key.as_ref() != Some(&want) {
                self.pv.key = Some(want.clone());
                // A drive selected in "This PC" gets a usage summary instead of a folder listing.
                let drive = {
                    let p = &self.panes[self.focus];
                    let t = &p.tabs[p.active];
                    if want.len() == 1 && places::is_virtual(&t.dir()) {
                        t.entries.iter().find(|e| e.path == want[0]).and_then(|e| e.usage.map(|(total, free)| preview::drive(&e.name, total, free)))
                    } else if want.len() == 1 && places::is_this_pc(&want[0]) {
                        Some(preview::message(&places::pc_name(), "Your drives", "Select a drive to see how full it is"))
                    } else {
                        None
                    }
                };
                self.pv.request(&ctx, want, drive);
            }
            self.pv.poll(&ctx);
            egui::Panel::right("preview")
                .default_size(310.0)
                .max_size(520.0)
                .min_size(220.0)
                .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(egui::Margin::symmetric(14, 12)))
                .show(ui, |ui| {
                    draw_preview(ui, &pal, &self.pv);
                });
        }

        let clip_has = self.clipboard.is_some();
        let focus = self.focus;
        let st = &self.st;
        let cloud_ctx = (self.links.as_slice(), &self.reports);
        let [left, right] = &mut self.panes;
        egui::CentralPanel::default().show(ui, |ui| {
            if split {
                ui.columns(2, |cols| {
                    ui_pane(&mut cols[0], left, 0, focus == 0, st, cloud_ctx, clip_has, &mut actions);
                    ui_pane(&mut cols[1], right, 1, focus == 1, st, cloud_ctx, clip_has, &mut actions);
                });
            } else {
                ui_pane(ui, left, 0, true, st, cloud_ctx, clip_has, &mut actions);
            }
        });

        // Dialogs.
        let mut rename_done: Option<Option<(PathBuf, String)>> = None;
        if let Some(d) = &mut self.rename {
            egui::Window::new("Rename").collapsible(false).resizable(false).show(&ctx, |ui| {
                let r = ui.text_edit_singleline(&mut d.text);
                if d.fresh {
                    r.request_focus();
                    d.fresh = false;
                }
                ui.horizontal(|ui| {
                    let ok = ui.button("OK").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if ok {
                        rename_done = Some(Some((d.path.clone(), d.text.clone())));
                    } else if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        rename_done = Some(None);
                    }
                });
            });
        }
        if let Some(done) = rename_done {
            self.rename = None;
            if let Some((path, name)) = done {
                match fileops::rename(&path, &name) {
                    Ok(new) => {
                        self.st.rekey(&path, &new);
                        self.dirty = true;
                        self.status = format!("Renamed to {name}");
                    }
                    Err(e) => self.status = format!("Rename failed: {e}"),
                }
                self.reload_all();
            }
        }

        let mut proton_go = false;
        let mut proton_cancel = false;
        if let Some(f) = &mut self.proton_form {
            let busy = self.proton_busy;
            egui::Window::new("Connect Proton Drive").collapsible(false).show(&ctx, |ui| {
                ui.label("Proton email or username");
                ui.text_edit_singleline(&mut f[0]);
                ui.label("Password");
                ui.add(egui::TextEdit::singleline(&mut f[1]).password(true));
                ui.label("Two-factor code (if you use one; it lasts about 30 seconds)");
                ui.text_edit_singleline(&mut f[3]);
                ui.label(RichText::new("Passed straight to rclone, which keeps a scrambled copy in its own config.").small());
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy && !f[0].is_empty() && !f[1].is_empty(), egui::Button::new(if busy { "Connecting..." } else { "Connect" })).clicked() {
                        proton_go = true;
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                        proton_cancel = true;
                    }
                });
            });
        }
        if proton_go {
            if let Some(f) = self.proton_form.clone() {
                self.proton_busy = true;
                let (tx, c) = (self.proton_tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(cloud::connect_proton(&f[0], &f[1], &f[2], &f[3]));
                    c.request_repaint();
                });
            }
        }
        if proton_cancel {
            self.proton_form = None;
        }

        let mut link_done: Option<bool> = None;
        if let Some((name, local, remote)) = &mut self.link_form {
            egui::Window::new("Link a folder to the cloud").collapsible(false).show(&ctx, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(name);
                ui.label("Local folder");
                ui.text_edit_singleline(local);
                ui.label("Remote folder (rclone path, e.g. proton:My files/Documents/Github)");
                ui.text_edit_singleline(remote);
                ui.horizontal(|ui| {
                    if ui.button("Link").clicked() {
                        link_done = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        link_done = Some(false);
                    }
                });
            });
        }
        if let Some(ok) = link_done {
            match (ok, self.link_form.take()) {
                (true, Some((name, local, remote))) => {
                    if name.trim().is_empty() || !Path::new(&local).is_dir() {
                        self.status = "Link needs a name and an existing local folder".into();
                    } else {
                        self.links.retain(|l| l.name != name.trim());
                        self.links.push(Link { name: name.trim().into(), local: local.into(), remote });
                        if let Some(p) = cloud::links_path() {
                            if let Err(e) = cloud::save_links(&p, &self.links) {
                                self.status = format!("Could not save links: {e}");
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some(p) = egui::DragAndDrop::payload::<DragPaths>(&ctx) {
            if let Some(pos) = ctx.pointer_latest_pos() {
                egui::Area::new("minos-drag-ghost".into()).order(egui::Order::Tooltip).fixed_pos(pos + vec2(14.0, 14.0)).interactable(false).show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(if p.0.len() == 1 { format!("Move {}", p.0[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) } else { format!("Move {} items", p.0.len()) });
                    });
                });
            }
        }

        chrome::outline(&ctx, &pal);
        self.apply(&ctx, actions);
        self.sync_state();
        if self.dirty && self.last_saved.elapsed() > Duration::from_secs(1) {
            self.save();
        }
    }

    fn on_exit(&mut self) {
        self.sync_state();
        self.save();
    }
}

fn ui_pane(ui: &mut egui::Ui, pane: &mut Pane, pi: usize, focused: bool, st: &State, cloud_ctx: (&[Link], &BTreeMap<String, Report>), clip_has: bool, actions: &mut Vec<Action>) {
    ui.push_id(pi, |ui| draw_pane(ui, pane, pi, focused, st, cloud_ctx, clip_has, actions));
}

/// A folder given on the command line: a path, a file:// URL, or trash:/ (the trash folder).
fn start_path(arg: &str) -> PathBuf {
    if arg.starts_with("trash:") {
        let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".local/share")));
        return base.map(|b| b.join("Trash/files")).unwrap_or_default();
    }
    PathBuf::from(arg.strip_prefix("file://").unwrap_or(arg))
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([640.0, 400.0])
            .with_title("Minos")
            // Minos draws its own title bar (chrome.rs): no grey system title bar or frame.
            .with_decorations(false)
            // Wayland/X11 application id: matches minos.desktop so docks and the
            // bar can find the icon. Must be a plain name; "Minos" was not.
            .with_app_id("minos"),
        ..Default::default()
    };
    eframe::run_native(
        "Minos",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let st = State::config_path().as_deref().map(State::load_from).unwrap_or_default();
            theme::apply(&cc.egui_ctx, st.dark, if st.follow_accent { theme::system_accent() } else { None });
            let mut app = App::new();
            // `minos FOLDER` (the dock's Trash, "open with"): that folder in a new tab.
            if let Some(p) = std::env::args().nth(1).map(|a| start_path(&a)).filter(|p| p.is_dir()) {
                let (sort, hidden) = (app.panes[0].sort, app.st.show_hidden);
                app.panes[0].tabs.push(TabView::new(p, false, sort, hidden));
                app.panes[0].active = app.panes[0].tabs.len() - 1;
            }
            Ok(Box::new(app))
        }),
    )
}
