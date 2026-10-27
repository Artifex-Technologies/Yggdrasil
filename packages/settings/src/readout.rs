//! What the Graphics card and Processor pages share: a reading with its small graph, a slider row that fits a narrow window,
//! and the pop-out windows (`yggdrasil-settings --gpu-window` / `--cpu-window`): the same panel in a window and process of its
//! own, which stays open after Settings is closed.

use std::process::{Command, Stdio};

use eframe::egui::{self, pos2, vec2, Margin, RichText, Sense, Shape, Stroke};
use yggdrasil_ui::chrome;
use yggdrasil_ui::theme::{self, Palette};

use crate::{cpu, gpu, sys, ui};

/// Two minutes of readings, one a second.
pub const KEEP: usize = 120;

/// A row with a slider: on the right as usual, or on a line of its own under the text when the panel is narrow (a pop-out
/// window), so the title never wraps letter by letter.
pub fn slider_row(ui: &mut egui::Ui, pal: &Palette, first: bool, title: &str, sub: &str, control: impl FnOnce(&mut egui::Ui)) {
    if ui.available_width() >= 600.0 {
        return ui::row(ui, pal, first, title, Some(sub), control);
    }
    ui::row(ui, pal, first, title, Some(sub), |_| {});
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 34.0), egui::Layout::right_to_left(egui::Align::Center), control);
    ui.add_space(4.0);
}

/// One reading: its name, its value (and a short detail after it) on the left, its last two minutes as a small graph on the
/// right. The graph always fills its box: until two minutes of readings are in, the time across it is shorter (hover says
/// how long ago a point was). Each graph has its own fixed scale (shown on hover), never a second axis.
#[allow(clippy::too_many_arguments)]
pub fn metric<T>(ui: &mut egui::Ui, pal: &Palette, first: bool, name: &str, value: &str, detail: &str, h: &[T], get: impl Fn(&T) -> f32, lo: f32, hi: f32, unit: &str) {
    if !first {
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.left()..=r.right(), r.top(), Stroke::new(1.0, pal.border));
    }
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 46.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_height(46.0);
        ui.allocate_ui_with_layout(vec2(150.0, 40.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(150.0);
            ui.label(RichText::new(name).size(12.0).color(pal.dim));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.label(RichText::new(value).size(15.0).color(pal.text));
                if !detail.is_empty() {
                    ui.add(egui::Label::new(RichText::new(detail).size(11.0).color(pal.dim)).truncate());
                }
            });
        });
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let painter = ui.painter_at(rect.expand(2.0));
        painter.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, pal.border));
        let h = &h[h.len().saturating_sub(KEEP)..];
        if h.len() < 2 {
            return;
        }
        let step = rect.width() / (h.len() - 1) as f32;
        let at = |i: usize, s: &T| pos2(rect.left() + i as f32 * step, rect.bottom() - ((get(s) - lo) / (hi - lo)).clamp(0.0, 1.0) * rect.height());
        let line: Vec<egui::Pos2> = h.iter().enumerate().map(|(i, s)| at(i, s)).collect();
        // a light fill under the line (drawn as thin columns: the area is not convex)
        let fill = pal.accent.gamma_multiply(0.18);
        for w in line.windows(2) {
            painter.add(Shape::convex_polygon(vec![w[0], w[1], pos2(w[1].x, rect.bottom()), pos2(w[0].x, rect.bottom())], fill, Stroke::NONE));
        }
        painter.add(Shape::line(line.clone(), Stroke::new(2.0, pal.accent)));
        if let Some(p) = resp.hover_pos() {
            let i = (((p.x - rect.left()) / step).round().max(0.0) as usize).min(h.len() - 1);
            let q = line[i];
            painter.vline(q.x, rect.y_range(), Stroke::new(1.0, pal.dim));
            painter.circle(q, 4.0, pal.accent, Stroke::new(2.0, pal.field));
            let ago = h.len() - 1 - i;
            let v = get(&h[i]);
            let when = if ago == 0 { "now".to_string() } else { format!("{ago} s ago") };
            let shown = if v < 10.0 && v.fract() != 0.0 { format!("{v:.1}") } else { format!("{v:.0}") };
            resp.on_hover_text_at_pointer(format!("{when}: {shown}{unit}   (scale {lo:.0}-{hi:.0}{unit})"));
        }
    });
}

/// Which page a pop-out window shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Gpu,
    Cpu,
}

impl Kind {
    fn arg(self) -> &'static str {
        match self { Kind::Gpu => "--gpu-window", Kind::Cpu => "--cpu-window" }
    }
    fn title(self) -> &'static str {
        match self { Kind::Gpu => "Graphics card", Kind::Cpu => "Processor" }
    }
    fn app_id(self) -> &'static str {
        match self { Kind::Gpu => "yggdrasil-gpu", Kind::Cpu => "yggdrasil-cpu" }
    }
    /// The pop-out asked for on the command line, if any.
    pub fn from_args() -> Option<Kind> {
        std::env::args().find_map(|a| [Kind::Gpu, Kind::Cpu].into_iter().find(|k| k.arg() == a))
    }
}

/// Opens a pop-out window: Settings started again as its own process, in a process group of its own, so it stays open when
/// Settings is closed.
pub fn pop_out(kind: Kind) {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().ok().filter(|p| p.exists()).map(|p| p.into_os_string()).unwrap_or_else(|| "yggdrasil-settings".into());
    if let Ok(mut child) = Command::new(exe).arg(kind.arg()).process_group(0).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

enum Body {
    Gpu(gpu::Panel),
    Cpu(cpu::Panel),
}

struct Window {
    kind: Kind,
    body: Body,
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        let dark = theme::system_dark();
        theme::apply(&ctx, dark, accent);
        let pal = theme::palette_for(dark, accent);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 16, right: 8, top: 6, bottom: 12 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.kind.title()).size(16.0).strong());
                    chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                    chrome::window_controls(ui, &pal);
                });
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width((ui.available_width() - 14.0).max(0.0));
                    match &mut self.body {
                        Body::Gpu(p) => p.show(ui, &pal, &ctx),
                        Body::Cpu(p) => p.show(ui, &pal, &ctx),
                    }
                });
            });
        chrome::outline(&ctx, &pal);
    }
}

/// The pop-out's own `main`. Only one of each kind at a time: a second start brings the first to the front and ends.
pub fn window_main(kind: Kind) -> eframe::Result {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let lock = std::fs::File::create(format!("{dir}/{}-window.lock", kind.app_id())).ok();
    if lock.as_ref().is_some_and(|f| f.try_lock().is_err()) {
        let _ = sys::act("hyprctl", &["dispatch", &format!(r#"hl.dsp.focus({{ window = "class:{}" }})"#, kind.app_id())]);
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 760.0])
            .with_min_inner_size([380.0, 420.0])
            .with_title(kind.title())
            .with_app_id(kind.app_id())
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        kind.title(),
        options,
        Box::new(move |cc| {
            let body = match kind { Kind::Gpu => Body::Gpu(gpu::Panel::new(&cc.egui_ctx)), Kind::Cpu => Body::Cpu(cpu::Panel::new(&cc.egui_ctx)) };
            Ok(Box::new(Window { kind, body }))
        }),
    )
    // the lock is held until here
}
