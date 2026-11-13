//! Settings > Battery (a computer with a battery only: a laptop, a handheld): the charge, when it was last charged, how much
//! was used each day, which apps used the processor, the battery's health and the charge limit (which moved here from Power).
//!
//! Nothing runs in the background for it. The history is UPower's own (it keeps the charge in /var/lib/upower, readable by
//! root only, so it is asked for over D-Bus with GetHistory: the same samples); the apps' share is the processor time of the
//! systemd scopes the launcher puts each app in (lapp_*.scope and app-*.scope, matched to apps by yggdrasil-appusage). Both
//! are read when the page opens and again every 30 seconds while it stays open; never otherwise.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eframe::egui::{self, pos2, vec2, Align2, CornerRadius, FontId, Rect, RichText, Sense};
use yggdrasil_ui::theme::Palette;

use crate::{loading, sys, ui, App, Page};

/// How far back the history goes (UPower keeps about a week; the chart has room for ten days).
pub const DAYS: usize = 10;

/// One UPower sample: when, the charge in percent, and the state then.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub time: i64,
    pub percent: f64,
    pub state: State,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Charging,
    Discharging,
    Full,
    Other,
}

impl State {
    /// UPower's state as its history files write it ("charging", "fully-charged", ...).
    pub fn from_text(s: &str) -> State {
        match s {
            "charging" | "pending-charge" => State::Charging,
            "discharging" | "pending-discharge" | "empty" => State::Discharging,
            "fully-charged" => State::Full,
            _ => State::Other,
        }
    }

    /// UPower's state as D-Bus gives it (UpDeviceState).
    pub fn from_code(c: u32) -> State {
        match c {
            1 | 5 => State::Charging,
            2 | 3 | 6 => State::Discharging,
            4 => State::Full,
            _ => State::Other,
        }
    }
}

/// A history-charge-*.dat file of UPower's: "TIME\tPERCENT\tSTATE" a line, oldest first. Lines that do not read are skipped.
pub fn parse_history(text: &str) -> Vec<Sample> {
    let mut v: Vec<Sample> = text
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let time = f.next()?.trim().parse().ok()?;
            let percent: f64 = f.next()?.trim().parse().ok()?;
            let state = State::from_text(f.next().unwrap_or("").trim());
            (0.0..=100.0).contains(&percent).then_some(Sample { time, percent, state })
        })
        .collect();
    v.sort_by_key(|s| s.time);
    v
}

/// The charge the battery reached the last time it was charged (the last charging or full sample before it went on
/// battery), and when. None while it has never been seen charging.
pub fn last_charged(h: &[Sample]) -> Option<(f64, i64)> {
    let mut best = None;
    for (i, s) in h.iter().enumerate() {
        let charged = matches!(s.state, State::Charging | State::Full);
        let next_on_battery = h.get(i + 1).is_none_or(|n| n.state == State::Discharging);
        if charged && next_on_battery {
            best = Some((s.percent, s.time));
        }
    }
    best
}

/// One day of the chart: how many percent of the battery were used on battery, and for how long it ran on battery.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Day {
    /// Days since 1970 in the local time zone.
    pub day: i64,
    pub used: f64,
    pub on_battery_secs: i64,
}

/// The last `days` days (today last), each with what was used on battery that day. `local_day` turns a time into its local
/// day number (the tests pass UTC). A gap of more than an hour between two samples (asleep, switched off) counts no time.
pub fn daily(h: &[Sample], now: i64, days: usize, local_day: impl Fn(i64) -> i64) -> Vec<Day> {
    let today = local_day(now);
    let first = today - days as i64 + 1;
    let mut out: Vec<Day> = (first..=today).map(|d| Day { day: d, ..Default::default() }).collect();
    for w in h.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b.state != State::Discharging {
            continue;
        }
        let d = local_day(b.time);
        let Some(slot) = out.iter_mut().find(|x| x.day == d) else { continue };
        if b.percent < a.percent {
            slot.used += a.percent - b.percent;
        }
        let gap = b.time - a.time;
        if a.state == State::Discharging && (0..=3600).contains(&gap) {
            slot.on_battery_secs += gap;
        }
    }
    out
}

