//! Clipboard history and snippets in a small Yggdrasil window (Super+V). A rewrite in Rust of the ideas in jwkicklighter's
//! omarchy-snipboard (github.com/jwkicklighter/omarchy-snipboard, MIT): a search line over the list, Enter puts the pick on the
//! clipboard, and snippets are texts kept on purpose.
//!
//!   yggdrasil-clipboard          the window (run again while it is open: it closes)
//!   yggdrasil-clipboard daemon   the history keeper (a systemd user service): in memory only, while Settings > Appearance >
//!                                "Clipboard history" is on

mod daemon;
mod snippets;
mod store;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, vec2, Align, CornerRadius, FontId, Key, Layout, Margin, Modifiers, RichText, Sense};
use snippets::Snippet;
use store::Entry;
use yggdrasil_ui::chrome;
use yggdrasil_ui::theme::{self, Palette};

/// One request to the daemon and its answer; None when it is not running.
fn ask(req: serde_json::Value) -> Option<String> {
    let mut s = UnixStream::connect(daemon::socket_path()).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    writeln!(s, "{req}").ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    Some(line)
}

fn ok(answer: Option<String>) -> bool {
    answer.and_then(|l| serde_json::from_str::<serde_json::Value>(&l).ok()).map(|v| v["ok"] == true).unwrap_or(false)
}

/// Every word typed is in the text, in any order, ignoring case.
fn matches(hay: &str, q: &str) -> bool {
    let hay = hay.to_lowercase();
    q.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
fn age(at: u64, now: u64) -> String {
    let d = now.saturating_sub(at);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => format!("{} h ago", d / 3600),
        _ => format!("{} day{} ago", d / 86_400, if d < 2 * 86_400 { "" } else { "s" }),
    }
}

/// The first non-blank line, with how much more there is.
fn summary(text: &str) -> (String, String) {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    let lines = text.lines().count();
    let chars = text.chars().count();
    (first, if lines > 1 { format!("{lines} lines · {chars} characters") } else { format!("{chars} characters") })
}

