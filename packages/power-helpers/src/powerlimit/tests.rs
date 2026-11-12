//! powerlimit without hardware: the cases of tests/test_settings_powerlimit.py and of helpers/tests/test_gpuunlock.py's
//! PowerHold (they test this helper's hold), with the same names and expectations, then what the Python tests left out: the
//! mode from busctl, the card from nvidia-smi, set, get and the watch, against fake trees with the commands' answers in
//! ROOT/commands.json (commands that change something are recorded, never run).

use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::testutil::{ctx, TempDir};

fn obj(text: &str) -> Obj {
    json::parse(text).unwrap().obj().unwrap().clone()
}

fn dumps(o: Obj) -> String {
    json::dumps(&o.into())
}

const RTX: &str = "NVIDIA GeForce RTX 2080 Ti, 260.00, 260.00, 100.00, 330.00\n";

/// A fake tree: four processor policies (0.8-4.0 GHz), the mode and the card through commands.json, and a limits file.
fn tree(d: &TempDir, mode: &str, limits: Option<&str>) {
    for i in 0..4 {
        d.files(
            &format!("sys/devices/system/cpu/cpufreq/policy{i}"),
            &[("cpuinfo_min_freq", "800000"), ("cpuinfo_max_freq", "4000000"), ("scaling_max_freq", "4000000")],
        );
    }
    let cmds = format!(
        r#"{{"busctl get-property org.freedesktop.UPower.PowerProfiles": "s \"{mode}\"\n", "nvidia-smi --query-gpu": {}}}"#,
        json::dumps(&Json::Str(RTX.into()))
    );
    d.files("", &[("commands.json", &cmds)]);
    if let Some(l) = limits {
        d.files("etc/yggdrasil", &[("power-limits.json", l)]);
    }
}

fn recorded(c: &Ctx, start: &str) -> Vec<String> {
    c.recorded.borrow().as_ref().unwrap().iter().filter(|l| l.starts_with(start)).cloned().collect()
}

fn speed_cap(d: &TempDir, i: usize) -> String {
    fs::read_to_string(d.path().join(format!("sys/devices/system/cpu/cpufreq/policy{i}/scaling_max_freq"))).unwrap()
}

// ---------------------------------------------------------------- tests/test_settings_powerlimit.py

#[test]
fn test_cpu_ceiling_never_goes_below_what_the_processor_can_run_at() {
    assert_eq!(cpu_target_khz(100, 800000, 5000000), 5000000);
    assert_eq!(cpu_target_khz(50, 800000, 5000000), 2500000);
    assert_eq!(cpu_target_khz(5, 800000, 5000000), 800000);
}

#[test]
fn test_cpu_limits_are_written_to_every_policy_and_restored() {
    let d = TempDir::new();
    for i in 0..3 {
        d.files(&format!("policy{i}"), &[("cpuinfo_min_freq", "800000"), ("cpuinfo_max_freq", "4000000"), ("scaling_max_freq", "4000000")]);
    }
    let c = ctx(d.path());
    assert_eq!(apply_cpu(&c, Some(75), Some(d.path())), 3);
    for i in 0..3 {
        assert_eq!(fs::read_to_string(d.path().join(format!("policy{i}/scaling_max_freq"))).unwrap(), "3000000");
    }
    apply_cpu(&c, None, Some(d.path()));
    assert_eq!(fs::read_to_string(d.path().join("policy0/scaling_max_freq")).unwrap(), "4000000");
}