/// Processor time of each app (its scopes added up) and of the rest of the session, in microseconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    /// (desktop id, microseconds), most first.
    pub apps: Vec<(String, u64)>,
    /// Everything else in the session: the desktop itself and background services.
    pub system: u64,
}

fn usage_usec(dir: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(dir.join("cpu.stat")).ok()?;
    text.lines().find_map(|l| l.strip_prefix("usage_usec ")).and_then(|v| v.trim().parse().ok())
}

/// Every scope under the user's manager (any depth of slices), with its folder.
fn scopes(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(dir) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Some(n) = e.file_name().to_str().map(String::from) else { continue };
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            if n.ends_with(".scope") {
                out.push((n, e.path()));
            } else if n.ends_with(".slice") {
                todo.push(e.path());
            }
        }
    }
    out
}

/// The apps' processor time under `root` (the user's systemd manager in the cgroup tree), matched to `ids` (desktop ids) the
/// way the launcher's usage counter matches them; the rest of the manager's time is the system's.
pub fn activity_in(root: &Path, ids: &std::collections::BTreeSet<String>) -> Activity {
    let mut per: BTreeMap<String, u64> = BTreeMap::new();
    for (name, dir) in scopes(root) {
        if let (Some(id), Some(u)) = (labyrinth_appusage::match_unit(&name, ids), usage_usec(&dir)) {
            *per.entry(id).or_default() += u;
        }
    }
    let apps_total: u64 = per.values().sum();
    let system = usage_usec(root).unwrap_or(0).saturating_sub(apps_total);
    let mut apps: Vec<(String, u64)> = per.into_iter().filter(|(_, u)| *u > 0).collect();
    apps.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Activity { apps, system }
}

/// Each entry's share of the whole, in percent, rounded so they add up to 100 (largest remainders first).
pub fn shares(values: &[u64]) -> Vec<u32> {
    let total: u64 = values.iter().sum();
    if total == 0 {
        return vec![0; values.len()];
    }
    let exact: Vec<f64> = values.iter().map(|v| *v as f64 * 100.0 / total as f64).collect();
    let mut out: Vec<u32> = exact.iter().map(|e| e.floor() as u32).collect();
    let mut left = 100 - out.iter().sum::<u32>();
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| (exact[b] - exact[b].floor()).total_cmp(&(exact[a] - exact[a].floor())));
    for i in order {
        if left == 0 {
            break;
        }
        out[i] += 1;
        left -= 1;
    }
    out
}

/// Whether this computer has a battery of its own: a power_supply of type Battery that is not a device's (a mouse's or a
/// headset's have scope Device) and is present. `root` is /sys/class/power_supply.
pub fn has_battery(root: &Path) -> bool {
    let read = |d: &Path, f: &str| std::fs::read_to_string(d.join(f)).map(|s| s.trim().to_string()).ok();
    std::fs::read_dir(root).into_iter().flatten().flatten().any(|e| {
        let d = e.path();
        read(&d, "type").as_deref() == Some("Battery")
            && read(&d, "scope").as_deref() != Some("Device")
            && read(&d, "present").as_deref() != Some("0")
    })
}

/// The answer for this computer, read once (a battery does not come and go while Settings is open). With a fixture: whether
/// it makes up a battery.
pub fn present() -> bool {
    static P: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *P.get_or_init(|| match crate::fixture::get() {
        Some(f) => f.answer("/usr/lib/yggdrasil/battery", &["json"]).and_then(|j| sys::parse_battery(&j)).is_some(),
        None => has_battery(Path::new("/sys/class/power_supply")),
    })
}

/// Everything the page shows.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub battery: Option<sys::Battery>,
    pub history: Vec<Sample>,
    pub activity: Activity,
    /// desktop id -> (name, icon)
    pub names: BTreeMap<String, (String, Option<PathBuf>)>,
    /// Why the history is empty, when it could not be asked for.
    pub note: Option<String>,
    pub read_at: i64,
}

