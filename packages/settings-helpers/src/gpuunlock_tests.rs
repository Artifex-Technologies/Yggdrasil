//! gpuunlock without a GPU, carried over from the Python script's tests (helpers/tests/test_gpuunlock.py, the PowerHold ones
//! aside: powerlimit is packages/power-helpers'), and the grub.d fragment's checks, now `grub-entry`: identity matching, the
//! support table, what blocks arming, the one-try boot entry, arming with every outside program faked, the attempt markers
//! across starts, the module's own result, the heat watch and the power hold. Nothing on the system is read or touched (a
//! fake /sys, /proc and /etc, a temporary state folder, a fake runner).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::rc::Rc;

use pycompat::json::{self, json, Value};

use super::*;
use crate::testutil::TempDir;

const MSI_BDF: &str = "0000:01:00.0";
const OPEN: &str = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.78.08  Release Build  (notroot@)  Fri Oct 23 2026\n";
const CLOSED: &str = "NVRM version: NVIDIA UNIX x86_64 Kernel Module  615.78.08  Release Build  (notroot@)  Fri Oct 23 2026\n";
const FIRST: &str = "menuentry 'Labyrinth Linux' --class labyrinth --class gnu-linux $menuentry_id_option 'gnulinux-simple-3118e509' {\n\
\tload_video\n\tinsmod btrfs\n\tsearch --no-floppy --fs-uuid --set=root 3118e509\n\
\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw rootflags=subvol=@ quiet splash\n\
\tinitrd\t/@/boot/initramfs-linux-cachyos.img\n}\n";
const UNLOCKED_LOG: &str = "turing_unlock: entry: plm=0xffffff8f ss0=0x2300000 ss1=0x6 wpr2=0x0:0x1ffffe00 boot=0x0\n\
turing_unlock: unlock result: ss0=0x88888888 ss1=0x8 readout=0x0 (want ss0=0x88888888 ss1=0x8 readout=0)\n\
turing_unlock: TU102 FEAT UNLOCKED\npci 0000:01:00.0: turing_unlock: unlocked\n";
const STATES: &[&str] = &["unsupported", "supported", "armed", "unlocked", "failed", "throttled", "secure-boot", "no-nvidia"];
const PMON_HEAD: &str = "# gpu         pid   type     sm    mem    enc    dec    jpg    ofa    command \n\
# Idx           #    C/G      %      %      %      %      %      %    name \n";
const PMON_IDLE: &str = "    0       1672     G      -      -      -      -      -      -    Hyprland       \n\
    0       2282   C+G      -      -      -      -      -      -    sunshine       \n\
    0       2686   C+G      -      -      -      -      -      -    Code --gp      \n\
    0       6195   C+G      2      1      -      -      -      -    renderD128 --cr\n";
const GOOD_ENTRY: &str =
    "menuentry 'Labyrinth Linux, graphics card unlock (one try)' --class gnu-linux --class os --id yggdrasil-gpu-unlock {\n\
\tsearch --no-floppy --fs-uuid --set=root 3118e509\n\
\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw quiet turing_unlock.targets=0000:01:00.0\n\
\tinitrd\t/@/boot/initramfs-linux-cachyos.img\n}\n";

fn grub_base() -> String {
    format!("### BEGIN /etc/grub.d/10_linux ###\n{FIRST}submenu 'Advanced options' $menuentry_id_option 'gnulinux-advanced' {{\n}}\n")
}

fn locked() -> Value {
    json!({"device": "NVIDIA GeForce RTX 2080 Ti", "pci": "0000:01:00.0", "sms": 68, "quick": false, "samples": 7,
           "check": {"compared": 4096, "wrong": 0, "max_abs_err": 0, "ok": true},
           "gemm_f16_acc_f32": [{"n": 4096, "tflops": 50.1}, {"n": 8192, "tflops": 52.8}], "mma_f16_acc_f32": 49.9, "mma_f16_acc_f16": 99.6})
}

fn unlocked() -> Value {
    with(
        &locked(),
        &[
            ("gemm_f16_acc_f32", json!([{"n": 4096, "tflops": 81.7}, {"n": 8192, "tflops": 77.9}])),
            ("mma_f16_acc_f32", json!(99.1)),
            ("mma_f16_acc_f16", json!(99.4)),
        ],
    )
}

/// dict(base, **changes)
fn with(base: &Value, changes: &[(&str, Value)]) -> Value {
    let mut m = base.as_object().unwrap().clone();
    for (k, v) in changes {
        m.insert(k.to_string(), v.clone());
    }
    Value::Object(m)
}

fn obj(v: &Value) -> Map {
    v.as_object().unwrap().clone()
}

