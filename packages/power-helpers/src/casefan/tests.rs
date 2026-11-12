//! casefan without the hardware (the cases of helpers/python/tests/test_casefan.py, with the same expectations): the speed
//! for each port (pump floor, curves from both parts, hot), settling, the saved setting, the stop speeds, the Commander Pro
//! found in a fake hwmon tree, and the service loop (a sensor failure puts every port at 100 %).

use std::collections::VecDeque;

use super::*;
use crate::json::parse;
use crate::testutil::{ctx, TempDir};

fn cfg(text: &str) -> Cfg {
    clean(&parse(text).unwrap()).unwrap()
}

fn port(text: &str, n: &str) -> Port {
    cfg(text).port(n).unwrap().clone()
}

fn pump() -> Port {
    Port { name: "pump".into(), role: Role::Pump { fixed: 100 } }
}

/// FAN = m.clean({"ports": {"6": {"role": "fan"}}})["ports"]["6"]
fn fan() -> Port {
    port(r#"{"ports": {"6": {"role": "fan"}}}"#, "6")
}

fn curve_of(p: &Port) -> (Curve, Curve, Option<String>) {
    match &p.role {
        Role::Fan { cpu, gpu, follow } => (cpu.clone(), gpu.clone(), follow.clone()),
        Role::Pump { .. } => panic!("a pump"),
    }
}

// ---------------------------------------------------------------- Speeds

#[test]
fn test_pump_never_below_its_floor() {
    assert_eq!(port(r#"{"ports": {"1": {"role": "pump", "fixed": 10}}}"#, "1").role, Role::Pump { fixed: PUMP_MIN_PCT });
    assert_eq!(target(&pump(), 40, 30), 100);
}

#[test]
fn test_fan_follows_the_hotter_part() {
    let idle = target(&fan(), 45, 35);
    assert_eq!(idle, 25);
    assert!(target(&fan(), 45, 70) > 60, "the card working, the processor idle");
    assert!(target(&fan(), 84, 35) > 70, "the processor working, the card idle");
}

#[test]
fn test_hot_is_full_speed() {
    assert_eq!(target(&fan(), CPU_HOT_C, 30), 100);
    assert_eq!(target(&fan(), 40, GPU_HOT_C), 100);
}

#[test]
fn test_never_below_the_fan_floor() {
    let low = port(r#"{"ports": {"2": {"role": "fan", "cpu": [[30, 0], [90, 100]], "gpu": [[30, 0], [90, 100]]}}}"#, "2");
    assert_eq!(target(&low, 30, 30), FAN_MIN_PCT);
}

#[test]
fn test_settle_waits_before_slowing() {
    assert_eq!(settle(Some((60, -10.0)), 40, -11.0), 60);
    assert_eq!(settle(Some((60, -10.0)), 40, -13.0), 40);
    assert_eq!(settle(Some((60, -10.0)), 80, -11.0), 80);
}

#[test]
fn test_stop_speeds() {
    assert_eq!(stop_speed(Some(&fan()), false), STOP_FAN_PCT);
    assert_eq!(stop_speed(Some(&fan()), true), 100);
    assert_eq!(stop_speed(Some(&pump()), false), 100);
}

// ---------------------------------------------------------------- Setting

#[test]
fn test_bad_entries_are_dropped_or_defaulted() {
    let c = cfg(r#"{"ports": {"9": {"role": "fan"}, "x": {}, "3": {"role": "fan", "cpu": [[60, 50], [50, 40]]}}}"#);
    assert_eq!(c.ports.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["3"]);
    assert_eq!(curve_of(c.port("3").unwrap()).0, DEFAULT_CPU.to_vec());
}

#[test]
fn test_round_trip() {
    let d = TempDir::new();
    let c = ctx(d.path());
    let p = d.path().join("case-fans.json");
    let both = Cfg { ports: vec![("1".into(), pump()), ("6".into(), fan())] };
    save(&c, &both, &p).unwrap();
    assert!(load(&c, &p).unwrap().port("1").unwrap().is_pump());
}

// ---------------------------------------------------------------- Hardware

#[test]
fn test_finds_ports_and_temperature() {
    let d = TempDir::new();
    let c = ctx(d.path());
    d.files(
        "hwmon5",
        &[
            ("name", "corsaircpro"),
            ("fan1_label", "fan1 4pin"),
            ("fan1_input", "3846"),
            ("fan6_label", "fan6 4pin"),
            ("fan6_input", "1399"),
            ("pwm1", ""),
            ("pwm6", ""),
        ],
    );
    d.files("hwmon3", &[("name", "coretemp"), ("temp1_label", "Package id 0"), ("temp1_input", "53000")]);
    let cp = find_cpro(&c, d.path()).unwrap();
    assert!(cp.ends_with("hwmon5"));
    let conn = |label: &str, rpm, pct| Connected { label: label.into(), rpm, pct };
    assert_eq!(ports(&c, &cp), vec![("1".to_string(), conn("fan1 4pin", 3846, None)), ("6".to_string(), conn("fan6 4pin", 1399, None))]);
    assert_eq!(cpu_temp(&c, d.path()), Ok(53));
    set_pct(&c, &cp, "6", 50).unwrap();
    assert_eq!(std::fs::read_to_string(cp.join("pwm6")).unwrap(), "128");
    assert_eq!(ports(&c, &cp)[1].1.pct, Some(50), "the speed last given, as Settings shows it");
}

// ---------------------------------------------------------------- the loop

/// The loop's outside, as the Python tests drive it: the setting given until the first write, temperatures from a list
/// (the processor's read first; reading past the end is a sensor failure), writes recorded, no waiting, the clock at 0.
struct Fake {
    cfg: Cfg,
    temps: VecDeque<(i64, i64)>,
    writes: Vec<(String, i64)>,
}

impl Fake {
    fn new(cfg: Cfg, temps: Vec<(i64, i64)>) -> Fake {
        Fake { cfg, temps: temps.into(), writes: Vec::new() }
    }

    fn speeds(&self) -> Vec<i64> {
        self.writes.iter().map(|(_, p)| *p).collect()
    }
}

impl Env for Fake {
    fn read_cfg(&mut self) -> Option<Result<Cfg, String>> {
        self.writes.is_empty().then(|| Ok(self.cfg.clone()))
    }
    fn load(&mut self) -> Result<Cfg, String> {
        clean(&Json::Null)
    }
    fn read_cpu(&mut self) -> Result<i64, Fail> {
        self.temps.front().map(|t| t.0).ok_or(Fail::Sensor("gone".into()))
    }
    fn read_gpu(&mut self) -> Result<i64, Fail> {
        Ok(self.temps.pop_front().unwrap().1)
    }
    fn write(&mut self, port: &str, pct: i64) -> Result<(), OsErr> {
        self.writes.push((port.to_string(), pct));
        Ok(())
    }
    fn sleep(&mut self, _: i64) {}
    fn should_stop(&mut self) -> bool {
        false
    }
    fn clock(&mut self) -> f64 {
        0.0
    }
}

#[test]
fn test_runs_then_full_speed_on_a_sensor_failure() {
    let mut f = Fake::new(
        cfg(r#"{"ports": {"1": {"name": "pump", "role": "pump", "fixed": 100}, "6": {"role": "fan"}}}"#),
        vec![(45, 35), (45, 35), (84, 35)],
    );
    assert_eq!(run(&mut f), 1);
    let w = |n: &str, p| (n.to_string(), p);
    assert!(f.writes.contains(&w("1", 100)));
    assert!(f.writes.contains(&w("6", 25)));
    assert!(f.writes.iter().any(|(n, p)| n == "6" && *p > 70), "the processor got hot");
    assert_eq!(f.writes[f.writes.len() - 2..], [w("1", 100), w("6", 100)], "then the sensor failed: everything at full speed");
}

// ---------------------------------------------------------------- Calm: the night-3 pattern, each eval case swinging the
// processor 42-64 C and the card 54-60 C within seconds

#[test]
fn test_average_and_ramp_keep_the_fan_steady() {
    let swing = [(58, 60), (58, 60), (43, 54), (42, 54), (58, 59), (64, 60), (58, 60), (43, 55), (42, 54)];
    let temps: Vec<_> = swing.iter().cycle().take(swing.len() * 8).copied().collect();
    let mut f = Fake::new(cfg(r#"{"ports": {"6": {"role": "fan"}}}"#), temps);
    run(&mut f);
    let all = f.speeds();
    let speeds = &all[..all.len() - 1]; // everything before the failure's own 100 % (a speed is written only when it changes)
    assert!(speeds.iter().max().unwrap() - speeds.iter().min().unwrap() <= 6, "{speeds:?}");
    assert!(speeds.len() < swing.len() * 8 / 4, "72 readings, only a handful of changes: {speeds:?}");
    let steps: Vec<i64> = speeds.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    assert!(steps.iter().max().copied().unwrap_or(0) <= RAMP_UP_PCT);
    assert!((30..=50).contains(speeds.last().unwrap()), "where the averaged temperatures put it on the curve: {speeds:?}");
}

#[test]
fn test_hot_is_still_immediate() {
    let mut temps = vec![(50, 40); 5];
    temps.push((CPU_HOT_C, 40));
    let mut f = Fake::new(cfg(r#"{"ports": {"6": {"role": "fan"}}}"#), temps);
    run(&mut f);
    let w = f.speeds();
    assert_eq!(w[w.len() - 2], 100, "the hot reading, before the failure's own 100 %");
    assert!(w[w.len() - 3] < 50);
}

#[test]
fn test_ramp() {
    assert_eq!(ramp(None, 60), 60);
    assert_eq!(ramp(Some(40), 60), 44);
    assert_eq!(ramp(Some(60), 40), 59);
}

// ---------------------------------------------------------------- Follow: a fan can run on another fan's curves (my request,
// 2026-11-05: the exhaust on the 80mm fan's map)

const QUIET: &str = "[[55, 35], [70, 40], [85, 80], [92, 100]]";

#[test]
fn test_a_follower_gets_the_leaders_curves() {
    let c =
        cfg(&format!(r#"{{"ports": {{"5": {{"role": "fan", "cpu": {QUIET}, "gpu": {QUIET}}}, "6": {{"role": "fan", "follow": "5"}}}}}}"#));
    let (cpu, _, follow) = curve_of(c.port("6").unwrap());
    assert_eq!(follow.as_deref(), Some("5"));
    assert_eq!(cpu, vec![[55, 35], [70, 40], [85, 80], [92, 100]]);
    assert_eq!(target(c.port("6").unwrap(), 70, 30), target(c.port("5").unwrap(), 70, 30));
}

#[test]
fn test_no_self_pump_chain_or_missing_leader() {
    let c = cfg(r#"{"ports": {"1": {"role": "pump", "fixed": 100}, "4": {"role": "fan", "follow": "4"},
                              "5": {"role": "fan", "follow": "6"}, "6": {"role": "fan", "follow": "5"},
                              "3": {"role": "fan", "follow": "1"}, "2": {"role": "fan", "follow": "9"}}}"#);
    for n in ["2", "3", "4", "5", "6"] {
        let (cpu, _, follow) = curve_of(c.port(n).unwrap());
        assert_eq!(follow, None, "{n}");
        assert_eq!(cpu, DEFAULT_CPU.to_vec(), "{n}");
    }
}

#[test]
fn test_a_pump_never_follows() {
    let c = cfg(r#"{"ports": {"1": {"role": "pump", "fixed": 80, "follow": "6"}, "6": {"role": "fan"}}}"#);
    assert_eq!(c.port("1").unwrap().role, Role::Pump { fixed: 80 });
    assert!(!crate::json::dumps(&to_json(&c)).contains("follow"));
}

// ---------------------------------------------------------------- SetFromStdin: Settings sends the setting on stdin
// (`pkexec casefan set -`); it is cleaned like a file's

#[test]
fn test_stdin_is_cleaned_and_saved() {
    let d = TempDir::new();
    let c = ctx(d.path());
    let mut stdin = std::io::Cursor::new(r#"{"ports": {"1": {"role": "pump", "fixed": 10}, "6": {"role": "fan"}, "x": {}}}"#);
    assert_eq!(cmd_set_from(&c, "-", &mut stdin), 0);
    let saved = load(&c, &d.path().join("etc/yggdrasil/case-fans.json")).unwrap();
    assert_eq!(saved.port("1").unwrap().role, Role::Pump { fixed: PUMP_MIN_PCT });
    let mut names: Vec<_> = saved.ports.iter().map(|(n, _)| n.as_str()).collect();
    names.sort();
    assert_eq!(names, ["1", "6"]);
    assert!(c.recorded.borrow().as_ref().unwrap().contains(&format!("systemctl restart {SERVICE}")));
}

// ---------------------------------------------------------------- beyond the Python tests

#[test]
fn an_unusable_setting_fails_safe() {
    // "ports" that is not an object crashed the Python helper (leaving the ports as they were); here it is a failure
    assert!(clean(&parse(r#"{"ports": [1]}"#).unwrap()).is_err());
    assert_eq!(clean(&parse(r#"{"ports": null}"#).unwrap()), Ok(Cfg::default()));
    let d = TempDir::new();
    let c = ctx(d.path());
    let cp =
        d.files("sys/class/hwmon/hwmon5", &[("name", "corsaircpro"), ("fan1_label", "fan1 4pin"), ("fan1_input", "3846"), ("pwm1", "100")]);
    d.files("etc/yggdrasil", &[("case-fans.json", r#"{"ports": [1]}"#)]);
    assert_eq!(cmd_reset(&c, true), 0);
    assert_eq!(std::fs::read_to_string(cp.join("pwm1")).unwrap(), "255", "a failed stop still puts the port at 100 %");
    std::fs::write(cp.join("pwm1"), "100").unwrap();
    assert_eq!(cmd_reset(&c, false), 0);
    assert_eq!(std::fs::read_to_string(cp.join("pwm1")).unwrap(), "255", "so does a clean one: which port is the pump is unknown");
}

#[test]
fn writes_round_halves_to_even_as_python() {
    let d = TempDir::new();
    let c = ctx(d.path());
    let cp = d.files("hwmon0", &[("pwm1", "")]);
    for (pct, raw) in [(30, "76"), (10, "26"), (70, "178"), (90, "230"), (100, "255"), (0, "0")] {
        set_pct(&c, &cp, "1", pct).unwrap();
        assert_eq!(std::fs::read_to_string(cp.join("pwm1")).unwrap(), raw, "{pct} %");
    }
}