/// UPower's charge history of the computer's battery over D-Bus (GetHistory), oldest first.
fn upower_history(span_secs: u32) -> Result<Vec<Sample>, String> {
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::OwnedObjectPath;
    let e = |e: zbus::Error| e.to_string();
    let conn = Connection::system().map_err(e)?;
    let up = Proxy::new(&conn, "org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.UPower").map_err(e)?;
    let devices: Vec<OwnedObjectPath> = up.call("EnumerateDevices", &()).map_err(e)?;
    for d in devices {
        let dev = Proxy::new(&conn, "org.freedesktop.UPower", d.as_str(), "org.freedesktop.UPower.Device").map_err(e)?;
        // 2: a battery; PowerSupply: the computer's own (not a mouse's)
        if dev.get_property::<u32>("Type").ok() != Some(2) || dev.get_property::<bool>("PowerSupply").ok() != Some(true) {
            continue;
        }
        let raw: Vec<(u32, f64, u32)> = dev.call("GetHistory", &("charge", span_secs, 2000u32)).map_err(e)?;
        let mut v: Vec<Sample> =
            raw.into_iter().map(|(t, p, s)| Sample { time: t as i64, percent: p, state: State::from_code(s) }).collect();
        v.retain(|s| (0.0..=100.0).contains(&s.percent));
        v.sort_by_key(|s| s.time);
        return Ok(v);
    }
    Err("UPower lists no battery".into())
}

/// Reads the page's data (on a background thread).
pub fn load() -> Info {
    let battery = sys::run("/usr/lib/yggdrasil/battery", &["json"]).and_then(|s| sys::parse_battery(&s));
    let apps = crate::appmgr::list_apps(&sys::current_icon_theme());
    let names = apps.iter().map(|a| (a.id.clone(), (a.name.clone(), a.icon_path.clone()))).collect();
    let (history, note, activity) = match crate::fixture::get() {
        Some(f) => {
            // made up: times at or below 0 are seconds before now, so the chart always shows the last days
            let fb = f.battery.clone().unwrap_or_default();
            let now = sys::unix_now();
            let mut h = parse_history(&fb.history);
            for s in &mut h {
                if s.time <= 0 {
                    s.time += now;
                }
            }
            let activity = Activity { apps: fb.activity, system: fb.system };
            (h, None, activity)
        }
        None => {
            let (history, note) = match upower_history((DAYS as u32) * 86_400) {
                Ok(h) => (h, None),
                Err(e) => (Vec::new(), Some(e)),
            };
            let ids = labyrinth_appusage::desktop_ids();
            let activity = labyrinth_appusage::watch::cgroup_root().map(|r| activity_in(&r, &ids)).unwrap_or_default();
            (history, note, activity)
        }
    };
    Info { battery, history, activity, names, note, read_at: sys::unix_now() }
}

/// Days since 1970 of a time, in the computer's time zone.
fn local_day(t: i64) -> i64 {
    let tz = jiff::tz::TimeZone::system();
    let off = jiff::Timestamp::from_second(t).map(|ts| tz.to_offset(ts).seconds() as i64).unwrap_or(0);
    (t + off).div_euclid(86_400)
}

/// "Mon" for a local day number.
fn weekday(day: i64) -> &'static str {
    // 1970-01-01 was a Thursday
    ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][day.rem_euclid(7) as usize]
}

