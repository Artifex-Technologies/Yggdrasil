//! Proton Authenticator's 2FA codes in a small Yggdrasil window: type a few letters, press Enter, and the code is on the clipboard
//! (marked sensitive, cleared after 20 seconds); the window then closes. Adapted from devSviat's Omarchy plugin
//! (github.com/devSviat/omarchy-proton-authenticator, MIT): everything that touches the vault is that plugin's own helper,
//! `pa-helper`, unchanged. It reads the official app's local vault read-only, takes the key from the keyring, never uses the
//! network, and answers one JSON line per command. This window only shows what it answers and asks it to copy.
//!
//!   yggdrasil-authenticator          the codes
//!   yggdrasil-authenticator --demo   the helper's demo accounts (public RFC test seeds), for screenshots
//!   yggdrasil-authenticator --bar    the top bar's card (search and codes, drawn by the panel; see bar.rs)
//!   yggdrasil-authenticator press ID [TEXT]   what the card's controls do

use std::process::Command;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, Align, CornerRadius, FontId, Layout, Margin, RichText, Sense};
use serde_json::Value;
use yggdrasil_ui::chrome;
use yggdrasil_ui::theme::{self, Palette};

mod bar;

const CLEAR_AFTER: u32 = 20;

fn helper() -> String {
    std::env::var("YGGDRASIL_PA_HELPER").unwrap_or_else(|_| "/usr/lib/yggdrasil-authenticator/pa-helper".into())
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    id: String,
    issuer: String,
    account: String,
    code: String,
    next: String,
    period: u64,
    remaining: u64,
}

/// One answer of the helper: its entries, or the error code it gave.
fn parse_codes(line: &str) -> Result<Vec<Entry>, String> {
    let v: Value = serde_json::from_str(line.trim()).map_err(|_| "INTERNAL".to_string())?;
    if v["ok"] != Value::Bool(true) {
        return Err(v["error"].as_str().unwrap_or("INTERNAL").to_string());
    }
    let s = |e: &Value, k: &str| e[k].as_str().unwrap_or("").to_string();
    Ok(v["entries"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| Entry {
                    id: s(e, "id"),
                    issuer: s(e, "issuer"),
                    account: s(e, "account"),
                    code: s(e, "code"),
                    next: s(e, "nextCode"),
                    period: e["period"].as_u64().unwrap_or(30).max(1),
                    remaining: e["remaining"].as_u64().unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// What an error code means for the person at the window, and what to do.
fn explain(code: &str) -> &'static str {
    match code {
        "VAULT_NOT_FOUND" => "Proton Authenticator has no codes on this computer yet. Open it and skip the import screen (that is for moving codes from another app). To bring the codes you already have in Proton, turn on sync in its settings and sign in with your Proton account; codes you add on this computer show up here too.",
        "KEYRING_LOCKED" => "The keyring is locked. Unlock it when asked (Try again asks for it).",
        "KEYRING_UNAVAILABLE" => "No keyring answers (gnome-keyring). Sign out and in again, or start it.",
        "KEY_NOT_FOUND" => "Proton Authenticator's key is not in the keyring. Open Proton Authenticator once so it stores it.",
        "APP_LOCKED" => "Proton Authenticator is locked with its own PIN or biometrics; unlock it there first.",
        "UNSAFE_KEY_STORAGE" => "Proton Authenticator keeps its key outside the keyring; the helper refuses to read it that way.",
        "VAULT_BUSY" => "Proton Authenticator is writing its vault right now. Try again in a moment.",
        "VAULT_FORMAT_CHANGED" => "Proton Authenticator's vault format changed (an app update). The helper needs an update too.",
        "DECRYPT_FAILED" => "The vault could not be opened with the stored key.",
        "DEPENDENCY_MISSING" => "A program the helper needs is missing (wl-clipboard, libsecret or glib2).",
        "MEMLOCK_FAILED" => "The locked-memory limit (ulimit -l) is too low for the helper; it needs 8 MiB, systemd's default.",
        "TIMEOUT" => "The helper took too long. Try again.",
        _ => "The helper could not read the codes.",
    }
}

fn run_helper(args: &[&str]) -> String {
    match Command::new(helper()).args(args).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).lines().last().unwrap_or("").to_string(),
        Err(_) => r#"{"ok":false,"error":"HELPER_MISSING"}"#.into(),
    }
}

/// "123456" as "123 456" (8 digits: "1234 5678").
fn spaced(code: &str) -> String {
    let n = code.chars().count();
    if n < 6 {
        return code.into();
    }
    let half = n / 2;
    format!("{} {}", code.chars().take(half).collect::<String>(), code.chars().skip(half).collect::<String>())
}

/// Entries whose issuer or account holds every word typed, in any order.
fn search<'a>(entries: &'a [Entry], q: &str) -> Vec<&'a Entry> {
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    entries.iter().filter(|e| {
        let hay = format!("{} {}", e.issuer, e.account).to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    }).collect()
}

