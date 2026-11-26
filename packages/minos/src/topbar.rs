//! The top of the window, laid out as the Files app lays it out (YGG-169): the tabs (in the title strip, or above each pane in
//! split view), then the navigation row (back, forward, up, refresh, the address bar, the favorite star, the sidebar button and
//! search), then the command bar over the list (New, Cut, Copy, Paste, Rename, Share, Delete, Properties on the left; Filter,
//! Sort, Group, the view and the preview pane on the right). The rules for which commands apply are in
//! `minos_core::commandbar`; the Properties dialog is here too.

use super::*;
use minos_core::commandbar::{self, Command, Facts};
use minos_core::grouping::GroupBy;
use minos_core::props;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// The navigation row: back, forward, up, refresh, the address bar (a text field when clicked: Enter goes there, Esc cancels),
/// the favorite star, the sidebar button (`sidebar_button`) and search.
#[allow(clippy::too_many_arguments)]
pub(crate) fn nav_bar(
    ui: &mut egui::Ui,
    pane: &mut Pane,
    pi: usize,
    pal: &Palette,
    st: &State,
    sidebar_button: bool,
    actions: &mut Vec<Action>,
) {
    let sort = pane.sort;
    let hidden = st.show_hidden;
    let t = pane.cur();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon_button(ui, pal, Icon::Back, "Back (Alt+Left)", t.tab.history.can_back(), false).clicked() && t.tab.history.back().is_some()
        {
            t.search = None;
            t.results = None;
            t.reload(sort, hidden);
        }
        if icon_button(ui, pal, Icon::Forward, "Forward (Alt+Right)", t.tab.history.can_forward(), false).clicked()
            && t.tab.history.forward().is_some()
        {
            t.search = None;
            t.results = None;
            t.reload(sort, hidden);
        }
        let dir = t.dir();
        if icon_button(ui, pal, Icon::Up, "Up (Alt+Up / Backspace)", !places::is_this_pc(&dir), false).clicked() {
            actions.push(Action::Up(pi));
        }
        if icon_button(ui, pal, Icon::Refresh, "Refresh (F5)", true, false).clicked() {
            actions.push(Action::Reload);
        }
        ui.add_space(6.0);
        let in_pc = places::is_virtual(&dir);
        let fav = st.favorites.contains(&dir);
        let right = 32.0 * if sidebar_button { 3.0 } else { 2.0 } + 8.0;
        breadcrumb(ui, pal, t, pi, ui.available_width() - right, actions);
        ui.add_space(2.0);
        let star_tip = if fav { "Remove from favorites" } else { "Add to favorites" };
        if icon_button(ui, pal, if fav { Icon::StarFilled } else { Icon::Star }, star_tip, !in_pc, fav).clicked() {
            actions.push(Action::ToggleFav(dir.clone()));
        }
        if sidebar_button {
            let tip = if st.sidebar_hidden { "Show the sidebar" } else { "Hide the sidebar" };
            if icon_button(ui, pal, Icon::Sidebar, tip, true, false).clicked() {
                actions.push(Action::ToggleSidebar);
            }
        }
        let magnifier = icon_button(ui, pal, Icon::Search, "Search (Ctrl+F)", true, t.search_open || !t.query.is_empty());
        if magnifier.clicked() {
            t.search_open = !t.search_open;
            t.search_focus = t.search_open;
        }
        if t.search_open {
            let ctx = ui.ctx().clone();
            let area = egui::Area::new(egui::Id::new(("minos-search", pi)))
                .order(egui::Order::Foreground)
                .fixed_pos(pos2(magnifier.rect.right() - 250.0, magnifier.rect.bottom() + 4.0))
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        let te = ui.add(egui::TextEdit::singleline(&mut t.query).hint_text("Search").desired_width(230.0));
                        if t.search_focus {
                            te.request_focus();
                            t.search_focus = false;
                        }
                        if te.changed() {
                            t.restart_search(&ctx, hidden);
                        }
                    });
                });
            let (esc, pressed) = ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.pointer.any_pressed()));
            if esc {
                t.search_open = false;
                t.query.clear();
                t.restart_search(&ctx, hidden);
            } else if pressed && !area.response.contains_pointer() && !magnifier.contains_pointer() {
                t.search_open = false; // the words stay, so the results do
            }
        }
    });
}