fn hours(secs: i64) -> String {
    match secs {
        0..=59 => "under a minute".into(),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

impl App {
    /// Settings > Battery.
    pub(crate) fn page_battery(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(info) = self.data.battery.clone() else { return loading(ui) };
        let Some(b) = info.battery.clone() else {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "The battery could not be read", None, |_| {}));
            return;
        };
        // the charge: big, the bar under it, and when it was last charged
        ui::group(ui, pal, None, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}%", b.percent)).size(40.0).strong());
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(&b.status).size(14.0));
                    let last = if b.status.starts_with("Charging") {
                        None
                    } else {
                        last_charged(&info.history).map(|(p, t)| {
                            let ago = crate::devices::ago(info.read_at, t);
                            format!("Last charged to {}% · {ago}", p.round())
                        })
                    };
                    if let Some(l) = last {
                        ui.label(RichText::new(l).size(12.0).color(pal.dim));
                    }
                });
            });
            ui.add_space(8.0);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(7), pal.hover);
            let mut fill = rect;
            fill.set_width((rect.width() * b.percent as f32 / 100.0).max(14.0));
            let col = if b.status.starts_with("Charging") || b.percent > 20 {
                pal.green
            } else if b.percent > 10 {
                pal.amber
            } else {
                pal.red
            };
            ui.painter().rect_filled(fill, CornerRadius::same(7), col);
            if let Some(l) = b.limit.filter(|l| *l < 100) {
                let x = rect.left() + rect.width() * l as f32 / 100.0;
                ui.painter().vline(x, rect.y_range().expand(3.0), egui::Stroke::new(2.0, pal.text));
            }
            ui.add_space(10.0);
        });
        if let Some(l) = b.limit.filter(|l| *l < 100) {
            ui::footnote(ui, pal, &format!("Charging stops at {l}% (the line on the bar): the charge limit below."));
        }

        // the last days: what was used on battery each day
        let days = daily(&info.history, info.read_at, DAYS, local_day);
        ui::group(ui, pal, Some(&format!("Last {DAYS} days")), |ui| {
            ui.add_space(8.0);
            if info.history.is_empty() {
                let why = info.note.clone().unwrap_or_else(|| "No history yet: UPower records it as the battery is used.".into());
                ui.label(RichText::new(why).size(12.5).color(pal.dim));
            } else {
                chart(ui, pal, &days);
            }
            ui.add_space(6.0);
        });
        let total: i64 = days.iter().map(|d| d.on_battery_secs).sum();
        ui::footnote(
            ui,
            pal,
            &format!(
                "How much of the battery was used each day while unplugged; {} on battery in all. UPower keeps this history.",
                hours(total)
            ),
        );

        // which apps used the processor
        let act = &info.activity;
        let mut entries: Vec<(String, Option<PathBuf>, u64, bool)> = act
            .apps
            .iter()
            .take(8)
            .map(|(id, u)| {
                let (name, icon) = info.names.get(id).cloned().unwrap_or_else(|| (id.clone(), None));
                (name, icon, *u, false)
            })
            .collect();
        let rest: u64 = act.apps.iter().skip(8).map(|(_, u)| u).sum::<u64>() + act.system;
        if rest > 0 {
            entries.push(("System and background".into(), None, rest, true));
        }
        let pct = shares(&entries.iter().map(|e| e.2).collect::<Vec<_>>());
        ui::group(ui, pal, Some("App & System activity"), |ui| {
            if entries.is_empty() {
                ui::row(ui, pal, true, "Nothing measured yet", None, |_| {});
            }
            for (i, ((name, icon, _, system), p)) in entries.iter().zip(&pct).enumerate() {
                ui::row_with(
                    ui,
                    pal,
                    i == 0,
                    |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                            match icon {
                                Some(path) => egui::Image::new(format!("file://{}", path.display())).paint_at(ui, r),
                                None if *system => crate::icons::paint(ui.painter(), r.shrink(2.0), crate::icons::Icon::Gear, pal.dim),
                                // an app without an icon: a tile with its first letter, as on the Apps page
                                None => {
                                    ui.painter().rect_filled(r, CornerRadius::same(5), pal.accent.gamma_multiply(0.55));
                                    let l: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                                    ui.painter().text(
                                        r.center(),
                                        Align2::CENTER_CENTER,
                                        l,
                                        FontId::proportional(12.0),
                                        egui::Color32::WHITE,
                                    );
                                }
                            }
                            ui.add_space(4.0);
                            ui.label(RichText::new(name).size(14.0));
                        });
                    },
                    None,
                    |ui| {
                        ui.label(RichText::new(format!("{p}%")).color(pal.dim));
                        let (r, _) = ui.allocate_exact_size(vec2(120.0, 6.0), Sense::hover());
                        ui.painter().rect_filled(r, 3.0, pal.hover);
                        let mut f = r;
                        f.set_width(r.width() * *p as f32 / 100.0);
                        ui.painter().rect_filled(f, 3.0, pal.accent);
                    },
                );
            }
        });
        ui::footnote(
            ui,
            pal,
            "Each app's share of the processor time used in this session (the processor uses the most power), from the apps open \
             now; apps already closed are not counted. Read when this page opens.",
        );

        // health and the charge limit (moved here from Power)
        let mut new_limit: Option<i64> = None;
        ui::group(ui, pal, Some("Battery Health"), |ui| {
            let mut first = true;
            if let Some(h) = b.health {
                let note = match b.cycles {
                    Some(c) => format!("Of its capacity when new, after {c} charge cycles"),
                    None => "Of its capacity when new".to_string(),
                };
                ui::row(ui, pal, true, "Maximum capacity", Some(&note), |ui| {
                    ui.label(RichText::new(format!("{h}%")).size(15.0));
                });
                first = false;
            }
            match b.limit {
                Some(limit) => {
                    const LIMITS: [i64; 4] = [60, 80, 90, 100];
                    let sel = LIMITS.iter().position(|&l| l == limit).unwrap_or(LIMITS.len() - 1);
                    ui::row(
                        ui,
                        pal,
                        first,
                        "Charge limit",
                        Some("Stop charging here: 80% is best for the battery's life when the laptop is mostly plugged in"),
                        |ui| {
                            if let Some(i) = ui::segmented(ui, pal, &["60%", "80%", "90%", "100%"], sel) {
                                new_limit = Some(LIMITS[i]);
                            }
                        },
                    );
                }
                None if first => ui::row(ui, pal, true, "No health readings", Some("This battery does not report them"), |_| {}),
                None => {}
            }
        });
        let mut power = false;
        ui::group(ui, pal, None, |ui| {
            power = ui::nav_row(ui, pal, true, "Power mode", Some("Power saver, Balanced or Performance: on the Power page"), |_| {});
        });
        if let Some(l) = new_limit {
            let arg = l.to_string();
            self.change(ctx, move || sys::act("pkexec", &["/usr/lib/yggdrasil/battery", "set-limit", &arg]));
        }
        if power {
            self.open_page(Page::Power, ctx);
        }
    }
}

