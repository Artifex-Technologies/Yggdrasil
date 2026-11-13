//! The picker: the map (click or tap a place, drag to move, pinch, Ctrl+wheel, double-click or the buttons to zoom) with the
//! UTC offsets along its foot, and the list beside it (below it when the room is narrow), with its search box.

use std::sync::Arc;

use eframe::egui::{
    self, text::LayoutJob, vec2, Align, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions,
};
use yggdrasil_ui::theme::Palette;

use crate::catalog::{utc_offset, Catalog};
use crate::draw::{self, Colors, View, LAT_BOTTOM, LAT_TOP};
use crate::world::World;

/// How the picker behaves where it is shown.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Off: everything shows, nothing can be changed (Settings, while the time zone is set automatically).
    pub enabled: bool,
    /// Local times as 14:05 (else 2:05 PM).
    pub clock_24h: bool,
    /// The list's height when it is below the map.
    pub list_height: f32,
}

impl Default for Options {
    fn default() -> Self {
        Options { enabled: true, clock_24h: true, list_height: 300.0 }
    }
}

#[derive(Clone, PartialEq)]
struct Drawn {
    view: View,
    size: [usize; 2],
    ppp: f32,
    colors: Colors,
    band: Option<i32>,
}

pub struct Picker {
    catalog: Arc<Catalog>,
    view: View,
    search: String,
    texture: Option<(Drawn, TextureHandle)>,
    /// The zone chosen when last shown: the list scrolls to it and the map follows it when it changes.
    shown: Option<String>,
}

const ROW: f32 = 46.0;
const HEADER: f32 = 30.0;
const AXIS: f32 = 38.0;