/// A button with an icon and a small down arrow: it opens a menu.
fn menu_button(ui: &mut egui::Ui, pal: &Palette, icon: Icon, tip: &str, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 30.0), Sense::click());
    if active {
        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
    }
    icons::paint(ui.painter(), Rect::from_min_size(rect.min + vec2(7.0, 7.0), vec2(16.0, 16.0)), icon, pal.text);
    icons::paint(ui.painter(), Rect::from_min_size(rect.min + vec2(27.0, 11.0), vec2(10.0, 10.0)), Icon::ChevronDown, pal.dim);
    resp.on_hover_text(tip)
}

/// The New button: a plus in a circle, "New" and a down arrow.
fn new_button(ui: &mut egui::Ui, pal: &Palette, enabled: bool) -> egui::Response {
    let font = FontId::proportional(13.5);
    let col = if enabled { pal.text } else { pal.dim.gamma_multiply(0.45) };
    let galley = ui.painter().layout_no_wrap("New".into(), font, col);
    let w = 8.0 + 16.0 + 6.0 + galley.size().x + 4.0 + 10.0 + 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
    }
    icons::paint(ui.painter(), Rect::from_min_size(rect.min + vec2(8.0, 7.0), vec2(16.0, 16.0)), Icon::NewItem, col);
    let tx = rect.left() + 30.0;
    ui.painter().galley(pos2(tx, rect.center().y - galley.size().y / 2.0), galley.clone(), col);
    let cx = tx + galley.size().x + 4.0;
    icons::paint(ui.painter(), Rect::from_min_size(pos2(cx, rect.top() + 10.0), vec2(10.0, 10.0)), Icon::ChevronDown, pal.dim);
    resp.on_hover_text("New folder or file")
}

fn command_icon(c: Command) -> Icon {
    match c {
        Command::NewFolder => Icon::NewFolder,
        Command::NewFile => Icon::NewFile,
        Command::Cut => Icon::Cut,
        Command::Copy => Icon::Copy,
        Command::Paste => Icon::Paste,
        Command::Rename => Icon::Rename,
        Command::Share => Icon::Share,
        Command::Delete => Icon::Trash,
        Command::Properties => Icon::Properties,
    }
}

/// Sort: by name, date modified or size, up or down. Changes `sort`.
fn sort_menu(ui: &mut egui::Ui, sort: &mut minos_core::listing::Sort) {
    use minos_core::listing::SortKey;
    ui.label(RichText::new("Sort by").small());
    for (k, label) in [(SortKey::Name, "Name"), (SortKey::Modified, "Date modified"), (SortKey::Size, "Size")] {
        if ui.selectable_label(sort.key == k, label).clicked() {
            sort.key = k;
            ui.close();
        }
    }
    ui.separator();
    for (desc, label) in [(false, "Ascending"), (true, "Descending")] {
        if ui.selectable_label(sort.descending == desc, label).clicked() {
            sort.descending = desc;
            ui.close();
        }
    }
}

fn group_menu(ui: &mut egui::Ui, st: &State, actions: &mut Vec<Action>) {
    ui.label(RichText::new("Group by").small());
    for g in GroupBy::ALL {
        if ui.selectable_label(st.group == g, g.label()).clicked() {
            actions.push(Action::SetGroup(g));
            ui.close();
        }
    }
}

fn view_menu(ui: &mut egui::Ui, st: &State, actions: &mut Vec<Action>) {
    ui.label(RichText::new("Layout").small());
    if ui.selectable_label(st.view == ViewMode::Details, "Details").clicked() {
        actions.push(Action::SetView(ViewMode::Details));
        ui.close();
    }
    if ui.selectable_label(st.view == ViewMode::Gallery, "Gallery").clicked() {
        actions.push(Action::SetView(ViewMode::Gallery));
        ui.close();
    }
}

