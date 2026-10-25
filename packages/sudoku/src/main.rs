//! Sudoku: Classic (Easy, Medium, Hard, with best times) and Survival (three lives, a wrong digit costs one, a cleared board gives
//! two more and the next difficulty, up to Extreme). Pencil notes, undo and redo, hints, a pause that hides the board. The game and
//! the stats are saved in the data folder; nothing goes over the network.

mod engine;
mod game;

use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Key, Margin, Rect, RichText, Sense, Stroke};
use engine::{Difficulty, Rng};
use game::{Game, Mode, Outcome, Stats};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use yggdrasil_ui::chrome;
use yggdrasil_ui::theme::{self, Palette};

fn data_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("yggdrasil").join("sudoku"))
}

fn load<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(data_dir()?.join(name)).ok()?).ok()
}

/// Written to a temporary file first, then moved over the old one: a crash never leaves half a file.
fn store<T: serde::Serialize>(name: &str, v: &T) {
    let Some(dir) = data_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(".{name}.tmp"));
    if let Ok(text) = serde_json::to_string(v) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join(name));
        }
    }
}

fn forget(name: &str) {
    if let Some(dir) = data_dir() {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// A puzzle being made on its own thread (Extreme can take a moment), and what it is for.
struct Making {
    rx: Receiver<(engine::Grid, engine::Grid)>,
    mode: Mode,
    difficulty: Difficulty,
    /// The survival run it continues.
    after: Option<Game>,
}

#[derive(PartialEq)]
enum End {
    None,
    /// Classic solved: the time, and whether it was a best.
    Solved { best: bool },
    /// Survival: this board is cleared, the next one is ready to make.
    Cleared,
    OutOfLives { best: bool },
}

struct App {
    game: Option<Game>,
    stats: Stats,
    sel: usize,
    notes_mode: bool,
    /// Brush: a digit picked on the numpad (right-click) that every click on a cell puts down.
    brush: Option<u8>,
    paused: bool,
    menu: bool,
    making: Option<Making>,
    end: End,
    /// A short line under the board (a life lost, a hint used).
    flash: Option<(String, Instant)>,
    tick: Instant,
    saved: Instant,
    dirty: bool,
}

impl App {
    fn new() -> App {
        let game: Option<Game> = load::<Game>("save.json").filter(|g| g.valid() && !g.solved() && !g.out_of_lives());
        let menu = game.is_none();
        App {
            game,
            stats: load("stats.json").unwrap_or_default(),
            sel: 40,
            notes_mode: false,
            brush: None,
            paused: false,
            menu,
            making: None,
            end: End::None,
            flash: None,
            tick: Instant::now(),
            saved: Instant::now(),
            dirty: false,
        }
    }

    fn start(&mut self, mode: Mode, difficulty: Difficulty, after: Option<Game>) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(engine::generate(difficulty, &mut Rng::from_time()));
        });
        self.making = Some(Making { rx, mode, difficulty, after });
        self.menu = false;
        self.end = End::None;
        self.paused = false;
        self.brush = None;
    }

    fn poll_making(&mut self) {
        let Some(m) = &self.making else { return };
        let Ok((p, s)) = m.rx.try_recv() else { return };
        let m = self.making.take().unwrap();
        let g = match m.after {
            Some(prev) => prev.next_survival(p, s),
            None => {
                self.stats.started += 1;
                store("stats.json", &self.stats);
                Game::new(m.mode, m.difficulty, p, s)
            }
        };
        self.sel = (0..81).find(|&i| g.puzzle[i] == 0).unwrap_or(40);
        self.game = Some(g);
        self.dirty = true;
    }

    fn save(&mut self) {
        if let Some(g) = &self.game {
            if g.solved() || g.out_of_lives() {
                forget("save.json");
            } else {
                store("save.json", g);
            }
        }
        self.dirty = false;
        self.saved = Instant::now();
    }

    fn say(&mut self, s: impl Into<String>) {
        self.flash = Some((s.into(), Instant::now()));
    }

    /// What a move did: lives, the end of the board.
    fn after(&mut self, o: Outcome) {
        self.dirty = true;
        let Some(g) = &self.game else { return };
        match o {
            Outcome::LifeLost => self.say(format!("Wrong digit: {} {} left", g.lives, if g.lives == 1 { "life" } else { "lives" })),
            Outcome::OutOfLives => {
                let cleared = g.cleared;
                let best = self.stats.record_survival(cleared);
                store("stats.json", &self.stats);
                self.end = End::OutOfLives { best };
            }
            Outcome::Solved => match g.mode {
                Mode::Classic => {
                    let (d, secs, hints) = (g.difficulty, g.elapsed_ms / 1000, g.hints);
                    let best = self.stats.record_solve(d, secs, hints);
                    store("stats.json", &self.stats);
                    self.end = End::Solved { best };
                }
                Mode::Survival => self.end = End::Cleared,
            },
            _ => {}
        }
        if self.end != End::None {
            self.brush = None;
            self.save();
        }
    }

    fn put(&mut self, i: usize, d: u8) {
        let Some(g) = &mut self.game else { return };
        if self.notes_mode {
            g.toggle_note(i, d);
            self.dirty = true;
        } else {
            let o = g.place(i, d);
            self.after(o);
        }
    }

    fn erase(&mut self, i: usize) {
        if let Some(g) = &mut self.game {
            if g.erase(i) {
                self.dirty = true;
            }
        }
    }

    fn hint(&mut self) {
        let sel = self.sel;
        let Some(g) = &mut self.game else { return };
        if let Some((i, o)) = g.hint(Some(sel)) {
            self.sel = i;
            self.say("Hint used: this game's time will not count as a best");
            self.after(o);
        }
    }

    fn playing(&self) -> bool {
        self.game.is_some() && !self.menu && self.making.is_none() && self.end == End::None
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if !self.playing() {
            return;
        }
        let (mods, pressed): (egui::Modifiers, Vec<Key>) = ctx.input(|i| {
            (
                i.modifiers,
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Key { key, pressed: true, .. } => Some(*key),
                        _ => None,
                    })
                    .collect(),
            )
        });
        for key in pressed {
            if matches!(key, Key::P | Key::Space) {
                self.paused = !self.paused;
                continue;
            }
            if self.paused {
                continue;
            }
            let (r, c) = (self.sel / 9, self.sel % 9);
            let digit = match key {
                Key::Num1 => Some(1),
                Key::Num2 => Some(2),
                Key::Num3 => Some(3),
                Key::Num4 => Some(4),
                Key::Num5 => Some(5),
                Key::Num6 => Some(6),
                Key::Num7 => Some(7),
                Key::Num8 => Some(8),
                Key::Num9 => Some(9),
                _ => None,
            };
            if let Some(d) = digit {
                self.put(self.sel, d);
                continue;
            }
            match key {
                Key::ArrowLeft | Key::H if !(key == Key::H && mods.shift) => self.sel = r * 9 + (c + 8) % 9,
                Key::H => self.hint(),
                Key::ArrowRight | Key::L => self.sel = r * 9 + (c + 1) % 9,
                Key::ArrowUp | Key::K => self.sel = (r + 8) % 9 * 9 + c,
                Key::ArrowDown | Key::J => self.sel = (r + 1) % 9 * 9 + c,
                Key::Backspace | Key::Delete | Key::Num0 => self.erase(self.sel),
                Key::N => self.notes_mode = !self.notes_mode,
                Key::Z if mods.command && mods.shift => self.redo(),
                Key::Z if mods.command => self.undo(),
                Key::U => self.undo(),
                Key::Y if mods.command => self.redo(),
                Key::Escape => self.brush = None,
                _ => {}
            }
        }
    }

    fn undo(&mut self) {
        if let Some(g) = &mut self.game {
            g.undo();
            self.dirty = true;
        }
    }

    fn redo(&mut self) {
        if let Some(g) = &mut self.game {
            g.redo();
            self.dirty = true;
        }
    }
}

