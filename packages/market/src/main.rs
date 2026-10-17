//! Labyrinth Marketplace: apps in one place, laid out like an app store (Discover, Create, Work, Play, Develop, Categories,
//! Updates). Sources: Labyrinth's own reviewed catalog, plus repositories you add (marked as not reviewed).

mod catalog;
mod details;
mod history;
mod install;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};

use catalog::{Catalog, Entry, Installed, Repo, Source};
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText, Sense, Stroke, UiBuilder};
use labyrinth_ui::{chrome, sysicon, theme};
use theme::Palette;

const TABS: [&str; 4] = ["Create", "Work", "Play", "Develop"];

#[derive(Clone, PartialEq)]
enum Page {
    Discover,
    Tab(&'static str),
    Categories,
    /// A category's sub-category: (category, sub-category)
    Sub(String, String),
    Updates,
    /// Installs and removals in progress, and finished ones.
    Activity,
    /// Apps installed through the Marketplace.
    History,
    Detail(usize),
    Sources,
    Search,
}

enum Msg {
    Installed(Installed),
    /// A repository's download finished: its index and the result.
    Fetched(usize, Result<String, String>),
    /// An install or remove finished (its entry id).
    Finished(String),
    /// More about an app arrived (None: nothing found), or a picture did.
    Details(String, Option<details::Details>),
    Image(String, Option<std::path::PathBuf>),
}

struct App {
    pal: Palette,
    page: Page,
    history: Vec<Page>,
    search: String,
    catalogs: Vec<Catalog>,
    repos: Vec<Repo>,
    /// Errors per repository, by index into `repos`.
    repo_status: Vec<String>,
    apps: Vec<Entry>,
    installed: Installed,
    busy: HashSet<String>,
    /// Installs, removals and updates run inside the Marketplace (newest last), and which of them show their log.
    jobs: Vec<install::Shared>,
    installs: Vec<history::Item>,
    show_log: HashSet<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    // the "add a repository" form
    add_name: String,
    add_url: String,
    add_kind: usize,
    add_confirm: bool,
    pending_add: Option<Repo>,
    status: String,
    // more about the app being looked at
    details: HashMap<String, Option<details::Details>>,
    details_loading: HashSet<String>,
    images: HashMap<String, Option<std::path::PathBuf>>,
    img_loading: HashSet<String>,
    online: Option<bool>,
}

fn hue_of(id: &str) -> f32 {
    let mut h: u32 = 2166136261;
    for b in id.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    (h % 360) as f32 / 360.0
}

fn tint(id: &str, s: f32, v: f32) -> Color32 {
    let c = egui::ecolor::Hsva::new(hue_of(id), s, v, 1.0);
    let [r, g, b] = c.to_srgb();
    Color32::from_rgb(r, g, b)
}

/// Run `script` in a terminal (password prompts and output are visible there) and tell the app when it ends.
fn run_in_terminal(script: String, id: String, tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let full = format!("{script}; echo; echo 'Finished. Press Enter to close this window.'; read _");
        let _ = std::process::Command::new("ghostty").args(["-e", "sh", "-c", &full]).status();
        let _ = tx.send(Msg::Finished(id));
        ctx.request_repaint();
    });
}

impl App {
    fn new(ctx: &egui::Context) -> App {
        let (tx, rx) = channel();
        let dark = theme::system_dark();
        let accent = theme::system_accent();
        theme::apply(ctx, dark, accent);
        let mut app = App {
            pal: theme::palette_for(dark, accent),
            page: Page::Discover,
            history: vec![],
            search: String::new(),
            catalogs: vec![],
            repos: catalog::load_repos(),
            repo_status: vec![],
            apps: vec![],
            installed: Installed::default(),
            busy: HashSet::new(),
            jobs: vec![],
            installs: history::load(),
            show_log: HashSet::new(),
            tx,
            rx,
            add_name: String::new(),
            add_url: String::new(),
            add_kind: 0,
            add_confirm: false,
            pending_add: None,
            status: String::new(),
            details: HashMap::new(),
            details_loading: HashSet::new(),
            images: HashMap::new(),
            img_loading: HashSet::new(),
            online: details::online_allowed(),
        };
        app.rebuild(ctx, false);
        app.refresh_installed(ctx);
        // LABYRINTH_MARKET_PAGE=tab:Create | categories | updates | sources | detail:krita | search:word (for screenshots and tests)
        if let Ok(p) = std::env::var("LABYRINTH_MARKET_PAGE") {
            let (kind, arg) = p.split_once(':').unwrap_or((p.as_str(), ""));
            app.page = match kind {
                "tab" => TABS.iter().find(|t| **t == arg).map(|t| Page::Tab(t)).unwrap_or(Page::Discover),
                "categories" => Page::Categories,
                "updates" => Page::Updates,
                "sources" => Page::Sources,
                "detail" => app.apps.iter().position(|a| a.id == arg).map(Page::Detail).unwrap_or(Page::Discover),
                "search" => {
                    app.search = arg.to_string();
                    Page::Search
                }
                _ => Page::Discover,
            };
        }
        app
    }

    /// Put the built-in catalog and every enabled repository (from its last download; `download` fetches them again) together.
    fn rebuild(&mut self, ctx: &egui::Context, download: bool) {
        let edition = catalog::edition();
        let mut cats = vec![catalog::for_edition(catalog::parse_catalog(catalog::BUILTIN, "Labyrinth", true).unwrap_or_default(), &edition)];
        self.repo_status = vec![String::new(); self.repos.len()];
        let repos = self.repos.clone();
        for (i, r) in repos.iter().enumerate() {
            if !r.enabled {
                continue;
            }
            match catalog::cached(&r.url) {
                Some(t) => match catalog::load_repo(r, &t) {
                    Ok(c) => cats.push(c),
                    Err(e) => self.repo_status[i] = e,
                },
                None => self.repo_status[i] = "not downloaded yet".into(),
            }
            if download || catalog::cached(&r.url).is_none() {
                self.fetch_repo(i, ctx);
            }
        }
        self.apps = cats.iter().flat_map(|c| c.apps.clone()).collect();
        self.catalogs = cats;
    }

