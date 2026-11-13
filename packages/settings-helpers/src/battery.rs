//! battery: a laptop's battery in the top bar, its drop-down, and its charge limit (/usr/lib/yggdrasil/battery).
//!
//!   battery watch           at login (autostart): sets the power mode for the power source when the laptop is plugged in or
//!                           unplugged (~/.config/yggdrasil/power.conf: plugged_in = performance, on_battery = balanced by
//!                           default; `keep` leaves it alone), and every few seconds writes the bar add-on's JSON
//!                           (~/.local/state/yggdrasil/battery/bar.json, read by yggdrasil-battery.conf) when it changed, and warns
//!                           when the battery runs low (20 %, 10 %; at 5 % the computer sleeps a minute later unless plugged in).
//!                           Writes "off" and stops on a computer with no battery.
//!   battery press ID        a press on the drop-down: profile:NAME (power-profiles-daemon) or limit:N (through pkexec)
//!   battery json            the battery for Settings > Power: {"present", "percent", "status", "time", "health", "cycles", "limit"}
//!   battery set-limit N     (root, through pkexec) stop charging at N % (50 to 100), kept in /etc/yggdrasil/charge-limit
//!   battery restore         (root, yggdrasil-charge-limit.service at boot) the kept limit again, for laptops that forget it
//!   battery sleep-delay S   (root, through pkexec) how long the lid's suspend-then-hibernate sleeps before hibernating (60 to
//!                           86400 seconds), in /etc/systemd/sleep.conf.d/50-yggdrasil-lid.conf (Settings > Power, the lid)

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const SAVED_LIMIT: &str = "/etc/yggdrasil/charge-limit";
/// The choices the drop-down and Settings offer.
pub const LIMITS: [u32; 4] = [60, 80, 90, 100];
pub const PROFILES: [(&str, &str, &str); 3] = [
    ("power-saver", "Power saver", "Longest battery, quieter"),
    ("balanced", "Balanced", "The everyday choice"),
    ("performance", "Performance", "Fastest; uses more power"),
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bat {
    pub dir: PathBuf,
    pub percent: u32,
    /// "Charging", "Discharging", "Not charging", "Full".
    pub status: String,
    /// Energy (µWh) or charge (µAh): now, full, full when new; rate (µW or µA).
    pub now: f64,
    pub full: f64,
    pub design: f64,
    pub rate: f64,
    pub cycles: Option<u32>,
    /// charge_control_end_threshold, when the laptop has one.
    pub limit: Option<u32>,
}

fn read(dir: &Path, f: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(f)).ok().map(|s| s.trim().to_string())
}

fn num(dir: &Path, f: &str) -> Option<f64> {
    read(dir, f)?.parse().ok()
}

/// One power_supply folder, if it is the computer's own battery (not a mouse's or a headset's: those have scope=Device).
pub fn read_battery(dir: &Path) -> Option<Bat> {
    if read(dir, "type")? != "Battery" || read(dir, "scope").as_deref() == Some("Device") || read(dir, "present").as_deref() == Some("0") {
        return None;
    }
    let (now, full, design, rate) = match num(dir, "energy_now") {
        Some(n) => (n, num(dir, "energy_full")?, num(dir, "energy_full_design").unwrap_or(0.0), num(dir, "power_now").unwrap_or(0.0)),
        None => (
            num(dir, "charge_now")?,
            num(dir, "charge_full")?,
            num(dir, "charge_full_design").unwrap_or(0.0),
            num(dir, "current_now").unwrap_or(0.0),
        ),
    };
    let percent = num(dir, "capacity").map(|c| c as u32).unwrap_or(if full > 0.0 { (now / full * 100.0).round() as u32 } else { 0 });
    Some(Bat {
        dir: dir.to_path_buf(),
        percent: percent.min(100),
        status: read(dir, "status").unwrap_or_default(),
        now,
        full,
        design,
        rate: rate.abs(),
        cycles: num(dir, "cycle_count").map(|c| c as u32).filter(|&c| c > 0),
        limit: num(dir, "charge_control_end_threshold").map(|c| c as u32),
    })
}

