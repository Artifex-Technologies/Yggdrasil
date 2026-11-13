//! Settings > Displays, laid out like Windows 11's Display settings in Yggdrasil's look. With two or more displays attached: a
//! box at the top with every display to scale, numbered, to drag into place (they land edge to edge, displays.rs) and to click,
//! an Identify button, and below only the selected display's settings, with Extend, Duplicate or Disconnect. A disconnected
//! display stays in the box, smaller and grey, to be connected again. With one display: its settings at once, nothing else.
//! A change that can leave a screen black asks "Keep these display settings?" and is put back after 15 seconds unless kept.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, RichText, Sense, Stroke, StrokeKind};
use yggdrasil_ui::theme::Palette;

use crate::displays::{self, Change, Keep, ScreenMode};
use crate::sys::{Displays, Output};
use crate::ui;

/// What the page asks the app to do.
pub enum Act {
    Change(Change),
    MakeMain(Output),
    Tiling(String, bool),
    Identify(Vec<(String, u32)>),
}

/// The answer to "Keep these display settings?", shared with the thread that puts the change back when none comes.
pub const WAITING: u8 = 0;
pub const KEPT: u8 = 1;
pub const PUT_BACK: u8 = 2;
/// The thread is done (kept, put back, timed out or the change failed): the question goes.
pub const OVER: u8 = 3;

pub struct Prompt {
    pub keep: Keep,
    pub answer: Arc<AtomicU8>,
}

#[derive(Default)]
pub struct State {
    /// The display whose settings show (its connector); the main display until another is clicked.
    selected: Option<String>,
    drag: Option<Drag>,
    pub prompt: Option<Prompt>,
}

/// A display being dragged: which, where it was taken (logical pixels from its corner), and the box's scale and origin as they
/// were when the drag began (so the picture does not shift under the pointer).
struct Drag {
    name: String,
    grab: (f32, f32),
    scale: f32,
    origin: egui::Pos2,
    want: (i32, i32),
}

const BOX_H: f32 = 240.0;
const OFF_STRIP: f32 = 64.0;

fn title_of(o: &Output, n: u32) -> String {
    let what = [o.make.as_str(), o.model.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
    let mut t = if what.is_empty() { o.name.clone() } else { format!("{what} ({})", o.name) };
    if n > 0 {
        t = format!("{n} · {t}");
    }
    if o.primary && displays::placed(o) && n > 0 {
        t.push_str(" · Main display");
    }
    t
}

/// The page. Returns what to do, if anything was chosen.
pub fn page(ui: &mut egui::Ui, pal: &Palette, d: &Displays, st: &mut State) -> Option<Act> {
    let outs = &d.outputs;
    if outs.is_empty() {
        ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "No displays reported", None, |_| {}));
        return None;
    }
    if st.selected.as_ref().is_none_or(|s| !outs.iter().any(|o| o.name == *s)) {
        st.selected = outs.iter().find(|o| o.primary).or(outs.first()).map(|o| o.name.clone());
    }
    let mut act = None;
    let several = displays::several(outs);
    if several {
        ui::group(ui, pal, Some("Arrangement"), |ui| {
            ui.add_space(8.0);
            if let Some(a) = arrangement(ui, pal, outs, st) {
                act = Some(a);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui::quiet(ui, pal, "Identify").on_hover_text("Shows each display's number on it").clicked() {
                    let on: Vec<(String, u32)> =
                        displays::numbers(outs).into_iter().filter(|(n, _)| outs.iter().any(|o| o.name == *n && o.enabled)).collect();
                    act = Some(Act::Identify(on));
                }
                ui.label(RichText::new("Drag the displays to arrange them. Click one to change its settings.").color(pal.dim));
            });
            ui.add_space(6.0);
        });
    }
    let Some(o) = outs.iter().find(|o| Some(&o.name) == st.selected.as_ref()) else { return act };
    // with one display there is nothing to tell it from: no number
    let title = if several { title_of(o, displays::number_of(outs, &o.name)) } else { title_of(o, 0) };
    ui::group(ui, pal, Some(&title), |ui| {
        if let Some(a) = settings(ui, pal, d, o, several) {
            act = Some(a);
        }
    });
    act
}