/// The days as bars (percent used on battery), the day's name under each.
fn chart(ui: &mut egui::Ui, pal: &Palette, days: &[Day]) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 150.0), Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    let plot = Rect::from_min_max(rect.min + vec2(34.0, 6.0), rect.max - vec2(0.0, 22.0));
    // the scale: the largest day rounded up to a multiple of 20 (at least 20 %), so its middle line is a round number too
    let top = days.iter().map(|d| d.used).fold(20.0_f64, f64::max).min(100.0);
    let top = (top / 20.0).ceil() * 20.0;
    for k in 0..=2 {
        let v = top * k as f64 / 2.0;
        let y = plot.bottom() - plot.height() * (v / top) as f32;
        p.hline(plot.x_range(), y, egui::Stroke::new(1.0, pal.border));
        p.text(pos2(plot.left() - 6.0, y), Align2::RIGHT_CENTER, format!("{v:.0}%"), FontId::proportional(10.5), pal.dim);
    }
    let n = days.len().max(1) as f32;
    let slot = plot.width() / n;
    let mut hover: Option<&Day> = None;
    for (i, d) in days.iter().enumerate() {
        let x = plot.left() + slot * (i as f32 + 0.5);
        let h = plot.height() * (d.used / top) as f32;
        let bar = Rect::from_min_max(pos2(x - slot * 0.3, plot.bottom() - h), pos2(x + slot * 0.3, plot.bottom()));
        let today = i + 1 == days.len();
        if h > 0.5 {
            p.rect_filled(bar, CornerRadius::same(3), if today { pal.accent } else { pal.accent.gamma_multiply(0.7) });
        }
        p.text(pos2(x, plot.bottom() + 11.0), Align2::CENTER_CENTER, weekday(d.day), FontId::proportional(10.5), pal.dim);
        if resp.hover_pos().is_some_and(|h| (h.x - x).abs() < slot / 2.0) {
            hover = Some(d);
        }
    }
    if let Some(d) = hover {
        resp.on_hover_text(format!("{}: {:.0}% used, {} on battery", weekday(d.day), d.used, hours(d.on_battery_secs)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up history-charge file: charged to 90 % on day 0, used on battery through day 1, plugged in and charged to 80 %
    /// on day 2, then on battery again.
    const DAT: &str = "\
86400\t60.000\tcharging
90000\t90.000\tfully-charged
93600\t85.000\tdischarging
97200\t80.000\tdischarging
180000\t70.000\tdischarging
183600\t55.000\tdischarging
185000\tnot a number\tdischarging
259200\t40.000\tcharging
262800\t80.000\tcharging
266400\t78.000\tdischarging
";

    fn utc_day(t: i64) -> i64 {
        t.div_euclid(86_400)
    }

    #[test]
    fn upower_history_files_read() {
        let h = parse_history(DAT);
        assert_eq!(h.len(), 9, "the line that does not read is left out");
        assert_eq!(h[1], Sample { time: 90000, percent: 90.0, state: State::Full });
        assert_eq!(parse_history("garbage\n\n12\t101\tcharging\n"), [], "out of range or broken: nothing");
        assert_eq!(State::from_code(2), State::Discharging);
        assert_eq!(State::from_code(4), State::Full);
    }

    #[test]
    fn last_charged_is_the_charge_when_it_was_unplugged() {
        let h = parse_history(DAT);
        assert_eq!(last_charged(&h), Some((80.0, 262800)));
        assert_eq!(last_charged(&h[..6]), Some((90.0, 90000)), "earlier: the full charge on day 1");
        assert_eq!(last_charged(&h[2..6]), None, "only seen on battery");
    }

    #[test]
    fn each_day_counts_what_was_used_on_battery() {
        let h = parse_history(DAT);
        let days = daily(&h, 266400 + 100, 4, utc_day);
        assert_eq!(days.iter().map(|d| d.day).collect::<Vec<_>>(), [0, 1, 2, 3]);
        // day 2 also has the 10 % lost while asleep unplugged (counted on the day it is seen), as it is battery used
        assert_eq!(days.iter().map(|d| d.used).collect::<Vec<_>>(), [0.0, 10.0, 25.0, 2.0]);
        // day 1: 3600 s between two on-battery samples; the long gap to day 2 (asleep) counts nothing
        assert_eq!(days[1].on_battery_secs, 3600);
        assert_eq!(days[2].on_battery_secs, 3600);
    }

    #[test]
    fn shares_add_up_to_a_hundred() {
        assert_eq!(shares(&[1, 1, 1]), [34, 33, 33]);
        assert_eq!(shares(&[50, 25, 25]), [50, 25, 25]);
        assert_eq!(shares(&[0, 0]), [0, 0]);
        assert_eq!(shares(&[999, 1]).iter().sum::<u32>(), 100);
    }

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("ygg-battery-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_page_shows_only_with_a_battery_of_the_computers_own() {
        let root = dir().join("power_supply");
        std::fs::create_dir_all(root.join("AC")).unwrap();
        std::fs::write(root.join("AC/type"), "Mains\n").unwrap();
        // a desktop: mains only, and a wireless mouse's battery
        std::fs::create_dir_all(root.join("hidpp_battery_0")).unwrap();
        std::fs::write(root.join("hidpp_battery_0/type"), "Battery\n").unwrap();
        std::fs::write(root.join("hidpp_battery_0/scope"), "Device\n").unwrap();
        assert!(!has_battery(&root));
        // a laptop's battery, taken out
        std::fs::create_dir_all(root.join("BAT0")).unwrap();
        std::fs::write(root.join("BAT0/type"), "Battery\n").unwrap();
        std::fs::write(root.join("BAT0/present"), "0\n").unwrap();
        assert!(!has_battery(&root));
        std::fs::write(root.join("BAT0/present"), "1\n").unwrap();
        assert!(has_battery(&root));
        assert!(!has_battery(&root.join("missing")));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn apps_get_their_scopes_processor_time() {
        let root = dir().join("user@1000.service");
        let mk = |p: &str, usec: u64| {
            std::fs::create_dir_all(root.join(p)).unwrap();
            std::fs::write(root.join(p).join("cpu.stat"), format!("usage_usec {usec}\nuser_usec 1\n")).unwrap();
        };
        mk("", 10_000);
        mk("app.slice", 9_000);
        mk("app.slice/lapp_example_editor.slice", 6_000);
        mk("app.slice/lapp_example_editor.slice/lapp_example_editor_12.scope", 4_000);
        mk("app.slice/lapp_example_editor.slice/lapp_example_editor_13.scope", 2_000);
        mk("app.slice/app-example\\x2dplayer-77.scope", 3_000);
        mk("app.slice/unknown-thing.scope", 500);
        let ids: std::collections::BTreeSet<String> = ["example-editor", "example-player"].into_iter().map(String::from).collect();
        let a = activity_in(&root, &ids);
        assert_eq!(a.apps, [("example-editor".to_string(), 6_000), ("example-player".to_string(), 3_000)]);
        assert_eq!(a.system, 1_000, "the rest of the session");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
