//! 2048 for Yggdrasil: slide the tiles with the arrow keys (or WASD, hjkl, or a drag), two equal tiles merge, reach the goal tile.
//! Four modes (Classic, Fibonacci, Evil, Daily), boards from 3x3 to 6x6, undo, stats, tiles in the accent's shades or the original
//! colours. The rules follow gabrielecirulli/2048, the modes imranZERO/omarchy-2048 (both MIT). No network; the game, best scores
//! and stats are kept in ~/.local/share/yggdrasil/2048/state.json.

mod game;

use eframe::egui::{self, vec2, Align2, Color32, CornerRadius, FontId, Margin, Pos2, Rect, RichText, Sense};
use game::{Dir, Game, Mode, Undo};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Instant;
use yggdrasil_ui::chrome;
use yggdrasil_ui::theme::{self, Palette};

/// How long tiles take to slide, and to pop or grow after it.
const SLIDE: f32 = 0.10;
const POP: f32 = 0.14;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Colors {
    /// Shades of the system accent.
    #[default]
    Accent,
    /// The original game's beige and orange.
    Original,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Settings {
    mode: Mode,
    size: usize,
    #[serde(default)]
    colors: Colors,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { mode: Mode::Classic, size: 4, colors: Colors::Accent }
    }
}

/// One mode and board size: its game in progress and best score.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Slot {
    game: Option<Game>,
    best: u32,
    /// Daily: the day `best` is for (the best is for today only).
    #[serde(default)]
    best_day: String,
    /// This game is already counted in the stats (ended, or left for a new one).
    #[serde(default)]
    counted: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
struct Stats {
    games: u32,
    wins: u32,
    best_tile: u32,
    best_score: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    settings: Settings,
    slots: BTreeMap<String, Slot>,
    stats: BTreeMap<String, Stats>,
}

fn key(mode: Mode, size: usize) -> String {
    format!("{}-{size}", mode.label())
}

fn state_path() -> std::path::PathBuf {
    yggdrasil_conf::dirs::data().join("2048").join("state.json")
}

