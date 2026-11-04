//! Huginn: videos without an account, in Yggdrasil's look (laid out like Agora). Home is the newest from your subscriptions; search,
//! channels, saved videos, history and downloads; videos play in yggdrasil-player. Everything you keep (subscriptions, history,
//! saved videos) stays on this computer (~/.local/share/yggdrasil/huginn); the videos come through yt-dlp (huginn-core).
//!
//! Named after Odin's raven that flies out over the world each day and brings back what it saw.

mod jobs;
mod widgets;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText, Sense, Stroke};
use huginn_core::library::Library;
use huginn_core::model::{count_text, Channel, Details, Video};
use huginn_core::ytdlp::YtDlp;
use huginn_core::{import, Extractor};
use jobs::{DlState, Download, Msg, Pictures};
use widgets::{columns, texture, video_card, Tex};
use yggdrasil_ui::{chrome, theme};
use theme::Palette;

#[derive(Clone, PartialEq, Debug)]
enum Page {
    Home,
    Subscriptions,
    Saved,
    History,
    Downloads,
    Search,
    Video(String),
    Channel(String),
    Settings,
}

/// ~/.config/yggdrasil/huginn.conf: `quality = 1080`, `per_channel = 15`, `folder = ~/Videos/Huginn`.
#[derive(Clone, Debug, PartialEq)]
struct Conf {
    quality: u32,
    per_channel: usize,
    folder: PathBuf,
}

impl Default for Conf {
    fn default() -> Self {
        let videos = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Videos");
        Conf { quality: 1080, per_channel: 15, folder: videos.join("Huginn") }
    }
}

impl Conf {
    fn path() -> PathBuf {
        yggdrasil_conf::dirs::config().join("huginn.conf")
    }

    fn parse(t: &str) -> Conf {
        let mut c = Conf::default();
        for line in t.lines() {
            match line.split('#').next().unwrap_or("").split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("quality", v)) => c.quality = v.parse().ok().filter(|q| [360, 480, 720, 1080, 1440, 2160].contains(q)).unwrap_or(c.quality),
                Some(("per_channel", v)) => c.per_channel = v.parse().ok().filter(|n| (5..=60).contains(n)).unwrap_or(c.per_channel),
                Some(("folder", v)) if !v.is_empty() => {
                    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
                    c.folder = v.strip_prefix("~/").map(|r| home.join(r)).unwrap_or_else(|| PathBuf::from(v));
                }
                _ => {}
            }
        }
        c
    }

    fn load() -> Conf {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    fn save(&self) {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let folder = self.folder.strip_prefix(&home).map(|r| format!("~/{}", r.display())).unwrap_or_else(|_| self.folder.display().to_string());
        let _ = std::fs::create_dir_all(Self::path().parent().unwrap_or(&home));
        let _ = std::fs::write(Self::path(), format!("# Huginn (its Settings page)\nquality = {}\nper_channel = {}\nfolder = {folder}\n", self.quality, self.per_channel));
    }
}

/// Something asked for and not here yet.
enum Loading<T> {
    Waiting,
    Ready(T),
    Failed(String),
}

struct App {
    pal: Palette,
    page: Page,
    back: Vec<Page>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    x: Arc<YtDlp>,
    lib: Library,
    conf: Conf,
    pictures: Pictures,
    images: HashMap<String, Option<PathBuf>>,
    search: String,
    searched: String,
    results: Option<Result<Vec<Video>, String>>,
    details: HashMap<String, Loading<Details>>,
    channels: HashMap<String, Loading<(Channel, Vec<Video>)>>,
    /// a refresh of the feed: (done, total) while it runs
    refreshing: Option<(usize, usize)>,
    feed_errors: Vec<String>,
    downloads: Vec<Arc<Mutex<Download>>>,
    note: String,
    import_path: String,
    /// a video asked to play before its streams arrived: (id, sound only, start)
    pending_play: Option<(String, bool, Option<f64>)>,
    ytdlp_ok: bool,
}

/// The video id in a YouTube link (`watch?v=`, `youtu.be/`, `/shorts/`, `/live/`), or None.
fn video_id_in(link: &str) -> Option<String> {
    let l = link.trim();
    let rest = l.split_once("watch?v=").map(|(_, r)| r)
        .or_else(|| l.split_once("youtu.be/").map(|(_, r)| r))
        .or_else(|| l.split_once("/shorts/").map(|(_, r)| r))
        .or_else(|| l.split_once("/live/").map(|(_, r)| r))?;
    let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    huginn_core::model::valid_video_id(&id).then_some(id)
}

fn library_dir() -> PathBuf {
    yggdrasil_conf::dirs::data().join("huginn")
}

