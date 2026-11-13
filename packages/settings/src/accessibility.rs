//! Settings > Accessibility: what Yggdrasil can really do today, on Hyprland and in its own apps.
//!
//! - Text and control size, and Reduce motion (panel.conf: the egui apps within a second; Reduce motion also turns Hyprland's
//!   animations off, through the session's config, and Yggdrasil's compositor's).
//! - Zoom: Hyprland's zoom around the pointer (cursor:zoom_factor), the same one Super+Alt+= / - / 0 change.
//! - Pointer size (`ygg-theme cursor-size`: the compositor at once, programs as they start, everything at the next sign-in).
//! - Colour filters: a screen shader Settings writes (~/.config/yggdrasil/shaders/NAME.frag), named in accessibility.conf, which
//!   the session gives Hyprland at every start (decoration:screen_shader); Settings also sets it at once.
//! - Links to key repeat (Keyboard) and the on-screen keyboard (tablet mode, Displays).
//!
//! Left out because nothing can do them yet: sticky, slow and bounce keys (Hyprland has no such setting), high contrast
//! (Yggdrasil's palette has no high-contrast variant yet), mono audio (PipeWire needs a filter chain set up for it).

use std::path::PathBuf;

use eframe::egui;
use yggdrasil_ui::theme::Palette;

use crate::{loading, peripherals, sys, ui, App, Page};

/// The colour filters: (name in accessibility.conf, title, what it is for).
pub const FILTERS: [(&str, &str, &str); 5] = [
    ("none", "Off", ""),
    ("greyscale", "Greyscale", "Shades of grey only"),
    ("protanopia", "Red-green (protanopia)", "Reds made easier to tell from greens"),
    ("deuteranopia", "Green-red (deuteranopia)", "Greens made easier to tell from reds"),
    ("tritanopia", "Blue-yellow (tritanopia)", "Blues made easier to tell from yellows"),
];

/// The pointer sizes ygg-theme takes.
pub const CURSOR_SIZES: [u32; 5] = [24, 32, 48, 64, 96];

/// The GLSL for a filter (Hyprland's screen shaders: GLSL ES 3.0, the screen as `tex`). Colour-blind filters move what a
/// person with that kind of colour blindness would lose into colours they see (Daltonize: simulate in LMS, then shift the
/// difference).
pub fn shader(name: &str) -> Option<String> {
    let body = match name {
        "greyscale" => "    float g = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));\n    vec3 o = vec3(g);\n".to_string(),
        "protanopia" | "deuteranopia" | "tritanopia" => {
            let sim = match name {
                "protanopia" => "mat3(0.0, 0.0, 0.0, 2.02344, 1.0, 0.0, -2.52581, 0.0, 1.0)",
                "deuteranopia" => "mat3(1.0, 0.494207, 0.0, 0.0, 0.0, 0.0, 0.0, 1.24827, 1.0)",
                _ => "mat3(1.0, 0.0, -0.395913, 0.0, 1.0, 0.801109, 0.0, 0.0, 0.0)",
            };
            format!(
                "    mat3 toLms = mat3(17.8824, 3.45565, 0.0299566, 43.5161, 27.1554, 0.184309, 4.11935, 3.86714, 1.46709);\n\
                 \x20   mat3 toRgb = mat3(0.0809444479, -0.0102485335, -0.000365296938, -0.130504409, 0.0540193266, -0.00412161469, 0.116721066, -0.113614708, 0.693511405);\n\
                 \x20   vec3 sim = toRgb * ({sim} * (toLms * c.rgb));\n\
                 \x20   vec3 err = c.rgb - sim;\n\
                 \x20   vec3 shift = vec3(0.0, 0.7 * err.r + err.g, 0.7 * err.r + err.b);\n\
                 \x20   vec3 o = clamp(c.rgb + shift, 0.0, 1.0);\n"
            )
        }
        _ => return None,
    };
    Some(format!(
        "#version 300 es\n// Settings > Accessibility > Colour filters: {name} (written by Settings; changed there)\nprecision highp float;\nin vec2 v_texcoord;\nuniform sampler2D tex;\nout vec4 fragColor;\n\nvoid main() {{\n    vec4 c = texture(tex, v_texcoord);\n{body}    fragColor = vec4(o, c.a);\n}}\n"
    ))
}

fn conf_path() -> PathBuf {
    yggdrasil_conf::dirs::config().join("accessibility.conf")
}

/// accessibility.conf's `color_filter` ("none" when unset or unknown).
pub fn filter_in(text: &str) -> String {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == "color_filter")
        .map(|(_, v)| v.split('#').next().unwrap_or("").trim().to_string())
        .filter(|v| FILTERS.iter().any(|f| f.0 == v))
        .unwrap_or_else(|| "none".into())
}

