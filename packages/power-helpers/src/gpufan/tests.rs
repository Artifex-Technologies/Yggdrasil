//! gpufan without a card (the cases of helpers/python/tests/test_gpufan.py, with the same expectations): the curve, the speed
//! for a temperature (minimum, hot), settling, the saved setting, and the service's loop against a fake card (the fans always
//! go back to the card at the end).

use super::*;
use crate::testutil::{ctx, TempDir};

struct FakeCard {
    temps: Vec<i64>,
    fail_at: Option<usize>,
    reads: usize,
    sets: Vec<(u32, i64)>,
    defaults: Vec<u32>,
}

impl FakeCard {
    fn new(temps: &[i64], fail_at: Option<usize>) -> FakeCard {
        FakeCard { temps: temps.to_vec(), fail_at, reads: 0, sets: Vec::new(), defaults: Vec::new() }
    }

    fn speeds(&self, fan: u32) -> Vec<i64> {
        self.sets.iter().filter(|(i, _)| *i == fan).map(|(_, p)| *p).collect()
    }

    fn defaults_sorted(&self) -> Vec<u32> {
        let mut d = self.defaults.clone();
        d.sort();
        d
    }
}

impl Card for FakeCard {
    fn fans(&mut self) -> Result<u32, NvmlError> {
        Ok(2)
    }
    fn minmax(&mut self) -> Result<(i64, i64), NvmlError> {
        Ok((24, 100))
    }
    fn temp(&mut self) -> Result<i64, NvmlError> {
        self.reads += 1;
        if self.fail_at.is_some_and(|f| self.reads >= f) {
            return Err(NvmlError("the card went away".into()));
        }
        Ok(self.temps[(self.reads - 1).min(self.temps.len() - 1)])
    }
    fn set_speed(&mut self, i: u32, pct: i64) -> Result<(), NvmlError> {
        self.sets.push((i, pct));
        Ok(())
    }
    fn set_default(&mut self, i: u32) -> Result<(), NvmlError> {
        self.defaults.push(i);
        Ok(())
    }
}

fn cfg(mode: &str, fixed: i64, curve: &[[i64; 2]]) -> Cfg {
    Cfg { mode: mode.into(), fixed, curve: curve.to_vec() }
}

// ---------------------------------------------------------------- Curve

#[test]
fn test_parse_and_refuse() {
    assert_eq!(parse_curve("40:30, 60:45,85:100"), Ok(vec![[40, 30], [60, 45], [85, 100]]));
    let nine = (0..9).map(|i| format!("{}:{}", 20 + 5 * i, 10 * i)).collect::<Vec<_>>().join(",");
    for bad in ["40:30", "40-30,60:45", "60:30,40:45", "40:50,60:40", "10:30,60:45", "40:30,60:120", "a:b,c:d", &nine] {
        assert!(parse_curve(bad).is_err(), "{bad}");
    }
    assert_eq!(parse_curve("40-30,60:45"), Err("'40-30' is not TEMPERATURE:PERCENT".into()), "the Python message");
}

#[test]
fn test_straight_lines_between_points() {
    let pts = [[40, 30], [60, 50], [80, 100]];
    assert_eq!(curve_at(&pts, 20), 30, "below the first point");
    assert_eq!(curve_at(&pts, 50), 40);
    assert_eq!(curve_at(&pts, 70), 75);
    assert_eq!(curve_at(&pts, 95), 100, "above the last point");
}

#[test]
fn test_never_below_the_cards_minimum_and_full_when_hot() {
    let c = cfg("curve", 60, &[[40, 0], [70, 50], [90, 80]]);
    assert_eq!(target(&c, 30, 24), 24);
    assert_eq!(target(&c, 84, 24), 71);
    assert_eq!(target(&c, 85, 24), 100, "hot: full speed whatever the curve");
    let fixed = Cfg { mode: "fixed".into(), fixed: 10, ..c.clone() };
    assert_eq!(target(&fixed, 50, 24), 24);
    assert_eq!(target(&fixed, 90, 24), 100);
}

#[test]
fn test_settling_up_at_once_down_when_cooler() {
    assert_eq!(settle(Some((40, 60)), 55, 63), 55);
    assert_eq!(settle(Some((55, 63)), 50, 61), 55, "only 2 C cooler: stays");
    assert_eq!(settle(Some((55, 63)), 45, 60), 45, "3 C cooler: comes down");
    assert_eq!(settle(None, 30, 40), 30);
}