impl App {
    fn new(ctx: &egui::Context) -> App {
        let (tx, rx) = channel();
        let dark = theme::system_dark();
        let accent = theme::system_accent();
        theme::apply(ctx, dark, accent);
        let x = Arc::new(YtDlp::default());
        let ytdlp_ok = x.available();
        let mut app = App {
            pal: theme::palette_for(dark, accent),
            page: Page::Home,
            back: vec![],
            pictures: Pictures::start(tx.clone(), ctx.clone(), 6),
            tx,
            rx,
            x,
            lib: Library::open(&library_dir()),
            conf: Conf::load(),
            images: HashMap::new(),
            search: String::new(),
            searched: String::new(),
            results: None,
            details: HashMap::new(),
            channels: HashMap::new(),
            refreshing: None,
            feed_errors: vec![],
            downloads: vec![],
            note: String::new(),
            import_path: String::new(),
            pending_play: None,
            ytdlp_ok,
        };
        // `huginn --search WORDS` or `huginn LINK` (a YouTube video or channel link) opens on it
        let args: Vec<String> = std::env::args().skip(1).collect();
        match args.first().map(String::as_str) {
            Some("--search") if args.len() > 1 => {
                app.search = args[1..].join(" ");
                app.do_search(ctx);
                app.back.clear();
            }
            Some(link) => {
                if let Some(id) = video_id_in(link) {
                    app.open_video(&Video { id, ..Default::default() }, ctx);
                    app.back = vec![Page::Home];
                } else if let Some(id) = import::channel_id_in(link).filter(|_| link.contains("/channel/")) {
                    app.open_channel(&id, ctx);
                    app.back = vec![Page::Home];
                }
            }
            None => {}
        }
        // a feed older than half an hour is refreshed when the window opens
        if ytdlp_ok && !app.lib.subscriptions.is_empty() && huginn_core::library::now().saturating_sub(app.lib.refreshed) > 1800 {
            app.refresh(ctx);
        }
        app
    }

    fn go(&mut self, p: Page) {
        if self.page != p {
            let old = std::mem::replace(&mut self.page, p);
            self.back.push(old);
            self.back.truncate(50);
        }
    }