/// The computer's battery (the first, as sorted), if it has one.
pub fn find(root: &Path) -> Option<Bat> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.iter().find_map(|d| read_battery(d))
}

pub fn system_battery() -> Option<Bat> {
    find(Path::new("/sys/class/power_supply"))
}

impl Bat {
    pub fn charging(&self) -> bool {
        self.status == "Charging"
    }
    pub fn discharging(&self) -> bool {
        self.status == "Discharging"
    }
    /// How much of its capacity when new it still holds, in percent.
    pub fn health(&self) -> Option<u32> {
        (self.design > 0.0).then(|| ((self.full / self.design) * 100.0).round().min(100.0) as u32)
    }
    /// Minutes until empty (discharging) or until it stops charging (at the limit, or full).
    pub fn minutes_left(&self) -> Option<u32> {
        if self.rate <= 0.0 {
            return None;
        }
        let hours = if self.discharging() {
            self.now / self.rate
        } else if self.charging() {
            let target = self.full * self.limit.unwrap_or(100).min(100) as f64 / 100.0;
            (target - self.now).max(0.0) / self.rate
        } else {
            return None;
        };
        let m = (hours * 60.0).round();
        (m > 0.0 && m < 60.0 * 48.0).then_some(m as u32)
    }
    /// "2 h 10 min left", "Full in 45 min", "Plugged in, held at 80 %".
    pub fn note(&self) -> String {
        let left = self.minutes_left().map(duration);
        match (self.status.as_str(), left) {
            ("Discharging", Some(t)) => format!("{t} left"),
            ("Discharging", None) => "On battery".into(),
            ("Charging", Some(t)) => match self.limit.filter(|&l| l < 100) {
                Some(l) => format!("Charging: {l} % in {t}"),
                None => format!("Charging: full in {t}"),
            },
            ("Charging", None) => "Charging".into(),
            ("Full", _) => "Plugged in, fully charged".into(),
            _ => match self.limit.filter(|&l| l < 100) {
                Some(l) => format!("Plugged in, held at {l} %"),
                None => "Plugged in, not charging".into(),
            },
        }
    }
    /// The icon theme's battery icon for this level and state.
    pub fn icon(&self) -> String {
        let level = (self.percent / 10 * 10).min(100);
        if !self.discharging() && level == 100 {
            return "battery-level-100-charged-symbolic".into();
        }
        if self.charging() {
            format!("battery-level-{level}-charging-symbolic")
        } else if !self.discharging() {
            format!("battery-level-{level}-plugged-in-symbolic")
        } else {
            format!("battery-level-{level}-symbolic")
        }
    }
}