#[test]
fn test_state_is_validated() {
    let d = TempDir::new();
    let p = d.path().join("s.json");
    fs::write(&p, r#"{"balanced": {"cpu": 80, "gpu": 200, "x": 1}, "turbo": {"cpu": 100}, "performance": {"cpu": 500, "gpu": "a"}, "power-saver": {"cpu": 40}}"#).unwrap();
    let c = ctx(d.path());
    assert_eq!(dumps(load_state(&c, Some(&p))), r#"{"balanced": {"cpu": 80, "gpu": 200}, "power-saver": {"cpu": 40}}"#);
    save_state(&c, &obj(r#"{"balanced": {"cpu": 60}}"#), Some(&p)).unwrap();
    assert_eq!(dumps(load_state(&c, Some(&p))), r#"{"balanced": {"cpu": 60}}"#);
    assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o644);
    assert!(load_state(&c, Some(&d.path().join("none.json"))).is_empty());
}

// ---------------------------------------------------------------- helpers/tests/test_gpuunlock.py, PowerHold

#[test]
fn test_performance_may_use_290_without_a_hold() {
    let d = TempDir::new();
    let c = ctx(d.path());
    assert!(hold_cap(&c, 260, Some(&d.path().join("power-hold"))).is_none());
    assert_eq!(gpu_target(Some(290), 260, 130, 290, None), 290);
}

#[test]
fn test_a_hold_keeps_every_mode_at_the_default() {
    let d = TempDir::new();
    let c = ctx(d.path());
    let hold = d.path().join("power-hold");
    fs::write(&hold, r#"{"watts": null}"#).unwrap();
    let cap = hold_cap(&c, 260, Some(&hold)).and_then(|v| v.int());
    assert_eq!(cap, Some(260));
    assert_eq!(gpu_target(Some(290), 260, 130, 290, cap), 260);
    assert_eq!(gpu_target(Some(200), 260, 130, 290, cap), 200, "a lower mode limit stays");
    assert_eq!(gpu_target(None, 260, 130, 290, cap), 260);
}

#[test]
fn test_the_heat_cap_and_a_broken_file_still_hold() {
    let d = TempDir::new();
    let c = ctx(d.path());
    let hold = d.path().join("power-hold");
    let cap = |text: &str| {
        fs::write(&hold, text).unwrap();
        hold_cap(&c, 260, Some(&hold)).and_then(|v| v.int())
    };
    assert_eq!(cap(r#"{"watts": 200}"#), Some(200));
    assert_eq!(cap(r#"{"watts": 400}"#), Some(260), "a hold never raises");
    assert_eq!(cap("not json"), Some(260));
}

// ---------------------------------------------------------------- beyond the Python tests

#[test]
fn the_mode_comes_from_power_profiles_daemon_newer_name_first() {
    let d = TempDir::new();
    let ask = |answers: &str| {
        d.files("", &[("commands.json", answers)]);
        active_mode(&ctx(d.path()))
    };
    assert_eq!(ask(r#"{"busctl get-property org.freedesktop.UPower.PowerProfiles": " s \"performance\"\n"}"#), "performance");
    // the newer name not there (an older daemon): the old one answers
    assert_eq!(
        ask(
            r#"{"busctl get-property org.freedesktop.UPower.PowerProfiles": {"rc": 1, "out": ""}, "busctl get-property net.hadess.PowerProfiles": "s \"power-saver\""}"#
        ),
        "power-saver"
    );
    assert_eq!(
        ask(
            r#"{"busctl get-property org.freedesktop.UPower.PowerProfiles": {"timeout": true}, "busctl get-property net.hadess.PowerProfiles": null}"#
        ),
        ""
    );
    assert_eq!(ask("{}"), "", "no busctl at all");
}

#[test]
fn the_cards_limits_come_from_nvidia_smi() {
    assert_eq!(
        parse_gpu_info(RTX),
        Some(GpuInfo { name: "NVIDIA GeForce RTX 2080 Ti".into(), watts: 260, default: 260, min: 100, max: 330 })
    );
    assert_eq!(
        parse_gpu_info("X, 250.50, 251.50, 99.5, 330\nY, 1, 2, 3, 4\n").map(|g| (g.watts, g.default, g.min, g.name)),
        Some((250, 252, 100, "X".into())),
        "halves to even, first card"
    );
    for bad in [
        "X, [N/A], 260.00, 100.00, 330.00",
        "X, 260, 260, 100",
        "X, 260, 260, 100, 330, 1",
        "X, inf, 260, 100, 330",
        "X, nan, 260, 100, 330",
    ] {
        assert_eq!(parse_gpu_info(bad), None, "{bad}");
    }
    let d = TempDir::new();
    for (answer, want) in [
        (r#"{"rc": 9, "out": "NVIDIA-SMI has failed"}"#, false),
        (r#"" \n""#, false),
        ("null", false),
        (&json::dumps(&Json::Str(RTX.into())) as &str, true),
    ] {
        d.files("", &[("commands.json", &format!(r#"{{"nvidia-smi --query-gpu": {answer}}}"#))]);
        assert_eq!(gpu_info(&ctx(d.path())).is_some(), want, "{answer}");
    }
}

#[test]
fn set_saves_sorted_and_applies_only_the_active_mode() {
    let d = TempDir::new();
    tree(&d, "balanced", None);
    let c = ctx(d.path());
    assert_eq!(cmd_set(&c, "performance", &["gpu=300".into()]), 0);
    assert!(recorded(&c, "nvidia-smi -pl").is_empty() && speed_cap(&d, 0) == "4000000", "not the active mode: nothing applied");
    assert_eq!(cmd_set(&c, "balanced", &["gpu=0200".into(), "cpu=80".into()]), 0);
    let text = fs::read_to_string(d.path().join("etc/yggdrasil/power-limits.json")).unwrap();
    assert_eq!(text, "{\n \"balanced\": {\n  \"cpu\": 80,\n  \"gpu\": 200\n },\n \"performance\": {\n  \"gpu\": 300\n }\n}");
    assert_eq!(recorded(&c, "nvidia-smi -pl"), ["nvidia-smi -pl 200"]);
    assert_eq!((0..4).map(|i| speed_cap(&d, i)).collect::<Vec<_>>(), ["3200000"; 4]);
    // default takes a limit away; a mode with none left goes from the file
    assert_eq!(cmd_set(&c, "performance", &["gpu=default".into()]), 0);
    assert_eq!(dumps(load_state(&c, None)), r#"{"balanced": {"cpu": 80, "gpu": 200}}"#);
}

#[test]
fn set_refuses_what_it_cannot_use() {
    let d = TempDir::new();
    tree(&d, "balanced", Some(r#"{"balanced": {"cpu": 70}}"#));
    let c = ctx(d.path());
    assert_eq!(cmd_set(&c, "turbo", &["cpu=50".into()]), 2);
    assert_eq!(cmd_set(&c, "balanced", &["fan=50".into()]), 2);
    assert_eq!(cmd_set(&c, "balanced", &["cpu=fast".into()]), 2);
    assert_eq!(cmd_set(&c, "balanced", &["cpu".into()]), 2);
    assert_eq!(cmd_set(&c, "balanced", &["cpu=-5".into()]), 2);
    c.as_root.set(Some(false));
    assert_eq!(cmd_set(&c, "balanced", &["cpu=50".into()]), 1);
    assert_eq!(fs::read_to_string(d.path().join("etc/yggdrasil/power-limits.json")).unwrap(), r#"{"balanced": {"cpu": 70}}"#, "untouched");
    // an out-of-range number is saved as given (as the Python helper did) and left out when read
    c.as_root.set(Some(true));
    assert_eq!(cmd_set(&c, "balanced", &["cpu=500".into()]), 0);
    assert!(fs::read_to_string(d.path().join("etc/yggdrasil/power-limits.json")).unwrap().contains("\"cpu\": 500"));
    assert!(load_state(&c, None).is_empty());
}

#[test]
fn the_old_place_is_read_until_the_first_change() {
    let d = TempDir::new();
    tree(&d, "power-saver", None);
    d.files("etc/labyrinth", &[("power-limits.json", r#"{"power-saver": {"cpu": 50}}"#)]);
    let c = ctx(d.path());
    assert_eq!(dumps(load_state(&c, None)), r#"{"power-saver": {"cpu": 50}}"#);
    assert_eq!(cmd_set(&c, "balanced", &["cpu=90".into()]), 0);
    assert_eq!(dumps(load_state(&c, None)), r#"{"balanced": {"cpu": 90}, "power-saver": {"cpu": 50}}"#);
    assert!(d.path().join("etc/yggdrasil/power-limits.json").exists());
}

#[test]
fn a_hold_caps_the_card_and_a_true_watts_stays_true_in_get() {
    let d = TempDir::new();
    tree(&d, "performance", Some(r#"{"performance": {"gpu": 320, "cpu": true}}"#));
    d.files("var/lib/yggdrasil/gpu-unlock", &[("power-hold", r#"{"watts": 200}"#)]);
    let c = ctx(d.path());
    let (mode, lim) = apply(&c, None, None);
    assert_eq!((mode.as_str(), dumps(lim)), ("performance", r#"{"cpu": true, "gpu": 320}"#.to_string()));
    assert_eq!(recorded(&c, "nvidia-smi -pl"), ["nvidia-smi -pl 200"]);
    assert_eq!(speed_cap(&d, 0), "800000", "true is 1 %: the lowest speed");
    d.files("var/lib/yggdrasil/gpu-unlock", &[("power-hold", r#"{"watts": true}"#)]);
    assert_eq!(hold_cap(&c, 260, None).map(|v| json::dumps(&v)), Some("true".into()));
}

#[test]
fn get_prints_what_settings_reads() {
    let d = TempDir::new();
    tree(&d, "balanced", Some(r#"{"balanced": {"gpu": 200}}"#));
    d.files("sys/devices/system/cpu/cpufreq/policy9", &[("cpuinfo_max_freq", "5000000")]);
    let c = ctx(d.path());
    assert_eq!(
        json::dumps(&get(&c)),
        r#"{"cpu_max_khz": 5000000, "gpu": {"name": "NVIDIA GeForce RTX 2080 Ti", "watts": 260, "default": 260, "min": 100, "max": 330, "hold": null}, "active": "balanced", "limits": {"balanced": {"gpu": 200}}}"#
    );
    d.files("var/lib/yggdrasil/gpu-unlock", &[("power-hold", "{}")]);
    d.files("", &[("commands.json", r#"{"nvidia-smi --query-gpu": null}"#)]);
    assert_eq!(
        json::dumps(&get(&ctx(d.path()))),
        r#"{"cpu_max_khz": 5000000, "gpu": null, "active": "", "limits": {"balanced": {"gpu": 200}}}"#
    );
}

#[test]
fn watch_applies_at_start_and_again_only_when_something_changed() {
    let d = TempDir::new();
    tree(&d, "balanced", Some(r#"{"balanced": {"gpu": 200, "cpu": 50}, "performance": {"gpu": 300}}"#));
    // one answer per busctl call: the round's look, then apply's own when it applies
    let modes = ["balanced", "balanced", "balanced", "balanced", "performance", "performance", "performance"];
    let answers = modes.iter().map(|m| format!("\"s \\\"{m}\\\"\"")).collect::<Vec<_>>().join(", ");
    let cmds = format!(
        r#"{{"busctl get-property org.freedesktop.UPower.PowerProfiles": [{answers}], "nvidia-smi --query-gpu": {}}}"#,
        json::dumps(&Json::Str(RTX.into()))
    );
    d.files("", &[("commands.json", &cmds)]);
    let mut c = ctx(d.path());
    c.steps = Some(4);
    assert_eq!(cmd_watch(&c), 0);
    assert_eq!(recorded(&c, "nvidia-smi -pl"), ["nvidia-smi -pl 200", "nvidia-smi -pl 300"], "at start, then at the mode change only");
    assert_eq!(recorded(&c, "busctl").len(), 6);
    assert_eq!(speed_cap(&d, 0), "4000000", "performance has no processor limit: the full speed again");
}

#[test]
fn apply_prints_the_mode_and_its_limits_as_python_did() {
    let d = TempDir::new();
    tree(&d, "balanced", Some(r#"{"balanced": {"cpu": 80}}"#));
    let c = ctx(d.path());
    let (m, l) = apply(&c, None, None);
    assert_eq!(format!("({}, {})", py::repr(&m), Json::Obj(l).repr()), "('balanced', {'cpu': 80})");
}