    fn tex(&mut self, ui: &egui::Ui, url: Option<&str>) -> Option<Tex> {
        let url = url?;
        match self.images.get(url) {
            Some(Some(p)) => texture(ui, Some(p)),
            Some(None) => None,
            None => {
                self.images.insert(url.to_string(), None);
                self.pictures.want(url);
                None
            }
        }
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Image(url, p) => {
                    self.images.insert(url, p);
                }
                Msg::Search(q, r) => {
                    if q == self.searched {
                        self.results = Some(r);
                    }
                }
                Msg::Details(id, r) => {
                    if let Some((pid, audio, start)) = self.pending_play.clone().filter(|(p, _, _)| *p == id) {
                        self.pending_play = None;
                        match &r {
                            Ok(d) => self.started(d, audio, start),
                            Err(e) => self.note = format!("Could not play: {e}"),
                        }
                        // a sound-only answer is not the video page's
                        if audio {
                            continue;
                        }
                        let _ = pid;
                    }
                    self.details.insert(id, match r {
                        Ok(d) => Loading::Ready(d),
                        Err(e) => Loading::Failed(e),
                    });
                }
                Msg::Channel(id, r) => {
                    if let Ok((c, _)) = &r {
                        if self.lib.subscribed(&c.id) {
                            self.lib.subscribe(c.clone());
                        }
                    }
                    self.channels.insert(id, match r {
                        Ok(x) => Loading::Ready(x),
                        Err(e) => Loading::Failed(e),
                    });
                }
                Msg::FeedChannel(c, vids, done, total) => {
                    self.lib.merge_channel(&c, vids, huginn_core::library::now());
                    self.refreshing = Some((done, total));
                }
                Msg::FeedFailed(e, done, total) => {
                    self.feed_errors.push(e);
                    self.refreshing = Some((done, total));
                }
                Msg::FeedDone => {
                    self.refreshing = None;
                    self.lib.refreshed = huginn_core::library::now();
                    let _ = self.lib.save();
                }
                Msg::Played(r) => {
                    self.note = match r {
                        Ok(t) => format!("Playing \u{201c}{t}\u{201d} in the player"),
                        Err(e) => format!("Could not play: {e}"),
                    }
                }
            }
        }
        if self.refreshing.is_some() || self.downloads.iter().any(|d| d.lock().map(|d| d.state == DlState::Running).unwrap_or(false)) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    // ------------------------------------------------------------ actions

    fn refresh(&mut self, ctx: &egui::Context) {
        if self.refreshing.is_some() || self.lib.subscriptions.is_empty() {
            return;
        }
        let subs = self.lib.subscriptions.clone();
        let total = subs.len();
        self.refreshing = Some((0, total));
        self.feed_errors.clear();
        let (x, tx, ctx, per) = (self.x.clone(), self.tx.clone(), ctx.clone(), self.conf.per_channel);
        std::thread::spawn(move || {
            // four channels at a time: quick, and gentle on YouTube
            let next = Arc::new(Mutex::new(0usize));
            let done = Arc::new(Mutex::new(0usize));
            let subs = Arc::new(subs);
            let workers: Vec<_> = (0..4).map(|_| {
                let (x, tx, ctx, next, done, subs) = (x.clone(), tx.clone(), ctx.clone(), next.clone(), done.clone(), subs.clone());
                std::thread::spawn(move || loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        let i = *n;
                        *n += 1;
                        i
                    };
                    let Some(c) = subs.get(i) else { break };
                    let r = x.channel(&c.id, per);
                    let d = {
                        let mut d = done.lock().unwrap();
                        *d += 1;
                        *d
                    };
                    let _ = tx.send(match r {
                        Ok((ch, vids)) => Msg::FeedChannel(ch, vids, d, subs.len()),
                        Err(e) => Msg::FeedFailed(format!("{}: {e}", c.name), d, subs.len()),
                    });
                    ctx.request_repaint();
                })
            }).collect();
            for w in workers {
                let _ = w.join();
            }
            let _ = tx.send(Msg::FeedDone);
            ctx.request_repaint();
        });
    }

    fn open_video(&mut self, v: &Video, ctx: &egui::Context) {
        if !self.details.contains_key(&v.id) {
            self.details.insert(v.id.clone(), Loading::Waiting);
            let (x, id, q) = (self.x.clone(), v.id.clone(), self.conf.quality);
            jobs::spawn(&self.tx, ctx, move || Msg::Details(id.clone(), x.details(&id, q, false).map_err(|e| e.to_string())));
        }
        self.go(Page::Video(v.id.clone()));
    }

    fn open_channel(&mut self, id: &str, ctx: &egui::Context) {
        if !matches!(self.channels.get(id), Some(Loading::Ready(_)) | Some(Loading::Waiting)) {
            self.channels.insert(id.to_string(), Loading::Waiting);
            let (x, cid, per) = (self.x.clone(), id.to_string(), 30);
            jobs::spawn(&self.tx, ctx, move || Msg::Channel(cid.clone(), x.channel(&cid, per).map_err(|e| e.to_string())));
        }
        self.go(Page::Channel(id.to_string()));
    }

    fn do_search(&mut self, ctx: &egui::Context) {
        let q = self.search.trim().to_string();
        if q.is_empty() || q == self.searched && self.results.is_some() {
            self.go(Page::Search);
            return;
        }
        self.searched = q.clone();
        self.results = None;
        let x = self.x.clone();
        jobs::spawn(&self.tx, ctx, move || Msg::Search(q.clone(), x.search(&q, 40).map_err(|e| e.to_string())));
        self.go(Page::Search);
    }

    /// Plays a video (or its sound): with the streams already here at once, else as soon as they arrive.
    fn play(&mut self, v: &Video, audio: bool, start: Option<f64>, ctx: &egui::Context) {
        if !audio {
            if let Some(Loading::Ready(d)) = self.details.get(&v.id) {
                let d = d.clone();
                self.started(&d, false, start);
                return;
            }
        }
        self.note = if audio { "Getting the sound…".into() } else { "Getting the video…".into() };
        self.pending_play = Some((v.id.clone(), audio, start));
        if audio || !matches!(self.details.get(&v.id), Some(Loading::Waiting)) {
            let (x, id, q) = (self.x.clone(), v.id.clone(), self.conf.quality);
            if !audio {
                self.details.insert(v.id.clone(), Loading::Waiting);
            }
            jobs::spawn(&self.tx, ctx, move || Msg::Details(id.clone(), x.details(&id, q, audio).map_err(|e| e.to_string())));
        }
    }

    fn started(&mut self, d: &Details, _audio: bool, start: Option<f64>) {
        let r = jobs::play(d, start);
        if r.is_ok() {
            self.lib.watched(d.video.clone(), start.map(|s| s as u64));
            let _ = self.lib.save();
        }
        let _ = self.tx.send(Msg::Played(r));
    }

    fn subscribe(&mut self, c: Channel, ctx: &egui::Context) {
        if self.lib.subscribed(&c.id) {
            self.lib.unsubscribe(&c.id);
            self.note = format!("Unsubscribed from {}", c.name);
        } else {
            let name = c.name.clone();
            self.lib.subscribe(c.clone());
            self.note = format!("Subscribed to {name}");
            // its videos join the feed at once
            if let Some(Loading::Ready((ch, vids))) = self.channels.get(&c.id) {
                let (ch, vids) = (ch.clone(), vids.clone());
                self.lib.merge_channel(&ch, vids.into_iter().take(self.conf.per_channel).collect(), huginn_core::library::now());
            } else {
                let (x, tx, cid, per) = (self.x.clone(), self.tx.clone(), c.id.clone(), self.conf.per_channel);
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    if let Ok((ch, vids)) = x.channel(&cid, per) {
                        let _ = tx.send(Msg::FeedChannel(ch, vids, 1, 1));
                        let _ = tx.send(Msg::FeedDone);
                        ctx.request_repaint();
                    }
                });
            }
        }
        let _ = self.lib.save();
    }

    fn import_file(&mut self, path: &std::path::Path, ctx: &egui::Context) {
        match std::fs::read_to_string(path) {
            Ok(t) => {
                let subs = import::subscriptions(&t);
                let new = subs.into_iter().filter(|c| self.lib.subscribe(c.clone())).count();
                let _ = self.lib.save();
                self.note = if new == 0 { format!("No new channels in {}", path.display()) } else { format!("{new} channels added; getting their videos…") };
                if new > 0 {
                    self.refresh(ctx);
                }
            }
            Err(e) => self.note = format!("{}: {e}", path.display()),
        }
    }

    // ------------------------------------------------------------ page parts

    fn heading(&self, ui: &mut egui::Ui, text: &str) {
        ui.add_space(6.0);
        ui.label(RichText::new(text).size(28.0).strong());
        ui.add_space(8.0);
    }

    fn section(&self, ui: &mut egui::Ui, text: &str) {
        ui.add_space(14.0);
        ui.label(RichText::new(text).size(19.0).strong());
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.left()..=r.right(), r.top() + 2.0, Stroke::new(1.0, self.pal.border));
        ui.add_space(10.0);
    }

    /// Cards in a grid; a click opens the video.
    fn grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, videos: &[Video], with_channel: bool) {
        let (n, w) = columns(ui.available_width(), 250.0);
        let mut open: Option<Video> = None;
        for row in videos.chunks(n) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                for v in row {
                    let tex = self.tex(ui, v.thumbnail.as_deref());
                    if video_card(ui, &self.pal, v, tex.as_ref(), w, with_channel).clicked() {
                        open = Some(v.clone());
                    }
                }
            });
            ui.add_space(10.0);
        }
        if let Some(v) = open {
            self.open_video(&v, ctx);
        }
    }

    /// Agora's big card: the newest video's picture, darker from the left where its title is.
    fn hero(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, v: &Video, kicker: &str) {
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 300.0), Sense::click());
        let tex = self.tex(ui, v.thumbnail.as_deref());
        widgets::picture(ui, rect, 16, tex.as_ref(), &self.pal);
        let (l, rw) = (rect.left(), rect.width());
        widgets::gradient_rounded(ui.painter(), rect.expand(1.0), 17.0, |p| {
            let f = ((p.x - l) / rw).clamp(0.0, 1.0);
            Color32::from_rgba_unmultiplied(8, 8, 16, ((0.92 - 0.70 * f) * 255.0) as u8)
        });
        let p = ui.painter();
        p.text(rect.left_top() + vec2(28.0, 34.0), Align2::LEFT_CENTER, kicker, FontId::proportional(12.5), Color32::from_white_alpha(210));
        let title = widgets::wrapped(ui, &v.title, 30.0, (w * 0.55).max(320.0), 3, Color32::WHITE, true);
        let th = title.size().y;
        ui.painter().galley(rect.left_top() + vec2(28.0, 56.0), title, Color32::WHITE);
        ui.painter().text(rect.left_top() + vec2(28.0, 70.0 + th), Align2::LEFT_CENTER, widgets::byline(v, true), FontId::proportional(14.0), Color32::from_white_alpha(220));
        // the play button on the card
        let play = Rect::from_min_size(rect.left_bottom() + vec2(28.0, -64.0), vec2(120.0, 40.0));
        let hov = ui.rect_contains_pointer(play);
        ui.painter().rect_filled(play, CornerRadius::same(20), if hov { self.pal.accent } else { Color32::from_white_alpha(235) });
        ui.painter().text(play.center(), Align2::CENTER_CENTER, "\u{25B6}  Play", FontId::proportional(15.0), if hov { self.pal.on_accent() } else { Color32::from_rgb(20, 20, 28) });
        if resp.clicked() {
            if hov {
                self.play(v, false, None, ctx);
            } else {
                self.open_video(v, ctx);
            }
        }
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        if !self.note.is_empty() {
            ui.label(RichText::new(&self.note).color(self.pal.dim));
        }
    }

    // ------------------------------------------------------------ pages

    fn page_home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.heading(ui, "Home");
        if !self.ytdlp_ok {
            ui.label(RichText::new("yt-dlp is not installed, so Huginn cannot reach any videos. Agora has it (or: sudo pacman -S yt-dlp).").color(self.pal.red));
            return;
        }
        let feed: Vec<Video> = self.lib.feed().into_iter().cloned().collect();
        if self.lib.subscriptions.is_empty() {
            let w = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(vec2(w, 220.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(16), self.pal.accent.gamma_multiply(0.55));
            widgets::gradient_rounded(ui.painter(), rect, 16.0, |p| {
                let f = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                Color32::from_rgba_unmultiplied(8, 8, 16, ((0.55 - 0.45 * f) * 255.0) as u8)
            });
            let p = ui.painter();
            p.text(rect.left_top() + vec2(28.0, 40.0), Align2::LEFT_CENTER, "No account needed", FontId::proportional(12.5), Color32::from_white_alpha(210));
            p.text(rect.left_top() + vec2(28.0, 82.0), Align2::LEFT_CENTER, "Subscribe to channels, here on this computer", FontId::proportional(28.0), Color32::WHITE);
            p.text(rect.left_top() + vec2(28.0, 122.0), Align2::LEFT_CENTER, "Search for a channel, or bring your subscriptions from NewPipe, FreeTube or OpenTubeX.", FontId::proportional(15.0), Color32::from_white_alpha(225));
            let b = Rect::from_min_size(rect.left_bottom() + vec2(28.0, -62.0), vec2(220.0, 38.0));
            let hov = ui.rect_contains_pointer(b);
            ui.painter().rect_filled(b, CornerRadius::same(19), if hov { Color32::WHITE } else { Color32::from_white_alpha(230) });
            ui.painter().text(b.center(), Align2::CENTER_CENTER, "Import subscriptions\u{2026}", FontId::proportional(14.5), Color32::from_rgb(20, 20, 28));
            if hov && ui.input(|i| i.pointer.primary_clicked()) {
                self.go(Page::Subscriptions);
            }
            return;
        }
        if let Some((d, t)) = self.refreshing {
            ui.label(RichText::new(format!("Looking for new videos: {d} of {t} channels")).color(self.pal.dim));
            ui.add_space(6.0);
        }
        if let Some(first) = feed.first().cloned() {
            self.hero(ui, ctx, &first, &format!("NEW FROM {}", first.channel.to_uppercase()));
        } else if self.refreshing.is_none() {
            ui.label(RichText::new("No videos yet: refresh on the Subscriptions page.").color(self.pal.dim));
        }
        self.status_line(ui);
        let unfinished: Vec<Video> = self.lib.history.iter().filter(|w| w.position.unwrap_or(0) > 0).take(4).map(|w| w.video.clone()).collect();
        if !unfinished.is_empty() {
            self.section(ui, "Continue watching");
            self.grid(ui, ctx, &unfinished, true);
        }
        if feed.len() > 1 {
            self.section(ui, "Latest from your subscriptions");
            let rest: Vec<Video> = feed.into_iter().skip(1).take(60).collect();
            self.grid(ui, ctx, &rest, true);
        }
    }

    fn page_subscriptions(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.heading(ui, "Subscriptions");
        ui.horizontal(|ui| {
            let busy = self.refreshing.is_some();
            if ui.add_enabled(!busy && !self.lib.subscriptions.is_empty(), egui::Button::new("Refresh")).clicked() {
                self.refresh(ctx);
            }
            if let Some((d, t)) = self.refreshing {
                ui.spinner();
                ui.label(RichText::new(format!("{d} of {t}")).color(self.pal.dim));
            } else if self.lib.refreshed > 0 {
                let mins = huginn_core::library::now().saturating_sub(self.lib.refreshed) / 60;
                ui.label(RichText::new(if mins < 1 { "Refreshed just now".to_string() } else { format!("Refreshed {mins} min ago") }).color(self.pal.dim));
            }
        });
        if !self.feed_errors.is_empty() {
            ui.label(RichText::new(format!("{} channels did not answer: {}", self.feed_errors.len(), self.feed_errors.join("; "))).color(self.pal.amber));
        }
        self.status_line(ui);

        self.section(ui, "Bring your subscriptions");
        ui.label(RichText::new("From NewPipe (Settings > Content > Export subscriptions), FreeTube or OpenTubeX (profiles.db), or Google Takeout (subscriptions.csv).").color(self.pal.dim));
        // export files that are already in Downloads, offered by name
        let downloads = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Downloads");
        let mut found: Vec<PathBuf> = std::fs::read_dir(&downloads).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            p.is_file() && (n.contains("newpipe") && n.ends_with(".json") || n == "profiles.db" || n == "subscriptions.csv" || n.contains("subscriptions") && n.ends_with(".json"))
        }).collect()).unwrap_or_default();
        found.sort();
        let mut import: Option<PathBuf> = None;
        for p in &found {
            ui.horizontal(|ui| {
                ui.label(p.file_name().unwrap_or_default().to_string_lossy());
                if ui.button("Import").clicked() {
                    import = Some(p.clone());
                }
            });
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.import_path).hint_text("Or the file's path").desired_width(360.0));
            if ui.add_enabled(!self.import_path.trim().is_empty(), egui::Button::new("Import")).clicked() {
                let t = self.import_path.trim();
                let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
                import = Some(t.strip_prefix("~/").map(|r| home.join(r)).unwrap_or_else(|| PathBuf::from(t)));
            }
        });
        if let Some(p) = import {
            self.import_file(&p, ctx);
        }

        self.section(ui, &format!("Your channels ({})", self.lib.subscriptions.len()));
        let subs = self.lib.subscriptions.clone();
        let mut open: Option<String> = None;
        let mut drop: Option<String> = None;
        for c in &subs {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                let tex = self.tex(ui, c.avatar.as_deref());
                match tex {
                    Some(t) => {
                        ui.painter().add(egui::epaint::RectShape::filled(r, CornerRadius::same(18), Color32::WHITE).with_texture(t.id, widgets::cover_uv(t.size, r)));
                    }
                    None => {
                        ui.painter().circle_filled(r.center(), 18.0, self.pal.field);
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, c.name.chars().next().unwrap_or('?').to_string(), FontId::proportional(16.0), self.pal.text);
                    }
                }
                if ui.add(egui::Label::new(RichText::new(&c.name).size(15.0)).sense(Sense::click())).clicked() {
                    open = Some(c.id.clone());
                }
                if let Some(f) = c.followers {
                    ui.label(RichText::new(format!("{} subscribers", count_text(f))).color(self.pal.dim));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Unsubscribe").clicked() {
                        drop = Some(c.id.clone());
                    }
                });
            });
        }
        if let Some(id) = open {
            self.open_channel(&id, ctx);
        }
        if let Some(id) = drop {
            self.lib.unsubscribe(&id);
            let _ = self.lib.save();
        }
    }

    fn page_search(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.heading(ui, &format!("Results for \u{201c}{}\u{201d}", self.searched));
        match self.results.clone() {
            None => {
                ui.spinner();
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("The search did not work: {e}")).color(self.pal.red));
            }
            Some(Ok(list)) if list.is_empty() => {
                ui.label(RichText::new("Nothing found").color(self.pal.dim));
            }
            Some(Ok(list)) => {
                // channels in the results lead to their page
                let mut seen: Vec<(String, String)> = vec![];
                for v in &list {
                    if !v.channel_id.is_empty() && !seen.iter().any(|(id, _)| *id == v.channel_id) {
                        seen.push((v.channel_id.clone(), v.channel.clone()));
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    for (id, name) in seen.iter().take(8) {
                        if ui.button(name).on_hover_text("The channel's page").clicked() {
                            self.open_channel(id, ctx);
                        }
                    }
                });
                ui.add_space(8.0);
                self.grid(ui, ctx, &list, true);
            }
        }
    }

    fn page_video(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, id: &str) {
        let d = match self.details.get(id) {
            Some(Loading::Ready(d)) => d.clone(),
            Some(Loading::Failed(e)) => {
                ui.label(RichText::new(format!("This video could not be opened: {e}")).color(self.pal.red));
                return;
            }
            _ => {
                ui.add_space(40.0);
                ui.spinner();
                return;
            }
        };
        let v = d.video.clone();
        // the picture, with the play button on it
        let w = ui.available_width().min(960.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, w * 9.0 / 16.0), Sense::click());
        let tex = self.tex(ui, v.thumbnail.as_deref());
        widgets::picture(ui, rect, 16, tex.as_ref(), &self.pal);
        ui.painter().rect_filled(rect, CornerRadius::same(16), Color32::from_black_alpha(if resp.hovered() { 40 } else { 70 }));
        ui.painter().circle_filled(rect.center(), 38.0, Color32::from_white_alpha(if resp.hovered() { 250 } else { 225 }));
        ui.painter().text(rect.center() + vec2(3.0, 0.0), Align2::CENTER_CENTER, "\u{25B6}", FontId::proportional(30.0), Color32::from_rgb(20, 20, 28));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            self.play(&v, false, None, ctx);
        }
        ui.add_space(12.0);
        ui.label(RichText::new(&v.title).size(22.0).strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add(egui::Label::new(RichText::new(&v.channel).size(15.0).color(self.pal.accent)).sense(Sense::click())).clicked() && !v.channel_id.is_empty() {
                self.open_channel(&v.channel_id.clone(), ctx);
            }
            if let Some(f) = d.followers {
                ui.label(RichText::new(format!("{} subscribers", count_text(f))).color(self.pal.dim));
            }
            let subscribed = self.lib.subscribed(&v.channel_id);
            if !v.channel_id.is_empty() && widgets::pill(ui, &self.pal, if subscribed { "Subscribed" } else { "Subscribe" }, !subscribed).on_hover_text(if subscribed { "Click to unsubscribe" } else { "Its videos come to Home" }).clicked() {
                self.subscribe(Channel { id: v.channel_id.clone(), name: v.channel.clone(), avatar: None, followers: d.followers }, ctx);
            }
        });
        let mut stats = widgets::byline(&v, false);
        if let Some(l) = d.likes {
            stats = format!("{stats} · {} likes", count_text(l));
        }
        if let Some(h) = d.streams.height {
            stats = format!("{stats} · {h}p");
        }
        ui.label(RichText::new(stats).color(self.pal.dim));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if widgets::pill(ui, &self.pal, "\u{25B6}  Play", true).clicked() {
                self.play(&v, false, None, ctx);
            }
            if widgets::pill(ui, &self.pal, "Listen", false).on_hover_text("The sound only").clicked() {
                self.play(&v, true, None, ctx);
            }
            if widgets::pill(ui, &self.pal, "Download", false).on_hover_text(format!("Into {}", self.conf.folder.display())).clicked() {
                self.downloads.insert(0, jobs::download(&v, false, self.conf.quality, &self.conf.folder, ctx));
                self.note = "Downloading: see Downloads".into();
            }
            if widgets::pill(ui, &self.pal, "Download sound", false).clicked() {
                self.downloads.insert(0, jobs::download(&v, true, self.conf.quality, &self.conf.folder, ctx));
                self.note = "Downloading: see Downloads".into();
            }
            let saved = self.lib.is_saved(&v.id);
            if widgets::pill(ui, &self.pal, if saved { "Saved" } else { "Save" }, false).clicked() {
                self.lib.toggle_saved(&v);
                let _ = self.lib.save();
            }
        });
        self.status_line(ui);
        // yt-dlp names chapters it made up "<Untitled Chapter N>": only real ones are shown
        let real: Vec<_> = d.chapters.iter().filter(|c| !c.title.starts_with("<Untitled")).cloned().collect();
        if real.len() > 1 {
            self.section(ui, "Chapters");
            let mut jump: Option<f64> = None;
            for c in &real {
                let t = c.start as u64;
                let stamp = if t >= 3600 { format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60) } else { format!("{}:{:02}", t / 60, t % 60) };
                if ui.add(egui::Label::new(RichText::new(format!("{stamp}   {}", c.title))).sense(Sense::click())).on_hover_text("Play from here").clicked() {
                    jump = Some(c.start);
                }
            }
            if let Some(s) = jump {
                self.play(&v, false, Some(s), ctx);
            }
        }
        if !d.description.trim().is_empty() {
            self.section(ui, "About this video");
            ui.add(egui::Label::new(RichText::new(d.description.trim()).size(13.5)).wrap());
        }
    }

    fn page_channel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, id: &str) {
        let (c, vids) = match self.channels.get(id) {
            Some(Loading::Ready(x)) => x.clone(),
            Some(Loading::Failed(e)) => {
                ui.label(RichText::new(format!("This channel could not be opened: {e}")).color(self.pal.red));
                return;
            }
            _ => {
                ui.add_space(40.0);
                ui.spinner();
                return;
            }
        };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
            match self.tex(ui, c.avatar.as_deref()) {
                Some(t) => {
                    ui.painter().add(egui::epaint::RectShape::filled(r, CornerRadius::same(32), Color32::WHITE).with_texture(t.id, widgets::cover_uv(t.size, r)));
                }
                None => {
                    ui.painter().circle_filled(r.center(), 32.0, self.pal.field);
                }
            }
            ui.vertical(|ui| {
                ui.label(RichText::new(&c.name).size(26.0).strong());
                if let Some(f) = c.followers {
                    ui.label(RichText::new(format!("{} subscribers", count_text(f))).color(self.pal.dim));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let subscribed = self.lib.subscribed(&c.id);
                if widgets::pill(ui, &self.pal, if subscribed { "Subscribed" } else { "Subscribe" }, !subscribed).clicked() {
                    self.subscribe(c.clone(), ctx);
                }
            });
        });
        self.status_line(ui);
        self.section(ui, "Latest videos");
        self.grid(ui, ctx, &vids, false);
    }

    fn page_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, title: &str, list: Vec<Video>, empty: &str) {
        self.heading(ui, title);
        if list.is_empty() {
            ui.label(RichText::new(empty).color(self.pal.dim));
            return;
        }
        self.grid(ui, ctx, &list, true);
    }

    fn page_downloads(&mut self, ui: &mut egui::Ui) {
        self.heading(ui, "Downloads");
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Into {}", self.conf.folder.display())).color(self.pal.dim));
            if ui.button("Open the folder").clicked() {
                let _ = std::fs::create_dir_all(&self.conf.folder);
                let _ = std::process::Command::new("xdg-open").arg(&self.conf.folder).stdin(std::process::Stdio::null()).spawn();
            }
        });
        if self.downloads.is_empty() {
            ui.label(RichText::new("Nothing downloaded in this session. A video's page has Download and Download sound.").color(self.pal.dim));
        }
        for d in self.downloads.clone() {
            let d = d.lock().map(|d| d.clone()).ok();
            let Some(d) = d else { continue };
            ui.add_space(6.0);
            ui.label(RichText::new(format!("{}{}", d.video.title, if d.audio_only { " (sound)" } else { "" })).strong());
            match &d.state {
                DlState::Running => {
                    ui.add(egui::ProgressBar::new(d.progress).show_percentage());
                }
                DlState::Done => {
                    ui.label(RichText::new("Done").color(self.pal.green));
                }
                DlState::Failed(e) => {
                    ui.label(RichText::new(format!("Stopped: {e}")).color(self.pal.red));
                }
            }
        }
    }

    fn page_settings(&mut self, ui: &mut egui::Ui) {
        self.heading(ui, "Settings");
        let mut c = self.conf.clone();
        ui.horizontal(|ui| {
            ui.label("Picture quality, at most");
            egui::ComboBox::from_id_salt("quality").selected_text(format!("{}p", c.quality)).show_ui(ui, |ui| {
                for q in [360, 480, 720, 1080, 1440, 2160] {
                    ui.selectable_value(&mut c.quality, q, format!("{q}p"));
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label("Videos per channel in the feed");
            ui.add(egui::Slider::new(&mut c.per_channel, 5..=60));
        });
        if c != self.conf {
            c.save();
            self.conf = c;
        }
        self.section(ui, "Your subscriptions elsewhere");
        if ui.button("Export for NewPipe").on_hover_text("Writes huginn-subscriptions.json into Downloads: NewPipe imports it").clicked() {
            let p = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Downloads").join("huginn-subscriptions.json");
            self.note = match std::fs::write(&p, import::to_newpipe(&self.lib.subscriptions)) {
                Ok(()) => format!("Written: {}", p.display()),
                Err(e) => format!("{}: {e}", p.display()),
            };
        }
        self.section(ui, "History");
        if ui.button("Clear the watch history").clicked() {
            self.lib.clear_history();
            let _ = self.lib.save();
            self.note = "History cleared".into();
        }
        self.status_line(ui);
        self.section(ui, "About");
        ui.label(RichText::new("Videos come through yt-dlp; nothing about you leaves this computer except what YouTube sees of any viewer. Your subscriptions, history and saved videos are kept in ~/.local/share/yggdrasil/huginn.").color(self.pal.dim));
    }
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

        egui::Panel::left("sections")
            .default_size(220.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(Margin::symmetric(10, 10)))
            .show(ui, |ui| {
                ui.add_space(2.0);
                let r = ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search videos").desired_width(f32::INFINITY).margin(vec2(8.0, 6.0)));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.do_search(&ctx);
                }
                ui.add_space(10.0);
                let running = self.downloads.iter().filter(|d| d.lock().map(|d| d.state == DlState::Running).unwrap_or(false)).count();
                let items: Vec<(&str, Page, Option<usize>)> = vec![
                    ("Home", Page::Home, None),
                    ("Subscriptions", Page::Subscriptions, None),
                    ("Saved", Page::Saved, None),
                    ("History", Page::History, None),
                    ("Downloads", Page::Downloads, if running > 0 { Some(running) } else { None }),
                ];
                for (label, page, badge) in items {
                    let sel = self.page == page;
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
                        ui.painter().text(c, Align2::CENTER_CENTER, n.to_string(), FontId::proportional(10.5), pal.on_accent());
                    }
                    if resp.clicked() {
                        self.go(page);
                    }
                }
                // the subscriptions, under the sections (a click opens the channel)
                if !self.lib.subscriptions.is_empty() {
                    ui.add_space(12.0);
                    ui.label(RichText::new("Channels").size(12.0).color(pal.dim));
                    let subs = self.lib.subscriptions.clone();
                    egui::ScrollArea::vertical().id_salt("channels").max_height(ui.available_height() - 50.0).show(ui, |ui| {
                        for c in subs {
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                            if resp.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(6), pal.hover);
                            }
                            let t = widgets::wrapped(ui, &c.name, 13.0, rect.width() - 20.0, 1, pal.text, false);
                            ui.painter().galley(rect.left_center() + vec2(12.0, -t.size().y / 2.0), t, pal.text);
                            if resp.clicked() {
                                self.open_channel(&c.id, &ctx);
                            }
                        }
                    });
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(4.0);
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                    if self.page == Page::Settings {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
                    }
                    ui.painter().text(rect.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, "Settings", FontId::proportional(14.0), pal.dim);
                    if resp.clicked() {
                        self.go(Page::Settings);
                    }
                });
            });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 28, right: 12, top: 6, bottom: 18 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.back.is_empty(), egui::Button::new("\u{2039}").min_size(vec2(30.0, 24.0))).clicked() {
                    if let Some(p) = self.back.pop() {
                        self.page = p;
                    }
                }
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(format!("{:?}", self.page)).show(ui, |ui| {
                ui.set_max_width((ui.available_width() - 20.0).min(1400.0));
                match self.page.clone() {
                    Page::Home => self.page_home(ui, &ctx),
                    Page::Subscriptions => self.page_subscriptions(ui, &ctx),
                    Page::Search => self.page_search(ui, &ctx),
                    Page::Video(id) => self.page_video(ui, &ctx, &id),
                    Page::Channel(id) => self.page_channel(ui, &ctx, &id),
                    Page::Saved => {
                        let l = self.lib.saved.clone();
                        self.page_list(ui, &ctx, "Saved", l, "Videos you save (on a video's page) are kept here.");
                    }
                    Page::History => {
                        let l: Vec<Video> = self.lib.history.iter().map(|w| w.video.clone()).take(120).collect();
                        self.page_list(ui, &ctx, "History", l, "Videos you play show up here.");
                    }
                    Page::Downloads => self.page_downloads(ui),
                    Page::Settings => self.page_settings(ui),
                }
            });
        });
        chrome::outline(&ctx, &pal);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1240.0, 800.0]).with_min_inner_size([760.0, 480.0]).with_title("Huginn").with_app_id("huginn").with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "Huginn",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        assert_eq!(video_id_in("https://www.youtube.com/watch?v=3lJ5oT_JviI&t=42s").as_deref(), Some("3lJ5oT_JviI"));
        assert_eq!(video_id_in("https://youtu.be/3lJ5oT_JviI?si=x").as_deref(), Some("3lJ5oT_JviI"));
        assert_eq!(video_id_in("https://www.youtube.com/shorts/3lJ5oT_JviI").as_deref(), Some("3lJ5oT_JviI"));
        assert_eq!(video_id_in("https://www.youtube.com/watch?v=short"), None);
        assert_eq!(video_id_in("--exec=x"), None);
    }

    #[test]
    fn conf_reads_and_refuses_odd_values() {
        let c = Conf::parse("quality = 720\nper_channel = 20\nfolder = /data/v\n");
        assert_eq!((c.quality, c.per_channel, c.folder.as_path()), (720, 20, std::path::Path::new("/data/v")));
        let d = Conf::parse("quality = 999\nper_channel = 5000\n");
        assert_eq!((d.quality, d.per_channel), (Conf::default().quality, Conf::default().per_channel));
    }
}
