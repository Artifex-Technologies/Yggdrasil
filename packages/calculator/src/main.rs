//! Labyrinth Calculator: a standard calculator with history that turns scientific when the window
//! is widened. Same look as Finder and Settings (labyrinth-ui).
//!
//! Width decides what shows: narrow is the standard pad, wider adds the history beside it, wider
//! still adds the scientific pad on the left. Long expressions are typed or pasted into the
//! display (evaluated live as you type); the keys just insert text.

mod eval;

use std::path::PathBuf;

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText, Sense, UiBuilder};
use eval::{eval, format_number, group_thousands, Angle, EvalError};
use labyrinth_ui::theme::{self, Palette};
use labyrinth_ui::chrome;
use serde::{Deserialize, Serialize};

const HISTORY_LIMIT: usize = 200;
/// Window widths (logical px) at which the layout grows.
const SHOW_HISTORY_AT: f32 = 600.0;
const SHOW_SCIENTIFIC_AT: f32 = 860.0;
const HISTORY_W: f32 = 270.0;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
struct Entry {
    expr: String,
    result: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Act {
    /// Insert text into the expression.
    Ins(&'static str),
    Clear,
    Back,
    Eval,
    Negate,
    MemClear,
    MemRecall,
    MemAdd,
    MemSub,
    ToggleAngle,
}

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Digit,
    Op,
    Func,
    Equals,
}

#[derive(Clone, Copy)]
struct Key {
    label: &'static str,
    act: Act,
    style: Style,
}

const fn k(label: &'static str, act: Act, style: Style) -> Key {
    Key { label, act, style }
}

const STANDARD: [[Key; 4]; 6] = [
    [k("(", Act::Ins("("), Style::Func), k(")", Act::Ins(")"), Style::Func), k("x²", Act::Ins("^2"), Style::Func), k("√", Act::Ins("sqrt("), Style::Func)],
    [k("C", Act::Clear, Style::Func), k("⌫", Act::Back, Style::Func), k("%", Act::Ins("%"), Style::Func), k("÷", Act::Ins("÷"), Style::Op)],
    [k("7", Act::Ins("7"), Style::Digit), k("8", Act::Ins("8"), Style::Digit), k("9", Act::Ins("9"), Style::Digit), k("×", Act::Ins("×"), Style::Op)],
    [k("4", Act::Ins("4"), Style::Digit), k("5", Act::Ins("5"), Style::Digit), k("6", Act::Ins("6"), Style::Digit), k("−", Act::Ins("−"), Style::Op)],
    [k("1", Act::Ins("1"), Style::Digit), k("2", Act::Ins("2"), Style::Digit), k("3", Act::Ins("3"), Style::Digit), k("+", Act::Ins("+"), Style::Op)],
    [k("±", Act::Negate, Style::Func), k("0", Act::Ins("0"), Style::Digit), k(".", Act::Ins("."), Style::Digit), k("=", Act::Eval, Style::Equals)],
];

const SCIENTIFIC: [[Key; 4]; 6] = [
    [k("π", Act::Ins("π"), Style::Func), k("e", Act::Ins("e"), Style::Func), k("xʸ", Act::Ins("^"), Style::Func), k("n!", Act::Ins("!"), Style::Func)],
    [k("sin", Act::Ins("sin("), Style::Func), k("cos", Act::Ins("cos("), Style::Func), k("tan", Act::Ins("tan("), Style::Func), k("1/x", Act::Ins("1/("), Style::Func)],
    [k("asin", Act::Ins("asin("), Style::Func), k("acos", Act::Ins("acos("), Style::Func), k("atan", Act::Ins("atan("), Style::Func), k("|x|", Act::Ins("abs("), Style::Func)],
    [k("ln", Act::Ins("ln("), Style::Func), k("log", Act::Ins("log("), Style::Func), k("eˣ", Act::Ins("exp("), Style::Func), k("10ˣ", Act::Ins("10^("), Style::Func)],
    [k("sinh", Act::Ins("sinh("), Style::Func), k("cosh", Act::Ins("cosh("), Style::Func), k("tanh", Act::Ins("tanh("), Style::Func), k("∛", Act::Ins("cbrt("), Style::Func)],
    [k("MC", Act::MemClear, Style::Func), k("MR", Act::MemRecall, Style::Func), k("M+", Act::MemAdd, Style::Func), k("M−", Act::MemSub, Style::Func)],
];

struct Calc {
    expr: String,
    angle: Angle,
    history: Vec<Entry>,
    memory: f64,
    /// The expression is the result of the last "=": a typed digit starts fresh, an operator continues.
    after_eval: bool,
    show_history_narrow: bool,
    error: Option<String>,
    focus_display: bool,
}

fn history_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("labyrinth").join("calculator-history.json"))
}