/// The command bar over the list. In a narrow pane Sort, Group, the layout and the preview pane share one menu.
#[allow(clippy::too_many_arguments)]
pub(crate) fn command_bar(
    ui: &mut egui::Ui,
    pane: &mut Pane,
    pi: usize,
    pal: &Palette,
    st: &State,
    clip_has: bool,
    actions: &mut Vec<Action>,
) {
    let mut sort = pane.sort;
    let t = pane.cur();
    let dir = t.dir();
    let facts = Facts {
        selected: t.selected.len(),
        clipboard: clip_has,
        virtual_folder: places::is_virtual(&dir),
        in_archive: archive::split(&dir).is_some(),
    };
    let mut filter_button: Option<egui::Response> = None;
    egui::Frame::new()
        .fill(pal.field)
        .stroke(egui::Stroke::new(1.0, pal.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let compact = ui.available_width() < 600.0;
                let new = new_button(ui, pal, commandbar::enabled(Command::NewFolder, &facts));
                egui::Popup::menu(&new).show(|ui| {
                    if ui.button("Folder    Ctrl+Shift+N").clicked() {
                        actions.push(Action::NewFolder(pi));
                        ui.close();
                    }
                    if ui.button("File").clicked() {
                        actions.push(Action::NewFile(pi));
                        ui.close();
                    }
                });
                ui.add_space(4.0);
                ui.add(egui::Separator::default().vertical().spacing(6.0));
                for c in commandbar::LEFT {
                    let on = commandbar::enabled(c, &facts);
                    if icon_button(ui, pal, command_icon(c), commandbar::tooltip(c), on, false).clicked() {
                        let sel = || t.selected.iter().cloned().collect::<Vec<PathBuf>>();
                        actions.push(match c {
                            Command::Cut => Action::Cut(sel()),
                            Command::Copy => Action::Copy(sel()),
                            Command::Paste => Action::Paste(pi),
                            Command::Rename => Action::Rename(sel().remove(0)),
                            Command::Delete => Action::Trash(sel()),
                            Command::Properties => Action::Properties(pi),
                            Command::NewFolder => Action::NewFolder(pi),
                            Command::NewFile => Action::NewFile(pi),
                            Command::Share => continue,
                        });
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if compact {
                        let more = menu_button(ui, pal, Icon::Sort, "Sort, group, layout and preview pane", false);
                        egui::Popup::menu(&more).show(|ui| {
                            sort_menu(ui, &mut sort);
                            ui.separator();
                            group_menu(ui, st, actions);
                            ui.separator();
                            view_menu(ui, st, actions);
                            ui.separator();
                            if ui.selectable_label(st.preview, "Preview pane (F3)").clicked() {
                                actions.push(Action::TogglePreview);
                                ui.close();
                            }
                        });
                    } else {
                        if icon_button(ui, pal, Icon::Preview, "Preview pane (F3)", true, st.preview).clicked() {
                            actions.push(Action::TogglePreview);
                        }
                        let view_icon = if st.view == ViewMode::Gallery { Icon::Grid } else { Icon::List };
                        let view = menu_button(ui, pal, view_icon, "Layout: details or gallery", false);
                        egui::Popup::menu(&view).show(|ui| view_menu(ui, st, actions));
                        let group = menu_button(ui, pal, Icon::Group, "Group by", st.group != GroupBy::None);
                        egui::Popup::menu(&group).show(|ui| group_menu(ui, st, actions));
                        let sorter = menu_button(ui, pal, Icon::Sort, "Sort", false);
                        egui::Popup::menu(&sorter).show(|ui| sort_menu(ui, &mut sort));
                    }
                    filter_button = Some(icon_button(
                        ui,
                        pal,
                        Icon::Filter,
                        "Filter this folder by name",
                        !facts.virtual_folder,
                        t.filter_open || !t.filter.is_empty(),
                    ));
                });
            });
        });
    if let Some(b) = filter_button {
        if b.clicked() {
            t.filter_open = !t.filter_open;
            t.filter_focus = t.filter_open;
        }
        if t.filter_open {
            let ctx = ui.ctx().clone();
            let area = egui::Area::new(egui::Id::new(("minos-filter", pi)))
                .order(egui::Order::Foreground)
                .fixed_pos(pos2(b.rect.right() - 250.0, b.rect.bottom() + 4.0))
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        let te = ui.add(egui::TextEdit::singleline(&mut t.filter).hint_text("Filter by name").desired_width(230.0));
                        if t.filter_focus {
                            te.request_focus();
                            t.filter_focus = false;
                        }
                    });
                });
            let (esc, pressed) = ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.pointer.any_pressed()));
            if esc {
                t.filter_open = false;
                t.filter.clear();
            } else if pressed && !area.response.contains_pointer() && !b.contains_pointer() {
                t.filter_open = false; // the words stay, so does the filter
            }
        }
    }
    if sort.key != pane.sort.key || sort.descending != pane.sort.descending {
        pane.sort = sort;
        actions.push(Action::Reload);
    }
}

// ---------------------------------------------------------------------------------------------- Properties

/// The Properties dialog: one item's facts, or for several (or a folder) the total size, counted on a thread.
pub(crate) struct PropsDlg {
    paths: Vec<PathBuf>,
    facts: Result<props::Facts, String>,
    location: String,
    /// The count so far and whether it is finished; None when nothing is counted (one file, or a drive that may stall).
    count: Option<Arc<Mutex<(props::Count, bool)>>>,
    /// Counting was left out: a network or cloud drive.
    stalls: bool,
    stop: Arc<AtomicBool>,
}