/// Chooses a filter: its shader written (when it has one), then accessibility.conf, then Hyprland told at once.
pub fn set_filter(name: &str) -> Result<(), String> {
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    let dir = yggdrasil_conf::dirs::config();
    let path = dir.join("shaders").join(format!("{name}.frag"));
    if let Some(text) = shader(name) {
        std::fs::create_dir_all(dir.join("shaders")).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
    } else if name != "none" {
        return Err(format!("no colour filter called {name}"));
    }
    let tmp = conf_path().with_extension("tmp");
    std::fs::write(&tmp, format!("# Settings > Accessibility (the session reads it when Hyprland starts)\ncolor_filter = {name}\n"))
        .and_then(|_| std::fs::rename(&tmp, conf_path()))
        .map_err(|e| e.to_string())?;
    if let Some(yggdrasil_wm::Desk::Hyprland(h)) = sys::desk() {
        let shown = if name == "none" { String::new() } else { path.display().to_string() };
        h.eval(&format!("hl.config({{ decoration = {{ screen_shader = {shown:?} }} }})"));
    }
    Ok(())
}

/// `getoption cursor:zoom_factor`'s "float: N".
pub fn parse_float(text: &str) -> Option<f32> {
    text.lines().find_map(|l| l.trim().strip_prefix("float:")).and_then(|v| v.trim().parse().ok())
}

/// Hyprland's zoom around the pointer, now.
pub fn set_zoom(z: f32) -> Result<(), String> {
    match sys::desk() {
        Some(yggdrasil_wm::Desk::Hyprland(h)) => {
            h.eval(&format!("hl.config({{ cursor = {{ zoom_factor = {:.1} }} }})", z.clamp(1.0, 8.0)));
            Ok(())
        }
        _ => Err("Zoom works on Hyprland for now.".into()),
    }
}

/// What the page reads when it opens.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Hyprland is the compositor (zoom and colour filters work there; with a fixture: as if it were).
    pub hyprland: bool,
    pub zoom: f32,
    pub cursor_size: u32,
    pub filter: String,
    /// The on-screen keyboard (wvkbd) is installed.
    pub osk: bool,
}

pub fn load() -> State {
    let fixture = crate::fixture::get().is_some();
    let (hyprland, zoom) = match sys::desk() {
        Some(yggdrasil_wm::Desk::Hyprland(h)) => {
            (true, h.query("getoption cursor:zoom_factor").and_then(|t| parse_float(&t)).unwrap_or(1.0))
        }
        _ => (fixture, 1.0),
    };
    State {
        hyprland,
        zoom,
        cursor_size: sys::run("ygg-theme", &["cursor-size"]).and_then(|s| s.trim().parse().ok()).unwrap_or(24),
        filter: filter_in(&std::fs::read_to_string(conf_path()).unwrap_or_default()),
        osk: std::path::Path::new("/usr/bin/wvkbd-mobintl").exists(),
    }
}

