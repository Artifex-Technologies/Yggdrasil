//! yggdrasil-share-picker: the question before a screen is shared. yggdrasil-portal starts it when a program asks to share the
//! screen (ScreenCast). It lists the screens; the person picks one (or several, when the program asked for several), and
//! Share prints their names, one a line, for the portal. Cancel, Escape or closing the window prints nothing and exits 1:
//! nothing is shared.
//!
//!   yggdrasil-share-picker [--app APP_ID] [--multiple]
//!
//! The program is named by its menu entry (Name= in APP_ID.desktop) when the portal knows its app id, else "A program": the
//! question never guesses who is asking.

use std::io::Write;
use std::path::PathBuf;

use eframe::egui::{self, pos2, vec2, Align, Color32, CornerRadius, Key, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind};
use yggdrasil_ui::{chrome, theme};
use yggdrasil_wm::Screen;

#[derive(Debug, PartialEq)]
struct Opts {
    app: Option<String>,
    multiple: bool,
}

fn parse_args(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { app: None, multiple: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--app" => o.app = Some(it.next().ok_or("--app needs an app id")?.clone()).filter(|s| !s.is_empty()),
            "--multiple" => o.multiple = true,
            other => return Err(format!("{other}: not an option (yggdrasil-share-picker [--app APP_ID] [--multiple])")),
        }
    }
    Ok(o)
}

/// The folders menu entries are in: the person's own first, then the system's.
fn application_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home =
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home.map(|h| h.join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data_home
        .into_iter()
        .chain(data_dirs.split(':').filter(|d| d.starts_with('/')).map(PathBuf::from))
        .map(|d| d.join("applications"))
        .collect()
}

/// The program's name from its menu entry: `Name=` in the `[Desktop Entry]` group of APP_ID.desktop.
fn app_name(app_id: &str, dirs: &[PathBuf]) -> Option<String> {
    // an app id is a D-Bus-style name: anything else is not looked up as a file name
    if app_id.is_empty() || !app_id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) || app_id.starts_with('.') {
        return None;
    }
    dirs.iter().find_map(|d| entry_name(&std::fs::read_to_string(d.join(format!("{app_id}.desktop"))).ok()?))
}

fn entry_name(text: &str) -> Option<String> {
    let mut in_entry = false;
    for l in text.lines().map(str::trim) {
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
        } else if in_entry {
            if let Some(v) = l.strip_prefix("Name=") {
                let v = v.trim();
                return (!v.is_empty()).then(|| v.to_string());
            }
        }
    }
    None
}

/// The question's words.
fn summary(app: Option<&str>, multiple: bool) -> String {
    let who = app.unwrap_or("A program");
    let what = if multiple { "the screens you choose" } else { "the screen you choose" };
    format!("{who} wants to see {what}.")
}

/// A screen as the list shows it: its maker and model with its name and size below, or (when it tells no model) its name with
/// its size below.
fn screen_label(s: &Screen) -> (String, String) {
    let model = format!("{} {}", s.make.trim(), s.model.trim()).trim().to_string();
    let (w, h) = if s.transform % 2 == 1 { (s.height, s.width) } else { (s.width, s.height) };
    if model.is_empty() {
        (s.name.clone(), format!("{w} × {h}"))
    } else {
        (model, format!("{} · {w} × {h}", s.name))
    }
}

/// Which screens are picked: one at a time, or any number when the program asked for several. A lone screen is picked from
/// the start (the person still says Share).
#[derive(Debug)]
struct Choice {
    picked: Vec<bool>,
    multiple: bool,
}

impl Choice {
    fn new(n: usize, multiple: bool) -> Choice {
        Choice { picked: (0..n).map(|_| n == 1).collect(), multiple }
    }

    fn click(&mut self, i: usize) {
        if i >= self.picked.len() {
            return;
        }
        if self.multiple {
            self.picked[i] = !self.picked[i];
        } else {
            self.picked.iter_mut().enumerate().for_each(|(k, p)| *p = k == i);
        }
    }

    fn chosen<'a>(&self, screens: &'a [Screen]) -> Vec<&'a str> {
        screens.iter().zip(&self.picked).filter(|(_, p)| **p).map(|(s, _)| s.name.as_str()).collect()
    }
}

struct App {
    question: String,
    screens: Vec<Screen>,
    choice: Choice,
}