fn put(path: &str, content: impl AsRef<[u8]>) {
    fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn text(path: &str) -> String {
    fs::read_to_string(path).unwrap()
}

fn js(path: &str) -> Value {
    json::loads(&text(path)).unwrap()
}

struct Card {
    bdf: &'static str,
    device: &'static str,
    sub: (&'static str, &'static str),
    vbios: &'static str,
    bar0: &'static str,
}

const MSI: Card = Card { bdf: "0000:01:00.0", device: "1e07", sub: ("1462", "3711"), vbios: "90.02.17.00.3e", bar0: "0x00000000de000000" };
const ASUS: Card = Card { bdf: "0000:02:00.0", device: "1e07", sub: ("1043", "866a"), vbios: "90.02.17.00.47", bar0: "0x00000000fa000000" };

/// The outside programs: records each call and answers like the real one would.
#[derive(Default)]
struct Fake {
    calls: Vec<Vec<String>>,
    fail: HashSet<String>,
    mkconfig_extra: String,
    mkconfig_tail: Vec<u8>,
    kernel_log: String,
    compute: String,
    pmon: String,
    power: String,
}

struct T {
    d: TempDir,
    g: Gpu,
    fake: Rc<RefCell<Fake>>,
    bench: Rc<RefCell<Result<Value, String>>>,
}

impl T {
    fn new() -> T {
        let d = TempDir::new();
        let fake = Rc::new(RefCell::new(Fake { power: "260.00, 260.00".into(), ..Default::default() }));
        let bench: Rc<RefCell<Result<Value, String>>> = Rc::new(RefCell::new(Err("the CUDA toolkit is not installed (a test)".into())));
        let (state_dir, grub_cfg) = (d.p("state"), d.p("grub/grub.cfg"));
        let f = fake.clone();
        let (sd, gc) = (state_dir.clone(), grub_cfg.clone());
        let run: Runner = Box::new(move |argv: &[String], _timeout: u64| {
            let mut fk = f.borrow_mut();
            fk.calls.push(argv.to_vec());
            let name = argv[0].as_str();
            if fk.fail.contains(name) {
                return Ok((1, format!("{name} failed")));
            }
            Ok(match name {
                "modinfo" => (0, "/usr/lib/modules/7.3.0-4-cachyos/updates/dkms/turing_unlock.ko.zst".into()),
                "snapper" => (0, "240".into()),
                "journalctl" => (0, fk.kernel_log.clone()),
                "nvidia-smi" if argv.iter().any(|a| a == "pmon") => (0, pycompat::strip(&format!("{PMON_HEAD}{}", fk.pmon)).to_string()),
                "nvidia-smi" if argv.iter().any(|a| a == "--query-gpu=power.limit,power.default_limit") => (0, fk.power.clone()),
                "nvidia-smi" => (0, fk.compute.clone()),
                "grub-mkconfig" => {
                    let frag = checked_entry(&py::join(&sd, "entry.cfg")).unwrap_or_default();
                    let mut cfg = format!("{}{}{frag}", fk.mkconfig_extra, grub_base()).into_bytes();
                    cfg.extend_from_slice(&fk.mkconfig_tail);
                    put(&gc, cfg);
                    (0, String::new())
                }
                _ => (0, String::new()),
            })
        });
        let b = bench.clone();
        let g = Gpu {
            root: d.p("root"),
            state_dir,
            table: format!("{}/../settings/data/gpu-unlock-supported.json", env!("CARGO_MANIFEST_DIR")),
            grub_cfg: grub_cfg.clone(),
            ollama: "127.0.0.1:9".into(), // nothing listens there: no model loaded
            exe_dir: d.p("bin"),
            run,
            is_root: Cell::new(true),
            bench_hook: Some(Box::new(move |_bdf: &str, _quick: bool| Ok(b.borrow().clone()))),
            sample_hook: None,
            watch: Cell::new((15.0, 10.0)),
            captured: Some(RefCell::new((String::new(), String::new()))),
        };
        put(&grub_cfg, grub_base());
        let t = T { d, g, fake, bench };
        t.root("etc/default/grub", "GRUB_DEFAULT=0\nGRUB_TIMEOUT=2\nGRUB_TIMEOUT_STYLE=menu\n");
        t.root("usr/lib/firmware/turing-unlock/610.43.03/v67-tu102-610.43.03.bin", "fw");
        t.root("etc/mkinitcpio.conf.d/zz-turing-unlock.conf", "MODULES+=(turing_unlock)\n");
        t.boot("aaaa-1111", "root=UUID=3118e509 rw quiet splash");
        t.card(&MSI, OPEN, true);
        // the locked baseline Phase 2 records (arming waits for it); no test runs the real benchmark
        put(
            &t.state("baseline.json"),
            json::dumps(&with(&locked(), &[("bdf", json!(MSI.bdf)), ("vbios", json!(MSI.vbios.to_uppercase()))])),
        );
        t
    }

    fn root(&self, rel: &str, content: impl AsRef<[u8]>) {
        put(&format!("{}/{rel}", self.g.root), content);
    }

    fn boot(&self, ident: &str, cmdline: &str) {
        self.root("proc/sys/kernel/random/boot_id", format!("{ident}\n"));
        self.root("proc/cmdline", format!("{cmdline}\n"));
    }

    fn card(&self, c: &Card, driver: &str, loaded: bool) {
        let root = &self.g.root;
        let _ = fs::remove_dir_all(format!("{root}/sys"));
        let _ = fs::remove_dir_all(format!("{root}/proc/driver"));
        let dev = format!("{root}/sys/bus/pci/devices/{}", c.bdf);
        for (k, v) in [
            ("vendor", "0x10de".to_string()),
            ("device", format!("0x{}", c.device)),
            ("subsystem_vendor", format!("0x{}", c.sub.0)),
            ("subsystem_device", format!("0x{}", c.sub.1)),
            ("class", "0x030000".into()),
            ("resource", format!("{} 0x00000000deffffff 0x0000000000040200\n", c.bar0)),
        ] {
            put(&format!("{dev}/{k}"), format!("{v}\n"));
        }
        if loaded {
            self.root("proc/driver/nvidia/version", driver);
            self.root(
                &format!("proc/driver/nvidia/gpus/{}/information", c.bdf),
                format!(
                    "Model: \t\t NVIDIA GeForce RTX 2080 Ti\nGPU UUID: \t GPU-0000\nVideo BIOS: \t {}\nBus Location: \t {}\n",
                    c.vbios, c.bdf
                ),
            );
        }
    }

    fn state(&self, name: &str) -> String {
        self.g.sp(name)
    }

    fn exists(&self, name: &str) -> bool {
        Path::new(&self.state(name)).exists()
    }

    /// What the helper wrote to its state folder (the baseline the set-up puts there aside).
    fn written(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.g.state_dir)
            .map(|r| {
                r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n != "baseline.json").collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    fn table(&mut self, change: impl FnOnce(&mut Map)) {
        let mut t = js(&self.g.table);
        change(t["entries"][0].as_object_mut().unwrap());
        let path = self.d.p("table.json");
        put(&path, json::dumps(&t));
        self.g.table = path;
    }

    fn status(&self) -> Map {
        self.g.status().unwrap()
    }

    fn reason(&self) -> String {
        self.status()["reason"].as_str().unwrap().to_string()
    }

    fn blocker(&self) -> Option<String> {
        self.status()["arm_blocker"].as_str().map(String::from)
    }

    fn ran(&self, name: &str) -> Vec<Vec<String>> {
        self.fake.borrow().calls.iter().filter(|c| c[0] == name).cloned().collect()
    }

    fn arm(&self) -> i32 {
        self.g.cmd_arm().unwrap()
    }

    fn picked(&self, ident: &str) {
        self.boot(ident, "root=UUID=3118e509 rw quiet splash turing_unlock.targets=0000:01:00.0");
    }

    fn set_bench(&self, r: Result<Value, String>) {
        *self.bench.borrow_mut() = r;
    }
}

// ---------------------------------------------------------------- identity

#[test]
fn the_owners_card_is_in_the_table() {
    let t = T::new();
    let s = t.status();
    assert_eq!(s["state"], "supported");
    assert_eq!(s["arm_blocker"], Value::Null);
    assert_eq!(
        (&s["gpu"]["bar0"], &s["gpu"]["driver_series"], &s["gpu"]["vbios"]),
        (&json!("0xde000000"), &json!("open"), &json!("90.02.17.00.3E"))
    );
    assert_eq!(s["gpu"]["driver"], "615.78.08");
}

#[test]
fn another_board_is_not_and_the_reason_names_it() {
    let t = T::new();
    t.card(&ASUS, OPEN, true);
    let s = t.status();
    assert_eq!(s["state"], "unsupported");
    assert_eq!(s["reason"], "No tested entry for ASUS 1043:866a with VBIOS 90.02.17.00.47 (tested: MSI 1462:3711, VBIOS 90.02.17.00.3E)");
}

#[test]
fn the_same_card_on_the_other_driver_is_not() {
    let t = T::new();
    t.card(&MSI, CLOSED, true);
    assert_eq!(t.status()["state"], "unsupported");
    assert!(t.reason().contains("another NVIDIA driver series"));
}

#[test]
fn no_driver_means_no_firmware_version_and_no_offer() {
    let t = T::new();
    t.card(&MSI, OPEN, false);
    assert!(t.reason().contains("driver is not loaded"));
}

#[test]
fn no_nvidia_card() {
    let t = T::new();
    fs::remove_dir_all(format!("{}/sys", t.g.root)).unwrap();
    assert_eq!(t.status()["state"], "no-nvidia");
}

#[test]
fn a_wildcard_a_missing_field_or_another_method_makes_the_table_refused() {
    let mut t = T::new();
    for (field, bad) in [
        ("subsystem", json!("1462:*")),
        ("vbios", json!("90.02.17.00.*")),
        ("device", json!("10de:1e0?")),
        ("subsystem", json!("")),
        ("driver_series", json!("any")),
        ("evidence", json!("")),
        ("method", json!("turing-unlock-efi")),
        ("method", Value::Null),
        // a final newline is not part of an exact identity (the script's $ let one through)
        ("subsystem", json!("1462:3711\n")),
        ("device", json!("10de:1e07\n")),
        ("vbios", json!("90.02.17.00.3E\n")),
    ] {
        t.table(|e| {
            e.insert(field.into(), bad.clone());
        });
        let s = t.status();
        assert_eq!(s["state"], "unsupported", "{field}={bad}");
        assert!(t.reason().contains("support table is not valid"), "{field}={bad}");
    }
}

#[test]
fn an_entry_no_longer_experimental_is_not_offered() {
    let mut t = T::new();
    t.table(|e| {
        e.insert("status".into(), json!("measured-no-benefit"));
    });
    assert_eq!(t.status()["state"], "unsupported");
    assert!(t.reason().contains("marked measured-no-benefit"));
}

#[test]
fn secure_boot_on_refuses_and_arm_writes_nothing() {
    let t = T::new();
    t.root(SECURE_BOOT_VAR.trim_start_matches('/'), b"\x06\x00\x00\x00\x01");
    assert_eq!(t.status()["state"], "secure-boot");
    assert_eq!(t.arm(), 2);
    assert!(t.written().is_empty());
    assert!(t.ran("snapper").is_empty());
}

#[test]
fn status_json_has_the_same_shape_in_every_state() {
    let t = T::new();
    let keys: HashSet<&str> =
        ["gpu", "secure_boot", "state", "reason", "arm_blocker", "last_attempt", "since", "ratio", "power_hold", "log"].into();
    let seen = RefCell::new(HashSet::new());
    let check = || {
        let s = t.status();
        assert_eq!(s.keys().map(|k| k.as_str()).collect::<HashSet<_>>(), keys);
        assert!(STATES.contains(&s["state"].as_str().unwrap()));
        assert!(s["reason"].is_string());
        assert!(s["power_hold"].is_boolean());
        seen.borrow_mut().insert(s["state"].as_str().unwrap().to_string());
    };
    check(); // supported
    put(&t.state("armed"), r#"{"boot_id": "aaaa-1111"}"#);
    check(); // armed
    fs::remove_file(t.state("armed")).unwrap();
    put(&t.state("state.json"), r#"{"state": "unlocked", "boot_id": "aaaa-1111", "since": "x", "ratio": 2.0}"#);
    check(); // unlocked
    put(&t.state("state.json"), r#"{"state": "throttled", "boot_id": "aaaa-1111"}"#);
    check(); // throttled
    put(&t.state("state.json"), r#"{"state": "failed", "why": "x"}"#);
    check(); // failed
    fs::remove_file(t.state("state.json")).unwrap();
    t.root(SECURE_BOOT_VAR.trim_start_matches('/'), b"\x06\x00\x00\x00\x01");
    check(); // secure-boot
    t.card(&ASUS, OPEN, true);
    check(); // unsupported
    fs::remove_dir_all(format!("{}/sys", t.g.root)).unwrap();
    check(); // no-nvidia
    assert_eq!(seen.into_inner(), STATES.iter().map(|s| s.to_string()).collect());
}

#[test]
fn an_unlock_from_an_earlier_start_is_gone() {
    let t = T::new();
    put(&t.state("state.json"), r#"{"state": "unlocked", "boot_id": "old-boot", "since": "x", "ratio": 2.0}"#);
    let s = t.status();
    assert_eq!(s["state"], "supported", "the unlock does not survive a restart");
    assert_eq!(s["ratio"], Value::Null);
}

// ---------------------------------------------------------------- blockers

#[test]
fn the_module_its_firmware_and_its_start_up_hook_must_be_installed() {
    let t = T::new();
    t.fake.borrow_mut().fail.insert("modinfo".into());
    assert!(t.blocker().unwrap().contains("module is not installed"));
    assert_eq!(t.arm(), 2);
    assert!(t.ran("snapper").is_empty(), "refused before anything was done");
    t.fake.borrow_mut().fail.clear();
    fs::remove_dir_all(format!("{}/usr/lib/firmware/turing-unlock", t.g.root)).unwrap();
    assert!(t.blocker().unwrap().contains("firmware is not installed"));
    t.root("usr/lib/firmware/turing-unlock/610.43.03/x", "fw");
    fs::remove_file(format!("{}/etc/mkinitcpio.conf.d/zz-turing-unlock.conf", t.g.root)).unwrap();
    assert!(t.blocker().unwrap().contains("start-up image"));
}

#[test]
fn a_hidden_boot_menu_blocks_arming() {
    let t = T::new();
    for grub in [
        "GRUB_TIMEOUT=2\nGRUB_TIMEOUT_STYLE=hidden\n",
        "GRUB_TIMEOUT=0\n",
        "GRUB_TIMEOUT='0'\nGRUB_TIMEOUT_STYLE=menu\n",
        "GRUB_TIMEOUT=\u{660}\n",
    ] {
        t.root("etc/default/grub", grub);
        assert!(t.blocker().unwrap().contains("boot menu is hidden"), "{grub}");
    }
    t.root("etc/default/grub", "GRUB_TIMEOUT=5\n");
    assert_eq!(t.blocker(), None);
}

#[test]
fn a_timeout_that_is_a_digit_but_not_a_number() {
    // "²" is a digit to str.isdigit() but not a number to int(): the script stopped with a traceback
    let t = T::new();
    t.root("etc/default/grub", "GRUB_TIMEOUT=\u{b2}\n");
    assert_eq!(t.blocker(), None);
}

// ---------------------------------------------------------------- the entry

#[test]
fn the_first_entry_copied_with_the_option() {
    let e = one_try_entry(Some(&grub_base()), "0000:01:00.0").unwrap();
    assert!(e.starts_with(
        "menuentry 'Labyrinth Linux, graphics card unlock (one try)' --class gnu-linux --class os --id yggdrasil-gpu-unlock {\n"
    ));
    assert!(e.contains(
        "\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw rootflags=subvol=@ quiet splash turing_unlock.targets=0000:01:00.0\n"
    ));
    assert!(e.contains("\tinitrd\t/@/boot/initramfs-linux-cachyos.img\n"));
    assert!(e.ends_with("\n}\n"));
    assert!(!e.contains("Advanced"));
    assert_eq!(checked_entry_text(&e), Some(e.clone()), "what arm writes is what the fragment prints");
}

#[test]
fn refused_when_it_is_not_one_plain_linux_entry() {
    let b = grub_base();
    assert_eq!(one_try_entry(Some(&b), "01:00.0"), None, "a full PCI address only");
    assert_eq!(one_try_entry(Some(&b), "0000:01:00.0\n"), None, "a final newline is not part of it");
    assert_eq!(one_try_entry(Some(&b.replace("quiet splash", "quiet splash turing_unlock.targets=all")), "0000:01:00.0"), None);
    assert_eq!(one_try_entry(Some(&b.replace("\tlinux\t", "\techo\t")), "0000:01:00.0"), None);
    assert_eq!(one_try_entry(Some("submenu 'x' {\n}\n"), "0000:01:00.0"), None);
    assert_eq!(one_try_entry(None, "0000:01:00.0"), None);
}

#[test]
fn the_option_on_the_command_line() {
    assert_eq!(unlock_requested("root=UUID=x rw quiet turing_unlock.targets=0000:01:00.0").as_deref(), Some("0000:01:00.0"));
    assert_eq!(unlock_requested("turing_unlock.targets=all"), Some("all".into()));
    assert_eq!(unlock_requested("root=UUID=x rw quiet"), None);
    assert_eq!(unlock_requested("xturing_unlock.targets=all"), None);
    assert_eq!(unlock_requested("é turing_unlock.targets= x"), None);
}

#[test]
fn the_first_entry_id() {
    assert_eq!(first_entry_id(Some(&grub_base())).as_deref(), Some("gnulinux-simple-3118e509"));
    assert_eq!(first_entry_id(Some("menuentry 'x' $menuentry_id_option '' $menuentry_id_option 'b' {\n")).as_deref(), Some("b"));
    assert_eq!(first_entry_id(Some("menuentry 'x' {\n")).as_deref(), Some("menuentry 'x' {"));
    assert_eq!(first_entry_id(Some("# nothing\n")), None);
}

// ---------------------------------------------------------------- arming

#[test]
fn arm_snapshots_adds_the_entry_and_holds_power() {
    let t = T::new();
    assert_eq!(t.arm(), 0);
    assert_eq!(t.ran("snapper")[0].last().unwrap(), "before GPU unlock");
    let cfg = text(&t.g.grub_cfg);
    assert_eq!(cfg.matches("menuentry 'Labyrinth Linux, graphics card unlock (one try)'").count(), 1);
    assert!(cfg.contains("turing_unlock.targets=0000:01:00.0\n"));
    assert_eq!(first_entry_id(Some(&cfg)).as_deref(), Some("gnulinux-simple-3118e509"), "the normal start stays first and default");
    assert!(t.ran("grub-reboot").is_empty(), "nothing picks the entry but me");
    assert!(t.ran("grub-editenv").is_empty());
    assert!(t.exists("power-hold"));
    assert_eq!(fs::metadata(t.state("armed")).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(t.status()["state"], "armed");
    assert_eq!(js(&t.state("armed"))["snapshot"], "240");
    assert_eq!(t.ran("systemctl").len(), 2);
}

#[test]
fn arm_stops_and_puts_back_if_the_first_entry_changes() {
    let t = T::new();
    t.fake.borrow_mut().mkconfig_extra = "menuentry 'Something else' $menuentry_id_option 'other' {\n}\n".into();
    assert_eq!(t.arm(), 1);
    assert_eq!(t.written(), ["log"]);
}

#[test]
fn a_boot_menu_that_is_not_utf8_is_put_back() {
    // another system's entry in latin-1 after grub-mkconfig: the script stopped with a traceback here, after the menu was
    // rewritten, leaving the one-try entry in it with no power hold and no follow-up services
    let t = T::new();
    t.fake.borrow_mut().mkconfig_tail = b"menuentry 'Syst\xe8me' {\n}\n".to_vec();
    assert_eq!(t.arm(), 1);
    assert_eq!(t.written(), ["log"]);
    assert_eq!(t.ran("grub-mkconfig").len(), 2, "the menu written again without the entry");
    // a menu that is not UTF-8 before arming is refused before anything is done
    let mut cfg = grub_base().into_bytes();
    cfg.extend_from_slice(b"menuentry 'Syst\xe8me' {\n}\n");
    put(&t.g.grub_cfg, cfg);
    t.fake.borrow_mut().calls.clear();
    assert_eq!(t.arm(), 2);
    assert!(t.ran("snapper").is_empty());
}

#[test]
fn links_in_the_state_folder_are_not_followed() {
    let t = T::new();
    let victim = t.d.p("victim");
    put(&victim, "mine\n");
    for name in ["power-hold.tmp", "armed.tmp", "entry.cfg", "entry.cfg.tmp", "log"] {
        std::os::unix::fs::symlink(&victim, t.state(name)).unwrap();
    }
    assert_eq!(t.arm(), 0);
    assert_eq!(text(&victim), "mine\n", "nothing written through a link");
    for name in ["power-hold", "armed", "entry.cfg"] {
        assert!(!fs::symlink_metadata(t.state(name)).unwrap().file_type().is_symlink(), "{name}");
    }
    assert!(text(&t.state("entry.cfg")).contains("turing_unlock.targets=0000:01:00.0"));
}

#[test]
fn a_file_that_cannot_be_written_puts_everything_back() {
    let t = T::new();
    fs::create_dir_all(t.state("power-hold.tmp/x")).unwrap();
    assert_eq!(t.arm(), 1);
    let err = t.g.captured.as_ref().unwrap().borrow().1.clone();
    assert!(err.contains(&format!("not armed: {}: Is a directory; everything was put back", t.state("power-hold.tmp"))), "{err}");
    assert!(!t.exists("entry.cfg") && !t.exists("armed") && !t.exists("power-hold"));
    assert!(!text(&t.g.grub_cfg).contains("graphics card unlock"));
}

#[test]
fn a_failed_snapshot_stops_before_the_menu_is_touched() {
    let t = T::new();
    t.fake.borrow_mut().fail.insert("snapper".into());
    assert_eq!(t.arm(), 1);
    assert!(t.ran("grub-mkconfig").is_empty());
    assert!(t.written().is_empty());
}

#[test]
fn a_busy_card_is_not_armed() {
    let t = T::new();
    t.fake.borrow_mut().pmon = "    0       4242     C     97     40      -      -      -      -    ollama         \n".into();
    assert_eq!(t.arm(), 2);
    assert!(t.ran("snapper").is_empty());
}

#[test]
fn not_root_does_nothing() {
    let t = T::new();
    t.g.is_root.set(false);
    assert_eq!(t.arm(), 1);
    assert!(t.fake.borrow().calls.is_empty());
    for c in [Gpu::cmd_disarm, Gpu::cmd_reenable, Gpu::cmd_release, Gpu::cmd_boot, Gpu::cmd_verify] {
        assert_eq!(c(&t.g).unwrap(), 1);
    }
    assert!(t.written().is_empty());
}

#[test]
fn disarm_takes_it_all_back() {
    let t = T::new();
    t.arm();
    assert_eq!(t.g.cmd_disarm().unwrap(), 0);
    assert_eq!(t.written(), ["log"]);
    assert!(!text(&t.g.grub_cfg).contains("graphics card unlock"));
    assert_eq!(t.status()["state"], "supported");
    assert_eq!(t.g.cmd_disarm().unwrap(), 0, "nothing is armed: said, not an error");
}

// ---------------------------------------------------------------- the markers across starts

fn armed() -> T {
    let t = T::new();
    assert_eq!(t.arm(), 0);
    t
}

#[test]
fn a_normal_start_while_armed_did_not_try() {
    let t = armed();
    t.boot("bbbb-2222", "root=UUID=3118e509 rw quiet splash");
    t.g.cmd_boot().unwrap();
    t.g.cmd_verify().unwrap();
    assert!(t.exists("armed"));
    assert!(!t.exists("trying"));
    assert!(text(&t.g.grub_cfg).contains("graphics card unlock"), "the entry stays offered");
}

#[test]
fn picking_the_entry_is_the_attempt() {
    let t = armed();
    t.picked("bbbb-2222");
    t.g.cmd_boot().unwrap();
    assert!(!t.exists("armed"));
    assert_eq!(js(&t.state("trying"))["boot_id"], "bbbb-2222");
    assert_eq!(t.status()["state"], "armed", "still pending until checked");
}

#[test]
fn an_attempt_that_never_reached_the_desktop_is_a_failure() {
    let t = armed();
    t.picked("bbbb-2222");
    t.g.cmd_boot().unwrap(); // the attempt starts ... and hangs: verify never runs
    t.boot("cccc-3333", "root=UUID=3118e509 rw quiet splash");
    t.g.cmd_boot().unwrap(); // the power button, the normal entry
    let s = t.status();
    assert_eq!(s["state"], "failed");
    assert!(t.reason().contains("did not reach the desktop"));
    assert_eq!(t.arm(), 2, "no arming until re-enabled");
    t.g.cmd_verify().unwrap(); // that normal start's desktop takes the entry out of the menu
    assert!(!t.exists("entry.cfg"));
    assert!(!text(&t.g.grub_cfg).contains("graphics card unlock"));
    assert_eq!(t.g.cmd_reenable().unwrap(), 0);
    assert_eq!(t.status()["state"], "supported");
}

#[test]
fn the_modules_own_failure_is_the_reason() {
    let t = armed();
    t.picked("bbbb-2222");
    t.g.cmd_boot().unwrap();
    t.fake.borrow_mut().kernel_log =
        "turing_unlock: failure unlock.c:314 gsp_fwsec_frts error=-5\npci 0000:01:00.0: turing_unlock: unlock failed (-5)\n".into();
    t.g.cmd_verify().unwrap();
    assert_eq!(t.status()["state"], "failed");
    assert!(t.reason().contains("The module reported: turing_unlock: failure unlock.c:314"));
    assert!(!t.exists("entry.cfg"), "one try: the entry is gone");
    assert!(t.exists("power-hold"), "the hold stays until released");
}

#[test]
fn a_module_that_did_not_run() {
    let t = armed();
    t.picked("bbbb-2222");
    t.g.cmd_boot().unwrap();
    t.g.cmd_verify().unwrap();
    assert!(t.reason().contains("did not run at this start"));
}

#[test]
fn unlocked_by_the_module_but_not_yet_confirmed() {
    let t = armed();
    t.picked("bbbb-2222");
    t.g.cmd_boot().unwrap();
    t.fake.borrow_mut().kernel_log = UNLOCKED_LOG.into();
    t.g.cmd_verify().unwrap();
    assert_eq!(t.status()["state"], "failed", "never 'Unlocked' without the correctness check and benchmark");
    assert!(t.reason().contains("module reports the unlock"));
}

#[test]
fn a_confirmed_unlock_shows_until_the_next_start() {
    let t = armed();
    t.fake.borrow_mut().kernel_log = UNLOCKED_LOG.into();
    t.set_bench(Ok(unlocked()));
    t.g.watch.set((0.0, 0.0));
    t.picked("bbbb-2222");
    t.g.cmd_boot().unwrap();
    t.g.cmd_verify().unwrap();
    let s = t.status();
    assert_eq!(s["state"], "unlocked");
    assert!((s["ratio"].as_f64().unwrap() - 99.1 / 49.9).abs() < 1e-12);
    assert!(!text(&t.g.grub_cfg).contains("graphics card unlock"), "one try: the entry is gone");
    t.boot("cccc-3333", "root=UUID=3118e509 rw quiet splash");
    assert_eq!(t.status()["state"], "supported");
}

#[test]
fn release_waits_for_a_pending_attempt() {
    let t = armed();
    assert_eq!(t.g.cmd_release().unwrap(), 2);
    assert!(t.exists("power-hold"));
    t.g.cmd_disarm().unwrap();
    assert_eq!(t.g.cmd_release().unwrap(), 0, "no hold: said, not an error");
}

// ---------------------------------------------------------------- the benchmark

#[test]
fn rule_eight_judges_the_unlock() {
    let (l, u) = (obj(&locked()), obj(&unlocked()));
    let (ok, ratio, why) = judge(Some(&l), &u).unwrap();
    assert!(ok, "{why}");
    assert!((ratio.unwrap() - 99.1 / 49.9).abs() < 1e-12);
    assert!(why.contains("cuBLAS 8192 77.9 TFLOPS against 52.8"), "{why}");
    assert_eq!(why, "Tensor rate 1.99x of locked (99.1 against 49.9 TFLOPS; cuBLAS 8192 77.9 TFLOPS against 52.8), results correct");
    let without = |k: &str| {
        let mut m = obj(&unlocked());
        m.remove(k);
        Value::Object(m)
    };
    for (now, expect) in [
        (with(&unlocked(), &[("check", json!({"compared": 4096, "wrong": 3, "ok": false}))]), "Wrong results: 3 of 4096"),
        (with(&unlocked(), &[("mma_f16_acc_f32", json!(60.0))]), "under the 1.5x"),
        (with(&unlocked(), &[("mma_f16_acc_f16", json!(70.0))]), "control rate"),
        (with(&unlocked(), &[("quick", json!(true))]), "quick run"),
        (without("mma_f16_acc_f16"), "incomplete"),
        // float() reads "nan" and "inf": a NaN passed both checks in the script, so it was judged unlocked
        (with(&unlocked(), &[("mma_f16_acc_f32", json!("nan"))]), "incomplete"),
        (with(&unlocked(), &[("mma_f16_acc_f32", json!("inf"))]), "incomplete"),
        (with(&unlocked(), &[("mma_f16_acc_f16", json!("nan"))]), "incomplete"),
    ] {
        let (ok, _, why) = judge(Some(&l), &obj(&now)).unwrap();
        assert!(!ok, "{expect}");
        assert!(why.contains(expect), "{why}");
    }
    for base in [with(&locked(), &[("mma_f16_acc_f32", json!("nan"))]), with(&locked(), &[("mma_f16_acc_f16", json!("inf"))])] {
        let (ok, _, why) = judge(Some(&obj(&base)), &u).unwrap();
        assert!(!ok);
        assert!(why.contains("not usable"), "{why}");
    }
    let (ok, _, why) = judge(None, &u).unwrap();
    assert!(!ok);
    assert!(why.contains("No locked baseline"));
    // numbers as text are read as float() reads them
    let (ok, _, _) = judge(Some(&l), &obj(&with(&unlocked(), &[("mma_f16_acc_f32", json!(" 99.1 "))]))).unwrap();
    assert!(ok);
}

#[test]
fn arming_waits_for_a_full_baseline_of_this_card() {
    let t = T::new();
    assert_eq!(t.blocker(), None);
    for (bad, expect) in [
        (None, "No locked baseline"),
        (Some(with(&locked(), &[("quick", json!(true))])), "not a full, correct run"),
        (Some(with(&locked(), &[("bdf", json!("0000:02:00.0")), ("vbios", json!("90.02.17.00.3E"))])), "another card"),
        (Some(with(&locked(), &[("bdf", json!(MSI_BDF)), ("vbios", json!("90.02.17.00.47"))])), "another card"),
    ] {
        let _ = fs::remove_file(t.state("baseline.json"));
        if let Some(b) = bad {
            put(&t.state("baseline.json"), json::dumps(&b));
        }
        assert!(t.blocker().unwrap().contains(expect), "{expect}");
        assert_eq!(t.arm(), 2);
    }
}

#[test]
fn the_baseline_is_measured_locked_idle_and_at_default_watts() {
    let t = T::new();
    fs::remove_file(t.state("baseline.json")).unwrap();
    t.set_bench(Ok(locked()));
    let bench = |args: &[&str]| t.g.cmd_bench(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
    t.fake.borrow_mut().power = "290.00, 260.00".into();
    assert_eq!(bench(&["--baseline"]), 2, "Performance mode's 290 W is not the baseline");
    // a limit nobody knows is not the default either (float() reads "nan", and abs(nan - 260) > 1 is false)
    for power in ["nan, 260.00", "260.00, nan", "inf, inf", "[N/A], [N/A]"] {
        t.fake.borrow_mut().power = power.into();
        assert_eq!(bench(&["--baseline"]), 2, "{power}");
    }
    t.fake.borrow_mut().power = "260.00, 260.00".into();
    t.fake.borrow_mut().pmon = format!("{PMON_IDLE}    0       4242     C     97     40      -      -      -      -    ollama         \n");
    assert_eq!(bench(&["--baseline"]), 2, "the card must be idle");
    t.fake.borrow_mut().pmon = PMON_IDLE.repeat(3);
    t.fake.borrow_mut().compute = "2282, /usr/bin/sunshine\n2686, /usr/share/code/code".into();
    assert_eq!(t.g.card_busy().unwrap(), None, "programs only holding the card (desktop, Sunshine, an editor) are idle");
    t.fake.borrow_mut().compute = String::new();
    t.fake.borrow_mut().kernel_log = UNLOCKED_LOG.into();
    assert_eq!(bench(&["--baseline"]), 2, "never on an unlocked card");
    t.fake.borrow_mut().kernel_log = String::new();
    assert!(!t.exists("baseline.json"));
    assert_eq!(bench(&["--baseline"]), 0);
    let b = js(&t.state("baseline.json"));
    assert_eq!((&b["bdf"], &b["vbios"], &b["watts"]), (&json!(MSI_BDF), &json!("90.02.17.00.3E"), &json!(260.0)));
    assert_eq!(t.blocker(), None);
    // wrong results on the locked card keep no baseline
    fs::remove_file(t.state("baseline.json")).unwrap();
    t.set_bench(Ok(with(&locked(), &[("check", json!({"compared": 4096, "wrong": 1, "ok": false}))])));
    assert_eq!(bench(&["--baseline"]), 1);
    assert!(!t.exists("baseline.json"));
    assert_eq!(bench(&["--quick", "--baseline"]), 2);
    assert_eq!(bench(&["--fast"]), 2);
}

#[test]
fn what_counts_as_busy() {
    assert!(pmon_busy(&format!("{PMON_HEAD}{PMON_IDLE}")).unwrap().is_empty());
    let game = "    0       7000     G     85     30      -      -      -      -    GameThread     \n";
    assert_eq!(pmon_busy(&format!("{PMON_HEAD}{game}{PMON_IDLE}{}", game.replace(" 85 ", " 75 "))).unwrap(), ["GameThread (80 %)"]);
    let blip = "    0       6195   C+G     20      5      -      -      -      -    renderD128 --cr\n";
    assert!(pmon_busy(&format!("{PMON_HEAD}{blip}{PMON_IDLE}{PMON_IDLE}")).unwrap().is_empty(), "one short redraw is not busy");
    assert!(pmon_busy("no header\n    0  1  C  99  -  -  -  -  -  x\n").unwrap().is_empty());
    // without pmon (an older nvidia-smi) the old list is the answer
    let t = T::new();
    t.fake.borrow_mut().fail.insert("nvidia-smi".into());
    assert_eq!(t.g.card_busy().unwrap(), None);
}

#[test]
fn a_confirmed_unlock_needs_the_benchmark() {
    let t = T::new();
    t.fake.borrow_mut().kernel_log = UNLOCKED_LOG.into();
    t.set_bench(Ok(unlocked()));
    let (ok, _, why) = t.g.check_unlock().unwrap();
    assert!(ok, "{why}");
    t.set_bench(Err("the CUDA toolkit is not installed".into()));
    let (ok, _, why) = t.g.check_unlock().unwrap();
    assert!(!ok);
    assert!(why.contains("could not be checked: the CUDA toolkit"));
}

// ---------------------------------------------------------------- Ollama's port

/// A one-connection server on a free port of 127.0.0.1 that sends `answer` to whatever asks; its address.
fn answering(answer: impl Into<Vec<u8>>) -> String {
    let answer: Vec<u8> = answer.into();
    let srv = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = srv.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        if let Ok((mut c, _)) = srv.accept() {
            let mut b = [0u8; 4096];
            let _ = c.read(&mut b);
            let _ = c.write_all(&answer);
        }
    });
    addr
}

#[test]
fn a_loaded_model_makes_the_card_busy() {
    let mut t = T::new();
    let body = "{\"models\": [{\"name\": \"qwen3:8b\"}, {\"size\": 1}]}\n";
    t.g.ollama = answering(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
    assert_eq!(t.g.card_busy().unwrap().as_deref(), Some("a language model is loaded: qwen3:8b, ?"));
    t.g.ollama =
        answering(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\n{\"mode\r\n15\r\nls\": [{\"name\": \"a\"}]}\r\n0\r\n\r\n");
    assert_eq!(t.g.card_busy().unwrap().as_deref(), Some("a language model is loaded: a"));
    t.g.ollama = answering(b"HTTP/1.0 200 OK\r\n\r\n{\"models\": []}");
    assert_eq!(t.g.card_busy().unwrap(), None);
}

#[test]
fn something_else_on_ollamas_port_is_not_a_model() {
    // an answer that is not HTTP stopped arming with a traceback in the script (http.client's BadStatusLine)
    let mut t = T::new();
    for answer in [
        &b"hello, this is not a web server\r\n\r\n"[..],
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 302 Found\r\nLocation: http://192.0.2.1/api/ps\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"models\": [{\"name\": \"cut\"}]}",
        b"HTTP/1.1 200 OK\r\n\r\nnot json",
    ] {
        t.g.ollama = answering(answer.to_vec());
        assert_eq!(t.g.card_busy().unwrap(), None, "{}", String::from_utf8_lossy(answer));
    }
}

// ---------------------------------------------------------------- the module's log, the heat

#[test]
fn reading_the_modules_result() {
    assert_eq!(module_result(UNLOCKED_LOG), (true, "The module reports the card unlocked".into()));
    let (ok, why) = module_result("turing_unlock: no supported card at \"0000:02:00.0\"\n");
    assert!(!ok);
    assert!(why.contains("no supported card"));
    let (_, why) = module_result("pci 0000:01:00.0: turing_unlock: firmware did not stop; retaining DMA resources until power off\n");
    assert!(why.contains("did not stop"));
    assert_eq!(
        module_result("usb 1-3: new device\n"),
        (false, "The module did not run at this start (it must be in the start-up image)".into())
    );
    assert_eq!(module_result("turing_unlock: loading\n"), (false, "The module gave no result (turing_unlock: loading)".into()));
}

#[test]
fn hot_or_slowing_down_holds_the_session_lower() {
    assert!(!thermal_verdict(&[(Some(70), false), (Some(84), false)]));
    assert!(thermal_verdict(&[(Some(70), false), (Some(86), false)]));
    assert!(thermal_verdict(&[(Some(60), true)]));
    assert!(!thermal_verdict(&[(None, false)]));
    let mut t = T::new();
    t.g.sample_hook = Some(Box::new(|| Ok((Some(88), false))));
    t.g.watch.set((1.0, 0.0));
    assert!(t.g.thermal_watch().unwrap());
    assert_eq!(js(&t.state("power-hold"))["watts"], 200);
    assert_eq!(js(&t.state("state.json"))["state"], "throttled");
    assert!(text(&t.state("log")).contains("heat watch: 88 C, slowdown False; power held at 200 W"));
}

#[test]
fn a_cool_card_is_watched_to_the_end() {
    let mut t = T::new();
    let n = Rc::new(Cell::new(0));
    let m = n.clone();
    t.g.sample_hook = Some(Box::new(move || {
        m.set(m.get() + 1);
        Ok((Some(70), false))
    }));
    t.g.watch.set((0.002, 0.01)); // 0.12 s, a sample every 10 ms
    assert!(!t.g.thermal_watch().unwrap());
    assert!(n.get() >= 3, "{}", n.get());
    assert!(!t.exists("power-hold"));
}

// ---------------------------------------------------------------- Python's ways

#[test]
fn numbers_as_python_reads_and_writes_them() {
    for (s, v) in [
        ("1.5", Some(1.5)),
        (" 260.00 ", Some(260.0)),
        ("1_0", Some(10.0)),
        ("+.5", Some(0.5)),
        ("1e2", Some(100.0)),
        ("\u{663}", Some(3.0)),
    ] {
        assert_eq!(py_float(s), v, "{s}");
    }
    assert!(py_float("nan").unwrap().is_nan());
    assert_eq!(py_float("-Infinity"), Some(f64::NEG_INFINITY));
    for s in ["", "_1", "1_", "1__0", "0x10", "1e", ".", "--1", "[N/A]", "e5", "1.5.5"] {
        assert_eq!(py_float(s), None, "{s}");
    }
    assert_eq!(
        (fmt_f(0.25, 1), fmt_f(0.35, 1), fmt_f(f64::NAN, 1), fmt_f(1.0 / 0.0, 2)),
        ("0.2".into(), "0.3".into(), "nan".into(), "inf".into())
    );
    assert_eq!((hex8(0xde000000), hex8(-16)), ("0xde000000".into(), "0x-0000010".into()));
    assert_eq!(split_max("  a  b c  ", 1), ["a", "b c  "]);
    assert_eq!(driver_version(OPEN.trim_end()).as_deref(), Some("615.78.08"));
    assert_eq!(driver_version(" 1.2.x 3.4 ").as_deref(), Some("3.4"));
    assert_eq!(driver_version(" 1.2.3.4 "), None);
}

// ---------------------------------------------------------------- the grub.d fragment (grub-entry)

fn checked_entry_text(content: &str) -> Option<String> {
    let d = TempDir::new();
    let p = d.p("entry.cfg");
    put(&p, content);
    checked_entry(&p)
}

#[test]
fn nothing_without_an_attempt() {
    let t = T::new();
    assert_eq!(t.g.grub_entry(), 0);
    assert_eq!(t.g.captured.as_ref().unwrap().borrow().0, "");
}

#[test]
fn the_entry_while_armed() {
    let t = T::new();
    put(&t.state("entry.cfg"), GOOD_ENTRY);
    assert_eq!(t.g.grub_entry(), 0);
    let c = t.g.captured.as_ref().unwrap().borrow();
    assert_eq!(c.0, GOOD_ENTRY);
    assert_eq!(c.1, "Adding the graphics card unlock entry (one try, picked by hand)\n");
}

#[test]
fn a_file_of_the_wrong_shape_prints_nothing() {
    let g = GOOD_ENTRY;
    for bad in [
        g.replace("--id yggdrasil-gpu-unlock", "--id other"),
        g.replace("targets=0000:01:00.0", "targets=all"),
        g.replace("targets=0000:01:00.0", "targets=0000:01:00.0 init=/bin/sh"),
        format!("{g}{g}"),
        g.replace("\tinitrd", "\tsubmenu 'x' {\n\tinitrd"),
        g.replace("\tinitrd", "\tlinux\t/other\n\tinitrd"),
        g.replace("\tinitrd", "\u{2003}linux\t/other\n\tinitrd"),
        g.replace("\tinitrd", "  menuentry 'x' {\n\tinitrd"),
        g.replace("\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw quiet", "\tlinux"),
        g.replace(" turing_unlock", "turing_unlock"),
        g.replace("0000:01:00.0", "0000:01:00.8"),
        g.replace("rw quiet", "rw\0quiet"),
        g[..g.len() - 2].to_string(),
        g[..g.len() - 1].to_string(), // no newline after the last brace: grub-mkconfig's "### END" line would be glued to it
        format!("{g}\n"),
        String::new(),
    ] {
        assert_eq!(checked_entry_text(&bad), None, "{bad:?}");
    }
    let mut latin1 = g.as_bytes().to_vec();
    latin1.splice(20..20, [0xe8]);
    let d = TempDir::new();
    put(&d.p("e"), latin1);
    assert_eq!(checked_entry(&d.p("e")), None, "not UTF-8");
    // what a kernel line may look like
    assert!(checked_entry_text(&g.replace("\tlinux\t", "  linux    ")).is_some());
    assert!(checked_entry_text(&g.replace("rw quiet", "rw\u{3000}quiet")).is_some());
}

#[test]
fn a_link_or_a_folder_prints_nothing() {
    let d = TempDir::new();
    put(&d.p("good.cfg"), GOOD_ENTRY);
    std::os::unix::fs::symlink(d.p("good.cfg"), d.p("entry.cfg")).unwrap();
    assert_eq!(checked_entry(&d.p("good.cfg")).as_deref(), Some(GOOD_ENTRY));
    assert_eq!(checked_entry(&d.p("entry.cfg")), None);
    fs::create_dir(d.p("dir.cfg")).unwrap();
    assert_eq!(checked_entry(&d.p("dir.cfg")), None);
}

#[test]
fn help_and_unknown_commands() {
    let t = T::new();
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(t.g.main(&a(&["gpuunlock"])), 2);
    assert_eq!(t.g.main(&a(&["gpuunlock", "--help"])), 0);
    assert_eq!(t.g.captured.as_ref().unwrap().borrow().0, format!("{DOC}\n{DOC}\n"));
    assert_eq!(t.g.main(&a(&["gpuunlock", "arm", "now"])), 2);
    assert_eq!(t.g.main(&a(&["gpuunlock", "frobnicate"])), 2);
    assert_eq!(
        t.g.captured.as_ref().unwrap().borrow().1,
        "gpuunlock: unknown command (see --help)\ngpuunlock: unknown command (see --help)\n"
    );
    assert_eq!(t.g.main(&a(&["gpuunlock", "status", "--json"])), 0);
    assert!(t.fake.borrow().calls.iter().all(|c| c[0] == "modinfo"), "status runs only modinfo");
}

#[test]
fn the_real_runner() {
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(run_program(&a(&["sh", "-c", "echo out; echo err >&2; exit 3"]), 10).unwrap(), (3, "out\nerr".into()));
    assert_eq!(run_program(&a(&["no-such-program-here"]), 10).unwrap(), (127, "no-such-program-here: not installed".into()));
    let t = std::time::Instant::now();
    assert_eq!(run_program(&a(&["sleep", "30"]), 1).unwrap(), (124, "sleep: timed out".into()));
    assert!(t.elapsed() < Duration::from_secs(10));
    let d = TempDir::new();
    put(&d.p("x"), "#!/bin/sh\n");
    fs::set_permissions(d.p("x"), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(run_program(&[d.p("x")], 10).unwrap_err().errno, libc::EACCES);
}