/// The rows of one display.
fn settings(ui: &mut egui::Ui, pal: &Palette, d: &Displays, o: &Output, several: bool) -> Option<Act> {
    let outs = &d.outputs;
    let mut act = None;
    let mut first = true;
    let mut next = || std::mem::replace(&mut first, false);
    if several {
        let mode = displays::mode_of(o);
        let label = |m: &ScreenMode| match m {
            ScreenMode::Extend => "Extend these displays".to_string(),
            ScreenMode::Duplicate(of) => format!("Duplicate display {}", displays::number_of(outs, of)),
            ScreenMode::Disconnect if mode == ScreenMode::Disconnect => "Disconnected".to_string(),
            ScreenMode::Disconnect => "Disconnect this display".to_string(),
        };
        let mut choices = vec![ScreenMode::Extend];
        if d.caps.mirror {
            choices.extend(displays::duplicate_targets(outs, &o.name).into_iter().map(ScreenMode::Duplicate));
        }
        if displays::can_disconnect(outs, &o.name) || mode == ScreenMode::Disconnect {
            choices.push(ScreenMode::Disconnect);
        }
        let sub = match &mode {
            ScreenMode::Disconnect => "Off. Choose Extend or Duplicate to use it again",
            ScreenMode::Duplicate(_) => "Shows the same picture as the other display",
            ScreenMode::Extend if !displays::can_disconnect(outs, &o.name) => "The last display showing the desktop stays connected",
            ScreenMode::Extend => "A part of the desktop of its own",
        };
        ui::row(ui, pal, next(), "Display mode", Some(sub), |ui| {
            egui::ComboBox::from_id_salt(format!("mode-{}", o.name)).selected_text(label(&mode)).width(220.0).show_ui(ui, |ui| {
                for c in &choices {
                    if ui.selectable_label(*c == mode, label(c)).clicked() && *c != mode {
                        act = Some(Act::Change(Change::Mode(o.name.clone(), c.clone())));
                    }
                }
            });
        });
        if !o.enabled {
            ui::row(ui, pal, false, "Connect", Some("As a part of the desktop, to the right of the others"), |ui| {
                if ui::action(ui, pal, "Connect").clicked() {
                    act = Some(Act::Change(Change::Mode(o.name.clone(), ScreenMode::Extend)));
                }
            });
        }
    }
    if !displays::placed(o) {
        return act;
    }
    let Some(cur) = o.current_mode().cloned() else { return act };
    ui::row(ui, pal, next(), "Resolution", None, |ui| {
        egui::ComboBox::from_id_salt(format!("res-{}", o.name)).selected_text(format!("{} × {}", cur.width, cur.height)).show_ui(
            ui,
            |ui| {
                for (w, h) in o.resolutions() {
                    if ui.selectable_label(w == cur.width && h == cur.height, format!("{w} × {h}")).clicked() {
                        if let Some(m) = o.rates(w, h).first().filter(|m| m.id != cur.id) {
                            act = Some(Act::Change(Change::Resolution(o.name.clone(), m.id.clone())));
                        }
                    }
                }
            },
        );
    });
    ui::row(ui, pal, false, "Refresh rate", None, |ui| {
        egui::ComboBox::from_id_salt(format!("hz-{}", o.name)).selected_text(format!("{:.0} Hz", cur.refresh)).show_ui(ui, |ui| {
            for m in o.rates(cur.width, cur.height) {
                if ui.selectable_label(m.id == cur.id, format!("{:.2} Hz", m.refresh)).clicked() && m.id != cur.id {
                    act = Some(Act::Change(Change::Resolution(o.name.clone(), m.id.clone())));
                }
            }
        });
    });
    ui::row(ui, pal, false, "Scale", Some("Makes text and windows larger"), |ui| {
        egui::ComboBox::from_id_salt(format!("scale-{}", o.name)).selected_text(format!("{:.0} %", o.scale * 100.0)).show_ui(ui, |ui| {
            for s in [1.0, 1.25, 1.5, 1.75, 2.0] {
                if ui.selectable_label((s - o.scale).abs() < 0.01, format!("{:.0} %", s * 100.0)).clicked() && (s - o.scale).abs() >= 0.01 {
                    act = Some(Act::Change(Change::Scale(o.name.clone(), s)));
                }
            }
        });
    });
    if d.caps.transform {
        const TURNS: [&str; 4] = ["Landscape", "Portrait", "Landscape (flipped)", "Portrait (flipped)"];
        let now = (o.transform % 4) as usize;
        ui::row(ui, pal, false, "Orientation", None, |ui| {
            egui::ComboBox::from_id_salt(format!("turn-{}", o.name)).selected_text(TURNS[now]).show_ui(ui, |ui| {
                for (i, t) in TURNS.iter().enumerate() {
                    if ui.selectable_label(i == now, *t).clicked() && i != now {
                        act = Some(Act::Change(Change::Turn(o.name.clone(), i as u32)));
                    }
                }
            });
        });
    }
    if d.caps.vrr {
        const VRR: [(Option<u8>, &str); 4] =
            [(None, "As the desktop sets it"), (Some(2), "Full-screen games"), (Some(1), "Always"), (Some(0), "Off")];
        let now = VRR.iter().find(|(v, _)| *v == o.vrr).map(|(_, l)| *l).unwrap_or(VRR[0].1);
        ui::row(ui, pal, false, "Variable refresh rate", Some("Smoother games on a display made for it (FreeSync, G-Sync)"), |ui| {
            egui::ComboBox::from_id_salt(format!("vrr-{}", o.name)).selected_text(now).show_ui(ui, |ui| {
                for (v, l) in VRR {
                    if ui.selectable_label(v == o.vrr, l).clicked() && v != o.vrr {
                        act = Some(Act::Change(Change::Vrr(o.name.clone(), v)));
                    }
                }
            });
        });
    }
    if several {
        let sub = if o.primary {
            "The dock, notifications and the start menu are here"
        } else {
            "The dock, notifications and the start menu move here"
        };
        ui::row(ui, pal, false, "Main display", Some(sub), |ui| {
            if o.primary {
                ui.label(RichText::new("This is your main display").color(pal.dim));
            } else if ui::quiet(ui, pal, "Make this my main display").clicked() {
                act = Some(Act::MakeMain(o.clone()));
            }
        });
    }
    let mut tiling = d.tiling.split(',').any(|t| t.trim() == o.name);
    ui::row(ui, pal, false, "Tiling screen", Some("Windows tile automatically, Omarchy-style, with their own workspaces"), |ui| {
        if ui::switch(ui, pal, &mut tiling).changed() {
            act = Some(Act::Tiling(o.name.clone(), tiling));
        }
    });
    act
}

