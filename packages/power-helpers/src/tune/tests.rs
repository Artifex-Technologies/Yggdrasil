//! tune without touching the hardware (the cases of helpers/python/tests/test_tune.py, with the same expectations):
//! multipliers in and out of the MSR's bytes, what is refused, the crash guard across starts, trying and keeping, and the
//! graphics driver's errors. MSRs, NVML and the files are stand-ins; plus the kernel's messages read as /dev/kmsg records.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};

use super::*;
use crate::json::parse;
use crate::testutil::{ctx, TempDir};

const STOCK: [i64; 8] = [50, 50, 48, 48, 47, 47, 47, 47];

/// The functions the Python tests replaced: the MSRs as a map, the processor unlocked (base 36), the card's settings
/// recorded, publish and log doing nothing, the boot id set by the test.
struct Fake {
    msr: RefCell<HashMap<u64, u64>>,
    programmable: Cell<bool>,
    root: Cell<bool>,
    gpu: RefCell<Vec<GpuVals>>,
    boot: RefCell<String>,
    texts: RefCell<Option<BTreeMap<String, String>>>,
    rapl: Cell<Option<(Option<i64>, Option<i64>)>>,
}

impl Hw for Fake {
    fn is_root(&self, _: &Tune) -> bool {
        self.root.get()
    }
    fn log(&self, _: &Tune, _: &str) {}
    fn boot_id(&self, _: &Tune) -> String {
        self.boot.borrow().clone()
    }
    fn rdmsr(&self, _: &Tune, reg: u64) -> Result<u64, OsErr> {
        self.msr.borrow().get(&reg).copied().ok_or(OsErr::Msg(format!("no MSR {reg:#x}")))
    }
    fn wrmsr(&self, _: &Tune, reg: u64, value: u64) -> Result<(), OsErr> {
        self.msr.borrow_mut().insert(reg, value);
        Ok(())
    }
    fn platform(&self, _: &Tune) -> Result<(i64, bool), OsErr> {
        Ok((36, self.programmable.get()))
    }
    fn apply_gpu(&self, _: &Tune, v: &GpuVals) -> Result<(), OsErr> {
        self.gpu.borrow_mut().push(v.clone());
        Ok(())
    }
    fn publish(&self, _: &Tune, _: Option<f64>, _: Option<&Gpu>) -> Result<(), OsErr> {
        Ok(())
    }
    fn write_text(&self, t: &Tune, path: &Path, text: &str) -> Result<(), OsErr> {
        match self.texts.borrow_mut().as_mut() {
            Some(m) => {
                m.insert(path.file_name().unwrap().to_string_lossy().into_owned(), text.to_string());
                Ok(())
            }
            None => t.ctx.write_text(path, text),
        }
    }
    fn rapl_limits(&self, t: &Tune) -> (Option<i64>, Option<i64>) {
        match self.rapl.get() {
            Some(r) => r,
            None => System.rapl_limits(t),
        }
    }
}

struct Base {
    _dir: TempDir,
    ctx: Ctx,
    hw: Fake,
}

impl Base {
    fn new() -> Base {
        let dir = TempDir::new();
        let ctx = ctx(dir.path());
        let hw = Fake {
            msr: RefCell::new(HashMap::from([(MSR_TURBO_RATIO_LIMIT, value_of(&STOCK))])),
            programmable: Cell::new(true),
            root: Cell::new(true),
            gpu: RefCell::new(Vec::new()),
            boot: RefCell::new("boot-1".into()),
            texts: RefCell::new(None),
            rapl: Cell::new(None),
        };
        let b = Base { _dir: dir, ctx, hw };
        let stock = parse(r#"{"ratios": [50, 50, 48, 48, 47, 47, 47, 47], "pl1": 95, "pl2": 125}"#).unwrap();
        b.t().write_json(&b.t().state("stock.json"), stock.obj().unwrap()).unwrap();
        b
    }

    fn t(&self) -> Tune<'_> {
        Tune::new(&self.ctx, &self.hw)
    }

    fn ratios(&self) -> Vec<i64> {
        ratios_of(self.hw.msr.borrow()[&MSR_TURBO_RATIO_LIMIT])
    }

    fn set_msr(&self, r: &[i64]) {
        self.hw.msr.borrow_mut().insert(MSR_TURBO_RATIO_LIMIT, value_of(r));
    }

    fn boot(&self, b: &str) {
        *self.hw.boot.borrow_mut() = b.into();
    }

    fn tuned(&self) -> bool {
        self.t().state("tuned").exists()
    }