impl PropsDlg {
    pub(crate) fn open(ctx: &egui::Context, paths: Vec<PathBuf>) -> PropsDlg {
        let first = paths.first().cloned().unwrap_or_default();
        let stalls = paths.iter().any(|p| places::may_stall(p));
        // on a drive that may stall even the item's own facts can wait on the line: they are read here once, as the list already
        // read them; only the walk through a folder is left out
        let facts = props::facts(&first).map_err(|e| e.to_string());
        let location = first.parent().map(|p| p.display().to_string()).unwrap_or_default();
        let needs_count = paths.len() > 1 || facts.as_ref().is_ok_and(|f| f.is_dir);
        let stop = Arc::new(AtomicBool::new(false));
        let count = (needs_count && !stalls).then(|| {
            let shared = Arc::new(Mutex::new((props::Count::default(), false)));
            let (s, stop, c, list) = (shared.clone(), stop.clone(), ctx.clone(), paths.clone());
            std::thread::spawn(move || {
                let total = props::count(&list, &stop, &mut |so_far| {
                    if let Ok(mut g) = s.lock() {
                        g.0 = so_far;
                    }
                    c.request_repaint();
                });
                if let Ok(mut g) = s.lock() {
                    *g = (total, true);
                }
                c.request_repaint();
            });
            shared
        });
        PropsDlg { paths, facts, location, count, stalls, stop }
    }

    /// Draws the dialog; false once it is closed.
    pub(crate) fn show(&mut self, ctx: &egui::Context, pal: &Palette) -> bool {
        let mut open = true;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let title = match (&self.facts, self.paths.len()) {
            (_, n) if n > 1 => format!("{n} items"),
            (Ok(f), _) => f.name.clone(),
            (Err(_), _) => "Properties".into(),
        };
        egui::Window::new(format!("Properties: {title}"))
            .id(egui::Id::new("minos-properties"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                    ui.label(RichText::new(k).color(pal.dim));
                    ui.add(egui::Label::new(v).wrap());
                    ui.end_row();
                };
                let size_line = |c: &props::Count, done: bool| {
                    let mut s = format!("{} ({} bytes)", listing::human_size(c.bytes), c.bytes);
                    s.push_str(&format!("\n{} files, {} folders", c.files, c.folders));
                    if c.unreadable > 0 {
                        s.push_str(&format!("; {} folders could not be read", c.unreadable));
                    }
                    if !done {
                        s.push_str("\nCounting...");
                    }
                    s
                };
                egui::Grid::new("minos-properties-grid").num_columns(2).spacing([16.0, 8.0]).max_col_width(420.0).show(
                    ui,
                    |ui| match &self.facts {
                        Err(e) => row(ui, "Error", e.clone()),
                        Ok(f) if self.paths.len() == 1 => {
                            row(ui, "Name", f.name.clone());
                            row(ui, "Type", f.kind.clone());
                            let size = if !f.is_dir {
                                format!("{} ({} bytes)", listing::human_size(f.size), f.size)
                            } else if self.stalls {
                                "Not counted: it is on a network or cloud drive".into()
                            } else {
                                self.count.as_ref().and_then(|c| c.lock().ok().map(|g| size_line(&g.0, g.1))).unwrap_or_default()
                            };
                            row(ui, "Size", size);
                            row(ui, "Location", self.location.clone());
                            row(ui, "Created", f.created.map(fmt_time).unwrap_or_else(|| "Not recorded by this drive".into()));
                            row(ui, "Modified", f.modified.map(fmt_time).unwrap_or_default());
                            row(ui, "Permissions", format!("{}\n{}", f.mode, f.access));
                            row(ui, "Owner", f.owner.clone());
                        }
                        Ok(_) => {
                            row(ui, "Items", format!("{} selected", self.paths.len()));
                            let size = if self.stalls {
                                "Not counted: they are on a network or cloud drive".into()
                            } else {
                                self.count.as_ref().and_then(|c| c.lock().ok().map(|g| size_line(&g.0, g.1))).unwrap_or_default()
                            };
                            row(ui, "Size", size);
                            row(ui, "Location", self.location.clone());
                        }
                    },
                );
                ui.add_space(8.0);
                ui.allocate_ui_with_layout(vec2(ui.available_width(), 28.0), Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        open && !close
    }
}

impl Drop for PropsDlg {
    fn drop(&mut self) {
        // a count still running stops when the dialog closes
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