/// The box: every display to scale, numbered; drag to arrange, click to select. Disconnected ones in a strip along the bottom.
fn arrangement(ui: &mut egui::Ui, pal: &Palette, outs: &[Output], st: &mut State) -> Option<Act> {
    let off: Vec<&Output> = outs.iter().filter(|o| !o.enabled).collect();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, BOX_H + if off.is_empty() { 0.0 } else { OFF_STRIP }), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(10), pal.bg);
    let area = egui::Rect::from_min_max(rect.min + vec2(20.0, 20.0), pos2(rect.max.x - 20.0, rect.min.y + BOX_H - 20.0));

    // where the displays are drawn: their own places, or the dragged one where it would land
    let mut shown: Vec<Output> = outs.to_vec();
    let mut act = None;
    let released = ui.input(|i| i.pointer.any_released());
    if let Some(dr) = &mut st.drag {
        if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
            let lx = (p.x - dr.origin.x) / dr.scale - dr.grab.0;
            let ly = (p.y - dr.origin.y) / dr.scale - dr.grab.1;
            dr.want = (lx.round() as i32, ly.round() as i32);
        }
        let align = (12.0 / dr.scale).round() as i32;
        // the arrangement as it would be, held still around a display that is not dragged (it settles at 0,0 when let go)
        let held = shown.iter().find(|o| displays::placed(o) && o.name != dr.name).map(|o| (o.name.clone(), o.x, o.y));
        displays::arrange(&mut shown, &dr.name, dr.want, align);
        if let Some((n, x, y)) = held {
            if let Some(h) = shown.iter().find(|o| o.name == n).map(|o| (o.x, o.y)) {
                let (dx, dy) = (x - h.0, y - h.1);
                for o in shown.iter_mut().filter(|o| o.enabled) {
                    o.x += dx;
                    o.y += dy;
                }
            }
        }
        if released {
            let before = outs.iter().map(|o| (o.x, o.y)).collect::<Vec<_>>();
            let after = shown.iter().map(|o| (o.x, o.y)).collect::<Vec<_>>();
            if before != after {
                act = Some(Act::Change(Change::Arrange(dr.name.clone(), dr.want, align)));
            }
        }
    }
    if released {
        st.drag = None;
    }

    // the scale and origin: frozen while dragging, else fitted around the placed displays
    let placed: Vec<&Output> = shown.iter().filter(|o| displays::placed(o)).collect();
    let (scale, origin) = match &st.drag {
        Some(dr) => (dr.scale, dr.origin),
        None => {
            let rs: Vec<displays::Rect> = placed.iter().map(|o| displays::rect_of(o)).collect();
            let (x0, y0) = (rs.iter().map(|r| r.x).min().unwrap_or(0), rs.iter().map(|r| r.y).min().unwrap_or(0));
            let (x1, y1) = (rs.iter().map(|r| r.x + r.w).max().unwrap_or(1), rs.iter().map(|r| r.y + r.h).max().unwrap_or(1));
            let (bw, bh) = ((x1 - x0).max(1) as f32, (y1 - y0).max(1) as f32);
            let s = (area.width() / bw).min(area.height() / bh);
            let o = area.center() - vec2(bw * s / 2.0, bh * s / 2.0) - vec2(x0 as f32 * s, y0 as f32 * s);
            (s, o)
        }
    };
    let screen_rect = |o: &Output| {
        let r = displays::rect_of(o);
        egui::Rect::from_min_size(origin + vec2(r.x as f32 * scale, r.y as f32 * scale), vec2(r.w as f32 * scale, r.h as f32 * scale))
    };

    let selected = st.selected.clone();
    let tile = |ui: &mut egui::Ui, o: &Output, r: egui::Rect, dark: bool| -> egui::Response {
        let sel = selected.as_deref() == Some(o.name.as_str());
        let (fill, text) = match (sel, dark) {
            (true, _) => (if dark { pal.accent.gamma_multiply(0.55) } else { pal.accent }, pal.on_accent()),
            (false, true) => (Color32::from_gray(48), pal.dim),
            (false, false) => (pal.hover, pal.text),
        };
        let r = r.shrink(1.5);
        painter.rect_filled(r, CornerRadius::same(6), fill);
        painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, if sel { pal.accent } else { pal.border }), StrokeKind::Inside);
        let n = displays::number_of(outs, &o.name);
        let size = (r.height() * 0.45).clamp(12.0, 44.0);
        painter.text(r.center(), egui::Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(size), text);
        ui.interact(r, ui.id().with(("display", &o.name)), Sense::click_and_drag())
    };
    let mut pick: Option<String> = None;
    let mut drag_from: Option<(String, egui::Pos2, egui::Rect)> = None;
    // the placed displays, then copies over what they copy (a little down and right, so both can be clicked)
    for o in &placed {
        let r = screen_rect(o);
        let resp = tile(ui, o, r, false);
        if resp.clicked() {
            pick = Some(o.name.clone());
        }
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                drag_from = Some((o.name.clone(), p, r));
            }
        }
    }
    for o in shown.iter().filter(|o| o.enabled && o.mirror_of.is_some()) {
        let Some(src) = placed.iter().find(|p| Some(&p.name) == o.mirror_of.as_ref()) else { continue };
        let r = screen_rect(src).translate(vec2(10.0, 10.0));
        if tile(ui, o, r, false).clicked() {
            pick = Some(o.name.clone());
        }
    }
    // disconnected: smaller and grey, along the bottom; a click selects (Connect is among its settings)
    let mut x = rect.max.x - 16.0;
    for o in off.iter().rev() {
        let (w, h) = o.logical();
        let th = OFF_STRIP - 22.0;
        let tw = (th * w.max(1) as f32 / h.max(1) as f32).clamp(th * 0.6, th * 2.4);
        let r = egui::Rect::from_min_size(pos2(x - tw, rect.min.y + BOX_H + 4.0), vec2(tw, th));
        x -= tw + 10.0;
        if tile(ui, o, r, true).on_hover_text(format!("{} is disconnected", o.name)).clicked() {
            pick = Some(o.name.clone());
        }
    }
    if !off.is_empty() {
        painter.text(
            pos2(rect.min.x + 20.0, rect.min.y + BOX_H + OFF_STRIP / 2.0 - 4.0),
            egui::Align2::LEFT_CENTER,
            "Disconnected",
            egui::FontId::proportional(13.0),
            pal.dim,
        );
    }
    if let Some(n) = pick {
        st.selected = Some(n);
    }
    if let Some((name, p, r)) = drag_from {
        let grab = ((p.x - r.min.x) / scale, (p.y - r.min.y) / scale);
        let o = outs.iter().find(|o| o.name == name).map(|o| (o.x as i32, o.y as i32)).unwrap_or_default();
        st.selected = Some(name.clone());
        st.drag = Some(Drag { name, grab, scale, origin, want: o });
    }
    if st.drag.is_some() {
        ui.ctx().request_repaint();
    }
    act
}

/// "Keep these display settings?" while a change waits for an answer; it goes when the change is kept, put back, or failed.
pub fn prompt(ctx: &egui::Context, pal: &Palette, st: &mut State) {
    let Some(p) = &st.prompt else { return };
    if p.answer.load(Ordering::Acquire) != WAITING {
        st.prompt = None;
        return;
    }
    let left = p.keep.left(Instant::now());
    let mut answer = None;
    egui::Window::new("Keep these display settings?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(format!("The previous display settings come back in {left} s."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui::action(ui, pal, "Keep changes").clicked() {
                    answer = Some(KEPT);
                }
                if ui::quiet(ui, pal, "Revert").clicked() {
                    answer = Some(PUT_BACK);
                }
            });
        });
    if let Some(a) = answer {
        let _ = p.answer.compare_exchange(WAITING, a, Ordering::AcqRel, Ordering::Acquire);
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
}