    fn write_conf(&self, text: &str) {
        self.t().write_json(&self.t().conf_path(), parse(text).unwrap().obj().unwrap()).unwrap();
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// What the service does at a start, without its endless loop.
    fn start(&self) -> bool {
        let t = self.t();
        let me = self.hw.boot.borrow().clone();
        let guarded = t.guard_check(&me).unwrap();
        let (c, st) = (t.conf(), t.status());
        if !guarded {
            t.refresh_mark().unwrap();
            for what in ["cpu", "gpu"] {
                if !st.at(&format!("{what}_off")).truthy() {
                    t.apply_kept(what, Some(&c)).unwrap();
                }
            }
        }
        guarded
    }
}

fn ratios_json(r: &[i64]) -> Vec<Json> {
    r.iter().map(|x| Json::Int(*x)).collect()
}

fn stock_json() -> Json {
    Json::Arr(ratios_json(&STOCK))
}

// ---------------------------------------------------------------- Multipliers

#[test]
fn test_bytes_round_trip() {
    assert_eq!(ratios_of(value_of(&STOCK)), STOCK);
    assert_eq!(value_of(&[50, 0, 0, 0, 0, 0, 0, 0]), 50);
}

#[test]
fn test_refused() {
    let bad: Vec<Vec<Json>> = vec![
        ratios_json(&[50; 7]),
        ratios_json(&[54; 8]),
        ratios_json(&[7; 8]),
        ratios_json(&[48, 50, 48, 48, 47, 47, 47, 47]),
        vec![Json::Str("50".into()); 8],
        vec![Json::Bool(true); 8],
    ];
    for b in bad {
        assert!(matches!(check_ratios(&b, &stock_json()), Err(CheckErr::Value(_))), "{b:?}");
    }
    assert_eq!(check_ratios(&ratios_json(&[53; 8]), &stock_json()), Ok(vec![53; 8]), "up to 3 above the processor's own top");
}

#[test]
fn test_kept_values_get_the_same_range_check_at_start() {
    // the root service applies conf() at every start: an out-of-range value written into the file by other means is dropped
    let b = Base::new();
    b.write_conf(r#"{"cpu": {"ratios": [70, 70, 70, 70, 70, 70, 70, 70]}}"#);
    assert_eq!(b.t().conf().cpu, None);
    b.write_conf(r#"{"cpu": {"ratios": [52, 52, 52, 52, 52, 52, 52, 52]}}"#);
    assert_eq!(b.t().conf().cpu, Some(vec![52; 8]));
}

#[test]
fn test_try_applies_now_and_keep_makes_it_start_with_the_computer() {
    let b = Base::new();
    assert_eq!(b.t().cmd_try("cpu", &Base::args(&["ratios=51,51,50,50,49,49,49,49"])), Ok(0));
    assert_eq!(b.ratios(), [51, 51, 50, 50, 49, 49, 49, 49]);
    assert!(b.tuned(), "the guard is set before anything is applied");
    assert_eq!(b.t().conf().cpu, None, "not kept yet");
    assert_eq!(b.t().cmd_keep("cpu"), Ok(0));
    assert_eq!(b.t().conf().cpu, Some(vec![51, 51, 50, 50, 49, 49, 49, 49]));
    assert!(b.t().status().at("trial").is_null());
}

#[test]
fn test_undo_goes_back_to_the_kept_values() {
    let b = Base::new();
    b.t().cmd_try("cpu", &Base::args(&["ratios=52,52,52,52,52,52,52,52"])).unwrap();
    b.t().cmd_undo("cpu").unwrap();
    assert_eq!(b.ratios(), STOCK, "nothing kept: the processor's own");
}

#[test]
fn test_a_locked_processor_is_refused() {
    let b = Base::new();
    b.hw.programmable.set(false);
    assert_eq!(b.t().cmd_try("cpu", &Base::args(&["ratios=50,50,50,50,50,50,50,50"])), Ok(2));
    assert_eq!(b.ratios(), STOCK);
}

#[test]
fn test_not_root_changes_nothing() {
    let b = Base::new();
    b.hw.root.set(false);
    assert_eq!(b.t().cmd_try("cpu", &Base::args(&["ratios=52,52,52,52,52,52,52,52"])), Ok(1));
    assert_eq!(b.ratios(), STOCK);
}

// ---------------------------------------------------------------- Guard

fn keep(b: &Base, ratios: &[i64]) {
    b.write_conf(&format!(r#"{{"cpu": {{"ratios": {}}}, "gpu": null}}"#, crate::json::dumps(&Json::Arr(ratios_json(ratios)))));
}

#[test]
fn test_a_clean_shutdown_keeps_the_values_and_a_freeze_does_not() {
    let b = Base::new();
    keep(&b, &[52; 8]);
    b.start();
    assert_eq!(b.ratios(), [52; 8]);
    // clean shutdown: the service removes the marker
    std::fs::remove_file(b.t().state("tuned")).unwrap();
    b.set_msr(&STOCK); // a restart puts the processor's own back
    b.boot("boot-2");
    assert!(!b.start());
    assert_eq!(b.ratios(), [52; 8]);
    // this time it freezes: the marker stays
    b.set_msr(&STOCK);
    b.boot("boot-3");
    assert!(b.start());
    assert_eq!(b.ratios(), STOCK, "after a freeze the defaults stay");
    assert!(b.t().status().at("cpu_off").str().unwrap().contains("did not shut down cleanly"));
    assert_eq!(b.t().conf().cpu, Some(vec![52; 8]), "the kept values are not lost");
    b.boot("boot-4");
    assert!(!b.start(), "the marker was cleared");
    assert_eq!(b.ratios(), STOCK, "still off until switched on again");
    assert_eq!(b.t().cmd_reenable(), Ok(0));
    assert_eq!(b.ratios(), [52; 8]);
}

#[test]
fn test_a_freeze_during_a_try_never_brings_the_tried_values_back() {
    let b = Base::new();
    b.t().cmd_try("cpu", &Base::args(&["ratios=53,53,53,53,53,53,53,53"])).unwrap();
    b.set_msr(&STOCK);
    b.boot("boot-2");
    assert!(b.start());
    assert_eq!(b.ratios(), STOCK);
}

#[test]
fn test_nothing_tuned_needs_no_guard() {
    let b = Base::new();
    b.start();
    assert!(!b.tuned());
}

#[test]
fn test_back_at_the_defaults_the_guard_lets_go() {
    let b = Base::new();
    b.t().cmd_try("cpu", &Base::args(&["ratios=51,51,51,51,51,51,51,51"])).unwrap();
    assert!(b.tuned());
    b.t().cmd_undo("cpu").unwrap();
    assert!(!b.tuned(), "an unrelated crash later must not switch tuning off");
    b.boot("boot-2");
    assert!(!b.start());
}

// ---------------------------------------------------------------- Stock

#[test]
fn test_the_processors_own_values_are_read_once_per_start() {
    let b = Base::new();
    b.hw.rapl.set(Some((Some(95), Some(125))));
    std::fs::remove_file(b.t().state("stock.json")).unwrap();
    let ratios = |s: Obj| s.at("ratios").clone();
    assert!(ratios(b.t().watch_stock().unwrap()).py_eq(&stock_json()));
    b.set_msr(&[49; 8]); // tuned, then the service restarts
    assert!(ratios(b.t().watch_stock().unwrap()).py_eq(&stock_json()), "a restart of the service alone keeps what it read");
    b.boot("boot-2");
    b.set_msr(&STOCK); // the computer restarted: the processor's own
    assert!(ratios(b.t().watch_stock().unwrap()).py_eq(&stock_json()));
}

// ---------------------------------------------------------------- GraphicsCard

#[test]
fn test_offsets_checked_and_tried() {
    for bad in [r#"{"core": 400, "mem": 0}"#, r#"{"core": 0, "mem": 3000}"#, r#"{"core": 0, "mem": 0, "cap": 100}"#, r#"{"core": "1"}"#] {
        assert!(check_gpu(&parse(bad).unwrap()).is_err(), "{bad}");
    }
    let b = Base::new();
    assert_eq!(b.t().cmd_try("gpu", &Base::args(&["core=120", "mem=600", "cap=1900"])), Ok(0));
    assert_eq!(b.hw.gpu.borrow().last(), Some(&GpuVals { core: 120, mem: 600, cap: Some(1900) }));
    assert_eq!(b.t().cmd_undo("gpu"), Ok(0));
    assert_eq!(b.hw.gpu.borrow().last(), Some(&GpuVals { core: 0, mem: 0, cap: None }));
}

#[test]
fn test_a_driver_error_puts_the_clocks_back() {
    let b = Base::new();
    b.t().cmd_try("gpu", &Base::args(&["core=150", "mem=900", "cap=none"])).unwrap();
    assert_eq!(b.t().on_kernel_line("usb 1-3: new device"), Ok(false));
    assert_eq!(b.t().on_kernel_line("NVRM: Xid (PCI:0000:01:00): 13, pid=4242, Graphics Exception"), Ok(true));
    assert_eq!(b.hw.gpu.borrow().last(), Some(&GPU_DEFAULT));
    let st = b.t().status();
    assert!(st.at("gpu_off").str().unwrap().contains("graphics driver reported an error"));
    assert!(st.at("trial").obj().is_none_or(|t| !t.contains("gpu")));
}

#[test]
fn test_a_driver_error_with_nothing_tuned_is_left_alone() {
    let b = Base::new();
    assert_eq!(b.t().on_kernel_line("NVRM: Xid (PCI:0000:01:00): 13, pid=4242"), Ok(false));
    assert!(b.hw.gpu.borrow().is_empty());
}

// ---------------------------------------------------------------- Power

#[test]
fn test_limits_checked_and_saved() {
    let b = Base::new();
    *b.hw.texts.borrow_mut() = Some(BTreeMap::new());
    let written = || b.hw.texts.borrow().clone().unwrap();
    assert_eq!(b.t().cmd_set("power", &Base::args(&["pl1=120", "pl2=160"])), Ok(0));
    assert_eq!(
        written(),
        BTreeMap::from([
            ("constraint_0_power_limit_uw".into(), "120000000".into()),
            ("constraint_1_power_limit_uw".into(), "160000000".into())
        ])
    );
    let c = b.t().conf();
    assert_eq!((c.pl1, c.pl2), (Some(120), Some(160)));
    assert_eq!(b.t().cmd_set("power", &Base::args(&["pl2=100"])), Ok(2), "short-term below long-term");
    assert_eq!(b.t().cmd_set("power", &Base::args(&["pl1=500"])), Ok(2));
    assert_eq!(b.t().cmd_set("power", &Base::args(&["pl1=default", "pl2=default"])), Ok(0));
    assert_eq!(written()["constraint_0_power_limit_uw"], "95000000", "default: the BIOS's own, read at start");
}

#[test]
fn test_turbo() {
    let b = Base::new();
    *b.hw.texts.borrow_mut() = Some(BTreeMap::new());
    assert_eq!(b.t().cmd_set("turbo", &Base::args(&["off"])), Ok(0));
    assert_eq!(b.hw.texts.borrow().as_ref().unwrap()["no_turbo"], "1");
    assert_eq!(b.t().conf().turbo, Some(false));
}

// ---------------------------------------------------------------- the kernel's messages (new: /dev/kmsg instead of journalctl)

#[test]
fn kmsg_records_give_the_message_after_the_semicolon() {
    let rec =
        b"4,1532,123456789,-;NVRM: Xid (PCI:0000:01:00): 13, pid=4242, Graphics Exception\n SUBSYSTEM=pci\n DEVICE=+pci:0000:01:00.0\n";
    assert_eq!(kmsg_message(rec).as_deref(), Some("NVRM: Xid (PCI:0000:01:00): 13, pid=4242, Graphics Exception"));
    assert_eq!(
        kmsg_message(b"6,1,2,-;a\\x5cb \\x1b[0m; c\n").as_deref(),
        Some("a\\b \x1b[0m; c"),
        "the kernel's \\xNN escapes undone; a later ';' is text"
    );
    assert_eq!(kmsg_message(b"6,99,5,c,caller=T42;usb 1-3: new device\n").as_deref(), Some("usb 1-3: new device"), "extra prefix fields");
    assert_eq!(kmsg_message(b"no prefix\n"), None);
}

#[test]
fn a_kmsg_record_reaches_the_xid_reaction() {
    let b = Base::new();
    b.t().cmd_try("gpu", &Base::args(&["core=150", "mem=900", "cap=none"])).unwrap();
    let line =
        kmsg_message(b"3,8812,99887766,-;NVRM: Xid (PCI:0000:01:00): 79, pid=1, GPU has fallen off the bus.\n SUBSYSTEM=pci\n").unwrap();
    assert_eq!(b.t().on_kernel_line(&line), Ok(true));
    assert_eq!(b.hw.gpu.borrow().last(), Some(&GPU_DEFAULT));
    assert!(b.t().status().at("gpu_off").str().unwrap().contains("(NVRM: Xid (PCI:0000:01:00): 79, pid=1, GPU has fallen off the bus.)"));
}

#[test]
fn the_kmsg_reader_reads_records_and_a_fake_file_as_lines() {
    // a fake tree's dev/kmsg is a plain file of lines, read from its start, then the source ends (as journalctl ending did)
    let d = TempDir::new();
    d.files("dev", &[("kmsg", "usb 1-3: new device\nNVRM: Xid (PCI:0000:01:00): 13, pid=4242\n")]);
    let c = Ctx::new(d.path().to_path_buf(), true, Some(1));
    let mut k = Kernel::open(&c).unwrap();
    let mut lines = Vec::new();
    loop {
        match k.wait(&c, 2.0) {
            Event::Lines(l) => lines.extend(l),
            Event::Ended => break,
            _ => {}
        }
    }
    assert_eq!(lines, ["usb 1-3: new device", "NVRM: Xid (PCI:0000:01:00): 13, pid=4242"]);
}
