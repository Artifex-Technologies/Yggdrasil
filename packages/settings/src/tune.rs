//! The tuning service's answer (`tune get`, /usr/lib/yggdrasil/tune) and the rows the Processor and Graphics card pages share
//! for it: values tried for this session (Keep or Undo), kept ones (Defaults), and the crash guard switching tuning off.

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;

use crate::{sys, ui};

/// The graphics card's clocks: offsets in MHz, and the highest core clock (None: no cap).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuClocks {
    pub core: i64,
    pub mem: i64,
    pub cap: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tune {
    /// Whether the root service yggdrasil-tune runs (it applies, guards and reads back).
    pub service: bool,
    /// The turbo multipliers with 1..8 cores busy: in use now, the processor's own, kept, being tried.
    pub ratios: Option<Vec<i64>>,
    pub stock_ratios: Option<Vec<i64>>,
    pub kept_ratios: Option<Vec<i64>>,
    pub trial_ratios: Option<Vec<i64>>,
    /// Whether the multipliers can be changed at all (a K processor and a BIOS that allows it), and the base multiplier.
    pub programmable: bool,
    pub base: Option<i64>,
    pub ratio_range: (i64, i64),
    pub cpu_off: Option<String>,
    /// Long- and short-term power limits in watts: now, the BIOS's own, and the range offered.
    pub pl: (Option<i64>, Option<i64>),
    pub stock_pl: (Option<i64>, Option<i64>),
    pub power_range: (i64, i64),
    pub turbo: Option<bool>,
    /// The processor package's power now, in watts (the service measures it).
    pub power_w: Option<f64>,
    pub gpu: Option<GpuClocks>,
    pub max_core: Option<i64>,
    pub kept_gpu: Option<GpuClocks>,
    pub trial_gpu: Option<GpuClocks>,
    pub gpu_off: Option<String>,
    pub core_range: (i64, i64),
    pub mem_range: (i64, i64),
}

fn ints(v: &serde_json::Value) -> Option<Vec<i64>> {
    let a: Vec<i64> = v.as_array()?.iter().filter_map(|x| x.as_i64()).collect();
    (a.len() == 8).then_some(a)
}

fn clocks(v: &serde_json::Value) -> Option<GpuClocks> {
    v.as_object()?;
    Some(GpuClocks { core: v["core"].as_i64().unwrap_or(0), mem: v["mem"].as_i64().unwrap_or(0), cap: v["cap"].as_i64() })
}

fn range(v: &serde_json::Value, dflt: (i64, i64)) -> (i64, i64) {
    match (v[0].as_i64(), v[1].as_i64()) {
        (Some(a), Some(b)) if a < b => (a, b),
        _ => dflt,
    }
}

pub fn parse(json: &str) -> Option<Tune> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("kept")?;
    let s = |x: &serde_json::Value| x.as_str().filter(|t| !t.is_empty()).map(String::from);
    let now = &v["now"];
    Some(Tune {
        service: v["service"].as_bool().unwrap_or(false),
        ratios: ints(&now["cpu"]["ratios"]),
        stock_ratios: ints(&v["stock"]["ratios"]),
        kept_ratios: ints(&v["kept"]["cpu"]["ratios"]),
        trial_ratios: ints(&v["status"]["trial"]["cpu"]["ratios"]),
        programmable: now["cpu"]["programmable"].as_bool().unwrap_or(false),
        base: now["cpu"]["base"].as_i64().or(v["stock"]["base"].as_i64()),
        ratio_range: range(&v["ranges"]["ratio"], (8, 53)),
        cpu_off: s(&v["status"]["cpu_off"]),
        pl: (v["rapl"]["pl1"].as_i64(), v["rapl"]["pl2"].as_i64()),
        stock_pl: (v["stock"]["pl1"].as_i64(), v["stock"]["pl2"].as_i64()),
        power_range: range(&v["ranges"]["power"], (35, 250)),
        turbo: v["turbo"].as_bool(),
        power_w: now["power_w"].as_f64(),
        gpu: now["gpu"].as_object().map(|_| GpuClocks { core: now["gpu"]["core"].as_i64().unwrap_or(0), mem: now["gpu"]["mem"].as_i64().unwrap_or(0), cap: None }),
        max_core: now["gpu"]["max_core"].as_i64(),
        kept_gpu: clocks(&v["kept"]["gpu"]),
        trial_gpu: clocks(&v["status"]["trial"]["gpu"]),
        gpu_off: s(&v["status"]["gpu_off"]),
        core_range: range(&v["ranges"]["gpu_core"], (-300, 300)),
        mem_range: range(&v["ranges"]["gpu_mem"], (-1000, 2000)),
    })
}

pub fn get() -> Option<Tune> {
    sys::run("/usr/lib/yggdrasil/tune", &["get"]).and_then(|s| parse(&s))
}

/// Changes one multiplier and keeps the rest in order (never higher with more cores busy): the ones for fewer cores rise to it
/// if they were lower, the ones for more cores come down to it if they were higher.
pub fn set_ratio(ratios: &mut [i64], i: usize, v: i64) {
    ratios[i] = v;
    for r in ratios[..i].iter_mut() {
        *r = (*r).max(v);
    }
    for r in ratios[i + 1..].iter_mut() {
        *r = (*r).min(v);
    }
}

/// What the rows under a tuned section asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Try,
    Keep,
    Undo,
    Defaults,
    Reenable,
}