fn now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Pasting is offered only where the compositor itself can send the key press (Hyprland's sendshortcut); nothing types into
/// other windows through a fake keyboard.
fn can_paste() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// After this window has closed and focus is back, asks Hyprland to press Ctrl+V (Ctrl+Shift+V in a terminal) in the window.
fn paste_after_close() {
    let script = r#"sleep 0.25
if hyprctl -j activewindow 2>/dev/null | grep -qiE '"class": *"[^"]*(kitty|foot|alacritty|ghostty|wezterm|konsole|terminal|minos)'; then m="CTRL SHIFT"; else m="CTRL"; fi
hyprctl dispatch sendshortcut "$m, V, activewindow" >/dev/null 2>&1"#;
    let _ = std::process::Command::new("sh").args(["-c", script]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).spawn();
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    History,
    Snippets,
}

/// The snippet being written: a new one (`None`) or the one at that index.
struct Editor {
    index: Option<usize>,
    title: String,
    text: String,
}

struct App {
    tab: Tab,
    query: String,
    sel: usize,
    history: Vec<Entry>,
    /// None: the daemon does not answer; Some(false): history is off in Settings.
    history_on: Option<bool>,
    listed: Instant,
    snippets: Vec<Snippet>,
    editor: Option<Editor>,
    note: String,
    close: bool,
}

impl App {
    fn new() -> App {
        let mut snippets = snippets::load();
        snippets::sort(&mut snippets);
        let mut a = App { tab: Tab::History, query: String::new(), sel: 0, history: vec![], history_on: None, listed: Instant::now(), snippets, editor: None, note: String::new(), close: false };
        a.refresh();
        a
    }

    fn refresh(&mut self) {
        self.listed = Instant::now();
        match ask(serde_json::json!({"op": "list"})).and_then(|l| store::parse_list(&l)) {
            Some((on, items)) => {
                self.history_on = Some(on);
                self.history = items;
            }
            None => {
                self.history_on = None;
                self.history.clear();
            }
        }
    }

    fn save_snippets(&mut self) {
        snippets::sort(&mut self.snippets);
        if let Err(e) = snippets::save(&self.snippets) {
            self.note = format!("Snippets could not be saved: {e}");
        }
    }

    /// What is shown on the current tab: indices into history or snippets.
    fn shown(&self) -> Vec<usize> {
        match self.tab {
            Tab::History => (0..self.history.len()).filter(|&i| {
                let e = &self.history[i];
                matches(if e.image.is_some() { "image picture" } else { &e.text }, &self.query)
            }).collect(),
            Tab::Snippets => (0..self.snippets.len()).filter(|&i| matches(&format!("{} {}", self.snippets[i].title, self.snippets[i].text), &self.query)).collect(),
        }
    }

    /// Puts the pick on the clipboard and closes; `paste` then presses Ctrl+V in the window below.
    fn pick(&mut self, i: usize, paste: bool) {
        let done = match self.tab {
            Tab::History => self.history.get(i).map(|e| ok(ask(serde_json::json!({"op": "copy", "id": e.id})))).unwrap_or(false),
            Tab::Snippets => {
                let Some(s) = self.snippets.get_mut(i) else { return };
                s.used += 1;
                let ok = daemon::wl_copy(s.text.as_bytes(), None);
                self.save_snippets();
                ok
            }
        };
        if done {
            if paste && can_paste() {
                paste_after_close();
            }
            self.close = true;
        } else {
            self.note = "Could not put it on the clipboard (is wl-clipboard installed?).".into();
        }
    }

    fn delete(&mut self, i: usize) {
        match self.tab {
            Tab::History => {
                if let Some(e) = self.history.get(i) {
                    ask(serde_json::json!({"op": "delete", "id": e.id}));
                }
                self.refresh();
            }
            Tab::Snippets => {
                if i < self.snippets.len() {
                    self.snippets.remove(i);
                    self.save_snippets();
                }
            }
        }
    }

    /// Ctrl+P: a copied text becomes a pinned snippet; a snippet is pinned or unpinned.
    fn pin(&mut self, i: usize) {
        match self.tab {
            Tab::History => {
                let Some(e) = self.history.get(i).filter(|e| e.image.is_none()) else { return };
                if !self.snippets.iter().any(|s| s.text == e.text) {
                    self.snippets.push(Snippet { title: snippets::title_from(&e.text), text: e.text.clone(), pinned: true, used: 0 });
                    self.save_snippets();
                    self.note = "Kept as a pinned snippet.".into();
                } else {
                    self.note = "Already a snippet.".into();
                }
            }
            Tab::Snippets => {
                if let Some(s) = self.snippets.get_mut(i) {
                    s.pinned = !s.pinned;
                }
                self.save_snippets();
            }
        }
    }

    /// Ctrl+E: edit a snippet, or start a new one from a copied text.
    fn edit(&mut self, i: Option<usize>) {
        self.editor = Some(match (self.tab, i) {
            (Tab::Snippets, Some(i)) if i < self.snippets.len() => Editor { index: Some(i), title: self.snippets[i].title.clone(), text: self.snippets[i].text.clone() },
            (Tab::History, Some(i)) => {
                let t = self.history.get(i).map(|e| e.text.clone()).unwrap_or_default();
                Editor { index: None, title: snippets::title_from(&t), text: t }
            }
            _ => Editor { index: None, title: String::new(), text: String::new() },
        });
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let ctx = ui.ctx().clone();
        let Some(ed) = &mut self.editor else { return };
        ui.label(RichText::new(if ed.index.is_some() { "Edit snippet" } else { "New snippet" }).color(pal.dim));
        ui.add_space(4.0);
        let t = ui.add(egui::TextEdit::singleline(&mut ed.title).hint_text("Title").desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)));
        if ed.title.is_empty() && ed.text.is_empty() && !t.has_focus() && ui.memory(|m| m.focused().is_none()) {
            t.request_focus();
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(ui.available_height() - 44.0).show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(&mut ed.text).hint_text("Text").desired_width(f32::INFINITY).desired_rows(12).font(egui::TextStyle::Monospace));
        });
        ui.add_space(6.0);
        let mut save = ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::S));
        let mut cancel = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        ui.horizontal(|ui| {
            save |= ui.button("Save (Ctrl+S)").clicked();
            cancel |= ui.button("Cancel (Esc)").clicked();
        });
        if save && !ed.text.trim().is_empty() {
            let title = if ed.title.trim().is_empty() { snippets::title_from(&ed.text) } else { ed.title.trim().to_string() };
            match ed.index {
                Some(i) if i < self.snippets.len() => {
                    self.snippets[i].title = title;
                    self.snippets[i].text = ed.text.clone();
                }
                _ => self.snippets.push(Snippet { title, text: ed.text.clone(), pinned: false, used: 0 }),
            }
            self.editor = None;
            self.tab = Tab::Snippets;
            self.save_snippets();
        } else if cancel {
            self.editor = None;
        }
    }
}