enum State {
    Loading,
    Ready,
    /// Why, and whether opening the official app fixes it (not signed in yet, no key stored, locked).
    Failed(String, bool),
}

/// The official app, when it is installed.
const PROTON_APP: &str = "/usr/bin/proton-authenticator";

fn needs_the_app(code: &str) -> bool {
    matches!(code, "VAULT_NOT_FOUND" | "KEY_NOT_FOUND" | "APP_LOCKED") && std::path::Path::new(PROTON_APP).exists()
}

struct App {
    demo: bool,
    state: State,
    entries: Vec<Entry>,
    fetched: Instant,
    query: String,
    sel: usize,
    rx: Option<Receiver<(bool, String)>>,
    copied: Option<(Instant, String)>,
    note: String,
}

impl App {
    fn new() -> App {
        let mut a = App { demo: std::env::args().any(|x| x == "--demo"), state: State::Loading, entries: vec![], fetched: Instant::now(), query: String::new(), sel: 0, rx: None, copied: None, note: String::new() };
        a.fetch(false);
        a
    }

    /// Asks the helper for the codes on a thread; `prompt` lets the keyring ask to be unlocked.
    fn fetch(&mut self, prompt: bool) {
        let (tx, rx) = channel();
        let mut args = vec!["codes"];
        if prompt {
            args.push("--allow-prompt");
        }
        if self.demo {
            args.push("--demo");
        }
        let args: Vec<String> = args.into_iter().map(String::from).collect();
        std::thread::spawn(move || {
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            let _ = tx.send((true, run_helper(&a)));
        });
        self.rx = Some(rx);
    }

    fn copy(&mut self, e: &Entry) {
        let secs = CLEAR_AFTER.to_string();
        let mut args = vec!["copy", e.id.as_str(), "--clear-after", secs.as_str()];
        if self.demo {
            args.push("--demo");
        }
        let line = run_helper(&args);
        let ok = serde_json::from_str::<Value>(&line).map(|v| v["ok"] == Value::Bool(true)).unwrap_or(false);
        if ok {
            self.copied = Some((Instant::now(), format!("{} copied; the clipboard clears in {CLEAR_AFTER} s", if e.issuer.is_empty() { &e.account } else { &e.issuer })));
        } else {
            let code = serde_json::from_str::<Value>(&line).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or_else(|| "INTERNAL".into());
            self.note = explain(&code).into();
        }
    }

    fn receive(&mut self) {
        let Some(rx) = &self.rx else { return };
        if let Ok((_, line)) = rx.try_recv() {
            self.rx = None;
            match parse_codes(&line) {
                Ok(es) => {
                    self.entries = es;
                    self.fetched = Instant::now();
                    self.state = State::Ready;
                }
                Err(code) if code == "KEYRING_LOCKED" && matches!(self.state, State::Loading) => self.fetch(true),
                Err(code) if code == "HELPER_MISSING" => self.state = State::Failed("The helper (pa-helper) is not installed; reinstall the plugin from Agora.".into(), false),
                Err(code) => self.state = State::Failed(explain(&code).into(), needs_the_app(&code)),
            }
        }
    }
}