fn load() -> Saved {
    std::fs::read_to_string(state_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Written to a new file and renamed over the old one, so a crash never leaves half a file.
fn save(s: &Saved) {
    let p = state_path();
    if let (Some(dir), Ok(text)) = (p.parent(), serde_json::to_string(s)) {
        let _ = std::fs::create_dir_all(dir);
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// Today's date (UTC, YYYY-MM-DD): the seed of the Daily game, the same everywhere.
fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // days since 1970-01-01 to a civil date (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

/// A move being drawn: the tiles sliding from the old board, then the merged ones popping and the new one growing.
struct Anim {
    t0: Instant,
    slides: Vec<(u32, (usize, usize), (usize, usize))>,
    merged: Vec<(usize, usize)>,
    spawned: Option<(usize, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Play,
    Stats,
    Settings,
}

struct App {
    saved: Saved,
    game: Game,
    undo: Undo,
    view: View,
    anim: Option<Anim>,
    drag_from: Option<Pos2>,
    /// Stats page: every mode and size, or just this one.
    stats_all: bool,
}

impl App {
    fn new() -> App {
        let saved = load();
        let mut app = App { game: Game::new(saved.settings.size, saved.settings.mode, &today()), saved, undo: Undo::default(), view: View::Play, anim: None, drag_from: None, stats_all: false };
        app.open_slot();
        app
    }

    fn slot(&mut self) -> &mut Slot {
        let k = key(self.game.mode, self.game.size);
        self.saved.slots.entry(k).or_default()
    }

    fn stats(&mut self) -> &mut Stats {
        let k = key(self.game.mode, self.game.size);
        self.saved.stats.entry(k).or_default()
    }

    /// The saved game of the chosen mode and size, or a new one (a Daily game from another day is replaced).
    fn open_slot(&mut self) {
        let (mode, size) = (self.saved.settings.mode, self.saved.settings.size);
        let day = today();
        let slot = self.saved.slots.entry(key(mode, size)).or_default();
        if mode == Mode::Daily && slot.best_day != day {
            slot.best = 0;
            slot.best_day = day.clone();
        }
        let resumable = slot.game.clone().filter(|g| g.size == size && g.mode == mode && (mode != Mode::Daily || g.day == day));
        self.game = match resumable {
            Some(g) => g,
            None => {
                slot.counted = false;
                Game::new(size, mode, &day)
            }
        };
        self.undo.clear();
        self.anim = None;
        self.store();
    }

    fn store(&mut self) {
        let g = self.game.clone();
        self.slot().game = Some(g);
        save(&self.saved);
    }

    /// Counts the game in the stats once: when it ends, or when it is left for a new one after a move.
    fn count(&mut self) {
        if self.game.moves == 0 || self.slot().counted {
            return;
        }
        self.slot().counted = true;
        self.stats().games += 1;
    }

    fn new_game(&mut self) {
        self.count();
        let (mode, size) = (self.game.mode, self.game.size);
        self.game = Game::new(size, mode, &today());
        self.slot().counted = false;
        self.undo.clear();
        self.anim = None;
        self.store();
    }

    fn play(&mut self, dir: Dir) {
        if self.game.terminated() {
            return;
        }
        let before = self.game.clone();
        let m = self.game.play(dir);
        if !m.moved {
            return;
        }
        self.undo.push(before);
        let (score, tile) = (self.game.score, self.game.max_tile());
        if score > self.slot().best {
            self.slot().best = score;
        }
        let st = self.stats();
        st.best_score = st.best_score.max(score);
        st.best_tile = st.best_tile.max(tile);
        if m.won {
            self.stats().wins += 1;
        }
        if self.game.over {
            self.count();
        }
        self.anim = Some(Anim { t0: Instant::now(), slides: m.slides, merged: m.merged, spawned: m.spawned });
        self.store();
    }

    fn take_back(&mut self) {
        if let Some(g) = self.undo.pop() {
            self.game = g;
            self.anim = None;
            self.store();
        }
    }

    fn choose(&mut self, mode: Mode, size: usize) {
        if (mode, size) == (self.saved.settings.mode, self.saved.settings.size) {
            return;
        }
        self.store();
        self.saved.settings.mode = mode;
        self.saved.settings.size = size;
        self.open_slot();
    }

    fn keys(&mut self, ctx: &egui::Context) {
        use egui::Key;
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        let ctrl = ctx.input(|i| i.modifiers.command);
        if ctrl && pressed(Key::Z) {
            self.take_back();
            return;
        }
        if ctrl {
            return;
        }
        for (keys, d) in [([Key::ArrowUp, Key::W, Key::K], Dir::Up), ([Key::ArrowRight, Key::D, Key::L], Dir::Right), ([Key::ArrowDown, Key::S, Key::J], Dir::Down), ([Key::ArrowLeft, Key::A, Key::H], Dir::Left)] {
            if keys.iter().any(|&k| pressed(k)) {
                self.play(d);
                return;
            }
        }
        if pressed(Key::U) {
            self.take_back();
        } else if pressed(Key::N) {
            self.new_game();
        }
    }
}

fn hex(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A tile's fill and text colour.
fn tile_colors(level: u32, colors: Colors, pal: &Palette) -> (Color32, Color32) {
    match colors {
        Colors::Original => {
            const BG: [u32; 11] = [0xeee4da, 0xede0c8, 0xf2b179, 0xf59563, 0xf67c5f, 0xf65e3b, 0xedcf72, 0xedcc61, 0xedc850, 0xedc53f, 0xedc22e];
            let bg = BG.get(level.saturating_sub(1) as usize).map(|&h| hex(h)).unwrap_or(hex(0x3c3a32));
            (bg, if level <= 2 { hex(0x776e65) } else { hex(0xf9f6f2) })
        }
        Colors::Accent => {
            let t = (level as f32 / 11.0).min(1.0);
            let bg = if level > 11 { mix(pal.accent, Color32::BLACK, ((level - 11) as f32 * 0.12).min(0.5)) } else { mix(pal.field, pal.accent, 0.12 + 0.88 * t) };
            (bg, if t > 0.4 { Color32::WHITE } else { pal.text })
        }
    }
}

fn board_colors(colors: Colors, pal: &Palette) -> (Color32, Color32) {
    match colors {
        Colors::Original => (hex(0xbbada0), hex(0xcdc1b4)),
        Colors::Accent => (pal.field, pal.bg),
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

impl App {
    fn draw_tile(&self, p: &egui::Painter, center: Pos2, cell: f32, value: u32, scale: f32, pal: &Palette) {
        let level = game::level_of(value, self.game.mode, self.game.size);
        let (bg, fg) = tile_colors(level, self.saved.settings.colors, pal);
        let r = Rect::from_center_size(center, vec2(cell, cell) * scale);
        p.rect_filled(r, CornerRadius::same((cell * 0.08) as u8), bg);
        let digits = value.to_string().len();
        let size = cell * scale * match digits { 1 | 2 => 0.45, 3 => 0.38, 4 => 0.30, _ => 0.24 };
        p.text(r.center(), Align2::CENTER_CENTER, value.to_string(), FontId::proportional(size), fg);
    }

    /// The board in `area` (square, centred); returns its rectangle.
    fn board(&mut self, ui: &mut egui::Ui, area: Rect, pal: &Palette) -> Rect {
        let side = area.width().min(area.height()).max(120.0);
        let rect = Rect::from_center_size(area.center(), vec2(side, side));
        let n = self.game.size;
        let gap = side * 0.025;
        let cell = (side - gap * (n as f32 + 1.0)) / n as f32;
        let at = |x: f32, y: f32| rect.min + vec2(gap + x * (cell + gap) + cell / 2.0, gap + y * (cell + gap) + cell / 2.0);
        let p = ui.painter_at(rect);
        let (bg, empty) = board_colors(self.saved.settings.colors, pal);
        p.rect_filled(rect, CornerRadius::same(10), bg);
        for x in 0..n {
            for y in 0..n {
                p.rect_filled(Rect::from_center_size(at(x as f32, y as f32), vec2(cell, cell)), CornerRadius::same((cell * 0.08) as u8), empty);
            }
        }
        let t = self.anim.as_ref().map(|a| a.t0.elapsed().as_secs_f32()).unwrap_or(f32::MAX);
        match &self.anim {
            // the old board's tiles on their way
            Some(a) if t < SLIDE => {
                let k = ease_out(t / SLIDE);
                for &(v, (fx, fy), (tx, ty)) in &a.slides {
                    let (x, y) = (fx as f32 + (tx as f32 - fx as f32) * k, fy as f32 + (ty as f32 - fy as f32) * k);
                    self.draw_tile(&p, at(x, y), cell, v, 1.0, pal);
                }
            }
            _ => {
                let after = t - SLIDE;
                for x in 0..n {
                    for y in 0..n {
                        let v = self.game.cells[x][y];
                        if v == 0 {
                            continue;
                        }
                        let mut scale = 1.0;
                        if let Some(a) = self.anim.as_ref().filter(|_| after < POP) {
                            let k = (after / POP).clamp(0.0, 1.0);
                            if a.merged.contains(&(x, y)) {
                                scale = 1.0 + 0.18 * (k * std::f32::consts::PI).sin();
                            } else if a.spawned == Some((x, y)) {
                                scale = ease_out(k);
                            }
                        }
                        if scale > 0.01 {
                            self.draw_tile(&p, at(x as f32, y as f32), cell, v, scale, pal);
                        }
                    }
                }
            }
        }
        if t >= SLIDE + POP {
            self.anim = None;
        } else {
            ui.ctx().request_repaint();
        }
        rect
    }

    /// Won, or no move left: a veil over the board with what can be done.
    fn veil(&mut self, ui: &mut egui::Ui, board: Rect, pal: &Palette) {
        let won = self.game.won && !self.game.keep_playing;
        if !won && !self.game.over {
            return;
        }
        ui.painter().rect_filled(board, CornerRadius::same(10), pal.bg.gamma_multiply(0.82));
        let title = if won { "You win!" } else { "Game over" };
        ui.painter().text(board.center() - vec2(0.0, 34.0), Align2::CENTER_CENTER, title, FontId::proportional(34.0), pal.text);
        let row = Rect::from_center_size(board.center() + vec2(0.0, 22.0), vec2(250.0, 34.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
            ui.horizontal_centered(|ui| {
                if won {
                    if ui.add(egui::Button::new(RichText::new("Keep going").color(pal.on_accent())).fill(pal.accent)).clicked() {
                        self.game.keep_playing = true;
                        self.store();
                    }
                    if ui.button("New game").clicked() {
                        self.new_game();
                    }
                } else {
                    if ui.add(egui::Button::new(RichText::new("Try again").color(pal.on_accent())).fill(pal.accent)).clicked() {
                        self.new_game();
                    }
                    if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo")).clicked() {
                        self.take_back();
                    }
                }
            });
        });
    }

    fn score_box(ui: &mut egui::Ui, pal: &Palette, label: &str, value: u32) {
        egui::Frame::new().fill(pal.field).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(12, 4)).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(11.0).color(pal.dim));
                ui.label(RichText::new(value.to_string()).size(18.0).strong().color(pal.text));
            });
        });
    }

    fn page_play(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.horizontal(|ui| {
            Self::score_box(ui, pal, "SCORE", self.game.score);
            let best = self.saved.slots.get(&key(self.game.mode, self.game.size)).map(|s| s.best).unwrap_or(0);
            Self::score_box(ui, pal, if self.game.mode == Mode::Daily { "TODAY'S BEST" } else { "BEST" }, best);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("New game").clicked() {
                    self.new_game();
                }
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo")).clicked() {
                    self.take_back();
                }
            });
        });
        ui.add_space(8.0);
        let avail = ui.available_rect_before_wrap();
        let area = Rect::from_min_max(avail.min, avail.max - vec2(0.0, 24.0));
        // a drag across the board is a move, like a swipe
        let resp = ui.interact(area, ui.id().with("board"), Sense::drag());
        if resp.drag_started() {
            self.drag_from = resp.interact_pointer_pos();
        }
        if resp.drag_stopped() {
            if let (Some(a), Some(b)) = (self.drag_from.take(), ui.ctx().input(|i| i.pointer.latest_pos())) {
                let d = b - a;
                if d.length() > 30.0 {
                    self.play(if d.x.abs() > d.y.abs() { if d.x > 0.0 { Dir::Right } else { Dir::Left } } else if d.y > 0.0 { Dir::Down } else { Dir::Up });
                }
            }
        }
        let board = self.board(ui, area, pal);
        self.veil(ui, board, pal);
        let hint = Rect::from_min_max(egui::pos2(avail.min.x, avail.max.y - 20.0), avail.max);
        ui.painter().text(hint.center(), Align2::CENTER_CENTER, format!("Arrows, WASD or hjkl to move · U undo · N new game · goal {}", self.game.goal), FontId::proportional(11.5), pal.dim);
        ui.allocate_rect(avail, Sense::hover());
    }

    fn page_stats(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.horizontal(|ui| {
            let this = format!("{} {}x{}", self.game.mode.label(), self.game.size, self.game.size);
            ui.selectable_value(&mut self.stats_all, false, this);
            ui.selectable_value(&mut self.stats_all, true, "All modes and sizes");
        });
        ui.add_space(10.0);
        let s = if self.stats_all {
            // a Fibonacci tile is not comparable with the others, so it is left out of the overall best tile
            self.saved.stats.iter().fold(Stats::default(), |a, (k, s)| Stats { games: a.games + s.games, wins: a.wins + s.wins, best_tile: if k.starts_with("Fibonacci") { a.best_tile } else { a.best_tile.max(s.best_tile) }, best_score: a.best_score.max(s.best_score) })
        } else {
            self.saved.stats.get(&key(self.game.mode, self.game.size)).copied().unwrap_or_default()
        };
        let rows = [("Games played", s.games.to_string()), ("Games won", s.wins.to_string()), ("Win rate", if s.games == 0 { "-".into() } else { format!("{}%", s.wins * 100 / s.games.max(1)) }), ("Best tile", s.best_tile.to_string()), ("Best score", s.best_score.to_string())];
        egui::Frame::new().fill(pal.field).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (k, v) in rows {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(k).color(pal.dim));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(RichText::new(v).strong().color(pal.text)));
                });
                ui.add_space(4.0);
            }
        });
    }

    fn page_settings(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.label(RichText::new("Tile colours").strong());
        let before = self.saved.settings.colors;
        ui.radio_value(&mut self.saved.settings.colors, Colors::Accent, "Shades of the accent colour");
        ui.radio_value(&mut self.saved.settings.colors, Colors::Original, "The original 2048 colours");
        if before != self.saved.settings.colors {
            save(&self.saved);
        }
        ui.add_space(14.0);
        ui.label(RichText::new("Modes").strong());
        for m in Mode::ALL {
            ui.add_space(4.0);
            ui.label(RichText::new(m.label()).color(pal.text));
            ui.label(RichText::new(m.about()).size(12.0).color(pal.dim));
        }
        ui.add_space(14.0);
        ui.label(RichText::new("Each mode and board size keeps its own game, best score and stats. Daily's game is the same for everyone on a given day (UTC) and board size.").size(12.0).color(pal.dim));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        theme::apply(&ctx, true, accent);
        let pal = theme::palette_for(true, accent);
        if self.view == View::Play {
            self.keys(&ctx);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.view = View::Play;
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 14, right: 8, top: 6, bottom: 12 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("2048").size(18.0).strong().color(pal.text));
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let (mut mode, mut size) = (self.saved.settings.mode, self.saved.settings.size);
                egui::ComboBox::from_id_salt("mode").selected_text(mode.label()).show_ui(ui, |ui| {
                    for m in Mode::ALL {
                        ui.selectable_value(&mut mode, m, m.label());
                    }
                });
                egui::ComboBox::from_id_salt("size").selected_text(format!("{size}x{size}")).width(70.0).show_ui(ui, |ui| {
                    for n in game::MIN_SIZE..=game::MAX_SIZE {
                        ui.selectable_value(&mut size, n, format!("{n}x{n}"));
                    }
                });
                self.choose(mode, size);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (v, label) in [(View::Settings, "Settings"), (View::Stats, "Stats")] {
                        let on = self.view == v;
                        if ui.selectable_label(on, label).clicked() {
                            self.view = if on { View::Play } else { v };
                        }
                    }
                });
            });
            ui.add_space(8.0);
            match self.view {
                View::Play => self.page_play(ui, &pal),
                View::Stats => self.page_stats(ui, &pal),
                View::Settings => self.page_settings(ui, &pal),
            }
        });
        chrome::outline(&ctx, &pal);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 620.0])
            .with_min_inner_size([340.0, 460.0])
            .with_title("2048")
            .with_app_id("yggdrasil-2048")
            // draws its own title bar like the other Yggdrasil apps (yggdrasil-ui chrome)
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native("2048", options, Box::new(|_| Ok(Box::new(App::new()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_date_is_worked_out_without_a_calendar_library() {
        assert_eq!(today().len(), 10);
        assert!(today().starts_with("20"));
    }

    #[test]
    fn colours_get_stronger_up_the_scale() {
        let pal = theme::palette_for(true, None);
        let (a, _) = tile_colors(1, Colors::Accent, &pal);
        let (b, fg) = tile_colors(11, Colors::Accent, &pal);
        assert_ne!(a, b);
        assert_eq!(fg, Color32::WHITE);
        assert_eq!(tile_colors(11, Colors::Original, &pal).0, hex(0xedc22e));
        assert_eq!(tile_colors(14, Colors::Original, &pal).0, hex(0x3c3a32));
    }
}