fn button(ui: &mut egui::Ui, label: &str, on: bool, enabled: bool) -> egui::Response {
    ui.add_enabled(enabled, egui::Button::new(RichText::new(label).size(13.0)).selected(on).min_size(vec2(0.0, 30.0)))
}

/// A small filled heart (lives).
fn heart(p: &egui::Painter, c: egui::Pos2, r: f32, color: Color32) {
    p.circle_filled(pos2(c.x - r * 0.5, c.y - r * 0.2), r * 0.55, color);
    p.circle_filled(pos2(c.x + r * 0.5, c.y - r * 0.2), r * 0.55, color);
    p.add(egui::Shape::convex_polygon(vec![pos2(c.x - r * 1.02, c.y - r * 0.05), pos2(c.x + r * 1.02, c.y - r * 0.05), pos2(c.x, c.y + r)], color, Stroke::NONE));
}

fn check(p: &egui::Painter, c: egui::Pos2, s: f32, color: Color32) {
    let st = Stroke::new(2.0, color);
    p.line_segment([pos2(c.x - s * 0.5, c.y), pos2(c.x - s * 0.1, c.y + s * 0.4)], st);
    p.line_segment([pos2(c.x - s * 0.1, c.y + s * 0.4), pos2(c.x + s * 0.6, c.y - s * 0.4)], st);
}