#[test]
fn test_the_saved_setting_is_cleaned() {
    let d = TempDir::new();
    let c = ctx(d.path());
    let path = d.path().join("gpu-fan.json");
    assert_eq!(load(&c, &path).mode, "auto", "nothing saved: the card's own control");
    std::fs::write(&path, r#"{"mode": "turbo", "fixed": 300, "curve": [[60, 30], [40, 50]]}"#).unwrap();
    let got = load(&c, &path);
    assert_eq!((got.mode.as_str(), got.fixed, got.curve), ("auto", 60, DEFAULT_CURVE.to_vec()));
    let mine = cfg("curve", 70, &[[45, 35], [80, 90]]);
    save(&c, &mine, &path).unwrap();
    assert_eq!(load(&c, &path), mine);
}

#[test]
fn test_set_needs_root_and_changes_nothing_without_it() {
    let d = TempDir::new();
    let c = ctx(d.path());
    c.as_root.set(Some(false));
    assert_eq!(cmd_set(&c, "fixed=50"), 1);
    assert!(!d.path().join("etc/yggdrasil/gpu-fan.json").exists());
    assert!(c.recorded.borrow().as_ref().unwrap().is_empty());
}

// ---------------------------------------------------------------- Service

/// The setting given once, then nothing new; no waiting; the clock at 0 (REAPPLY_S never comes); a stop after `steps` turns.
struct Drive {
    given: Option<Cfg>,
    steps: usize,
    n: usize,
}

impl Env for Drive {
    fn read_cfg(&mut self) -> Option<Cfg> {
        self.given.take()
    }
    fn load(&mut self) -> Cfg {
        clean(&Json::Null)
    }
    fn sleep(&mut self, _: i64) {}
    fn should_stop(&mut self) -> bool {
        self.n += 1;
        self.n > self.steps
    }
    fn clock(&mut self) -> f64 {
        0.0
    }
}

fn drive(card: &mut FakeCard, c: Cfg, steps: usize) -> i32 {
    run(card, &mut Drive { given: Some(c), steps, n: 0 })
}

#[test]
fn test_follows_the_curve_settles_and_gives_back() {
    let mut card = FakeCard::new(&[40, 60, 75, 73, 71, 70], None);
    assert_eq!(drive(&mut card, cfg("curve", 60, &[[40, 30], [60, 45], [75, 75], [85, 100]]), 6), 0);
    assert_eq!(card.speeds(0), [30, 45, 75, 67], "up at once; at 73 C it stays at 75 %; at 71 C (3 cooler) it comes down to 67 %, and at 70 C it stays there");
    assert_eq!(card.defaults_sorted(), [0, 1], "both fans back to the card at the end");
}

#[test]
fn test_a_card_that_cannot_be_read_goes_back_to_its_own_control() {
    let mut card = FakeCard::new(&[50, 50, 50], Some(3));
    assert_eq!(drive(&mut card, cfg("fixed", 70, &DEFAULT_CURVE), 10), 1);
    assert_eq!(card.defaults_sorted(), [0, 1]);
}

#[test]
fn test_auto_ends_the_loop() {
    let mut card = FakeCard::new(&[50], None);
    assert_eq!(drive(&mut card, cfg("auto", 60, &DEFAULT_CURVE), 5), 0);
    assert!(card.sets.is_empty());
    assert_eq!(card.defaults_sorted(), [0, 1]);
}

#[test]
fn test_hot_overrides_a_low_fixed_speed() {
    let mut card = FakeCard::new(&[70, 86], None);
    drive(&mut card, cfg("fixed", 30, &DEFAULT_CURVE), 2);
    assert_eq!(card.speeds(1), [30, 100]);
}

// ---------------------------------------------------------------- beyond the Python tests

#[test]
fn the_fake_card_of_a_fake_root_and_a_dry_run() {
    let d = TempDir::new();
    d.files("", &[("nvml.json", r#"{"temp": [50, 51], "fans": [40, 41], "min": 30, "max": 100}"#)]);
    let c = ctx(d.path());
    let mut nv = Nvml::open(&c).unwrap();
    assert_eq!((nv.fans(), nv.minmax(), nv.speed(1)), (Ok(2), Ok((30, 100)), Ok(41)));
    assert_eq!((nv.temp(), nv.temp()), (Ok(50), Ok(51)));
    assert_eq!(nv.temp(), Err(NvmlError("nvmlDeviceGetTemperature failed (NVML error 15)".into())), "the readings ran out: the card is lost");
    let gone = TempDir::new();
    assert_eq!(Nvml::open(&ctx(gone.path())).err(),
               Some(NvmlError("libnvidia-ml is not available (libnvidia-ml.so.1: cannot open shared object file: No such file or directory)".into())));
}