impl App {
    pub(crate) fn page_accessibility(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(st) = self.data.access.clone() else { return loading(ui) };
        let mut conf = yggdrasil_conf::PanelConf::load();
        let mut conf_changed = false;
        let mut motion_changed = false;
        let mut zoom: Option<f32> = None;
        let mut size: Option<u32> = None;
        let mut filter: Option<String> = None;
        ui::group(ui, pal, Some("Vision"), |ui| {
            let sizes = ["100%", "125%", "150%", "200%"];
            let cur = match conf.text_scale {
                0..=112 => 0,
                113..=137 => 1,
                138..=174 => 2,
                _ => 3,
            };
            ui::row(
                ui,
                pal,
                true,
                "Text and control size",
                Some("Everything in the desktop's own apps (Settings, Minos, Calculator, the password box)"),
                |ui| {
                    if let Some(i) = ui::segmented(ui, pal, &sizes, cur) {
                        conf.text_scale = [100, 125, 150, 200][i];
                        conf_changed = true;
                    }
                },
            );
            ui::row(ui, pal, false, "Pointer size", None, |ui| {
                let labels: Vec<String> = CURSOR_SIZES.iter().map(|s| s.to_string()).collect();
                let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                let sel = CURSOR_SIZES.iter().position(|&s| s == st.cursor_size).unwrap_or(0);
                if let Some(i) = ui::segmented(ui, pal, &labels, sel) {
                    size = Some(CURSOR_SIZES[i]);
                }
            });
            if st.hyprland {
                ui::row(
                    ui,
                    pal,
                    false,
                    "Zoom",
                    Some("Magnifies around the pointer. Super+Alt+= zooms in, Super+Alt+- out, Super+Alt+0 off"),
                    |ui| {
                        let mut z = self.zoom_drag.unwrap_or(st.zoom);
                        // half steps, shown as they are (2.5×), as Super+Alt+= and - step
                        let r = ui
                            .scope(|ui| {
                                ui.spacing_mut().slider_width = (ui.available_width() * 0.55).clamp(120.0, 260.0);
                                let v = &mut ui.style_mut().visuals;
                                v.widgets.inactive.bg_fill = pal.hover;
                                v.selection.bg_fill = pal.accent;
                                ui.add(egui::Slider::new(&mut z, 1.0..=8.0).step_by(0.5).trailing_fill(true).fixed_decimals(1).suffix("×"))
                            })
                            .inner;
                        if r.changed() {
                            self.zoom_drag = Some((z * 2.0).round() / 2.0);
                        }
                        if r.drag_stopped() || (r.changed() && !r.dragged()) {
                            zoom = Some((z * 2.0).round() / 2.0);
                        }
                    },
                );
                let cur = FILTERS.iter().find(|f| f.0 == st.filter).unwrap_or(&FILTERS[0]);
                ui::row(
                    ui,
                    pal,
                    false,
                    "Colour filters",
                    Some(if cur.2.is_empty() { "For colour blindness, or to see the screen in grey" } else { cur.2 }),
                    |ui| {
                        egui::ComboBox::from_id_salt("colour-filter").selected_text(cur.1).width(220.0).show_ui(ui, |ui| {
                            for (id, title, _) in FILTERS {
                                if ui.selectable_label(id == st.filter, title).clicked() && id != st.filter {
                                    filter = Some(id.to_string());
                                }
                            }
                        });
                    },
                );
            }
        });
        if !st.hyprland {
            ui::footnote(ui, pal, "Zoom and colour filters work on Hyprland; Yggdrasil's own compositor does not offer them yet.");
        }
        ui::group(ui, pal, Some("Motion"), |ui| {
            ui::row(ui, pal, true, "Reduce motion", Some("No fades or slides in the desktop's own apps, and no window animations"), |ui| {
                if ui::switch(ui, pal, &mut conf.reduce_motion).changed() {
                    conf_changed = true;
                    motion_changed = true;
                }
            });
        });
        let mut open: Option<Page> = None;
        ui::group(ui, pal, Some("Keyboard"), |ui| {
            if ui::nav_row(ui, pal, true, "Key repeat", Some("How soon and how fast a held key repeats: under Keyboard"), |_| {}) {
                open = Some(Page::Peripherals);
            }
            if st.osk
                && ui::nav_row(
                    ui,
                    pal,
                    false,
                    "On-screen keyboard",
                    Some("Shows in tablet mode while no keyboard is plugged in: under Displays"),
                    |_| {},
                )
            {
                open = Some(Page::Displays);
            }
        });
        ui::footnote(
            ui,
            pal,
            "Not here yet: sticky, slow and bounce keys (Hyprland has no such setting), high contrast and mono audio. \
             Super+Alt+S starts or stops the screen reader (Orca) when it is installed.",
        );
        if conf_changed {
            self.change(ctx, move || {
                conf.save()?;
                // window animations come from the session's config: read again (Hyprland) so they follow
                if motion_changed {
                    if let Some(d) = sys::desk() {
                        d.reload()?;
                    }
                }
                Ok(())
            });
        }
        if let Some(n) = size {
            let arg = n.to_string();
            self.change(ctx, move || sys::act("ygg-theme", &["cursor-size", &arg]));
        }
        if let Some(z) = zoom {
            self.zoom_drag = None;
            self.change(ctx, move || set_zoom(z));
        }
        if let Some(f) = filter {
            self.change(ctx, move || set_filter(&f));
        }
        match open {
            Some(Page::Peripherals) => {
                self.rewind(Page::Peripherals);
                self.peri.sub = peripherals::Sub::Keyboard;
                self.open_page(Page::Peripherals, ctx);
            }
            Some(p) => self.open_page(p, ctx),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_is_read_from_accessibility_conf() {
        assert_eq!(filter_in(""), "none");
        assert_eq!(filter_in("# Settings\ncolor_filter = deuteranopia\n"), "deuteranopia");
        assert_eq!(filter_in("color_filter = greyscale # chosen\n"), "greyscale");
        assert_eq!(filter_in("color_filter = ../../x\n"), "none", "only the filters Settings knows");
    }

    #[test]
    fn every_filter_has_a_shader_for_hyprland() {
        assert_eq!(shader("none"), None);
        let mut seen = std::collections::HashSet::new();
        for (id, ..) in FILTERS.iter().skip(1) {
            let s = shader(id).unwrap();
            assert!(s.starts_with("#version 300 es\n"), "{id}: Hyprland's shaders are GLSL ES 3.0");
            assert!(s.contains("uniform sampler2D tex;") && s.contains("in vec2 v_texcoord;") && s.contains("fragColor = vec4(o, c.a);"));
            assert_eq!(s.matches('{').count(), s.matches('}').count(), "{id}");
            assert!(seen.insert(s), "{id} differs from the others");
        }
        assert_eq!(shader("../x"), None);
    }

    #[test]
    fn hyprlands_zoom_factor_reads() {
        assert_eq!(parse_float("float: 2.500000\nset: true\n"), Some(2.5));
        assert_eq!(parse_float("no such option"), None);
    }
}