impl App {
    fn header(&self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(g) = &self.game else { return };
        ui.horizontal(|ui| {
            let what = match g.mode {
                Mode::Classic => format!("Classic · {}", g.difficulty.label()),
                Mode::Survival => format!("Survival · {} · board {}", g.difficulty.label(), g.cleared + 1),
            };
            ui.label(RichText::new(what).color(pal.dim));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(clock(g.elapsed_ms)).monospace().size(15.0).color(pal.text));
                if g.mode == Mode::Survival {
                    let n = g.lives.min(12);
                    let (r, _) = ui.allocate_exact_size(vec2(n as f32 * 18.0 + 8.0, 20.0), Sense::hover());
                    for k in 0..n {
                        heart(ui.painter(), pos2(r.left() + 9.0 + k as f32 * 18.0, r.center().y), 7.0, pal.red);
                    }
                    if g.lives > 12 {
                        ui.label(RichText::new(format!("{}", g.lives)).color(pal.red));
                    }
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let (can_undo, can_redo) = self.game.as_ref().map(|g| (g.can_undo(), g.can_redo())).unwrap_or((false, false));
        let live = self.end == End::None && !self.paused;
        ui.horizontal_wrapped(|ui| {
            if button(ui, "Undo", false, live && can_undo).on_hover_text("U or Ctrl+Z").clicked() {
                self.undo();
            }
            if button(ui, "Redo", false, live && can_redo).on_hover_text("Ctrl+Y or Ctrl+Shift+Z").clicked() {
                self.redo();
            }
            if button(ui, "Erase", false, live).on_hover_text("Backspace, Delete or 0; or right-click a cell").clicked() {
                self.erase(self.sel);
            }
            if button(ui, if self.notes_mode { "Notes: on" } else { "Notes: off" }, self.notes_mode, live).on_hover_text("N").clicked() {
                self.notes_mode = !self.notes_mode;
            }
            if button(ui, "Hint", false, live).on_hover_text("Shift+H").clicked() {
                self.hint();
            }
            if button(ui, if self.paused { "Resume" } else { "Pause" }, self.paused, self.end == End::None).on_hover_text("P or Space").clicked() {
                self.paused = !self.paused;
            }
            if button(ui, "Menu", false, true).clicked() {
                self.save();
                self.menu = true;
                self.paused = false;
            }
        });
    }

    fn board(&mut self, ui: &mut egui::Ui, pal: &Palette, size: f32) {
        let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
        let p = ui.painter_at(rect.expand(2.0));
        p.rect_filled(rect, CornerRadius::same(8), pal.field);
        let cell = size / 9.0;
        let at = |i: usize| Rect::from_min_size(pos2(rect.left() + (i % 9) as f32 * cell, rect.top() + (i / 9) as f32 * cell), vec2(cell, cell));
        if self.paused {
            p.text(rect.center(), Align2::CENTER_CENTER, "Paused", FontId::proportional(26.0), pal.text);
            p.text(rect.center() + vec2(0.0, 30.0), Align2::CENTER_CENTER, "P, Space or a click to go on", FontId::proportional(13.0), pal.dim);
            if resp.clicked() {
                self.paused = false;
            }
            return;
        }
        if let Some(pos) = resp.interact_pointer_pos() {
            let (c, r) = (((pos.x - rect.left()) / cell) as usize, ((pos.y - rect.top()) / cell) as usize);
            if r < 9 && c < 9 {
                let i = r * 9 + c;
                if resp.secondary_clicked() {
                    self.sel = i;
                    self.erase(i);
                } else if resp.clicked() {
                    self.sel = i;
                    if let Some(d) = self.brush {
                        if self.end == End::None {
                            self.put(i, d);
                        }
                    }
                }
            }
        }
        let Some(g) = &self.game else { return };
        let sel = self.sel;
        let (sr, sc) = (sel / 9, sel % 9);
        let same = if g.values[sel] != 0 { g.values[sel] } else { self.brush.unwrap_or(0) };
        let digit_tint = Color32::from_rgba_unmultiplied(pal.accent.r(), pal.accent.g(), pal.accent.b(), 60);
        for i in 0..81 {
            let (r, c) = (i / 9, i % 9);
            let rc = at(i);
            if i == sel {
                p.rect_filled(rc, CornerRadius::ZERO, pal.select);
            } else if same != 0 && g.values[i] == same {
                p.rect_filled(rc, CornerRadius::ZERO, digit_tint);
            } else if r == sr || c == sc || (r / 3 == sr / 3 && c / 3 == sc / 3) {
                p.rect_filled(rc, CornerRadius::ZERO, pal.hover);
            }
            let v = g.values[i];
            if v != 0 {
                let color = if g.conflict(i) { pal.red } else if g.given(i) { pal.text } else { pal.accent };
                let font = if g.given(i) { FontId::proportional(cell * 0.58) } else { FontId::proportional(cell * 0.54) };
                p.text(rc.center(), Align2::CENTER_CENTER, v.to_string(), font, color);
            } else if g.notes[i] != 0 {
                for d in 1..=9u8 {
                    if g.notes[i] & (1 << d) != 0 {
                        let k = (d - 1) as f32;
                        let np = pos2(rc.left() + cell * (0.2 + 0.3 * (k % 3.0)), rc.top() + cell * (0.2 + 0.3 * (k / 3.0).floor()));
                        let col = if same == d { pal.accent } else { pal.dim };
                        p.text(np, Align2::CENTER_CENTER, d.to_string(), FontId::proportional(cell * 0.22), col);
                    }
                }
            }
        }
        for k in 1..9 {
            let thick = k % 3 == 0;
            let st = if thick { Stroke::new(2.0, pal.dim) } else { Stroke::new(1.0, pal.border) };
            let o = k as f32 * cell;
            p.line_segment([pos2(rect.left() + o, rect.top()), pos2(rect.left() + o, rect.bottom())], st);
            p.line_segment([pos2(rect.left(), rect.top() + o), pos2(rect.right(), rect.top() + o)], st);
        }
        p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(2.0, pal.dim), egui::StrokeKind::Inside);
    }

    fn numpad(&mut self, ui: &mut egui::Ui, pal: &Palette, width: f32) {
        let gap = 6.0;
        let w = (width - gap * 8.0) / 9.0;
        let h = 52.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for d in 1..=9u8 {
                let left = self.game.as_ref().map(|g| g.remaining(d)).unwrap_or(9);
                let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
                let on = self.brush == Some(d);
                let fill = if on { pal.accent } else if resp.hovered() { pal.hover } else { pal.field };
                ui.painter().rect_filled(r, CornerRadius::same(8), fill);
                let ink = if on { pal.on_accent() } else if left == 0 { pal.dim } else { pal.text };
                ui.painter().text(pos2(r.center().x, r.top() + h * 0.4), Align2::CENTER_CENTER, d.to_string(), FontId::proportional(22.0), ink);
                if left == 0 {
                    check(ui.painter(), pos2(r.center().x, r.bottom() - 12.0), 8.0, if on { pal.on_accent() } else { pal.green });
                } else {
                    ui.painter().text(pos2(r.center().x, r.bottom() - 11.0), Align2::CENTER_CENTER, left.to_string(), FontId::proportional(11.0), if on { pal.on_accent() } else { pal.dim });
                }
                let resp = resp.on_hover_text("Click: put it in the selected cell. Right-click: brush (every cell you click gets it)");
                if !self.playing() || self.paused {
                    continue;
                }
                if resp.secondary_clicked() {
                    self.brush = if on { None } else { Some(d) };
                } else if resp.clicked() {
                    if on {
                        self.brush = None;
                    } else {
                        self.put(self.sel, d);
                    }
                }
            }
        });
    }

    fn end_card(&mut self, ui: &mut egui::Ui, pal: &Palette, over: Rect) {
        let Some(g) = self.game.clone() else { return };
        let (title, line) = match self.end {
            End::None => return,
            End::Solved { best } => (
                format!("Solved in {}", clock(g.elapsed_ms)),
                if best { "A new best time for this difficulty".to_string() } else if g.hints > 0 { format!("With {} hint(s)", g.hints) } else { String::new() },
            ),
            End::Cleared => ("Board cleared".to_string(), format!("+{} lives · next: {}", game::BONUS_LIVES, g.difficulty.next().label())),
            End::OutOfLives { best } => (
                "Out of lives".to_string(),
                format!("Boards cleared: {}{}", g.cleared, if best { " · a new best" } else { "" }),
            ),
        };
        let card = Rect::from_center_size(over.center(), vec2(over.width().min(320.0), 150.0));
        ui.painter().rect_filled(card, CornerRadius::same(12), pal.sidebar);
        ui.painter().rect_stroke(card, CornerRadius::same(12), Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
        ui.painter().text(card.center_top() + vec2(0.0, 32.0), Align2::CENTER_CENTER, title, FontId::proportional(20.0), pal.text);
        ui.painter().text(card.center_top() + vec2(0.0, 60.0), Align2::CENTER_CENTER, line, FontId::proportional(13.0), pal.dim);
        let row = Rect::from_center_size(card.center_bottom() - vec2(0.0, 32.0), vec2(card.width() - 32.0, 32.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            match self.end {
                End::Solved { .. } => {
                    if button(ui, &format!("New {} game", g.difficulty.label()), false, true).clicked() {
                        self.start(Mode::Classic, g.difficulty, None);
                    }
                }
                End::Cleared => {
                    if button(ui, &format!("Next: {}", g.difficulty.next().label()), false, true).clicked() {
                        self.start(Mode::Survival, g.difficulty.next(), Some(g.clone()));
                    }
                }
                End::OutOfLives { .. } => {
                    if button(ui, "Try again", false, true).clicked() {
                        self.start(Mode::Survival, Difficulty::Easy, None);
                    }
                }
                End::None => {}
            }
            if button(ui, "Menu", false, true).clicked() {
                self.menu = true;
                self.end = End::None;
                self.game = None;
            }
        });
    }

    fn menu_page(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Sudoku").size(30.0).strong().color(pal.text));
            ui.add_space(14.0);
            if self.game.is_some() && button(ui, "Continue", false, true).clicked() {
                self.menu = false;
            }
            ui.add_space(16.0);
            ui.label(RichText::new("CLASSIC").size(11.0).strong().color(pal.dim));
            ui.add_space(6.0);
            for d in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard] {
                let best = self.stats.best.get(&d).map(|s| format!("best {}", clock(s * 1000))).unwrap_or_else(|| "no best time yet".into());
                if ui.add(egui::Button::new(RichText::new(format!("{}   ·   {}", d.label(), best)).size(14.0)).min_size(vec2(260.0, 36.0))).clicked() {
                    self.start(Mode::Classic, d, None);
                }
            }
            ui.add_space(16.0);
            ui.label(RichText::new("SURVIVAL").size(11.0).strong().color(pal.dim));
            ui.add_space(6.0);
            let best = if self.stats.survival_best > 0 { format!("best: {} boards", self.stats.survival_best) } else { "3 lives, Easy up to Extreme".into() };
            if ui.add(egui::Button::new(RichText::new(format!("Survival challenge   ·   {best}")).size(14.0)).min_size(vec2(260.0, 36.0))).clicked() {
                self.start(Mode::Survival, Difficulty::Easy, None);
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new("A wrong digit costs a life; a cleared board gives two more and the next difficulty.")
                    .size(12.0)
                    .color(pal.dim),
            );
            ui.add_space(22.0);
            let solved: u32 = self.stats.solved.values().sum();
            ui.label(RichText::new(format!("{} started · {} solved", self.stats.started, solved)).size(12.0).color(pal.dim));
            ui.add_space(10.0);
            ui.label(
                RichText::new("Keys: arrows or hjkl move · 1-9 place · 0 or Backspace erase · N notes · U undo · Shift+H hint · P pause")
                    .size(11.0)
                    .color(pal.dim),
            );
        });
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

        self.poll_making();
        self.keys(&ctx);

        // the clock runs while a board is in play, not paused, and the window has the focus
        let now = Instant::now();
        let step = now.duration_since(self.tick).as_millis() as u64;
        self.tick = now;
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if self.playing() && !self.paused && focused {
            if let Some(g) = &mut self.game {
                g.elapsed_ms += step.min(2000);
            }
        }
        if self.dirty || (self.playing() && self.saved.elapsed() > Duration::from_secs(10)) {
            self.save();
        }
        if self.flash.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(4)) {
            self.flash = None;
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 14, right: 10, top: 6, bottom: 14 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Sudoku").strong().color(pal.text));
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });
            if let Some(m) = &self.making {
                ui.add_space(120.0);
                ui.vertical_centered(|ui| {
                    ui.add(egui::Spinner::new().size(28.0));
                    ui.add_space(10.0);
                    ui.label(RichText::new(format!("Making a {} puzzle…", m.difficulty.label())).color(pal.dim));
                });
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            if self.menu || self.game.is_none() {
                self.menu_page(ui, &pal);
                return;
            }
            self.header(ui, &pal);
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(8.0);
            let avail = ui.available_size();
            let size = avail.x.min(avail.y - 52.0 - 40.0).max(200.0);
            let left = (avail.x - size) / 2.0;
            let mut board_rect = Rect::NOTHING;
            ui.horizontal(|ui| {
                ui.add_space(left.max(0.0));
                ui.vertical(|ui| {
                    self.board(ui, &pal, size);
                    board_rect = ui.min_rect();
                    ui.add_space(10.0);
                    self.numpad(ui, &pal, size);
                    ui.add_space(6.0);
                    let note = match (&self.flash, self.brush) {
                        (Some((s, _)), _) => s.clone(),
                        (None, Some(d)) => format!("Brush: {d} (right-click it again or Escape to stop)"),
                        _ => String::new(),
                    };
                    ui.label(RichText::new(note).size(12.0).color(pal.dim));
                });
            });
            self.end_card(ui, &pal, board_rect);
        });
        chrome::outline(&ctx, &pal);
        if self.playing() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 720.0])
            .with_min_inner_size([420.0, 560.0])
            .with_title("Sudoku")
            .with_app_id("yggdrasil-sudoku")
            // draws its own title bar like the rest of Yggdrasil's apps (yggdrasil-ui chrome)
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native("Sudoku", options, Box::new(|_| Ok(Box::new(App::new()))))
}