impl Act {
    /// `tune` arguments for one part ("cpu" or "gpu"); Try's own values are added by the caller.
    pub fn args(self, part: &str) -> Vec<String> {
        match self {
            Act::Try => vec!["try".into(), part.into()],
            Act::Keep => vec!["keep".into(), part.into()],
            Act::Undo => vec!["undo".into(), part.into()],
            Act::Defaults => vec!["reset".into(), part.into()],
            Act::Reenable => vec!["reenable".into()],
        }
    }
}

/// The guard's rows: switched off after a freeze or a driver error (and why), the values being tried this session (Keep or
/// Undo), and the buttons to try what was set above or go back to the defaults.
pub fn guard_rows(ui: &mut egui::Ui, pal: &Palette, off: Option<&str>, trying: bool, edited: bool, kept: bool, busy: bool) -> Option<Act> {
    let mut act = None;
    if let Some(why) = off {
        ui::row(ui, pal, false, "Switched off", Some(&format!("{why}. Your values are kept.")), |ui| {
            if ui.add_enabled(!busy, egui::Button::new("Turn back on")).clicked() {
                act = Some(Act::Reenable);
            }
            ui.label(RichText::new("Off").color(pal.amber).strong());
        });
    }
    if trying {
        ui::row(ui, pal, false, "Trying, for this session", Some("If it holds up in a game or a stress test, Keep makes it start with the computer. If the computer freezes, the next start uses your previous values."), |ui| {
            if ui.add_enabled(!busy, egui::Button::new("Undo")).clicked() {
                act = Some(Act::Undo);
            }
            if ui.add_enabled(!busy, egui::Button::new("Keep")).clicked() {
                act = Some(Act::Keep);
            }
        });
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(edited && !busy, egui::Button::new("Try")).clicked() {
            act = Some(Act::Try);
        }
        if ui.add_enabled((kept || trying) && !busy, egui::Button::new("Defaults")).clicked() {
            act = Some(Act::Defaults);
        }
        let hint = if edited { "Try applies it now, for this session" } else { "Move a slider, then Try" };
        ui.label(RichText::new(hint).size(11.5).color(pal.dim));
    });
    ui.add_space(6.0);
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_services_answer_parses() {
        let t = parse(r#"{"kept": {"cpu": {"ratios": [51,51,50,50,49,49,49,49]}, "gpu": {"core": 100, "mem": 500, "cap": null}, "power": {"pl1": null, "pl2": null}, "turbo": null},
            "status": {"trial": {"gpu": {"core": 150, "mem": 800, "cap": 1900}}, "cpu_off": null, "gpu_off": "The graphics driver reported an error"},
            "now": {"cpu": {"ratios": [50,50,48,48,47,47,47,47], "base": 36, "programmable": true}, "gpu": {"core": 150, "mem": 800, "max_core": 2160}, "power_w": 23.4},
            "stock": {"ratios": [50,50,48,48,47,47,47,47], "pl1": 95, "pl2": 125, "base": 36, "programmable": true},
            "rapl": {"pl1": 95, "pl2": 125}, "turbo": true, "ranges": {"gpu_core": [-300, 300], "gpu_mem": [-1000, 2000], "power": [35, 250], "ratio": [8, 53]}, "service": true}"#).unwrap();
        assert!(t.service && t.programmable);
        assert_eq!(t.ratios.as_deref(), Some(&[50, 50, 48, 48, 47, 47, 47, 47][..]));
        assert_eq!(t.kept_ratios.as_deref(), Some(&[51, 51, 50, 50, 49, 49, 49, 49][..]));
        assert_eq!(t.trial_ratios, None);
        assert_eq!(t.trial_gpu, Some(GpuClocks { core: 150, mem: 800, cap: Some(1900) }));
        assert_eq!(t.kept_gpu, Some(GpuClocks { core: 100, mem: 500, cap: None }));
        assert_eq!((t.pl, t.stock_pl, t.turbo, t.power_w), ((Some(95), Some(125)), (Some(95), Some(125)), Some(true), Some(23.4)));
        assert_eq!((t.ratio_range, t.core_range, t.max_core), ((8, 53), (-300, 300), Some(2160)));
        assert!(t.gpu_off.is_some() && t.cpu_off.is_none());
        // before the service ever ran: nothing read back, nothing kept
        let fresh = parse(r#"{"kept": {"cpu": null, "gpu": null, "power": {"pl1": null, "pl2": null}, "turbo": null}, "status": {}, "now": {}, "stock": {}, "rapl": {"pl1": 95, "pl2": 125}, "turbo": true, "ranges": {}, "service": false}"#).unwrap();
        assert!(!fresh.service && !fresh.programmable && fresh.ratios.is_none() && fresh.gpu.is_none());
        assert!(parse("nope").is_none());
    }

    #[test]
    fn multipliers_stay_in_order() {
        let mut r = vec![50, 50, 48, 48, 47, 47, 47, 47];
        set_ratio(&mut r, 7, 49);
        assert_eq!(r, vec![50, 50, 49, 49, 49, 49, 49, 49], "all-core up: the ones below it rise");
        set_ratio(&mut r, 0, 45);
        assert_eq!(r, vec![45, 45, 45, 45, 45, 45, 45, 45], "one-core down: the ones for more cores come down");
        set_ratio(&mut r, 3, 52);
        assert_eq!(r, vec![52, 52, 52, 52, 45, 45, 45, 45]);
    }
}