/// "45 min", "2 h", "2 h 10 min".
pub fn duration(min: u32) -> String {
    match (min / 60, min % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// The bar add-on's JSON: the pill, and the drop-down with the power mode and the charge limit.
pub fn view(b: Option<&Bat>, profile: &str, profiles: &[String]) -> serde_json::Value {
    let Some(b) = b else { return serde_json::json!({"state": "off"}) };
    let state = match (b.discharging(), b.percent) {
        (true, p) if p <= 10 => "alert",
        (true, p) if p <= 20 => "warn",
        _ => "ok",
    };
    let mut lines = vec![
        serde_json::json!({"title": "Battery", "value": format!("{} %", b.percent), "percent": b.percent, "note": b.note(), "alert": state == "alert"}),
    ];
    if let Some(h) = b.health() {
        let note = match b.cycles {
            Some(c) => format!("of its capacity when new · {c} charge cycles"),
            None => "of its capacity when new".into(),
        };
        lines.push(serde_json::json!({"title": "Health", "value": format!("{h} %"), "note": note}));
    }
    let mut card = vec![];
    if !profiles.is_empty() {
        card.push(serde_json::json!({"section": "POWER MODE"}));
        let rows: Vec<_> = PROFILES
            .iter()
            .filter(|(id, _, _)| profiles.iter().any(|p| p == id))
            .map(|(id, title, note)| serde_json::json!({"id": format!("profile:{id}"), "title": title, "note": note, "on": *id == profile}))
            .collect();
        card.push(serde_json::json!({"list": rows}));
    }
    if let Some(limit) = b.limit {
        card.push(serde_json::json!({"section": "CHARGE LIMIT"}));
        let rows: Vec<_> = LIMITS
            .iter()
            .map(|&l| {
                let note = match l {
                    60 => "Plugged in most of the time",
                    80 => "Best for the battery's life",
                    90 => "A little more for the road",
                    _ => "A full charge",
                };
                serde_json::json!({"id": format!("limit:{l}"), "title": format!("{l} %"), "note": note, "on": l == limit})
            })
            .collect();
        card.push(serde_json::json!({"list": rows}));
    }
    serde_json::json!({
        "text": format!("{} %", b.percent),
        "icon": b.icon(),
        "state": state,
        "lines": lines,
        "card": card,
        "help": "Settings > Power has the same, and more.",
    })
}

fn out(cmd: &str, args: &[&str]) -> Option<String> {
    let o =
        std::process::Command::new(cmd).args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// busctl's `s "balanced"` -> balanced.
pub fn parse_busctl_string(s: &str) -> String {
    s.split('"').nth(1).unwrap_or_default().to_string()
}

const PPD: [&str; 3] = ["net.hadess.PowerProfiles", "/net/hadess/PowerProfiles", "net.hadess.PowerProfiles"];

/// busctl's answer for Profiles (`aa{sv} 3 2 "Driver" s "..." "Profile" s "performance" ...`) -> the modes it has, in the
/// drop-down's order.
pub fn parse_profiles(answer: &str) -> Vec<String> {
    PROFILES.iter().map(|(id, _, _)| id.to_string()).filter(|id| answer.contains(&format!("\"{id}\""))).collect()
}

/// The power mode in use and the ones there are (power-profiles-daemon), empty when it is not running. Two busctl calls: the
/// watch asks every three seconds (it asked for Profiles once per mode, four programs each time).
pub fn profiles() -> (String, Vec<String>) {
    let get = |prop: &str| out("busctl", &["--system", "get-property", PPD[0], PPD[1], PPD[2], prop]);
    let active = get("ActiveProfile").map(|s| parse_busctl_string(&s)).unwrap_or_default();
    let list = get("Profiles").map(|s| parse_profiles(&s)).unwrap_or_default();
    (active, list)
}

fn bar_file() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("yggdrasil/battery/bar.json"))
}

/// Writes the add-on's file (whole, then renamed, so the panel never reads half of it); false when nothing changed.
fn write_view(last: &mut String) -> bool {
    let (profile, list) = profiles();
    let text = view(system_battery().as_ref(), &profile, &list).to_string();
    if text == *last {
        return false;
    }
    if let Some(f) = bar_file() {
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = f.with_extension("json.tmp");
        if std::fs::write(&tmp, &text).is_ok() {
            let _ = std::fs::rename(&tmp, &f);
        }
    }
    *last = text;
    true
}

fn notify(urgency: &str, title: &str, body: &str) {
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "Battery", "-u", urgency, "-i", "battery-caution-symbolic", title, body])
        .stdin(std::process::Stdio::null())
        .status();
}

/// What the low-battery watch does on one reading: the warnings already given (20, 10, 5) and when the 5 % one was.
#[derive(Debug, Default, PartialEq)]
pub struct Low {
    pub warned: u32,
    pub sleep_at: Option<Instant>,
}

#[derive(Debug, PartialEq)]
pub enum LowAct {
    Nothing,
    Warn(u32),
    Sleep,
}