    fn fetch_repo(&mut self, i: usize, ctx: &egui::Context) {
        let (url, tx, ctx) = (self.repos[i].url.clone(), self.tx.clone(), ctx.clone());
        self.repo_status[i] = "downloading...".into();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Fetched(i, catalog::fetch(&url)));
            ctx.request_repaint();
        });
    }

    fn refresh_installed(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Installed(catalog::read_installed()));
            ctx.request_repaint();
        });
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Installed(i) => self.installed = i,
                Msg::Fetched(i, res) => {
                    if i < self.repos.len() {
                        match res.and_then(|t| catalog::load_repo(&self.repos[i], &t)) {
                            Ok(_) => self.rebuild(ctx, false),
                            Err(e) => {
                                if i < self.repo_status.len() {
                                    self.repo_status[i] = e;
                                }
                            }
                        }
                    }
                }
                Msg::Finished(id) => {
                    self.busy.remove(&id);
                    self.refresh_installed(ctx);
                    self.installs = history::load();
                }
                Msg::Details(id, d) => {
                    self.details_loading.remove(&id);
                    self.details.insert(id, d);
                }
                Msg::Image(url, p) => {
                    self.img_loading.remove(&url);
                    self.images.insert(url, p);
                }
            }
        }
    }

    /// Start looking up more about an app (once), and its pictures when that is allowed.
    fn want_details(&mut self, ctx: &egui::Context, idx: usize) {
        let Some(e) = self.apps.get(idx).cloned() else { return };
        if e.appstream.is_empty() && e.flathub.is_empty() {
            return;
        }
        if !self.details.contains_key(&e.id) && !self.details_loading.contains(&e.id) {
            self.details_loading.insert(e.id.clone());
            let (tx, ctx2, online, e2) = (self.tx.clone(), ctx.clone(), self.online == Some(true), e.clone());
            std::thread::spawn(move || {
                let d = details::load(&e2.id, &e2.appstream, &e2.flathub, online);
                let _ = tx.send(Msg::Details(e2.id.clone(), d));
                ctx2.request_repaint();
            });
        }
        if self.online == Some(true) {
            let urls: Vec<String> = self.details.get(&e.id).and_then(|d| d.as_ref()).map(|d| d.screenshots.iter().take(6).map(|s| s.url.clone()).collect()).unwrap_or_default();
            for url in urls {
                if !self.images.contains_key(&url) && !self.img_loading.contains(&url) {
                    self.img_loading.insert(url.clone());
                    let (tx, ctx2) = (self.tx.clone(), ctx.clone());
                    std::thread::spawn(move || {
                        let p = details::fetch_image(&url);
                        let _ = tx.send(Msg::Image(url, p));
                        ctx2.request_repaint();
                    });
                }
            }
        }
    }

    /// The one-time question about fetching screenshots and fuller descriptions (nothing is fetched before it is answered).
    fn consent_banner(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new().fill(self.pal.field).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Show screenshots and fuller descriptions?").strong());
            ui.label(RichText::new("They are fetched from Flathub and the app's own site, which can see your address and which app you looked at. Nothing is sent until you say yes; you can change it later in Sources.").color(self.pal.dim));
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Allow").color(self.pal.on_accent())).fill(self.pal.accent).min_size(vec2(90.0, 28.0))).clicked() {
                    self.set_online(true);
                }
                if ui.add(egui::Button::new("Not now").min_size(vec2(90.0, 28.0))).clicked() {
                    self.online = Some(false);
                    details::set_online_allowed(false);
                }
            });
        });
        ui.add_space(8.0);
    }

    fn set_online(&mut self, on: bool) {
        self.online = Some(on);
        details::set_online_allowed(on);
        // what was looked up without the internet is looked up again with it
        self.details.retain(|_, d| d.is_some());
    }

    fn go(&mut self, p: Page) {
        if self.page != p {
            let old = std::mem::replace(&mut self.page, p);
            self.history.push(old);
        }
    }

    // ------------------------------------------------------------ pieces

    /// The app's icon in `rect`: the system icon if there is one, else a coloured tile with its first letter.
    fn icon(&self, ui: &egui::Ui, rect: Rect, e: &Entry) {
        let p = ui.painter();
        let names: Vec<&str> = [e.icon.as_str(), e.id.as_str()].into_iter().filter(|n| !n.is_empty()).collect();
        let r = rect.width() * 0.22;
        if !sysicon::paint(p, rect, &names) {
            p.rect_filled(rect, CornerRadius::same(r as u8), tint(&e.id, 0.55, 0.78));
            p.text(rect.center(), Align2::CENTER_CENTER, e.name.chars().next().unwrap_or('?').to_uppercase().to_string(), FontId::proportional(rect.height() * 0.5), Color32::WHITE);
        }
    }

    /// The Get / Open / Update pill for an entry. Returns what was asked.
    fn pill(&mut self, ui: &mut egui::Ui, e: &Entry, big: bool) -> Option<&'static str> {
        let job = self.job_snapshot(&e.id).filter(|j| j.status == install::Status::Running);
        let running_label = job.as_ref().map(|j| j.frac.map(|f| format!("{}%", (f * 100.0) as u32)).unwrap_or_else(|| "...".into()));
        let (label, kind) = if let Some(l) = running_label {
            (l, "busy")
        } else if self.busy.contains(&e.id) {
            ("...".to_string(), "busy")
        } else if matches!(e.source, Source::Plugin { .. }) {
            ("VIEW".to_string(), "view")
        } else if self.installed.update_for(e) {
            ("UPDATE".to_string(), "get")
        } else if self.installed.has(e) {
            ("OPEN".to_string(), "open")
        } else {
            ("GET".to_string(), "get")
        };
        let size = if big { vec2(92.0, 30.0) } else { vec2(70.0, 26.0) };
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let fill = match (kind, resp.hovered()) {
            ("get", true) => self.pal.accent.gamma_multiply(0.85),
            ("get", false) => self.pal.accent,
            (_, true) => self.pal.hover,
            _ => self.pal.field,
        };
        ui.painter().rect_filled(rect, CornerRadius::same(255), fill);
        if let Some(j) = &job {
            // the pill fills with the accent as the install goes (a moving bit when no percentage is known)
            let b = self.shown_frac(ui.ctx(), j);
            if b > 0.0 {
                let bar = Rect::from_min_max(rect.min, pos2(rect.left() + rect.width() * b.max(0.06), rect.bottom()));
                ui.painter().with_clip_rect(rect).rect_filled(bar, CornerRadius::same(255), self.pal.accent.gamma_multiply(0.45));
            }
        }
        let col = if kind == "get" { self.pal.on_accent() } else { self.pal.accent };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, &label, FontId::proportional(if big { 14.0 } else { 12.5 }), col);
        if resp.clicked() && kind != "busy" {
            return Some(kind);
        }
        None
    }

    /// The newest job for an entry (running, or the last one that finished).
    fn job_snapshot(&self, id: &str) -> Option<install::Job> {
        self.jobs.iter().rev().filter_map(|j| j.lock().ok().map(|g| g.clone())).find(|j| j.id == id)
    }

    fn is_running(&self, id: &str) -> bool {
        self.job_snapshot(id).map(|j| j.status == install::Status::Running).unwrap_or(false)
    }

    /// Install, reinstall or remove an entry. Reviewed apps run here with progress (the password asked for in Labyrinth's own box);
    /// anything else opens a terminal, where the prompts can be read first.
    fn run_job(&mut self, ctx: &egui::Context, idx: usize, op: install::Op) {
        let e = self.apps[idx].clone();
        if self.is_running(&e.id) {
            return;
        }
        if e.reviewed {
            if let Some((prog, args)) = install::command(&e, op) {
                // what to remove again if it is cancelled: an AUR build's folders, a Flatpak that was not there before
                let mut undo = install::Undo::default();
                if op == install::Op::Install || op == install::Op::Reinstall {
                    if let (Source::Aur(p), Some(h)) = (&e.source, std::env::var_os("HOME")) {
                        undo.dirs = p.iter().map(|x| std::path::PathBuf::from(&h).join(".cache/yay").join(x)).collect();
                    }
                    if let Source::Flatpak(a) = &e.source {
                        if op == install::Op::Install && !self.installed.has(&e) {
                            undo.flatpak = Some(a.clone());
                        }
                    }
                }
                self.start_job(ctx, &e.id, &e.name, op, prog, args, undo);
                return;
            }
        }
        let script = if op == install::Op::Remove { catalog::remove_script(&e) } else { catalog::install_script(&e) };
        if let Some(script) = script {
            self.busy.insert(e.id.clone());
            run_in_terminal(script, e.id.clone(), self.tx.clone(), ctx.clone());
        }
    }

    fn run_update_all(&mut self, ctx: &egui::Context) {
        if self.is_running("*update*") {
            return;
        }
        let dummy = Entry { id: "*update*".into(), name: "Updates".into(), subtitle: String::new(), description: String::new(), category: String::new(), subcategory: String::new(), source: Source::Pacman(vec![]), icon: String::new(), reviewed: true, note: String::new(), tags: vec![], from: String::new(), launch: String::new(), appstream: String::new(), flathub: String::new(), editions: vec![], preinstalled: false };
        if let Some((prog, args)) = install::command(&dummy, install::Op::UpdateAll) {
            self.start_job(ctx, "*update*", "All updates", install::Op::UpdateAll, prog, args, install::Undo::default());
        }
    }

    fn start_job(&mut self, ctx: &egui::Context, id: &str, name: &str, op: install::Op, prog: String, args: Vec<String>, undo: install::Undo) {
        let (tx, ctx_a, ctx_b, id2, name2) = (self.tx.clone(), ctx.clone(), ctx.clone(), id.to_string(), name.to_string());
        let job = install::start(id, name, op, prog, args, undo, move || ctx_a.request_repaint(), move |status| {
            // tell the desktop, and the window (so it looks at what is installed again)
            let text = match &status {
                install::Status::Done => format!("{name2} {}", op.done()),
                install::Status::Failed(why) => format!("{name2}: {why}"),
                install::Status::Cancelled => format!("{name2}: cancelled, nothing was kept"),
                install::Status::Running => String::new(),
            };
            // only a finished install goes in the history: not a cancelled or failed one, and not a removal or an update
            if status == install::Status::Done && matches!(op, install::Op::Install | install::Op::Reinstall) {
                history::record(&id2, &name2, op.done());
            }
            let _ = std::process::Command::new("notify-send").args(["-a", "Marketplace", "-i", "labyrinth-market", &text]).status();
            let _ = tx.send(Msg::Finished(id2));
            ctx_b.request_repaint();
        });
        self.jobs.push(job);
        // hide the earlier finished jobs of the same entry
        let keep = self.jobs.len() - 1;
        let mut i = 0;
        self.jobs.retain(|j| {
            let k = i;
            i += 1;
            k == keep || j.lock().map(|g| g.id != id || g.status == install::Status::Running).unwrap_or(true)
        });
    }

    /// What an install is doing: its phase, a bar, and (on request) the output.
    fn progress_card(&mut self, ui: &mut egui::Ui, j: &install::Job) {
        let (fill, tone) = match &j.status {
            install::Status::Running => (self.pal.field, self.pal.accent),
            install::Status::Done => (self.pal.field, self.pal.green),
            install::Status::Failed(_) => (self.pal.red.gamma_multiply(0.15), self.pal.red),
            install::Status::Cancelled => (self.pal.field, self.pal.amber),
        };
        egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let head = match &j.status {
                    install::Status::Running => format!("{} {}", j.op.verb(), j.title),
                    install::Status::Done => format!("{} {}", j.title, j.op.done()),
                    install::Status::Failed(_) => format!("{} could not be {}", j.title, j.op.done()),
                    install::Status::Cancelled => format!("{} was cancelled", j.title),
                };
                ui.label(RichText::new(head).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if j.status == install::Status::Running && !j.cancel_requested {
                        if ui.add(egui::Label::new(RichText::new("Cancel").size(12.5).color(self.pal.red)).sense(Sense::click())).clicked() {
                            if let Some(h) = self.jobs.iter().find(|h| h.lock().map(|g| g.id == j.id && g.status == install::Status::Running).unwrap_or(false)) {
                                install::request_cancel(h);
                            }
                        }
                        ui.add_space(10.0);
                    }
                    let open = self.show_log.contains(&j.id);
                    if ui.add(egui::Label::new(RichText::new(if open { "Hide details" } else { "Show details" }).size(12.5).color(self.pal.accent)).sense(Sense::click())).clicked() {
                        if open {
                            self.show_log.remove(&j.id);
                        } else {
                            self.show_log.insert(j.id.clone());
                        }
                    }
                });
            });
            match &j.status {
                install::Status::Failed(why) => {
                    ui.label(RichText::new(why).color(self.pal.red));
                }
                _ => {
                    ui.label(RichText::new(&j.phase).size(12.5).color(self.pal.dim));
                }
            }
            // the bar
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(4), self.pal.border.gamma_multiply(0.8));
            let b = self.shown_frac(ui.ctx(), j);
            if b > 0.0 {
                ui.painter().rect_filled(Rect::from_min_max(r.min, pos2(r.left() + r.width() * b.max(0.015), r.bottom())), CornerRadius::same(4), tone);
            }
            if self.show_log.contains(&j.id) {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().id_salt(("log", &j.id)).max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
                    for l in j.log.iter().rev().take(60).rev() {
                        ui.label(RichText::new(l).monospace().size(11.5).color(self.pal.dim));
                    }
                });
            }
        });
    }

    /// How far along to draw the bar: what the tool reported, moved on gently while nothing new comes in (a build gives no numbers),
    /// and always eased toward, so it glides instead of jumping.
    fn shown_frac(&self, ctx: &egui::Context, j: &install::Job) -> f32 {
        let since = j.moved.elapsed().as_secs_f32();
        let target = match &j.status {
            install::Status::Done => 1.0,
            install::Status::Failed(_) | install::Status::Cancelled => j.frac.unwrap_or(0.0),
            install::Status::Running => {
                // before anything is known the bar still starts moving, smoothly, from the moment it is begun
                let start = if j.frac.is_none() { 0.05 * (1.0 - (-j.started.elapsed().as_secs_f32() / 1.2).exp()) } else { 0.0 };
                (j.frac.unwrap_or(0.0).max(start) + 0.10 * (1.0 - (-since / 5.0).exp())).min(0.97)
            }
        };
        ctx.animate_value_with_time(egui::Id::new(("bar", &j.id)), target, 0.5)
    }

    fn page_history(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.heading(ui, "History");
        if self.installs.is_empty() {
            ui.label(RichText::new("Apps you install through the Marketplace appear here once the installation has finished. Cancelled installs are not listed.").color(self.pal.dim));
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} app{}", self.installs.len(), if self.installs.len() == 1 { "" } else { "s" })).color(self.pal.dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Label::new(RichText::new("Clear history").size(12.5).color(self.pal.accent)).sense(Sense::click())).clicked() {
                    history::forget(None);
                    self.installs = history::load();
                }
            });
        });
        ui.add_space(6.0);
        let items = self.installs.clone();
        let w = ui.available_width();
        for it in &items {
            let idx = self.apps.iter().position(|a| a.id == it.id);
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 64.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(10), self.pal.hover);
            }
            if let Some(i) = idx {
                let e = self.apps[i].clone();
                self.icon(ui, Rect::from_min_size(rect.min + vec2(6.0, 8.0), vec2(48.0, 48.0)), &e);
            } else {
                ui.painter().rect_filled(Rect::from_min_size(rect.min + vec2(6.0, 8.0), vec2(48.0, 48.0)), CornerRadius::same(11), tint(&it.id, 0.55, 0.78));
            }
            ui.painter().text(pos2(rect.left() + 64.0, rect.top() + 22.0), Align2::LEFT_CENTER, &it.name, FontId::proportional(14.5), self.pal.text);
            ui.painter().text(pos2(rect.left() + 64.0, rect.top() + 42.0), Align2::LEFT_CENTER, format!("{} on {}", it.what[..1].to_uppercase() + &it.what[1..], it.when), FontId::proportional(12.0), self.pal.dim);
            // its button: Open if it is still installed, Get to install it again
            if let Some(i) = idx {
                let e = self.apps[i].clone();
                let mut asked = None;
                ui.scope_builder(UiBuilder::new().max_rect(Rect::from_min_size(pos2(rect.right() - 82.0, rect.top() + 19.0), vec2(76.0, 28.0))), |ui| {
                    asked = self.pill(ui, &e, false);
                });
                if let Some(a) = asked {
                    self.do_pill(ctx, i, a);
                } else if resp.clicked() {
                    self.go(Page::Detail(i));
                }
            }
        }
    }

    fn page_activity(&mut self, ui: &mut egui::Ui) {
        self.heading(ui, "Activity");
        let jobs: Vec<install::Job> = self.jobs.iter().rev().filter_map(|j| j.lock().ok().map(|g| g.clone())).collect();
        if jobs.is_empty() {
            ui.label(RichText::new("Nothing running. Installs, removals and updates show up here while they work.").color(self.pal.dim));
            return;
        }
        for j in &jobs {
            self.progress_card(ui, j);
            ui.add_space(8.0);
        }
        if jobs.iter().any(|j| j.status != install::Status::Running) && ui.button("Clear finished").clicked() {
            self.jobs.retain(|j| j.lock().map(|g| g.status == install::Status::Running).unwrap_or(false));
        }
    }

    fn do_pill(&mut self, ctx: &egui::Context, idx: usize, what: &str) {
        let e = self.apps[idx].clone();
        match what {
            "get" => self.run_job(ctx, idx, install::Op::Install),
            "open" => {
                let name = if e.launch.is_empty() { e.id.clone() } else { e.launch.clone() };
                let _ = std::process::Command::new("gtk-launch").arg(name).spawn();
            }
            "view" => {
                if let Source::Plugin { repo } = &e.source {
                    let _ = std::process::Command::new("xdg-open").arg(repo).spawn();
                }
            }
            _ => {}
        }
    }

    /// One app as a row: icon, name, subtitle, button. Clicking elsewhere opens its page.
    fn row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, idx: usize, width: f32) {
        let e = self.apps[idx].clone();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 64.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(10), self.pal.hover);
        }
        self.icon(ui, Rect::from_min_size(rect.min + vec2(6.0, 8.0), vec2(48.0, 48.0)), &e);
        let tx = rect.left() + 64.0;
        let max_w = rect.width() - 64.0 - 86.0;
        let name = fit(ui, &e.name, 14.5, max_w, true);
        ui.painter().text(pos2(tx, rect.top() + 22.0), Align2::LEFT_CENTER, name, FontId::proportional(14.5), self.pal.text);
        let sub = fit(ui, &e.subtitle, 12.0, max_w, false);
        ui.painter().text(pos2(tx, rect.top() + 42.0), Align2::LEFT_CENTER, sub, FontId::proportional(12.0), self.pal.dim);
        if !e.reviewed {
            ui.painter().circle_filled(pos2(rect.right() - 92.0, rect.top() + 12.0), 3.0, self.pal.amber);
        }
        let mut asked = None;
        ui.scope_builder(UiBuilder::new().max_rect(Rect::from_min_size(pos2(rect.right() - 82.0, rect.top() + 19.0), vec2(76.0, 28.0))), |ui| {
            asked = self.pill(ui, &e, false);
        });
        if let Some(a) = asked {
            self.do_pill(ctx, idx, a);
        } else if resp.clicked() {
            self.go(Page::Detail(idx));
        }
    }

    /// Rows in two columns.
    fn grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, list: &[usize]) {
        let w = ui.available_width();
        let colw = ((w - 16.0) / 2.0).floor();
        for pair in list.chunks(2) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                for &i in pair {
                    self.row(ui, ctx, i, colw);
                }
            });
        }
    }

    fn card(&mut self, ui: &mut egui::Ui, id: &str, tagline: &str, kicker: &str, size: egui::Vec2, big: bool) {
        let Some(idx) = self.apps.iter().position(|a| a.id == id) else { return };
        let e = self.apps[idx].clone();
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        // a screenshot of the app behind the card, when there is one (and the internet was allowed): dimmed from the left,
        // where the text is, so it stays readable. Otherwise a colour gradient.
        let shot = self.details.get(&e.id).cloned().flatten().and_then(|d| d.screenshots.first().map(|s| s.url.clone())).and_then(|u| self.images.get(&u).cloned().flatten());
        let tex = shot.and_then(|path| ui.ctx().try_load_texture(&format!("file://{}", path.display()), egui::TextureOptions::LINEAR, egui::SizeHint::default()).ok()).and_then(|poll| match poll {
            egui::load::TexturePoll::Ready { texture } => Some(texture),
            _ => None,
        });
        if let Some(t) = &tex {
            let (ta, ca) = (t.size.x / t.size.y, rect.width() / rect.height());
            let uv = if ta > ca {
                let fw = ca / ta;
                Rect::from_min_max(pos2((1.0 - fw) / 2.0, 0.0), pos2((1.0 + fw) / 2.0, 1.0))
            } else {
                let fh = ta / ca;
                let v0 = (1.0 - fh) * 0.35;
                Rect::from_min_max(pos2(0.0, v0), pos2(1.0, v0 + fh))
            };
            ui.painter().add(egui::epaint::RectShape::filled(rect, CornerRadius::same(14), Color32::WHITE).with_texture(t.id, uv));
            // the app's own colour over it, then the dark band from the left
            let tintc = tint(&e.id, 0.7, 0.5);
            ui.painter().rect_filled(rect, CornerRadius::same(14), Color32::from_rgba_unmultiplied(tintc.r(), tintc.g(), tintc.b(), 70));
            // the dark band from the left, as one smooth gradient (dark where the text is, light on the right)
            let (l, w) = (rect.left(), rect.width());
            // a pixel wider than the picture, so its soft rounded edge never shows as light dots at the corners
            gradient_rounded(ui.painter(), rect.expand(1.0), 15.0, |p| {
                let f = ((p.x - l) / w).clamp(0.0, 1.0);
                Color32::from_rgba_unmultiplied(8, 8, 16, ((0.88 - 0.64 * f) * 255.0) as u8)
            });
        }
        if tex.is_none() {
            // one flat colour (no light from the top): the sideways shading below is the only shading
            let base = self.catalogs.first().and_then(|c| c.tile_colors.get(&e.id)).map(|&(r, g, b)| Color32::from_rgb(r, g, b)).unwrap_or_else(|| tint(&e.id, 0.62, 0.64));
            ui.painter().rect_filled(rect, CornerRadius::same(14), base);
        }
        if tex.is_none() {
            // the same sideways shading as the screenshot tiles, lighter because a flat colour needs less help: dark where the text is
            let (l, w) = (rect.left(), rect.width());
            gradient_rounded(ui.painter(), rect, 14.0, |p| {
                let f = ((p.x - l) / w).clamp(0.0, 1.0);
                Color32::from_rgba_unmultiplied(8, 8, 16, ((0.50 - 0.48 * f) * 255.0) as u8)
            });
        }
        let p = ui.painter();
        p.text(rect.left_top() + vec2(22.0, 24.0), Align2::LEFT_CENTER, kicker, FontId::proportional(11.5), Color32::from_white_alpha(200));
        p.text(rect.left_top() + vec2(22.0, if big { 58.0 } else { 50.0 }), Align2::LEFT_CENTER, &e.name, FontId::proportional(if big { 32.0 } else { 22.0 }), Color32::WHITE);
        let t = fit(ui, tagline, if big { 17.0 } else { 14.0 }, size.x - 44.0 - if big { 90.0 } else { 62.0 }, false);
        ui.painter().text(rect.left_top() + vec2(22.0, if big { 92.0 } else { 78.0 }), Align2::LEFT_CENTER, t, FontId::proportional(if big { 17.0 } else { 14.0 }), Color32::from_white_alpha(230));
        let isz = if big { 72.0 } else { 52.0 };
        self.icon(ui, Rect::from_min_size(rect.right_bottom() - vec2(isz + 20.0, isz + 18.0), vec2(isz, isz)), &e);
        if resp.clicked() {
            self.go(Page::Detail(idx));
        }
    }

    fn heading(&self, ui: &mut egui::Ui, text: &str) {
        ui.add_space(6.0);
        ui.label(RichText::new(text).size(28.0).strong());
        ui.add_space(8.0);
    }

    fn section(&mut self, ui: &mut egui::Ui, text: &str, see_all: Option<Page>) {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(text).size(19.0).strong());
            if let Some(p) = see_all {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Label::new(RichText::new("See All").size(13.0).color(self.pal.accent)).sense(Sense::click())).clicked() {
                        self.go(p);
                    }
                });
            }
        });
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.left()..=r.right(), r.top() + 2.0, Stroke::new(1.0, self.pal.border));
        ui.add_space(8.0);
    }

    // ------------------------------------------------------------ pages

    fn page_discover(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.heading(ui, "Discover");
        if self.online.is_none() {
            self.consent_banner(ui);
        }
        let featured: Vec<(String, String, String)> = self.catalogs.first().map(|c| c.featured.clone()).unwrap_or_default();
        let w = ui.available_width();
        if let Some((id, tag, kick)) = featured.first() {
            self.card(ui, id, tag, kick, vec2(w, 230.0), true);
        }
        ui.add_space(12.0);
        let rest: Vec<&(String, String, String)> = featured.iter().skip(1).take(3).collect();
        if !rest.is_empty() {
            let cw = ((w - 12.0 * (rest.len() as f32 - 1.0)) / rest.len() as f32).floor();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for (id, tag, kick) in rest {
                    self.card(ui, id, tag, kick, vec2(cw, 150.0), false);
                }
            });
        }
        // the featured apps' screenshots, for the cards (only once the internet was allowed)
        for (id, _, _) in featured.iter().take(4) {
            if let Some(i) = self.apps.iter().position(|a| &a.id == id) {
                self.want_details(ctx, i);
            }
        }
        for tab in TABS {
            let list: Vec<usize> = (0..self.apps.len()).filter(|&i| self.apps[i].category == tab && self.apps[i].from == "Labyrinth").take(6).collect();
            if list.is_empty() {
                continue;
            }
            self.section(ui, &format!("Great for {}", tab.to_lowercase()), Some(Page::Tab(tab)));
            self.grid(ui, ctx, &list);
        }
    }

    fn page_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, tab: &'static str) {
        self.heading(ui, tab);
        let subs = catalog::subcategories(&self.apps, tab);
        if subs.is_empty() {
            ui.label(RichText::new("Nothing here yet.").color(self.pal.dim));
        }
        for sub in subs {
            let list: Vec<usize> = (0..self.apps.len()).filter(|&i| self.apps[i].category == tab && self.apps[i].subcategory == sub).take(8).collect();
            self.section(ui, &sub, Some(Page::Sub(tab.to_string(), sub.clone())));
            self.grid(ui, ctx, &list);
        }
    }

    fn page_categories(&mut self, ui: &mut egui::Ui) {
        self.heading(ui, "Categories");
        let mut cats: Vec<String> = vec![];
        for a in &self.apps {
            if a.category != "Plugins" && !cats.contains(&a.category) {
                cats.push(a.category.clone());
            }
        }
        let order = ["Create", "Work", "Play", "Develop", "Internet", "Security", "Utilities", "Plugins"];
        cats.sort_by_key(|c| order.iter().position(|o| o == c).unwrap_or(99));
        if self.apps.iter().any(|a| a.category == "Plugins") {
            cats.push("Plugins".into());
        }
        for cat in cats {
            self.section(ui, &cat, None);
            let w = ui.available_width();
            let subs = catalog::subcategories(&self.apps, &cat);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                for sub in subs {
                    let n = self.apps.iter().filter(|a| a.category == cat && a.subcategory == sub).count();
                    let (rect, resp) = ui.allocate_exact_size(vec2(((w - 30.0) / 4.0).floor().max(150.0), 56.0), Sense::click());
                    let fill = if resp.hovered() { self.pal.hover } else { self.pal.field };
                    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
                    ui.painter().circle_filled(rect.left_center() + vec2(22.0, 0.0), 9.0, tint(&sub, 0.55, 0.85));
                    ui.painter().text(rect.left_center() + vec2(42.0, -8.0), Align2::LEFT_CENTER, &sub, FontId::proportional(13.5), self.pal.text);
                    ui.painter().text(rect.left_center() + vec2(42.0, 10.0), Align2::LEFT_CENTER, format!("{n} app{}", if n == 1 { "" } else { "s" }), FontId::proportional(11.5), self.pal.dim);
                    if resp.clicked() {
                        self.go(Page::Sub(cat.clone(), sub.clone()));
                    }
                }
            });
        }
    }

    fn page_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, title: &str, list: Vec<usize>) {
        self.heading(ui, title);
        if list.is_empty() {
            ui.label(RichText::new("Nothing found.").color(self.pal.dim));
            return;
        }
        let total = list.len();
        let shown: Vec<usize> = list.into_iter().take(300).collect();
        self.grid(ui, ctx, &shown);
        if total > shown.len() {
            ui.add_space(8.0);
            ui.label(RichText::new(format!("Showing {} of {}. Search to narrow it down.", shown.len(), total)).color(self.pal.dim));
        }
    }

    fn page_updates(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.heading(ui, "Updates");
        let n = self.installed.updates.len();
        ui.horizontal(|ui| {
            ui.label(RichText::new(if n == 0 { "Everything is up to date.".to_string() } else { format!("{n} package{} can be updated.", if n == 1 { "" } else { "s" }) }).color(self.pal.dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Update everything").color(self.pal.on_accent())).fill(self.pal.accent).min_size(vec2(150.0, 30.0))).clicked() {
                    self.run_update_all(ctx);
                }
            });
        });
        ui.add_space(8.0);
        let mine: Vec<usize> = (0..self.apps.len()).filter(|&i| self.installed.update_for(&self.apps[i])).collect();
        if !mine.is_empty() {
            self.section(ui, "Apps with updates", None);
            self.grid(ui, ctx, &mine);
        }
        if n > 0 {
            self.section(ui, "All packages with updates", None);
            let list = self.installed.updates.join("   ");
            ui.label(RichText::new(list).size(12.0).color(self.pal.dim));
        }
    }

    fn page_detail(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, idx: usize) {
        let Some(e) = self.apps.get(idx).cloned() else { return };
        self.want_details(ctx, idx);
        let info = self.details.get(&e.id).cloned().flatten();
        ui.add_space(6.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 120.0), Sense::hover());
        self.icon(ui, Rect::from_min_size(rect.min, vec2(110.0, 110.0)), &e);
        ui.painter().text(rect.left_top() + vec2(130.0, 24.0), Align2::LEFT_CENTER, &e.name, FontId::proportional(28.0), self.pal.text);
        ui.painter().text(rect.left_top() + vec2(130.0, 54.0), Align2::LEFT_CENTER, &e.subtitle, FontId::proportional(14.0), self.pal.dim);
        let (badge, bcol) = if e.reviewed { ("Reviewed by Labyrinth", self.pal.green) } else { ("Not reviewed by Labyrinth", self.pal.amber) };
        ui.painter().circle_filled(rect.left_top() + vec2(136.0, 82.0), 4.0, bcol);
        ui.painter().text(rect.left_top() + vec2(148.0, 82.0), Align2::LEFT_CENTER, format!("{badge}  \u{b7}  from {}", e.from), FontId::proportional(12.5), self.pal.dim);
        let mut asked = None;
        ui.scope_builder(UiBuilder::new().max_rect(Rect::from_min_size(rect.left_top() + vec2(130.0, 92.0), vec2(110.0, 30.0))), |ui| {
            asked = self.pill(ui, &e, true);
        });
        if let Some(a) = asked {
            self.do_pill(ctx, idx, a);
        }
        let installed = self.installed.has(&e);
        let can_remove = installed && !e.preinstalled && catalog::remove_script(&e).is_some();
        let idle = !self.is_running(&e.id) && !self.busy.contains(&e.id);
        let mut x = 232.0;
        // reinstall: only for reviewed apps, where it can run here with progress
        if installed && idle && e.reviewed && install::command(&e, install::Op::Reinstall).is_some() {
            ui.scope_builder(UiBuilder::new().max_rect(Rect::from_min_size(rect.left_top() + vec2(x, 92.0), vec2(108.0, 30.0))), |ui| {
                let (r, resp) = ui.allocate_exact_size(vec2(108.0, 30.0), Sense::click());
                ui.painter().rect_filled(r, CornerRadius::same(255), if resp.hovered() { self.pal.hover } else { self.pal.field });
                ui.painter().text(r.center(), Align2::CENTER_CENTER, "REINSTALL", FontId::proportional(14.0), self.pal.accent);
                if resp.clicked() {
                    self.run_job(ctx, idx, install::Op::Reinstall);
                }
            });
            x += 118.0;
        }
        if can_remove && idle {
            // the same pill as OPEN, beside it, with red text
            ui.scope_builder(UiBuilder::new().max_rect(Rect::from_min_size(rect.left_top() + vec2(x, 92.0), vec2(92.0, 30.0))), |ui| {
                let (r, resp) = ui.allocate_exact_size(vec2(92.0, 30.0), Sense::click());
                ui.painter().rect_filled(r, CornerRadius::same(255), if resp.hovered() { self.pal.hover } else { self.pal.field });
                ui.painter().text(r.center(), Align2::CENTER_CENTER, "REMOVE", FontId::proportional(14.0), self.pal.red);
                if resp.clicked() {
                    self.run_job(ctx, idx, install::Op::Remove);
                }
            });
        }
        if let Some(j) = self.job_snapshot(&e.id) {
            ui.add_space(8.0);
            self.progress_card(ui, &j);
        }
        ui.add_space(14.0);
        if !e.reviewed {
            egui::Frame::new().fill(self.pal.amber.gamma_multiply(0.18)).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Labyrinth has not reviewed this app").strong());
                ui.label(RichText::new("It comes from a catalog or repository that is not Labyrinth's own. Whether it works, and whether it is safe, is up to you: look at where it comes from before you install it.").color(self.pal.dim));
            });
            ui.add_space(10.0);
        }
        if !e.note.is_empty() {
            self.section(ui, if e.reviewed { "Review notes" } else { "Notes" }, None);
            ui.label(RichText::new(&e.note).color(self.pal.dim));
        }
        // pictures: shown when allowed; asked about once
        let has_remote = !e.appstream.is_empty() || !e.flathub.is_empty();
        if has_remote {
            match self.online {
                None => self.consent_banner(ui),
                Some(true) => {
                    if let Some(d) = &info {
                        // only the pictures that came (or are still coming); with none at all there is no Screenshots section
                        let shots: Vec<details::Shot> = d.screenshots.iter().take(6).filter(|s| !matches!(self.images.get(&s.url), Some(None))).cloned().collect();
                        if !shots.is_empty() {
                            self.section(ui, "Screenshots", None);
                            egui::ScrollArea::horizontal().id_salt("shots").auto_shrink([false, true]).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    for s in shots.iter() {
                                        ui.vertical(|ui| {
                                            match self.images.get(&s.url).cloned().flatten() {
                                                Some(path) => {
                                                    ui.add(egui::Image::new(format!("file://{}", path.display())).fit_to_exact_size(vec2(400.0, 250.0)).corner_radius(10));
                                                }
                                                None => {
                                                    let (r, _) = ui.allocate_exact_size(vec2(400.0, 250.0), Sense::hover());
                                                    ui.painter().rect_filled(r, CornerRadius::same(10), self.pal.field);
                                                    ui.painter().text(r.center(), Align2::CENTER_CENTER, if self.img_loading.contains(&s.url) { "Loading..." } else { "No picture" }, FontId::proportional(13.0), self.pal.dim);
                                                }
                                            }
                                            if !s.caption.is_empty() {
                                                ui.add(egui::Label::new(RichText::new(&s.caption).size(12.0).color(self.pal.dim)).wrap());
                                            }
                                        });
                                    }
                                });
                            });
                        }
                    } else if self.details_loading.contains(&e.id) {
                        ui.label(RichText::new("Looking up screenshots...").color(self.pal.dim));
                    }
                }
                Some(false) => {}
            }
        }
        self.section(ui, "About", None);
        match info.as_ref().filter(|d| d.description.len() > e.description.len()) {
            Some(d) => {
                ui.label(&d.description);
            }
            None => {
                ui.label(&e.description);
            }
        }
        self.section(ui, "Information", None);
        if let Some(d) = &info {
            for (k, v) in [("Developer", d.developer.clone()), ("License", d.license.clone())] {
                if !v.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_sized([110.0, 20.0], egui::Label::new(RichText::new(k).color(self.pal.dim)));
                        ui.label(v);
                    });
                }
            }
            if d.homepage.starts_with("https://") {
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, 20.0], egui::Label::new(RichText::new("Website").color(self.pal.dim)));
                    if ui.add(egui::Label::new(RichText::new(&d.homepage).color(self.pal.accent)).sense(Sense::click())).clicked() {
                        let _ = std::process::Command::new("xdg-open").arg(&d.homepage).spawn();
                    }
                });
            }
        }
        let src = match &e.source {
            Source::Pacman(p) => format!("Arch repositories: {}", p.join(", ")),
            Source::Aur(p) => format!("AUR (built with yay): {}", p.join(", ")),
            Source::Flatpak(a) => format!("Flathub: {a}"),
            Source::Component(c) => format!("Labyrinth component: {c}"),
            Source::Plugin { repo } => format!("Plugin repository: {repo}"),
        };
        for (k, v) in [("Source", src), ("Category", format!("{} \u{203a} {}", e.category, e.subcategory)), ("Catalog", e.from.clone())] {
            ui.horizontal(|ui| {
                ui.add_sized([110.0, 20.0], egui::Label::new(RichText::new(k).color(self.pal.dim)));
                ui.label(v);
            });
        }
        if let Some(script) = catalog::install_script(&e) {
            ui.add_space(6.0);
            ui.label(RichText::new(if e.reviewed { format!("Installing runs: {script}") } else { format!("Installing runs, in a terminal: {script}") }).size(12.0).color(self.pal.dim));
        }
    }

    fn page_sources(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.heading(ui, "Sources");
        ui.label(RichText::new("Where apps come from. Labyrinth's own catalog is built in and reviewed. Anything you add is not.").color(self.pal.dim));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let mut on = self.online == Some(true);
            if ui.checkbox(&mut on, "Show screenshots and fuller descriptions from the internet").changed() {
                self.set_online(on);
            }
        });
        ui.label(RichText::new("Fetched from Flathub and the app's own site when you open an app. Off, nothing leaves this computer when you browse.").size(12.0).color(self.pal.dim));
        self.section(ui, "Your sources", None);
        let mut changed = false;
        let mut remove: Option<usize> = None;
        let mut refetch = false;
        egui::Frame::new().fill(self.pal.field).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.painter().circle_filled(ui.cursor().left_center() + vec2(6.0, 0.0), 4.0, self.pal.green);
                ui.add_space(16.0);
                ui.label(RichText::new("Labyrinth").strong());
                ui.label(RichText::new(format!("built in, reviewed, {} apps", self.catalogs.first().map(|c| c.apps.len()).unwrap_or(0))).color(self.pal.dim));
            });
        });
        for i in 0..self.repos.len() {
            ui.add_space(6.0);
            egui::Frame::new().fill(self.pal.field).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.repos[i].enabled, "").changed() {
                        changed = true;
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&self.repos[i].name).strong());
                        ui.label(RichText::new(format!("{}  \u{b7}  {}", if self.repos[i].kind == "omarchy" { "Omarchy plugin registry" } else { "Labyrinth catalog" }, self.repos[i].url)).size(11.5).color(self.pal.dim));
                        if !self.repo_status.get(i).map(|s| s.is_empty()).unwrap_or(true) {
                            ui.label(RichText::new(&self.repo_status[i]).size(11.5).color(self.pal.amber));
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Remove").clicked() {
                            remove = Some(i);
                        }
                        if ui.button("Refresh").clicked() {
                            refetch = true;
                            self.repo_status[i] = String::new();
                            self.fetch_repo(i, ctx);
                        }
                    });
                });
            });
        }
        let _ = refetch;
        self.section(ui, "Add a source", None);
        egui::Frame::new().fill(self.pal.amber.gamma_multiply(0.18)).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Sources you add are not tested or reviewed by Labyrinth").strong());
            ui.label(RichText::new("Their apps can do anything your account can do. Add a source only if you trust who runs it, and look at an app before you install it. Whether something works, and whether it is safe, is up to you.").color(self.pal.dim));
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_sized([60.0, 24.0], egui::Label::new("Name"));
            ui.add(egui::TextEdit::singleline(&mut self.add_name).desired_width(240.0));
        });
        ui.horizontal(|ui| {
            ui.add_sized([60.0, 24.0], egui::Label::new("Address"));
            ui.add(egui::TextEdit::singleline(&mut self.add_url).hint_text("https://example.org/catalog.json").desired_width(420.0));
        });
        ui.horizontal(|ui| {
            ui.add_sized([60.0, 24.0], egui::Label::new("Type"));
            egui::ComboBox::from_id_salt("addkind").selected_text(["Labyrinth catalog (a JSON file)", "Omarchy plugin registry"][self.add_kind]).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.add_kind, 0, "Labyrinth catalog (a JSON file)");
                ui.selectable_value(&mut self.add_kind, 1, "Omarchy plugin registry");
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ok = self.add_url.trim().starts_with("https://") && !self.add_name.trim().is_empty();
            if ui.add_enabled(ok, egui::Button::new("Add source").min_size(vec2(110.0, 28.0))).clicked() {
                self.pending_add = Some(Repo { name: self.add_name.trim().to_string(), url: self.add_url.trim().to_string(), kind: if self.add_kind == 1 { "omarchy".into() } else { "labyrinth".into() }, enabled: true });
                self.add_confirm = false;
            }
            if ui.button("Add the Omarchy plugin market").clicked() {
                self.pending_add = Some(Repo { name: "Omarchy plugins".into(), url: catalog::OMARCHY_URL.into(), kind: "omarchy".into(), enabled: true });
                self.add_confirm = false;
            }
        });
        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).color(self.pal.amber));
        }
        if let Some(i) = remove {
            self.repos.remove(i);
            changed = true;
        }
        if changed {
            catalog::save_repos(&self.repos);
            self.rebuild(ctx, false);
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(r) = self.pending_add.clone() else { return };
        egui::Window::new("Add this source?").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.label(RichText::new(&r.name).strong());
            ui.label(RichText::new(&r.url).size(12.0).color(self.pal.dim));
            ui.add_space(8.0);
            ui.label("Labyrinth has not tested or reviewed anything from this source. Its apps can do anything your account can do, so it is up to you to decide what to trust.");
            ui.add_space(6.0);
            ui.checkbox(&mut self.add_confirm, "I understand, add it");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(self.add_confirm, egui::Button::new("Add")).clicked() {
                    if self.repos.iter().any(|x| x.url == r.url) {
                        self.status = "That address is already in the list.".into();
                    } else {
                        self.repos.push(r.clone());
                        catalog::save_repos(&self.repos);
                        self.add_name.clear();
                        self.add_url.clear();
                        self.status.clear();
                        self.rebuild(ctx, false);
                    }
                    self.pending_add = None;
                }
                if ui.button("Cancel").clicked() {
                    self.pending_add = None;
                }
            });
        });
    }

    fn search_results(&self) -> Vec<usize> {
        let q = self.search.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<(i32, usize)> = vec![];
        for (i, a) in self.apps.iter().enumerate() {
            let hay = format!("{} {} {} {} {} {}", a.name, a.subtitle, a.description, a.category, a.subcategory, a.tags.join(" ")).to_lowercase();
            if terms.iter().all(|t| hay.contains(t)) {
                let name = a.name.to_lowercase();
                let score = if name == q { 0 } else if name.starts_with(&q) { 1 } else if name.contains(&q) { 2 } else { 3 };
                hits.push((score, i));
            }
        }
        hits.sort();
        hits.into_iter().map(|h| h.1).collect()
    }
}

