//! Minos draws its own title bar, as Vivaldi and Spotify do: the top strip (menu, theme, status)
//! doubles as the title bar, with the window buttons at its right end, so there is no separate
//! grey system title bar above it and no frame below. The buttons are macOS traffic lights in the
//! Windows order (minimize, maximize, close), matching the rest of the desktop.
//!
//! Without a system frame the window manager offers no edges to resize by, so `resize_edges`
//! turns the outer few pixels of the window into resize handles.

use eframe::egui::{self, pos2, vec2, Color32, CursorIcon, Rect, Sense, Stroke, ViewportCommand};
use eframe::egui::viewport::ResizeDirection;

use crate::theme::Palette;

const BUTTON: f32 = 13.0; // diameter of a traffic light
const GAP: f32 = 8.0;
const EDGE: f32 = 6.0; // width of the invisible resize handles

const MINIMIZE: Color32 = Color32::from_rgb(0xfe, 0xbc, 0x2e);
const MAXIMIZE: Color32 = Color32::from_rgb(0x28, 0xc8, 0x40);
const CLOSE: Color32 = Color32::from_rgb(0xff, 0x5f, 0x57);

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// The rest of the strip: drag to move the window, double-click to maximize or restore.
pub fn drag_area(ui: &mut egui::Ui, width: f32) {
    let (_, resp) = ui.allocate_exact_size(vec2(width.max(0.0), ui.available_height().max(30.0)), Sense::click_and_drag());
    if resp.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        let m = maximized(ui.ctx());
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!m));
    }
}

/// Width the window buttons take, for laying out the strip.
pub fn controls_width() -> f32 {
    3.0 * BUTTON + 2.0 * GAP + 10.0
}

/// Minimize, maximize and close, drawn as traffic lights; the symbol shows while hovering the group.
pub fn window_controls(ui: &mut egui::Ui, pal: &Palette) {
    let (area, group) = ui.allocate_exact_size(vec2(controls_width(), 30.0), Sense::hover());
    let hover_group = group.hovered() || ui.rect_contains_pointer(area);
    let ctx = ui.ctx().clone();
    let y = area.center().y;
    let buttons = [(MINIMIZE, "Minimize"), (MAXIMIZE, if maximized(&ctx) { "Restore" } else { "Maximize" }), (CLOSE, "Close")];
    for (i, (colour, tip)) in buttons.iter().enumerate() {
        let cx = area.left() + 5.0 + BUTTON / 2.0 + i as f32 * (BUTTON + GAP);
        let rect = Rect::from_center_size(pos2(cx, y), vec2(BUTTON, BUTTON));
        let resp = ui.interact(rect.expand(3.0), ui.id().with(("window-button", i)), Sense::click()).on_hover_text(*tip);
        let p = ui.painter();
        p.circle_filled(rect.center(), BUTTON / 2.0, *colour);
        p.circle_stroke(rect.center(), BUTTON / 2.0, Stroke::new(0.5, Color32::from_black_alpha(60)));
        if hover_group {
            let ink = Stroke::new(1.4, Color32::from_black_alpha(150));
            let c = rect.center();
            let r = BUTTON * 0.24;
            match i {
                0 => {
                    p.line_segment([c - vec2(r, 0.0), c + vec2(r, 0.0)], ink);
                }
                1 => {
                    p.line_segment([c - vec2(r, 0.0), c + vec2(r, 0.0)], ink);
                    p.line_segment([c - vec2(0.0, r), c + vec2(0.0, r)], ink);
                }
                _ => {
                    p.line_segment([c - vec2(r, r), c + vec2(r, r)], ink);
                    p.line_segment([c + vec2(-r, r), c + vec2(r, -r)], ink);
                }
            }
        }
        if resp.clicked() {
            match i {
                0 => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                1 => ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(&ctx))),
                _ => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
    }
    let _ = pal;
}

/// The outer EDGE pixels of the window resize it (not while maximized or full screen).
pub fn resize_edges(ctx: &egui::Context) {
    let (full, max) = ctx.input(|i| (i.viewport().fullscreen.unwrap_or(false), i.viewport().maximized.unwrap_or(false)));
    if full || max {
        return;
    }
    let screen = ctx.viewport_rect();
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
    let (l, r) = (pos.x - screen.left() < EDGE, screen.right() - pos.x < EDGE);
    let (t, b) = (pos.y - screen.top() < EDGE, screen.bottom() - pos.y < EDGE);
    let dir = match (l, r, t, b) {
        (true, _, true, _) => Some((ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest)),
        (_, true, true, _) => Some((ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast)),
        (true, _, _, true) => Some((ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest)),
        (_, true, _, true) => Some((ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast)),
        (true, _, _, _) => Some((ResizeDirection::West, CursorIcon::ResizeWest)),
        (_, true, _, _) => Some((ResizeDirection::East, CursorIcon::ResizeEast)),
        (_, _, true, _) => Some((ResizeDirection::North, CursorIcon::ResizeNorth)),
        (_, _, _, true) => Some((ResizeDirection::South, CursorIcon::ResizeSouth)),
        _ => None,
    };
    if let Some((d, cursor)) = dir {
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(d));
        }
    }
}

/// A hairline around the window, so its edge stays visible against a dark background now that the
/// system frame (and its shadow) is gone.
pub fn outline(ctx: &egui::Context, pal: &Palette) {
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let rect = ctx.viewport_rect().shrink(0.5);
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window-outline")))
        .rect_stroke(rect, 0.0, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
}