impl Picker {
    pub fn new(catalog: Arc<Catalog>) -> Picker {
        Picker { catalog, view: View::default(), search: String::new(), texture: None, shown: None }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Shows the picker with `current` chosen; returns the zone picked (a tz database name), if one was.
    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette, current: &str, opts: Options) -> Option<String> {
        let changed = self.shown.as_deref() != Some(current);
        self.shown = Some(current.to_string());
        let w = ui.available_width();
        let beside = w >= 860.0;
        let list_w = if beside { (w * 0.30).clamp(300.0, 400.0) } else { w };
        let map_w = if beside { w - list_w - 18.0 } else { w };
        let map_h = (map_w * (LAT_TOP - LAT_BOTTOM) / 360.0).round();
        let mut picked = None;
        if beside {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| picked = picked.take().or(self.map(ui, pal, current, vec2(map_w, map_h), changed, &opts)));
                ui.add_space(18.0 - ui.spacing().item_spacing.x);
                ui.vertical(|ui| picked = picked.take().or(self.list(ui, pal, current, vec2(list_w, map_h + AXIS), changed, &opts)));
            });
        } else {
            picked = self.map(ui, pal, current, vec2(map_w, map_h), changed, &opts);
            ui.add_space(10.0);
            picked = picked.or(self.list(ui, pal, current, vec2(list_w, opts.list_height), changed, &opts));
        }
        picked.filter(|p| p != current)
    }

    fn colors(pal: &Palette) -> Colors {
        let light = pal.bg.r() > 128;
        let mix = |a: Color32, b: Color32, t: f32| {
            let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
            Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
        };
        Colors {
            dots: if light { pal.accent.gamma_multiply(0.16) } else { pal.accent.gamma_multiply(0.14) },
            land: if light { mix(pal.bg, pal.accent, 0.22) } else { mix(pal.strip, pal.accent, 0.30) },
            lines: if light { pal.accent.gamma_multiply(0.20) } else { pal.text.gamma_multiply(0.13) },
            highlight: pal.highlight,
            light,
        }
    }

    fn map(&mut self, ui: &mut egui::Ui, pal: &Palette, current: &str, size: egui::Vec2, changed: bool, opts: &Options) -> Option<String> {
        let world = World::get();
        let cat = self.catalog.clone();
        let cur = cat.index(current);
        let sense = if opts.enabled { Sense::click_and_drag() } else { Sense::hover() };
        let (rect, resp) = ui.allocate_exact_size(size, sense);
        let (w, h) = (rect.width(), rect.height());
        let local = |p: Pos2| (p.x - rect.left(), p.y - rect.top());
        let mut picked = None;
        if opts.enabled {
            if resp.dragged() {
                let d = resp.drag_delta();
                let s = self.view.scale(w);
                self.view.lon -= d.x / s;
                self.view.lat += d.y / s;
                self.view = self.view.clamp(w, h);
            }
            if let Some(p) = resp.hover_pos() {
                let (x, y) = local(p);
                // pinching and Ctrl+wheel (the plain wheel scrolls the page around the map)
                let mut z = ui.input(|i| i.zoom_delta());
                if resp.double_clicked() {
                    z *= 2.0;
                }
                if (z - 1.0).abs() > 1e-4 {
                    self.view = self.view.zoom_at(w, h, z, x, y);
                }
                if resp.clicked() && !resp.double_clicked() {
                    let (lon, lat) = self.view.to_deg(w, h, x, y);
                    picked = cat.nearest(lon, lat, world.offset_at(lon, lat)).map(|i| cat.zones[i].id.clone());
                }
            }
        }
        // a zone chosen in the list, off the zoomed-in map: the map moves to it
        if changed {
            if let Some((lon, lat)) = cur.and_then(|i| cat.zones[i].at) {
                let (x, y) = self.view.to_px(w, h, lon, lat);
                if !(0.0..=w).contains(&x) || !(0.0..=h).contains(&y) {
                    self.view = View { lon, lat, ..self.view }.clamp(w, h);
                }
            }
        }
        self.view = self.view.clamp(w, h);

        // the picture, drawn again only when what it shows changed
        let ppp = ui.ctx().pixels_per_point();
        let shown_zone = picked.as_deref().and_then(|p| cat.index(p)).or(cur);
        let want = Drawn {
            view: self.view,
            size: [(w * ppp).round() as usize, (h * ppp).round() as usize],
            ppp,
            colors: Self::colors(pal),
            band: shown_zone.map(|i| cat.zones[i].standard).filter(|_| shown_zone.is_some_and(|i| cat.zones[i].id != "UTC")),
        };
        if self.texture.as_ref().is_none_or(|(d, _)| *d != want) {
            let img = draw::render(world, &want.view, want.size[0], want.size[1], ppp, &want.colors, want.band);
            match &mut self.texture {
                Some((d, t)) => {
                    t.set(img, TextureOptions::LINEAR);
                    *d = want.clone();
                }
                None => self.texture = Some((want.clone(), ui.ctx().load_texture("tz-map", img, TextureOptions::LINEAR))),
            }
        }
        let light = want.colors.light;
        let painter = ui.painter_at(rect.expand(2.0));
        let radius = CornerRadius::same(12);
        painter.rect_filled(rect, radius, if light { Color32::WHITE } else { pal.strip });
        if let Some((_, t)) = &self.texture {
            painter.add(
                egui::epaint::RectShape::filled(rect, radius, Color32::WHITE)
                    .with_texture(t.id(), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))),
            );
        }
        painter.rect_stroke(rect, radius, Stroke::new(1.0, pal.border), StrokeKind::Inside);
        let to_screen = |lon: f32, lat: f32| {
            let (x, y) = self.view.to_px(w, h, lon, lat);
            Pos2::new(rect.left() + x, rect.top() + y)
        };
        let hi = pal.highlight;
        // the place chosen: a ring, as in the band's colour
        if let Some(at) = shown_zone.and_then(|i| cat.zones[i].at) {
            let p = to_screen(at.0, at.1);
            if rect.contains(p) {
                painter.circle_filled(p, 13.0, hi.gamma_multiply(0.16));
                painter.circle_stroke(p, 7.5, Stroke::new(2.0, hi));
                painter.circle_filled(p, 2.6, hi);
            }
        }
        // the place under the pointer (a mouse; a tap picks at once)
        let hovered = resp.hover_pos().filter(|_| opts.enabled && !resp.dragged()).and_then(|p| {
            let (x, y) = local(p);
            let (lon, lat) = self.view.to_deg(w, h, x, y);
            cat.nearest(lon, lat, world.offset_at(lon, lat))
        });
        if let Some(at) = hovered.filter(|&i| Some(i) != shown_zone).and_then(|i| cat.zones[i].at) {
            let p = to_screen(at.0, at.1);
            painter.circle_stroke(p, 6.0, Stroke::new(1.5, pal.text.gamma_multiply(0.7)));
        }
        if resp.hovered() && opts.enabled {
            ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Crosshair });
        }
        // the label: what is under the pointer, else what is chosen
        if let Some(i) = hovered.or(shown_zone) {
            let z = &cat.zones[i];
            let g = &cat.groups[cat.group_of[i]];
            let title = if g.named || z.id == "UTC" { g.name.clone() } else { format!("{}, {}", z.city, z.country) };
            let mono = format!("{}  {}  {}", z.id, utc_offset(z.now), local_time(&z.id, opts.clock_24h));
            let max = (w - 120.0).max(120.0);
            let t1 = one_line(ui, &title, FontId::proportional(14.0), pal.text, max);
            let t2 = one_line(ui, &mono, FontId::monospace(12.0), if hovered.is_some() { pal.dim } else { hi }, max);
            let bw = t1.size().x.max(t2.size().x) + 30.0;
            // bottom left: the South Pacific, where nobody's place is hidden
            let b = Rect::from_min_size(rect.left_bottom() + vec2(12.0, -60.0), vec2(bw, 48.0));
            let fill = if light { Color32::from_white_alpha(235) } else { pal.field.gamma_multiply(0.92) };
            painter.rect_filled(b, CornerRadius::same(8), fill);
            painter.rect_stroke(b, CornerRadius::same(8), Stroke::new(1.0, pal.border), StrokeKind::Inside);
            painter.rect_filled(Rect::from_min_size(b.min, vec2(3.0, b.height())), CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, pal.accent);
            painter.galley(b.min + vec2(16.0, 7.0), t1, pal.text);
            painter.galley(b.min + vec2(16.0, 27.0), t2, pal.text);
        }
        // zoom buttons
        if opts.enabled {
            let mut z = 1.0;
            for (k, (label, factor)) in [("+", 2.0f32), ("\u{2212}", 0.5)].into_iter().enumerate() {
                let c = Pos2::new(rect.right() - 24.0, rect.bottom() - 58.0 + k as f32 * 34.0);
                let r = Rect::from_center_size(c, vec2(28.0, 28.0));
                let off = factor < 1.0 && self.view.zoom <= 1.0;
                let b = ui.interact(r, ui.id().with(("tz-zoom", k)), if off { Sense::hover() } else { Sense::click() });
                let fill = if b.hovered() && !off {
                    pal.hover
                } else if light {
                    Color32::WHITE
                } else {
                    pal.field
                };
                painter.circle_filled(c, 14.0, fill);
                painter.circle_stroke(c, 14.0, Stroke::new(1.0, pal.border));
                painter.text(
                    c,
                    Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(17.0),
                    if off { pal.dim.gamma_multiply(0.5) } else { pal.text },
                );
                if b.clicked() {
                    z = factor;
                }
            }
            if z != 1.0 {
                self.view = self.view.zoom_at(w, h, z, w / 2.0, h / 2.0);
            }
        } else {
            painter.rect_filled(rect, radius, if light { Color32::from_white_alpha(110) } else { Color32::from_black_alpha(110) });
        }
        self.axis(ui, pal, rect, shown_zone.map(|i| cat.zones[i].standard));
        picked
    }

    /// The UTC offsets under the map, -12 to +12 hours where the map shows them, the chosen one in the highlight colour.
    fn axis(&self, ui: &mut egui::Ui, pal: &Palette, map: Rect, chosen: Option<i32>) {
        let (rect, _) = ui.allocate_exact_size(vec2(map.width(), AXIS), Sense::hover());
        let painter = ui.painter_at(rect.expand(4.0));
        let (w, h) = (map.width(), map.height());
        let x_of = |hours: f32| map.left() + self.view.to_px(w, h, hours * 15.0, 0.0).0;
        let y = rect.top() + 12.0;
        let (x0, x1) = (x_of(-12.0).max(map.left()), x_of(12.0).min(map.right()));
        painter.line_segment([Pos2::new(x0, y), Pos2::new(x1, y)], Stroke::new(1.0, pal.border));
        // the chosen offset where the map shows it (+13 and +14 lie past the date line: at the end of the axis)
        let chosen_x = chosen.map(|s| x_of((s as f32 / 3600.0).min(12.0))).filter(|x| *x >= map.left() - 1.0 && *x <= map.right() + 1.0);
        let every = if self.view.zoom >= 2.5 { 1 } else { 3 };
        let font = FontId::monospace(11.0);
        for hrs in -12..=12 {
            let x = x_of(hrs as f32);
            if x < map.left() - 1.0 || x > map.right() + 1.0 {
                continue;
            }
            let p = Pos2::new(x, y);
            painter.circle_filled(p, 2.6, pal.bg);
            painter.circle_stroke(p, 2.6, Stroke::new(1.0, pal.dim.gamma_multiply(0.8)));
            let near = chosen_x.is_some_and(|c| (c - x).abs() < 34.0);
            if hrs % every == 0 && !near {
                let t = if hrs == 0 { "0".to_string() } else { format!("{hrs:+}") };
                let align = if x < map.left() + 10.0 {
                    Align2::LEFT_TOP
                } else if x > map.right() - 10.0 {
                    Align2::RIGHT_TOP
                } else {
                    Align2::CENTER_TOP
                };
                painter.text(Pos2::new(x, y + 8.0), align, t, font.clone(), pal.dim);
            }
        }
        if let (Some(x), Some(s)) = (chosen_x, chosen) {
            let p = Pos2::new(x, y);
            painter.circle_filled(p, 7.0, pal.highlight.gamma_multiply(0.22));
            painter.circle_filled(p, 4.2, pal.highlight);
            let label = utc_offset(s).trim_start_matches("UTC").to_string();
            let align = if x < map.left() + 30.0 {
                Align2::LEFT_TOP
            } else if x > map.right() - 30.0 {
                Align2::RIGHT_TOP
            } else {
                Align2::CENTER_TOP
            };
            painter.text(Pos2::new(x, y + 8.0), align, label, FontId::monospace(12.0), pal.highlight);
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, pal: &Palette, current: &str, size: egui::Vec2, changed: bool, opts: &Options) -> Option<String> {
        let cat = self.catalog.clone();
        let cur = cat.index(current);
        let cur_group = cur.map(|i| cat.group_of[i]);
        let mut picked = None;
        ui.set_width(size.x);
        let light = pal.bg.r() > 128;
        let search = egui::Frame::new()
            .fill(if light { Color32::WHITE } else { pal.strip })
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.add_enabled(
                    opts.enabled,
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search: a city, a country or UTC-07")
                        .desired_width(size.x - 22.0)
                        .frame(egui::Frame::NONE),
                )
            });
        let (search, search_h) = (search.inner, search.response.rect.height());
        let rows = cat.search(&self.search);
        if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.search.trim().is_empty() {
            picked = rows.first().map(|&(_, m)| cat.zones[m].id.clone());
        }
        ui.add_space(6.0);
        // the rows, under a heading for each offset
        // (top, the row's group and member or None for a heading, the offset)
        type Item = (f32, Option<(usize, usize)>, i32);
        let mut items: Vec<Item> = vec![];
        let mut y = 0.0;
        let mut last = None;
        for &(g, m) in &rows {
            let off = cat.groups[g].standard;
            if last != Some(off) {
                items.push((y, None, off));
                y += HEADER;
                last = Some(off);
            }
            items.push((y, Some((g, m)), off));
            y += ROW;
        }
        let total = y;
        let list_h = (size.y - search_h - 6.0 - ui.spacing().item_spacing.y).max(80.0);
        let (outer, _) = ui.allocate_exact_size(vec2(size.x, list_h), Sense::hover());
        ui.painter().rect_filled(outer, CornerRadius::same(12), if light { Color32::WHITE } else { pal.strip });
        ui.painter().rect_stroke(outer, CornerRadius::same(12), Stroke::new(1.0, pal.border), StrokeKind::Inside);
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(outer.shrink2(vec2(6.0, 6.0))));
        let scroll_to = (changed || self.search.is_empty() && search.changed())
            .then(|| items.iter().find(|(_, r, _)| r.is_some_and(|(g, _)| Some(g) == cur_group)).map(|(y, ..)| *y))
            .flatten();
        if rows.is_empty() {
            inner.add_space(10.0);
            inner.label(egui::RichText::new("No time zone matches").color(pal.dim));
        }
        egui::ScrollArea::vertical().id_salt("tz-list").auto_shrink([false, false]).show_viewport(&mut inner, |ui, viewport| {
            let width = ui.available_width();
            let (all, _) = ui.allocate_exact_size(vec2(width, total), Sense::hover());
            let origin = all.min;
            if let Some(y) = scroll_to {
                ui.scroll_to_rect(Rect::from_min_size(origin + vec2(0.0, y), vec2(width, ROW)), Some(Align::Center));
            }
            let first = items.partition_point(|(y, ..)| *y + ROW < viewport.min.y);
            for &(y, row, off) in items[first.saturating_sub(1)..].iter().take_while(|(y, ..)| *y <= viewport.max.y) {
                let r = Rect::from_min_size(origin + vec2(0.0, y), vec2(width, if row.is_some() { ROW } else { HEADER }));
                let p = ui.painter();
                let Some((g, m)) = row else {
                    p.text(r.left_bottom() + vec2(8.0, -7.0), Align2::LEFT_BOTTOM, utc_offset(off), FontId::monospace(11.5), pal.dim);
                    continue;
                };
                let gr = &cat.groups[g];
                let chosen = cur_group == Some(g);
                // the zone in use is what its row shows, unless a search found another city of it
                let shows = if chosen && self.search.trim().is_empty() { cur.unwrap_or(m) } else { m };
                let resp = ui.interact(r, ui.id().with(("tz-row", g)), if opts.enabled { Sense::click() } else { Sense::hover() });
                let body = r.shrink2(vec2(0.0, 2.0));
                if chosen {
                    p.rect_filled(body, CornerRadius::same(8), pal.select);
                    p.rect_filled(
                        Rect::from_min_size(body.min + vec2(0.0, 8.0), vec2(3.0, body.height() - 16.0)),
                        CornerRadius::same(2),
                        pal.highlight,
                    );
                } else if resp.hovered() && opts.enabled {
                    p.rect_filled(body, CornerRadius::same(8), pal.hover);
                }
                let time = local_time(&cat.zones[shows].id, opts.clock_24h);
                let tg = one_line(ui, &time, FontId::monospace(12.0), pal.dim, 90.0);
                let text_w = width - 30.0 - tg.size().x;
                let t1 = one_line(ui, &gr.name, FontId::proportional(14.0), pal.text, text_w);
                let t2 = one_line(ui, &cat.row_detail(g, shows, 4), FontId::proportional(12.0), pal.dim, text_w);
                p.galley(body.min + vec2(14.0, 5.0), t1, pal.text);
                p.galley(body.min + vec2(14.0, 24.0), t2, pal.dim);
                p.galley(Pos2::new(body.right() - 10.0 - tg.size().x, body.center().y - tg.size().y / 2.0), tg, pal.dim);
                if resp.clicked() && (!chosen || shows != cur.unwrap_or(m)) {
                    picked = Some(cat.zones[shows].id.clone());
                }
            }
        });
        picked
    }
}

/// One line of text, cut with "…" when it is wider than `max`.
fn one_line(ui: &egui::Ui, text: &str, font: FontId, color: Color32, max: f32) -> Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The time now in a zone: "14:05" or "2:05 PM".
pub fn local_time(id: &str, h24: bool) -> String {
    let Ok(tz) = jiff::tz::TimeZone::get(id) else { return String::new() };
    let t = jiff::Timestamp::now().to_zoned(tz);
    if h24 {
        format!("{:02}:{:02}", t.hour(), t.minute())
    } else {
        let h = (t.hour() + 11) % 12 + 1;
        format!("{h}:{:02} {}", t.minute(), if t.hour() < 12 { "AM" } else { "PM" })
    }
}