fn row(ui: &mut egui::Ui, pal: &Palette, title: &str, meta: &str, pinned: bool, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::click());
    let p = ui.painter();
    if selected || resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(10), if selected { pal.select } else { pal.hover });
    }
    let clip = rect.shrink2(vec2(12.0, 0.0));
    let p = p.with_clip_rect(clip);
    let x = if pinned {
        p.text(rect.left_top() + vec2(14.0, 9.0), egui::Align2::LEFT_TOP, "★", FontId::proportional(13.5), pal.accent);
        30.0
    } else {
        14.0
    };
    p.text(rect.left_top() + vec2(x, 9.0), egui::Align2::LEFT_TOP, title, FontId::proportional(13.5), pal.text);
    p.text(rect.left_top() + vec2(14.0, 29.0), egui::Align2::LEFT_TOP, meta, FontId::proportional(11.0), pal.dim);
    resp
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        theme::apply(&ctx, true, accent);
        let pal = theme::palette_for(true, accent);
        ctx.request_repaint_after(Duration::from_secs(1));
        if self.close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        // what is copied while the window is open shows up
        if self.editor.is_none() && self.listed.elapsed() > Duration::from_secs(1) {
            self.refresh();
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 12, right: 10, top: 6, bottom: 12 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Clipboard").strong().color(pal.text));
                ui.add_space(10.0);
                for (t, name) in [(Tab::History, "History"), (Tab::Snippets, "Snippets")] {
                    let txt = RichText::new(name).color(if self.tab == t { pal.text } else { pal.dim });
                    if ui.add(egui::Button::new(txt).selected(self.tab == t).frame(self.tab == t)).clicked() && self.editor.is_none() {
                        self.tab = t;
                        self.sel = 0;
                    }
                }
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });
            ui.add_space(6.0);
            if self.editor.is_some() {
                self.editor_ui(ui, &pal);
                return;
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
                self.tab = if self.tab == Tab::History { Tab::Snippets } else { Tab::History };
                self.sel = 0;
            }
            let resp = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(if self.tab == Tab::History { "Search what you copied" } else { "Search snippets" }).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)));
            if !resp.has_focus() {
                resp.request_focus();
            }
            if resp.changed() {
                self.sel = 0;
            }
            let shown = self.shown();
            self.sel = self.sel.min(shown.len().saturating_sub(1));
            let (down, up, enter, shift_enter, del, pin, edit, new) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    i.consume_key(Modifiers::SHIFT, Key::Enter),
                    // Delete with an empty search line removes the entry; while typing it edits the search
                    self.query.is_empty() && i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::CTRL, Key::Delete),
                    i.consume_key(Modifiers::CTRL, Key::P),
                    i.consume_key(Modifiers::CTRL, Key::E),
                    i.consume_key(Modifiers::CTRL, Key::N),
                )
            });
            if down && self.sel + 1 < shown.len() {
                self.sel += 1;
            }
            if up {
                self.sel = self.sel.saturating_sub(1);
            }
            let cur = shown.get(self.sel).copied();
            let mut picked: Option<(usize, bool)> = None;
            if let Some(i) = cur {
                if enter || shift_enter {
                    picked = Some((i, shift_enter));
                }
                if del {
                    self.delete(i);
                }
                if pin {
                    self.pin(i);
                }
            }
            if edit {
                self.edit(cur);
            }
            if new {
                self.tab = Tab::Snippets;
                self.edit(None);
            }
            ui.add_space(8.0);
            let footer_h = 28.0;
            let t = now();
            egui::ScrollArea::vertical().max_height(ui.available_height() - footer_h).auto_shrink([false, false]).show(ui, |ui| {
                if shown.is_empty() {
                    ui.add_space(20.0);
                    let msg = match (self.tab, self.history_on) {
                        (Tab::History, None) => "The clipboard service is not running (systemctl --user enable --now yggdrasil-clipboard).",
                        (Tab::History, Some(false)) => "Clipboard history is off. Turn it on in Settings > Appearance.",
                        (Tab::History, _) if self.history.is_empty() => "Nothing copied yet.",
                        (Tab::Snippets, _) if self.snippets.is_empty() => "No snippets yet. Ctrl+N writes one; Ctrl+P keeps a copied text as one.",
                        _ => "Nothing matches.",
                    };
                    ui.vertical_centered(|ui| ui.label(RichText::new(msg).color(pal.dim)));
                }
                for (n, &i) in shown.iter().enumerate() {
                    let r = match self.tab {
                        Tab::History => {
                            let e = &self.history[i];
                            let (title, meta) = match e.image {
                                Some((w, h)) => (format!("Picture {w} × {h}"), "image".to_string()),
                                None => summary(&e.text),
                            };
                            row(ui, &pal, &title, &format!("{meta} · {}", age(e.at, t)), false, n == self.sel)
                        }
                        Tab::Snippets => {
                            let s = &self.snippets[i];
                            let (first, meta) = summary(&s.text);
                            row(ui, &pal, if s.title.is_empty() { &first } else { &s.title }, &meta, s.pinned, n == self.sel)
                        }
                    };
                    if n == self.sel && (down || up) {
                        r.scroll_to_me(None);
                    }
                    if r.clicked() {
                        picked = Some((i, false));
                    }
                }
            });
            if let Some((i, paste)) = picked {
                self.pick(i, paste);
            }
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                ui.horizontal(|ui| {
                    let hint = if !self.note.is_empty() {
                        RichText::new(self.note.as_str()).color(pal.amber)
                    } else {
                        let paste = if can_paste() { " · Shift+Enter pastes" } else { "" };
                        RichText::new(format!("Enter copies{paste} · Tab switches · Ctrl+P pin · Ctrl+E edit · Ctrl+N new · Del removes")).color(pal.dim).size(11.0)
                    };
                    ui.label(hint);
                    if self.tab == Tab::History && !self.history.is_empty() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Clear").clicked() {
                                ask(serde_json::json!({"op": "clear"}));
                                self.refresh();
                            }
                        });
                    }
                });
            });
        });
        chrome::outline(&ctx, &pal);
    }
}

fn main() -> std::process::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("daemon") {
        return daemon::run();
    }
    if yggdrasil_ui::single::toggle("yggdrasil-clipboard") {
        return std::process::ExitCode::SUCCESS;
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 560.0])
            .with_min_inner_size([360.0, 340.0])
            .with_title("Clipboard")
            .with_app_id("yggdrasil-clipboard")
            .with_decorations(false),
        ..Default::default()
    };
    match eframe::run_native("Clipboard", options, Box::new(|_| Ok(Box::new(App::new())))) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yggdrasil-clipboard: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_ages_and_summaries() {
        assert!(matches("Hello World from Rust", "rust hello"));
        assert!(!matches("Hello", "hello there"));
        assert!(matches("anything", ""));
        assert_eq!(age(100, 130), "just now");
        assert_eq!(age(0, 300), "5 min ago");
        assert_eq!(age(0, 7200), "2 h ago");
        assert_eq!(age(0, 86_400), "1 day ago");
        assert_eq!(age(0, 3 * 86_400), "3 days ago");
        assert_eq!(summary("\n first \nsecond"), ("first".to_string(), "3 lines · 15 characters".to_string()));
        assert_eq!(summary("one").1, "3 characters");
    }
}