fn row(ui: &mut egui::Ui, pal: &Palette, e: &Entry, left: u64, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click());
    let p = ui.painter();
    if selected || resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(10), if selected { pal.select } else { pal.hover });
    }
    let title = if e.issuer.is_empty() { e.account.clone() } else { e.issuer.clone() };
    p.text(rect.left_top() + vec2(14.0, 10.0), egui::Align2::LEFT_TOP, &title, FontId::proportional(14.0), pal.text);
    if !e.issuer.is_empty() && !e.account.is_empty() {
        p.text(rect.left_top() + vec2(14.0, 30.0), egui::Align2::LEFT_TOP, &e.account, FontId::proportional(11.5), pal.dim);
    }
    // the code, and a ring that empties as the period runs out (red in its last five seconds)
    let ring_c = rect.right_center() - vec2(24.0, 0.0);
    let frac = left as f32 / e.period as f32;
    let col = if left <= 5 { pal.red } else { pal.accent };
    p.circle_stroke(ring_c, 9.0, egui::Stroke::new(2.5, pal.border));
    let steps = 40;
    let pts: Vec<egui::Pos2> = (0..=((steps as f32 * frac) as usize)).map(|i| {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 / steps as f32 * std::f32::consts::TAU;
        ring_c + vec2(a.cos(), a.sin()) * 9.0
    }).collect();
    if pts.len() > 1 {
        p.add(egui::Shape::line(pts, egui::Stroke::new(2.5, col)));
    }
    p.text(rect.right_center() - vec2(46.0, 0.0), egui::Align2::RIGHT_CENTER, spaced(&e.code), FontId::monospace(20.0), if left <= 5 { pal.red } else { pal.text });
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
        self.receive();
        ctx.request_repaint_after(Duration::from_millis(250));

        if let Some((t, _)) = &self.copied {
            if t.elapsed() > Duration::from_millis(900) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // codes roll over together at the end of each period: ask again then
        let elapsed = self.fetched.elapsed().as_secs();
        if matches!(self.state, State::Ready) && self.rx.is_none() && self.entries.iter().any(|e| e.remaining <= elapsed) {
            self.fetch(false);
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 12, right: 10, top: 6, bottom: 12 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(if self.demo { "Authenticator (demo)" } else { "Authenticator" }).strong().color(pal.text));
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });
            ui.add_space(6.0);
            match &self.state {
                State::Loading => {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new("Reading your codes…").color(pal.dim)));
                }
                State::Failed(why, open_app) => {
                    let open_app = *open_app;
                    ui.add_space(20.0);
                    ui.label(RichText::new(why.as_str()).color(pal.text));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if open_app && ui.button("Open Proton Authenticator").clicked() {
                            let _ = std::process::Command::new(PROTON_APP).stdin(std::process::Stdio::null()).spawn();
                        }
                        if ui.button("Try again").clicked() {
                            self.state = State::Loading;
                            self.fetch(true);
                        }
                    });
                }
                State::Ready => {
                    let resp = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search accounts").desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)));
                    if !resp.has_focus() && self.copied.is_none() {
                        resp.request_focus();
                    }
                    if resp.changed() {
                        self.sel = 0;
                    }
                    let shown: Vec<Entry> = search(&self.entries, &self.query).into_iter().cloned().collect();
                    if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) && self.sel + 1 < shown.len() {
                        self.sel += 1;
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                        self.sel = self.sel.saturating_sub(1);
                    }
                    let mut to_copy: Option<Entry> = None;
                    if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                        to_copy = shown.get(self.sel).cloned();
                    }
                    ui.add_space(8.0);
                    let footer_h = 28.0;
                    egui::ScrollArea::vertical().max_height(ui.available_height() - footer_h).auto_shrink([false, false]).show(ui, |ui| {
                        if shown.is_empty() {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| ui.label(RichText::new(if self.entries.is_empty() { "No accounts in Proton Authenticator yet." } else { "No account matches." }).color(pal.dim)));
                        }
                        for (i, e) in shown.iter().enumerate() {
                            let left = e.remaining.saturating_sub(elapsed).max(1);
                            if row(ui, &pal, e, left, i == self.sel).clicked() {
                                to_copy = Some(e.clone());
                            }
                        }
                    });
                    if let Some(e) = to_copy {
                        self.copy(&e);
                    }
                    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                        let text = match &self.copied {
                            Some((_, msg)) => RichText::new(msg.as_str()).color(pal.green),
                            None if !self.note.is_empty() => RichText::new(self.note.as_str()).color(pal.amber),
                            None => RichText::new("Enter copies the selected code · Esc closes").color(pal.dim).size(11.5),
                        };
                        ui.label(text);
                    });
                }
            }
        });
        chrome::outline(&ctx, &pal);
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let demo = args.iter().any(|a| a == "--demo");
    if args.iter().any(|a| a == "--bar") {
        println!("{}", bar::card(demo));
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("press") {
        let rest: Vec<&str> = args[1..].iter().map(String::as_str).filter(|a| *a != "--demo").collect();
        bar::press(rest.first().copied().unwrap_or(""), rest.get(1).copied(), demo);
        return Ok(());
    }
    // from the app menu or Agora; running it again closes the open window
    if yggdrasil_ui::single::toggle("yggdrasil-authenticator") {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 520.0])
            .with_min_inner_size([340.0, 320.0])
            .with_title("Authenticator")
            .with_app_id("yggdrasil-authenticator")
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native("Authenticator", options, Box::new(|_| Ok(Box::new(App::new()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_answer_is_read() {
        let ok = r#"{"ok":true,"protocol":1,"now":1700000000,"entries":[{"id":"a1","issuer":"GitHub","account":"ada@example.com","kind":"totp","code":"123456","nextCode":"654321","period":30,"digits":6,"remaining":12}]}"#;
        let es = parse_codes(ok).unwrap();
        assert_eq!(es[0].issuer, "GitHub");
        assert_eq!(es[0].remaining, 12);
        assert_eq!(parse_codes(r#"{"ok":false,"protocol":1,"error":"KEYRING_LOCKED"}"#), Err("KEYRING_LOCKED".into()));
        assert_eq!(parse_codes("garbage"), Err("INTERNAL".into()));
    }

    #[test]
    fn codes_are_spaced_and_search_takes_every_word() {
        assert_eq!(spaced("123456"), "123 456");
        assert_eq!(spaced("12345678"), "1234 5678");
        let e = |i: &str, a: &str| Entry { id: i.into(), issuer: i.into(), account: a.into(), code: String::new(), next: String::new(), period: 30, remaining: 30 };
        let es = vec![e("GitHub", "ada@work"), e("Proton", "ada@home"), e("GitLab", "bob@work")];
        assert_eq!(search(&es, "git work").len(), 2);
        assert_eq!(search(&es, "ada git").iter().map(|x| x.issuer.as_str()).collect::<Vec<_>>(), ["GitHub"]);
        assert_eq!(search(&es, "").len(), 3);
    }
}