impl Low {
    pub fn step(&mut self, discharging: bool, percent: u32, now: Instant) -> LowAct {
        if !discharging {
            *self = Low::default();
            return LowAct::Nothing;
        }
        if let Some(t) = self.sleep_at {
            if now >= t {
                self.sleep_at = None;
                return LowAct::Sleep;
            }
            return LowAct::Nothing;
        }
        for level in [5, 10, 20] {
            if percent <= level && (self.warned == 0 || self.warned > level) {
                self.warned = level;
                if level == 5 {
                    self.sleep_at = Some(now + Duration::from_secs(60));
                }
                return LowAct::Warn(level);
            }
        }
        LowAct::Nothing
    }
}

/// Whether the computer runs on mains power now (a power_supply of type Mains that is online); None without one.
pub fn on_mains(root: &Path) -> Option<bool> {
    let dirs = std::fs::read_dir(root).ok()?;
    let mut seen = false;
    for d in dirs.flatten().map(|e| e.path()) {
        if read(&d, "type").as_deref() == Some("Mains") {
            seen = true;
            if read(&d, "online").as_deref() == Some("1") {
                return Some(true);
            }
        }
    }
    seen.then_some(false)
}

/// The power mode for a power source, from power.conf (`plugged_in`, `on_battery`): Performance plugged in and Balanced on
/// battery unless set otherwise; None for `keep` (leave it as it is).
pub fn mode_for(conf: &str, mains: bool) -> Option<String> {
    let key = if mains { "plugged_in" } else { "on_battery" };
    let v = conf
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_else(|| if mains { "performance".into() } else { "balanced".into() });
    PROFILES.iter().any(|(id, _, _)| *id == v).then_some(v)
}

fn power_conf() -> String {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| std::fs::read_to_string(d.join("yggdrasil/power.conf")).unwrap_or_default())
        .unwrap_or_default()
}

fn set_profile(p: &str) -> bool {
    out("busctl", &["--system", "set-property", PPD[0], PPD[1], PPD[2], "ActiveProfile", "s", p]).is_some()
}