fn load_history() -> Vec<Entry> {
    history_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_history(h: &[Entry]) {
    if let Some(p) = history_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::to_string(h).unwrap_or_default());
    }
}

/// True when the number being typed at the end of `expr` already has a decimal point.
fn last_number_has_dot(expr: &str) -> bool {
    expr.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').any(|c| c == '.')
}

/// True when any number in `expr` has two decimal points, like "12.5.3".
fn has_double_dot(expr: &str) -> bool {
    let mut dots = 0;
    for c in expr.chars() {
        match c {
            '.' => {
                dots += 1;
                if dots > 1 {
                    return true;
                }
            }
            c if c.is_ascii_digit() || c == ',' => {}
            _ => dots = 0,
        }
    }
    false
}

impl Calc {
    fn new() -> Calc {
        Calc { expr: String::new(), angle: Angle::Degrees, history: load_history(), memory: 0.0, after_eval: false, show_history_narrow: false, error: None, focus_display: true }
    }

    fn live_result(&self) -> Result<f64, EvalError> {
        eval(&self.expr, self.angle)
    }

    fn insert(&mut self, text: &str) {
        let continues = matches!(text.chars().next(), Some('+' | '−' | '×' | '÷' | '^' | '%' | '!'));
        if self.after_eval {
            // "=" leaves the equation in place; an operator carries on from its result, anything else starts over.
            self.expr = if continues { self.live_result().map(format_number).unwrap_or_default() } else { String::new() };
        }
        self.after_eval = false;
        self.error = None;
        if text == "." && last_number_has_dot(&self.expr) {
            return; // a number can only have one decimal point
        }
        self.expr.push_str(text);
    }