/// A rounded rectangle filled with a smooth gradient: a triangle fan whose corner colours come from `color_at`, so the colour
/// changes continuously (no steps) and the corners are properly round.
fn gradient_rounded(painter: &egui::Painter, rect: Rect, radius: f32, color_at: impl Fn(egui::Pos2) -> Color32) {
    let mut pts: Vec<egui::Pos2> = vec![];
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [(rect.right() - r, rect.top() + r, -90.0f32), (rect.right() - r, rect.bottom() - r, 0.0), (rect.left() + r, rect.bottom() - r, 90.0), (rect.left() + r, rect.top() + r, 180.0)];
    for (cx, cy, start) in corners {
        for k in 0..=8 {
            let a = (start + 90.0 * k as f32 / 8.0).to_radians();
            pts.push(pos2(cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    let mut mesh = egui::epaint::Mesh::default();
    mesh.colored_vertex(rect.center(), color_at(rect.center()));
    for p in &pts {
        mesh.colored_vertex(*p, color_at(*p));
    }
    for i in 0..pts.len() {
        mesh.add_triangle(0, 1 + i as u32, 1 + ((i + 1) % pts.len()) as u32);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// `text` cut with an ellipsis to fit `width` points.
fn fit(ui: &egui::Ui, text: &str, size: f32, width: f32, _bold: bool) -> String {
    let font = FontId::proportional(size);
    let fits = |t: &str| ui.fonts_mut(|f| f.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x) <= width;
    if fits(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 1 {
        n -= 1;
        let t: String = chars[..n].iter().collect::<String>() + "\u{2026}";
        if fits(&t) {
            return t;
        }
    }
    "\u{2026}".into()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        theme::apply_accessibility(&ctx);
        self.receive(&ctx);
        chrome::resize_edges(&ctx);
        let dark = theme::system_dark();
        let accent = theme::system_accent();
        theme::apply(&ctx, dark, accent);
        self.pal = theme::palette_for(dark, accent);
        let pal = self.pal.clone();

        // sidebar: search, then the sections
        egui::Panel::left("sections")
            .default_size(220.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(Margin::symmetric(10, 10)))
            .show(ui, |ui| {
                ui.add_space(2.0);
                let r = ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search").desired_width(f32::INFINITY).margin(vec2(8.0, 6.0)));
                if r.changed() {
                    if self.search.trim().is_empty() {
                        if self.page == Page::Search {
                            self.page = Page::Discover;
                        }
                    } else if self.page != Page::Search {
                        let old = std::mem::replace(&mut self.page, Page::Search);
                        self.history.push(old);
                    }
                }
                ui.add_space(10.0);
                let n_up = self.installed.updates.len();
                let items: Vec<(&str, Page, Option<usize>)> = vec![
                    ("Discover", Page::Discover, None),
                    ("Create", Page::Tab("Create"), None),
                    ("Work", Page::Tab("Work"), None),
                    ("Play", Page::Tab("Play"), None),
                    ("Develop", Page::Tab("Develop"), None),
                    ("Categories", Page::Categories, None),
                    ("Updates", Page::Updates, if n_up > 0 { Some(n_up) } else { None }),
                    ("Activity", Page::Activity, { let n = self.jobs.iter().filter(|j| j.lock().map(|j| j.status == install::Status::Running).unwrap_or(false)).count(); if n > 0 { Some(n) } else { None } }),
                ];
                for (label, page, badge) in items {
                    let sel = self.page == page || (matches!(page, Page::Tab(t) if matches!(&self.page, Page::Sub(c, _) if c == t)));
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                    if sel {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
                    }
                    ui.painter().text(rect.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(14.0), if sel { pal.accent } else { pal.text });
                    if let Some(n) = badge {
                        let c = rect.right_center() - vec2(20.0, 0.0);
                        ui.painter().circle_filled(c, 10.0, pal.accent.gamma_multiply(0.9));
                        ui.painter().text(c, Align2::CENTER_CENTER, n.to_string().chars().take(3).collect::<String>(), FontId::proportional(10.5), pal.on_accent());
                    }
                    if resp.clicked() {
                        self.search.clear();
                        self.go(page);
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(4.0);
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                    if self.page == Page::Sources {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
                    }
                    ui.painter().text(rect.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, "Sources", FontId::proportional(14.0), pal.dim);
                    if resp.clicked() {
                        self.go(Page::Sources);
                    }
                    // History sits just above it
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                    if self.page == Page::History {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
                    }
                    ui.painter().text(rect.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, "History", FontId::proportional(14.0), pal.dim);
                    if resp.clicked() {
                        self.go(Page::History);
                    }
                });
            });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 28, right: 12, top: 6, bottom: 18 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                let can_back = !self.history.is_empty();
                if ui.add_enabled(can_back, egui::Button::new("\u{2039}").min_size(vec2(30.0, 24.0))).clicked() {
                    if let Some(p) = self.history.pop() {
                        self.page = p;
                    }
                }
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width((ui.available_width() - 20.0).min(1100.0));
                match self.page.clone() {
                    Page::Discover => self.page_discover(ui, &ctx),
                    Page::Tab(t) => self.page_tab(ui, &ctx, t),
                    Page::Categories => self.page_categories(ui),
                    Page::Sub(cat, sub) => {
                        let list: Vec<usize> = (0..self.apps.len()).filter(|&i| self.apps[i].category == cat && self.apps[i].subcategory == sub).collect();
                        self.page_list(ui, &ctx, &sub, list);
                    }
                    Page::Updates => self.page_updates(ui, &ctx),
                    Page::Activity => self.page_activity(ui),
                    Page::History => self.page_history(ui, &ctx),
                    Page::Detail(i) => self.page_detail(ui, &ctx, i),
                    Page::Sources => self.page_sources(ui, &ctx),
                    Page::Search => {
                        let list = self.search_results();
                        let title = format!("Results for \u{201c}{}\u{201d}", self.search.trim());
                        self.page_list(ui, &ctx, &title, list);
                    }
                }
            });
        });
        self.confirm_dialog(&ctx);
        chrome::outline(&ctx, &pal);
        if !self.busy.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
        if self.jobs.iter().any(|j| j.lock().map(|j| j.status == install::Status::Running).unwrap_or(false)) {
            ctx.request_repaint_after(std::time::Duration::from_millis(33)); // the bars glide
        }
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1180.0, 760.0]).with_min_inner_size([760.0, 480.0]).with_title("Marketplace").with_app_id("labyrinth-market").with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "Marketplace",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
    )
}