/// The answer: the names printed (Share) and exit 0, or nothing and exit 1.
fn finish(names: Option<Vec<&str>>) -> ! {
    match names {
        Some(n) if !n.is_empty() => {
            let mut out = std::io::stdout().lock();
            for name in n {
                let _ = writeln!(out, "{name}");
            }
            let _ = out.flush();
            std::process::exit(0)
        }
        _ => std::process::exit(1),
    }
}

/// A screen on a stand, in a rounded accent square.
fn screen_icon(ui: &mut egui::Ui, pal: &theme::Palette, size: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same((size * 0.26) as u8), pal.accent);
    let ink = pal.on_accent();
    let c = r.center();
    let face = Rect::from_center_size(c - vec2(0.0, size * 0.06), vec2(size * 0.56, size * 0.36));
    p.rect_stroke(face, CornerRadius::same(3), Stroke::new(size * 0.06, ink), StrokeKind::Middle);
    p.line_segment([pos2(c.x, face.bottom()), pos2(c.x, face.bottom() + size * 0.1)], Stroke::new(size * 0.06, ink));
    let foot = face.bottom() + size * 0.12;
    p.line_segment([pos2(c.x - size * 0.13, foot), pos2(c.x + size * 0.13, foot)], Stroke::new(size * 0.06, ink));
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        let dark = theme::system_dark();
        let accent = theme::system_accent();
        theme::apply(&ctx, dark, accent);
        let pal = theme::palette_for(dark, accent);
        if ui.input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape)) {
            finish(None);
        }
        // 1 to 9 pick a screen from the keyboard
        for (k, key) in
            [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].iter().enumerate()
        {
            if ui.input(|i| i.key_pressed(*key)) {
                self.choice.click(k);
            }
        }
        let mut share = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 26, right: 26, top: 6, bottom: 20 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    chrome::drag_area(ui, ui.available_width() - 26.0);
                    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 24.0), Sense::click());
                    ui.painter().circle_filled(r.center(), 6.5, Color32::from_rgb(255, 95, 87));
                    if resp.on_hover_text("Cancel").clicked() {
                        finish(None);
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    screen_icon(ui, &pal, 48.0);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Share your screen").size(19.0).strong());
                        ui.add(egui::Label::new(RichText::new(self.question.as_str()).color(pal.dim)).wrap());
                    });
                });
                ui.add_space(12.0);
                if self.screens.is_empty() {
                    ui.label(RichText::new("No screen can be shared: the compositor did not list any.").color(pal.dim));
                }
                for (i, s) in self.screens.iter().enumerate() {
                    let picked = self.choice.picked[i];
                    let (title, detail) = screen_label(s);
                    let resp = egui::Frame::new()
                        .fill(if picked { pal.select } else { pal.field })
                        .stroke(Stroke::new(if picked { 2.0 } else { 1.0 }, if picked { pal.accent } else { pal.border }))
                        .corner_radius(CornerRadius::same(8))
                        // the picked row's border is a pixel wider: its margin a pixel narrower, so nothing moves
                        .inner_margin(if picked { Margin::symmetric(11, 7) } else { Margin::symmetric(12, 8) })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(title).size(14.5).strong());
                                    ui.label(RichText::new(detail).size(12.5).color(pal.dim));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if picked {
                                        ui.label(RichText::new("✔").size(16.0).color(pal.accent));
                                    } else if i < 9 {
                                        ui.label(RichText::new(format!("{}", i + 1)).size(12.5).color(pal.dim));
                                    }
                                });
                            });
                        })
                        .response
                        .interact(Sense::click());
                    if resp.clicked() {
                        self.choice.click(i);
                    }
                    ui.add_space(6.0);
                }
                ui.add_space(10.0);
                let ready = !self.choice.chosen(&self.screens).is_empty();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let go = egui::Button::new(RichText::new("Share").size(14.5).strong().color(pal.on_accent()))
                        .fill(pal.accent)
                        .corner_radius(CornerRadius::same(8))
                        .min_size(vec2(120.0, 36.0));
                    if ui.add_enabled(ready, go).clicked() || (ready && ui.input(|i| i.key_pressed(Key::Enter))) {
                        share = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Cancel").size(14.5))
                                .corner_radius(CornerRadius::same(8))
                                .min_size(vec2(96.0, 36.0)),
                        )
                        .clicked()
                    {
                        finish(None);
                    }
                });
            });
        chrome::outline(&ctx, &pal);
        if share {
            finish(Some(self.choice.chosen(&self.screens)));
        }
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse_args(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("yggdrasil-share-picker: {e}");
            std::process::exit(2);
        }
    };
    let name = opts.app.as_deref().and_then(|a| app_name(a, &application_dirs()));
    let mut screens: Vec<Screen> =
        yggdrasil_wm::Desk::detect().map(|d| d.screens()).unwrap_or_default().into_iter().filter(|s| !s.disabled).collect();
    screens.sort_by_key(|s| (s.x, s.y));
    let height = (208.0 + 64.0 * screens.len().max(1) as f32).min(660.0);
    let app = App { question: summary(name.as_deref(), opts.multiple), choice: Choice::new(screens.len(), opts.multiple), screens };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([500.0, height])
            .with_resizable(false)
            .with_decorations(false)
            .with_title("Share your screen")
            .with_app_id("yggdrasil-share-picker")
            .with_always_on_top(),
        ..Default::default()
    };
    eframe::run_native(
        "Share your screen",
        options,
        Box::new(move |cc| {
            yggdrasil_ui::fonts::install(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(name: &str, make: &str, model: &str, transform: u32) -> Screen {
        Screen {
            id: None,
            name: name.into(),
            make: make.into(),
            model: model.into(),
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
            scale: Some(1.0),
            transform,
            reserved: [0; 4],
            focused: false,
            disabled: false,
            active_workspace: None,
            special_workspace: None,
            special_workspace_id: None,
            refresh: None,
            modes: vec![],
            mirror_of: None,
        }
    }

    #[test]
    fn options() {
        let a = |v: &[&str]| parse_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&[]), Ok(Opts { app: None, multiple: false }));
        assert_eq!(a(&["--app", "org.example.Call", "--multiple"]), Ok(Opts { app: Some("org.example.Call".into()), multiple: true }));
        assert_eq!(a(&["--app", ""]), Ok(Opts { app: None, multiple: false }));
        assert!(a(&["--app"]).is_err() && a(&["--window"]).is_err());
    }

    #[test]
    fn the_program_is_named_by_its_menu_entry_and_never_guessed() {
        let dir = std::env::temp_dir().join(format!("share-picker-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("org.example.Call.desktop"),
            "[Desktop Action new]\nName=New call\n[Desktop Entry]\nType=Application\nName=Example Call\n",
        )
        .unwrap();
        let dirs = vec![PathBuf::from("/nonexistent"), dir.clone()];
        assert_eq!(app_name("org.example.Call", &dirs).as_deref(), Some("Example Call"));
        // no entry, or an id that is not one: no name, so the question says "A program"
        assert_eq!(app_name("org.example.Other", &dirs), None);
        assert_eq!(app_name("../org.example.Call", &dirs), None);
        assert_eq!(app_name("", &dirs), None);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(summary(None, false), "A program wants to see the screen you choose.");
        assert_eq!(summary(Some("Example Call"), true), "Example Call wants to see the screens you choose.");
    }

    #[test]
    fn screens_are_named_by_model_with_their_size() {
        assert_eq!(screen_label(&screen("DP-1", "Maker", "Model 34", 0)), ("Maker Model 34".into(), "DP-1 · 2560 × 1440".into()));
        // no model told: the connector's name; turned a quarter: the size turned too
        assert_eq!(screen_label(&screen("HDMI-A-1", "", "", 1)), ("HDMI-A-1".into(), "1440 × 2560".into()));
    }

    #[test]
    fn one_screen_or_several() {
        let screens = vec![screen("DP-1", "", "", 0), screen("DP-2", "", "", 0)];
        let mut one = Choice::new(2, false);
        assert!(one.chosen(&screens).is_empty(), "nothing is picked for the person when there is a choice");
        one.click(0);
        one.click(1);
        assert_eq!(one.chosen(&screens), vec!["DP-2"]);
        let mut many = Choice::new(2, true);
        many.click(0);
        many.click(1);
        assert_eq!(many.chosen(&screens), vec!["DP-1", "DP-2"]);
        many.click(0);
        assert_eq!(many.chosen(&screens), vec!["DP-2"]);
        many.click(7);
        assert_eq!(many.chosen(&screens), vec!["DP-2"]);
        // a lone screen is picked from the start
        assert_eq!(Choice::new(1, false).chosen(&screens[..1]), vec!["DP-1"]);
    }
}