    fn evaluate(&mut self) {
        match eval(&self.expr, self.angle) {
            Ok(v) => {
                let result = format_number(v);
                let entry = Entry { expr: self.expr.trim().to_string(), result: result.clone() };
                if self.history.first() != Some(&entry) && entry.expr != entry.result {
                    self.history.insert(0, entry);
                    self.history.truncate(HISTORY_LIMIT);
                    save_history(&self.history);
                }
                self.after_eval = true;
                self.error = None;
            }
            Err(EvalError::Empty) => {}
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn act(&mut self, a: Act) {
        match a {
            Act::Ins(t) => self.insert(t),
            Act::Clear => {
                self.expr.clear();
                self.error = None;
                self.after_eval = false;
            }
            Act::Back => {
                self.expr.pop();
                self.after_eval = false;
                self.error = None;
            }
            Act::Eval => self.evaluate(),
            Act::Negate => {
                let e = self.expr.trim().to_string();
                self.expr = if e.is_empty() {
                    "−".into()
                } else if let Some(inner) = e.strip_prefix("−(").and_then(|r| r.strip_suffix(')')) {
                    inner.to_string()
                } else if let Some(rest) = e.strip_prefix('−') {
                    rest.to_string()
                } else {
                    format!("−({e})")
                };
                self.after_eval = false;
            }
            Act::MemClear => self.memory = 0.0,
            Act::MemRecall => {
                let m = format_number(self.memory);
                self.insert(&m);
            }
            Act::MemAdd | Act::MemSub => {
                if let Ok(v) = self.live_result() {
                    self.memory += if a == Act::MemAdd { v } else { -v };
                }
            }
            Act::ToggleAngle => self.angle = if self.angle == Angle::Degrees { Angle::Radians } else { Angle::Degrees },
        }
        self.focus_display = true;
    }
}

// ---------------------------------------------------------------- drawing

fn key_button(ui: &mut egui::Ui, pal: &Palette, rect: Rect, key: Key, angle: Angle) -> bool {
    let id = ui.id().with(key.label).with(rect.min.x as i32).with(rect.min.y as i32);
    let resp = ui.interact(rect, id, Sense::click());
    let (fill, text) = match key.style {
        Style::Digit => (pal.field, pal.text),
        Style::Op => (pal.select, pal.text),
        Style::Func => (pal.hover, pal.text),
        Style::Equals => (pal.accent, Color32::WHITE),
    };
    let fill = if resp.is_pointer_button_down_on() {
        fill.gamma_multiply(0.8)
    } else if resp.hovered() {
        lighten(fill)
    } else {
        fill
    };
    ui.painter().rect_filled(rect.shrink(2.0), CornerRadius::same(10), fill);
    let size = if key.style == Style::Digit { 20.0 } else if key.label.chars().count() > 2 { 14.0 } else { 18.0 };
    let label = if key.act == Act::ToggleAngle { if angle == Angle::Degrees { "DEG" } else { "RAD" } } else { key.label };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(size), text);
    resp.clicked()
}

fn lighten(c: Color32) -> Color32 {
    let f = |v: u8| (v as f32 + (255.0 - v as f32) * 0.10) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// A grid of keys filling `rect`; returns the action of the key clicked, if any.
fn pad(ui: &mut egui::Ui, pal: &Palette, rect: Rect, rows: &[[Key; 4]], angle: Angle) -> Option<Act> {
    let (cw, ch) = (rect.width() / 4.0, rect.height() / rows.len() as f32);
    let mut clicked = None;
    for (r, row) in rows.iter().enumerate() {
        for (c, key) in row.iter().enumerate() {
            let cell = Rect::from_min_size(pos2(rect.left() + c as f32 * cw, rect.top() + r as f32 * ch), vec2(cw, ch));
            if key_button(ui, pal, cell, *key, angle) {
                clicked = Some(key.act);
            }
        }
    }
    clicked
}

fn clock_icon(ui: &mut egui::Ui, pal: &Palette, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    if active {
        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
    }
    let c = rect.center();
    let s = egui::Stroke::new(1.6, pal.text);
    ui.painter().circle_stroke(c, 8.0, s);
    ui.painter().line_segment([c, c + vec2(0.0, -5.0)], s);
    ui.painter().line_segment([c, c + vec2(4.0, 2.0)], s);
    resp.on_hover_text("History")
}

impl eframe::App for Calc {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        theme::apply(&ctx, true, accent);
        let pal = theme::palette_for(true, accent);

        // Enter evaluates, Escape clears (the display has the keyboard focus).
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.evaluate();
            self.focus_display = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.act(Act::Clear);
        }

        let width = ctx.content_rect().width();
        let show_hist_pane = width >= SHOW_HISTORY_AT;
        let show_sci = width >= SHOW_SCIENTIFIC_AT;
        let showing_history = show_hist_pane || self.show_history_narrow;

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 10, right: 8, top: 6, bottom: 10 })).show(ui, |ui| {
            // Top row: the history button, window buttons at the right.
            ui.horizontal(|ui| {
                if !show_hist_pane && clock_icon(ui, &pal, self.show_history_narrow).clicked() {
                    self.show_history_narrow = !self.show_history_narrow;
                }
                if self.memory != 0.0 {
                    ui.label(RichText::new("M").color(pal.accent).strong());
                }
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });

            let body = ui.available_rect_before_wrap();
            let hist_w = if show_hist_pane { HISTORY_W } else { 0.0 };
            let main_w = body.width() - hist_w - if show_hist_pane { 10.0 } else { 0.0 };
            let main = Rect::from_min_size(body.min, vec2(main_w, body.height()));

            // Display: the expression (editable, right-aligned) and the live result under it.
            let display_h = 118.0;
            let display = Rect::from_min_size(main.min, vec2(main.width(), display_h));
            let mut disp_ui = ui.new_child(UiBuilder::new().max_rect(display).layout(Layout::top_down(Align::Max)));
            let font = FontId::proportional(if self.expr.chars().count() > 22 { 20.0 } else { 28.0 });
            let before = self.expr.clone();
            let edit = egui::TextEdit::singleline(&mut self.expr).font(font).horizontal_align(Align::RIGHT).frame(egui::Frame::new()).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 4));
            let r = disp_ui.add(edit);
            if r.changed() && has_double_dot(&self.expr) {
                self.expr = before; // typed or pasted a second decimal point in one number: undo it
            } else if r.changed() {
                self.error = None;
                self.after_eval = false;
            }
            if self.focus_display {
                r.request_focus();
                self.focus_display = false;
            }
            let live = if let Some(e) = &self.error {
                RichText::new(e).size(15.0).color(pal.red)
            } else {
                match self.live_result() {
                    Ok(v) if !self.after_eval => RichText::new(group_thousands(&format_number(v))).size(34.0).strong(),
                    Ok(v) => RichText::new(group_thousands(&format_number(v))).size(34.0).strong(),
                    Err(_) => RichText::new(if self.expr.is_empty() { "0" } else { "" }).size(34.0).strong().color(pal.dim),
                }
            };
            disp_ui.add_space(2.0);
            disp_ui.label(live);

            // Keypads.
            let keys = Rect::from_min_max(pos2(main.left(), main.top() + display_h + 4.0), main.max);
            let gap = 8.0;
            let (std_rect, sci_rect) = if show_sci {
                let half = (keys.width() - gap) / 2.0;
                (Rect::from_min_size(pos2(keys.left() + half + gap, keys.top()), vec2(half, keys.height())), Some(Rect::from_min_size(keys.min, vec2(half, keys.height()))))
            } else {
                (keys, None)
            };
            let mut action = None;
            if show_hist_pane || !self.show_history_narrow {
                if let Some(a) = pad(ui, &pal, std_rect, &STANDARD, self.angle) {
                    action = Some(a);
                }
                if let Some(sr) = sci_rect {
                    // The scientific pad's last row is the memory keys; the angle toggle sits above it.
                    let mut rows = SCIENTIFIC;
                    rows[0][2] = k("DEG", Act::ToggleAngle, Style::Func);
                    rows[0][3] = k("n!", Act::Ins("!"), Style::Func);
                    rows[1][3] = k("xʸ", Act::Ins("^"), Style::Func);
                    if let Some(a) = pad(ui, &pal, sr, &rows, self.angle) {
                        action = Some(a);
                    }
                }
            }
            // History: beside the keys when wide, in place of them when narrow.
            let mut reuse: Option<String> = None;
            let mut clear_history = false;
            let hist_rect = if show_hist_pane {
                Some(Rect::from_min_max(pos2(main.right() + 10.0, body.top()), body.max))
            } else if self.show_history_narrow {
                Some(Rect::from_min_max(pos2(main.left(), main.top() + display_h + 4.0), main.max))
            } else {
                None
            };
            if let (true, Some(hr)) = (showing_history, hist_rect) {
                let mut hui = ui.new_child(UiBuilder::new().max_rect(hr).layout(Layout::top_down(Align::Min)));
                egui::Frame::new().fill(pal.field).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(10, 8)).show(&mut hui, |ui| {
                    ui.set_min_size(hr.size() - vec2(20.0, 16.0));
                    ui.set_max_size(hr.size() - vec2(20.0, 16.0));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("History").strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !self.history.is_empty() && ui.add(egui::Button::new(RichText::new("Clear").color(pal.dim)).frame(false)).clicked() {
                                clear_history = true;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    if self.history.is_empty() {
                        ui.label(RichText::new("Nothing calculated yet").color(pal.dim));
                    }
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        for e in &self.history {
                            ui.with_layout(Layout::top_down(Align::Max), |ui| {
                                if ui.add(egui::Label::new(RichText::new(&e.expr).size(13.0).color(pal.dim)).sense(Sense::click()).truncate()).on_hover_text("Use this expression").clicked() {
                                    reuse = Some(e.expr.clone());
                                }
                                if ui.add(egui::Label::new(RichText::new(group_thousands(&e.result)).size(20.0).strong()).sense(Sense::click())).on_hover_text("Use this result").clicked() {
                                    reuse = Some(e.result.clone());
                                }
                            });
                            ui.add_space(6.0);
                        }
                    });
                });
            }
            if clear_history {
                self.history.clear();
                save_history(&self.history);
            }
            if let Some(t) = reuse {
                self.expr = t;
                self.after_eval = true;
                self.focus_display = true;
            }
            if let Some(a) = action {
                self.act(a);
            }
        });
        chrome::outline(&ctx, &pal);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 640.0])
            .with_min_inner_size([340.0, 520.0])
            .with_title("Calculator")
            .with_app_id("labyrinth-calculator")
            // Draws its own title bar like Finder and Settings (labyrinth-ui chrome).
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native("Calculator", options, Box::new(|_| Ok(Box::new(Calc::new()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calc() -> Calc {
        Calc { expr: String::new(), angle: Angle::Degrees, history: vec![], memory: 0.0, after_eval: false, show_history_narrow: false, error: None, focus_display: false }
    }

    #[test]
    fn keys_build_an_expression_and_equals_evaluates() {
        let mut c = calc();
        for a in [Act::Ins("1"), Act::Ins("2"), Act::Ins("+"), Act::Ins("3"), Act::Ins("×"), Act::Ins("4")] {
            c.act(a);
        }
        assert_eq!(c.expr, "12+3×4");
        assert_eq!(format_number(c.live_result().unwrap()), "24");
        c.evaluate();
        assert_eq!(c.expr, "12+3×4", "the equation stays so it can be corrected");
        assert_eq!(c.history[0], Entry { expr: "12+3×4".into(), result: "24".into() });
    }

    #[test]
    fn an_operator_continues_the_result_but_a_digit_starts_over() {
        let mut c = calc();
        c.expr = "10+5".into();
        c.evaluate();
        c.act(Act::Ins("×"));
        c.act(Act::Ins("2"));
        assert_eq!(c.expr, "15×2");
        c.evaluate();
        assert_eq!(c.expr, "15×2");
        c.act(Act::Ins("7"));
        assert_eq!(c.expr, "7");
    }

    #[test]
    fn a_number_gets_only_one_decimal_point() {
        let mut c = calc();
        for t in ["1", ".", "5", ".", "3", "+", ".", "2", ".", "1"] {
            c.act(Act::Ins(t));
        }
        assert_eq!(c.expr, "1.53+.21");
        assert!(has_double_dot("12.5.3") && !has_double_dot("1.5+2.5"));
    }

    #[test]
    fn negate_toggles_and_errors_do_not_touch_history() {
        let mut c = calc();
        c.expr = "5+3".into();
        c.act(Act::Negate);
        assert_eq!(c.expr, "−(5+3)");
        assert_eq!(format_number(c.live_result().unwrap()), "-8");
        c.act(Act::Negate);
        assert_eq!(c.expr, "5+3");
        c.expr = "5÷0".into();
        c.evaluate();
        assert_eq!(c.error.as_deref(), Some("Cannot divide by zero"));
        assert!(c.history.is_empty());
        assert_eq!(c.expr, "5÷0");
    }

    #[test]
    fn memory_keys_work_on_the_current_value() {
        let mut c = calc();
        c.expr = "20+5".into();
        c.act(Act::MemAdd);
        c.expr = "3".into();
        c.act(Act::MemSub);
        assert_eq!(c.memory, 22.0);
        c.act(Act::Clear);
        c.act(Act::MemRecall);
        assert_eq!(c.expr, "22");
        c.act(Act::MemClear);
        assert_eq!(c.memory, 0.0);
    }

    #[test]
    fn angle_mode_changes_trig_results() {
        let mut c = calc();
        c.expr = "sin(90)".into();
        assert_eq!(format_number(c.live_result().unwrap()), "1");
        c.act(Act::ToggleAngle);
        assert_eq!(c.angle, Angle::Radians);
        assert_eq!(format_number(c.live_result().unwrap()), "0.893996663601");
    }
}