fn watch() -> i32 {
    let mut last = String::new();
    let mut low = Low::default();
    let mut mains: Option<bool> = None;
    if system_battery().is_none() {
        write_view(&mut last);
        return 0;
    }
    loop {
        // plugged in or unplugged (and once at login): the power mode for that source
        let now_mains = on_mains(Path::new("/sys/class/power_supply"));
        if now_mains.is_some() && now_mains != mains {
            if let Some(mode) = now_mains.and_then(|m| mode_for(&power_conf(), m)) {
                set_profile(&mode);
            }
            mains = now_mains;
        }
        write_view(&mut last);
        if let Some(b) = system_battery() {
            match low.step(b.discharging(), b.percent, Instant::now()) {
                LowAct::Warn(20) => notify("normal", "Battery low", &format!("{} % left. Plug in soon.", b.percent)),
                LowAct::Warn(10) => notify("critical", "Battery very low", &format!("{} % left. Plug in now.", b.percent)),
                LowAct::Warn(_) => {
                    notify("critical", "Battery almost empty", "The computer goes to sleep in a minute unless it is plugged in.")
                }
                LowAct::Sleep => {
                    let _ = std::process::Command::new("systemctl").arg("suspend").status();
                }
                LowAct::Nothing => {}
            }
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

fn press(id: &str) -> i32 {
    let ok = if let Some(p) = id.strip_prefix("profile:").filter(|p| PROFILES.iter().any(|(x, _, _)| x == p)) {
        out("busctl", &["--system", "set-property", PPD[0], PPD[1], PPD[2], "ActiveProfile", "s", p]).is_some()
    } else if let Some(n) = id.strip_prefix("limit:").and_then(|n| n.parse::<u32>().ok()).filter(|n| LIMITS.contains(n)) {
        std::process::Command::new("pkexec")
            .args(["/usr/lib/yggdrasil/battery", "set-limit", &n.to_string()])
            .status()
            .is_ok_and(|s| s.success())
    } else {
        eprintln!("battery: no such choice: {id}");
        return 2;
    };
    write_view(&mut String::new());
    if ok {
        0
    } else {
        1
    }
}

/// The thresholds to write for a limit, in a safe order: start goes to 0 first (always below any end), then the end, then the
/// start a little under the end (so a laptop held at the limit does not top up for every percent it loses).
pub fn threshold_writes(limit: u32) -> Vec<(&'static str, u32)> {
    let start = if limit >= 100 { 0 } else { limit.saturating_sub(5) };
    vec![("charge_control_start_threshold", 0), ("charge_control_end_threshold", limit), ("charge_control_start_threshold", start)]
}

fn apply_limit(b: &Bat, limit: u32) -> Result<(), String> {
    for (f, v) in threshold_writes(limit) {
        let p = b.dir.join(f);
        if f.contains("start") && !p.exists() {
            continue;
        }
        std::fs::write(&p, v.to_string()).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}

fn set_limit(arg: Option<&String>) -> i32 {
    let Some(limit) = arg.and_then(|a| a.parse::<u32>().ok()).filter(|n| (50..=100).contains(n)) else {
        eprintln!("battery: set-limit takes a number from 50 to 100");
        return 2;
    };
    let Some(b) = system_battery().filter(|b| b.limit.is_some()) else {
        eprintln!("battery: this computer's battery has no charge limit");
        return 1;
    };
    if let Err(e) = apply_limit(&b, limit) {
        eprintln!("battery: {e}");
        return 1;
    }
    let _ = std::fs::create_dir_all("/etc/yggdrasil");
    let _ = std::fs::write(SAVED_LIMIT, format!("{limit}\n"));
    // set again at every boot, for laptops whose firmware forgets it
    let _ = std::process::Command::new("systemctl")
        .args(["enable", "yggdrasil-charge-limit.service"])
        .stderr(std::process::Stdio::null())
        .status();
    0
}

fn restore() -> i32 {
    let limit = std::fs::read_to_string(SAVED_LIMIT).ok().and_then(|s| s.trim().parse::<u32>().ok()).filter(|n| (50..=100).contains(n));
    match (limit, system_battery().filter(|b| b.limit.is_some())) {
        (Some(l), Some(b)) => match apply_limit(&b, l) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("battery: {e}");
                1
            }
        },
        _ => 0,
    }
}

/// The drop-in for suspend-then-hibernate's delay (the idle package ships the default, 25 minutes, under /usr/lib).
pub const SLEEP_DELAY_CONF: &str = "/etc/systemd/sleep.conf.d/50-yggdrasil-lid.conf";

pub fn sleep_delay_text(secs: u32) -> String {
    format!("# Written by Settings > Power (the lid): suspend-then-hibernate hibernates after this long asleep.\n[Sleep]\nHibernateDelaySec={secs}s\n")
}

fn sleep_delay(arg: Option<&String>) -> i32 {
    let Some(secs) = arg.and_then(|a| a.parse::<u32>().ok()).filter(|n| (60..=86_400).contains(n)) else {
        eprintln!("battery: sleep-delay takes seconds from 60 to 86400");
        return 2;
    };
    let path = std::path::Path::new(SLEEP_DELAY_CONF);
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    match std::fs::write(path, sleep_delay_text(secs)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("battery: {}: {e}", path.display());
            1
        }
    }
}

fn json() -> i32 {
    let v = match system_battery() {
        None => serde_json::json!({"present": false}),
        Some(b) => serde_json::json!({
            "present": true,
            "percent": b.percent,
            "status": b.note(),
            "health": b.health(),
            "cycles": b.cycles,
            "limit": b.limit,
        }),
    };
    println!("{v}");
    0
}

/// What may run as root through pkexec: only set-limit. The polkit action (org.yggdrasil.battery.set-limit) names the program,
/// not its arguments, so without this an administrator's cached password would also run the watch (and its suspend), a press
/// or restore as root.
pub fn allowed_through_pkexec(cmd: Option<&str>) -> bool {
    cmd == Some("set-limit")
}

pub fn main(args: &[String]) -> i32 {
    if std::env::var_os("PKEXEC_UID").is_some() && !allowed_through_pkexec(args.first().map(String::as_str)) {
        eprintln!("battery: only set-limit runs through pkexec");
        return 2;
    }
    match args.first().map(String::as_str) {
        Some("watch") => watch(),
        Some("press") => args.get(1).map(|id| press(id)).unwrap_or(2),
        Some("json") => json(),
        Some("set-limit") => set_limit(args.get(1)),
        Some("restore") => restore(),
        Some("sleep-delay") => sleep_delay(args.get(1)),
        _ => {
            eprintln!("usage: battery watch | press ID | json | set-limit N | restore | sleep-delay SECONDS");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bat(dir: &Path, files: &[(&str, &str)]) {
        std::fs::create_dir_all(dir).unwrap();
        for (f, v) in files {
            std::fs::write(dir.join(f), format!("{v}\n")).unwrap();
        }
    }

    #[test]
    fn the_computers_battery_is_found_and_a_mouses_is_not() {
        let root = std::env::temp_dir().join(format!("ygg-bat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        bat(&root.join("AC"), &[("type", "Mains"), ("online", "1")]);
        bat(
            &root.join("BAT0"),
            &[
                ("type", "Battery"),
                ("status", "Discharging"),
                ("capacity", "95"),
                ("energy_now", "36700000"),
                ("energy_full", "49290000"),
                ("energy_full_design", "57050000"),
                ("power_now", "20174000"),
                ("cycle_count", "209"),
                ("charge_control_end_threshold", "100"),
            ],
        );
        bat(&root.join("hidpp_battery_0"), &[("type", "Battery"), ("scope", "Device"), ("capacity", "40")]);
        let b = find(&root).unwrap();
        assert_eq!((b.percent, b.health(), b.cycles, b.limit), (95, Some(86), Some(209), Some(100)));
        assert_eq!(b.minutes_left(), Some(109));
        assert_eq!(b.note(), "1 h 49 min left");
        assert_eq!(b.icon(), "battery-level-90-symbolic");
        bat(&root.join("BAT0"), &[("status", "Charging"), ("charge_control_end_threshold", "80")]);
        let b = find(&root).unwrap();
        // to 80 % of 49.29 Wh from 36.7 Wh at 20.17 W
        assert_eq!(b.note(), "Charging: 80 % in 8 min");
        assert_eq!(b.icon(), "battery-level-90-charging-symbolic");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_drop_down_lists_the_modes_and_limits_with_the_chosen_ones_marked() {
        let b = Bat {
            percent: 15,
            status: "Discharging".into(),
            now: 1.0,
            full: 10.0,
            design: 10.0,
            rate: 0.0,
            limit: Some(80),
            ..Default::default()
        };
        let profiles: Vec<String> = ["power-saver", "balanced"].iter().map(|s| s.to_string()).collect();
        let v = view(Some(&b), "balanced", &profiles);
        assert_eq!((v["text"].as_str(), v["state"].as_str()), (Some("15 %"), Some("warn")));
        let card = v["card"].as_array().unwrap();
        let modes = card[1]["list"].as_array().unwrap();
        assert_eq!(modes.len(), 2);
        assert_eq!((modes[1]["id"].as_str(), modes[1]["on"].as_bool()), (Some("profile:balanced"), Some(true)));
        let limits = card[3]["list"].as_array().unwrap();
        assert!(limits.iter().any(|r| r["id"] == "limit:80" && r["on"] == true));
        assert_eq!(view(None, "", &[])["state"], "off");
    }

    #[test]
    fn low_battery_warns_once_per_level_then_sleeps_a_minute_after_five() {
        let t = Instant::now();
        let mut low = Low::default();
        assert_eq!(low.step(true, 30, t), LowAct::Nothing);
        assert_eq!(low.step(true, 20, t), LowAct::Warn(20));
        assert_eq!(low.step(true, 19, t), LowAct::Nothing);
        assert_eq!(low.step(true, 9, t), LowAct::Warn(10));
        assert_eq!(low.step(true, 5, t), LowAct::Warn(5));
        assert_eq!(low.step(true, 4, t + Duration::from_secs(30)), LowAct::Nothing);
        assert_eq!(low.step(true, 4, t + Duration::from_secs(61)), LowAct::Sleep);
        // plugged in: it starts over
        assert_eq!(low.step(false, 4, t), LowAct::Nothing);
        assert_eq!(low.step(true, 15, t), LowAct::Warn(20));
    }

    #[test]
    fn the_modes_come_from_one_answer_in_the_drop_downs_order() {
        let answer = r#"aa{sv} 3 2 "Driver" s "intel_pstate" "Profile" s "performance" 2 "Driver" s "intel_pstate" "Profile" s "balanced" 2 "Driver" s "platform_profile" "Profile" s "power-saver""#;
        assert_eq!(parse_profiles(answer), ["power-saver", "balanced", "performance"]);
        assert_eq!(parse_profiles(r#"aa{sv} 1 2 "Driver" s "placeholder" "Profile" s "balanced""#), ["balanced"]);
        assert!(parse_profiles("").is_empty());
    }

    #[test]
    fn only_set_limit_runs_through_pkexec() {
        // the polkit action lets an administrator run the program as root with any arguments
        assert!(allowed_through_pkexec(Some("set-limit")));
        for other in [Some("watch"), Some("press"), Some("json"), Some("restore"), None] {
            assert!(!allowed_through_pkexec(other), "{other:?}");
        }
    }

    #[test]
    fn the_sleep_delay_drop_in() {
        assert_eq!(
            sleep_delay_text(1500),
            "# Written by Settings > Power (the lid): suspend-then-hibernate hibernates after this long asleep.\n[Sleep]\nHibernateDelaySec=1500s\n"
        );
    }

    #[test]
    fn the_power_mode_follows_the_power_source() {
        assert_eq!(mode_for("", true).as_deref(), Some("performance"));
        assert_eq!(mode_for("", false).as_deref(), Some("balanced"));
        assert_eq!(mode_for("plugged_in = balanced\non_battery = power-saver\n", false).as_deref(), Some("power-saver"));
        assert_eq!(mode_for("plugged_in = keep\n", true), None);
        assert_eq!(mode_for("on_battery = turbo\n", false), None, "not a mode");
        let root = std::env::temp_dir().join(format!("ygg-ac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        bat(&root.join("AC"), &[("type", "Mains"), ("online", "1")]);
        bat(&root.join("BAT0"), &[("type", "Battery")]);
        assert_eq!(on_mains(&root), Some(true));
        bat(&root.join("AC"), &[("online", "0")]);
        assert_eq!(on_mains(&root), Some(false));
        std::fs::remove_dir_all(&root).unwrap();
        let desk = std::env::temp_dir().join(format!("ygg-noac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&desk);
        bat(&desk.join("BAT0"), &[("type", "Battery")]);
        assert_eq!(on_mains(&desk), None, "no mains supply listed");
        std::fs::remove_dir_all(&desk).unwrap();
    }

    #[test]
    fn thresholds_are_written_in_an_order_the_kernel_accepts() {
        assert_eq!(
            threshold_writes(80),
            [("charge_control_start_threshold", 0), ("charge_control_end_threshold", 80), ("charge_control_start_threshold", 75)]
        );
        assert_eq!(threshold_writes(100)[2], ("charge_control_start_threshold", 0));
        assert_eq!(parse_busctl_string("s \"balanced\""), "balanced");
        assert_eq!(duration(45), "45 min");
        assert_eq!(duration(120), "2 h");
    }
}
