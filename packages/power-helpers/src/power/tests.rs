//! The merged service without hardware: stand-in parts (a loop, one that fails, one that panics, one that stands still, one
//! that ends by itself) on real threads, switched through files in a temporary folder. What each part did is logged, so the
//! isolation rules can be checked: a failure stays with its part, a panic stops every part with its failure stop and ends
//! the service with an error, a part standing still gets its safe state from the supervisor. Then `power on`/`off` against
//! a running supervisor, and `power stopped` on a fake Commander Pro.

use std::fs;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::testutil::{ctx, TempDir};

/// What the parts did, and the hold a part standing still waits on until the test lets it go.
#[derive(Default)]
struct Log(Mutex<Vec<String>>, Mutex<bool>, Condvar);

impl Log {
    /// as if stuck inside a driver: no beat until the test lets go (a minute at most, so a broken test cannot hang)
    fn hold(&self) {
        let free = self.1.lock().unwrap();
        let _ = self.2.wait_timeout_while(free, Duration::from_secs(60), |free| !*free).unwrap();
    }
    fn let_go(&self) {
        *self.1.lock().unwrap() = true;
        self.2.notify_all();
    }
    fn push(&self, s: String) {
        self.0.lock().unwrap().push(s);
    }
    fn count(&self, s: &str) -> usize {
        self.0.lock().unwrap().iter().filter(|l| *l == s).count()
    }
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Loop,
    Fail,
    Panic,
    Stall,
    End,
}

struct Fake {
    name: &'static str,
    kind: Kind,
    log: Arc<Log>,
    restart: Restart,
}

impl Part for Fake {
    fn name(&self) -> &'static str {
        self.name
    }
    fn run(&self, ctx: &Ctx) -> i32 {
        self.log.push(format!("{} run", self.name));
        match self.kind {
            Kind::Loop => {
                while !ctx.should_stop() {
                    ctx.sleep(0.01);
                }
                0
            }
            Kind::Fail => 1,
            Kind::Panic => {
                ctx.wait(0.05);
                panic!("a stand-in part panicking on purpose");
            }
            Kind::Stall => {
                self.log.hold(); // no beat: as if stuck inside a driver
                0
            }
            Kind::End => 0,
        }
    }
    fn after(&self, _ctx: &Ctx, failed: bool) {
        self.log.push(format!("{} {}", self.name, if failed { "failure stop" } else { "clean stop" }));
    }
    fn restart(&self) -> Restart {
        self.restart
    }
    fn stall(&self) -> Option<f64> {
        (self.kind == Kind::Stall).then_some(0.2)
    }
    fn on_stall(&self, _ctx: &Ctx) {
        self.log.push(format!("{} safe state from the supervisor", self.name));
    }
}

const QUICK: Restart = Restart { delay: 0.05, burst: 3, interval: 60.0 };

fn fake(name: &'static str, kind: Kind, log: &Arc<Log>) -> Arc<dyn Part> {
    Arc::new(Fake { name, kind, log: log.clone(), restart: QUICK })
}

fn base(d: &TempDir) -> Base {
    Base { root: d.path().to_path_buf(), dry: false, steps: None, real_card: false, cards: None, test: true }
}

fn service(d: &TempDir, parts: Vec<Arc<dyn Part>>) -> Service {
    let on = d.path().join("on");
    fs::create_dir_all(&on).unwrap();
    let mut s = Service::new(parts, base(d), Switches::Dir(on), Some(d.path().join("state.json"))).unwrap();
    s.stop_wait = 3.0;
    s.tick = 0.05;
    s
}

/// A switch as `power on`/`off` make it: the token is written to a file beside it and renamed into place, so the supervisor
/// never sees the switch half written. (fs::write straight to the switch creates it empty first; inotify wakes the supervisor
/// on that, it starts the part on the token "", then sees "t1" and takes it as switched on anew: a clean stop and a second
/// start that the tests counted as wrong. That was the runners' failures, never the service's.)
fn switch(d: &TempDir, name: &str, token: Option<&str>) {
    let p = d.path().join("on").join(name);
    match token {
        Some(t) => {
            let tmp = p.with_extension("tmp");
            fs::write(&tmp, format!("{t}\n")).unwrap();
            fs::rename(&tmp, &p).unwrap();
        }
        None => fs::remove_file(p).unwrap(),
    }
}

/// (state, token) of a part in DIR/state.json
fn state(dir: &Path, name: &str) -> (String, String) {
    let Ok(v) = fs::read_to_string(dir.join("state.json")).map(|t| json::parse(&t).unwrap()) else { return (String::new(), String::new()) };
    let p = v.obj().unwrap().at("parts").obj().unwrap().at(name).clone();
    let o = p.obj().unwrap();
    (o.at("state").str().unwrap_or("").into(), o.at("token").str().unwrap_or("").into())
}

/// Waits for an event the test can see; the limit only stops a broken test from hanging (a busy runner is slow, not wrong).
fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("never: {what}");
}

fn spawn(svc: Service) -> (Arc<Stopper>, JoinHandle<i32>) {
    let quit = svc.quit.clone();
    let mut svc = svc;
    (quit, std::thread::spawn(move || svc.run()))
}

#[test]
fn a_part_switched_on_runs_and_switched_off_makes_its_clean_stop() {
    let (d, log) = (TempDir::new(), Arc::new(Log::default()));
    let (quit, h) = spawn(service(&d, vec![fake("a", Kind::Loop, &log), fake("b", Kind::Loop, &log)]));
    switch(&d, "a", Some("t1"));
    until("a running", || state(d.path(), "a") == ("running".into(), "t1".into()));
    assert_eq!(state(d.path(), "b").0, "off", "only what is switched on runs");
    switch(&d, "a", None);
    until("a off", || state(d.path(), "a").0 == "off");
    assert_eq!(log.lines(), ["a run", "a clean stop"]);
    quit.stop(Why::Clean);
    assert_eq!(h.join().unwrap(), 0);
}

#[test]
fn switched_on_anew_is_a_clean_stop_then_a_start_and_the_others_are_left_alone() {
    let (d, log) = (TempDir::new(), Arc::new(Log::default()));
    let (quit, h) = spawn(service(&d, vec![fake("a", Kind::Loop, &log), fake("b", Kind::Loop, &log)]));
    switch(&d, "a", Some("t1"));
    switch(&d, "b", Some("t1"));
    until("both running", || state(d.path(), "a").0 == "running" && state(d.path(), "b").0 == "running");
    switch(&d, "a", Some("t2")); // systemctl restart yggdrasil-a
    until("a again", || state(d.path(), "a") == ("running".into(), "t2".into()));
    assert_eq!((log.count("a run"), log.count("a clean stop")), (2, 1));
    assert_eq!((log.count("b run"), log.count("b clean stop")), (1, 0), "b never stopped");
    quit.stop(Why::Clean);
    assert_eq!(h.join().unwrap(), 0);
    assert_eq!((log.count("a clean stop"), log.count("b clean stop")), (2, 1), "a clean stop for each at the end");
}

#[test]
fn a_failure_stays_with_its_part_which_starts_again_until_its_limit() {
    let (d, log) = (TempDir::new(), Arc::new(Log::default()));
    let (quit, h) = spawn(service(&d, vec![fake("f", Kind::Fail, &log), fake("a", Kind::Loop, &log)]));
    switch(&d, "a", Some("t1"));
    switch(&d, "f", Some("t1"));
    until("f given up", || state(d.path(), "f").0 == "failed");
    assert_eq!((log.count("f run"), log.count("f failure stop")), (3, 3), "three starts (its limit), each with its failure stop");
    assert_eq!(
        (state(d.path(), "a").0.as_str(), log.count("a run"), log.count("a clean stop") + log.count("a failure stop")),
        ("running", 1, 0)
    );
    // switched on anew: a fresh start limit
    switch(&d, "f", Some("t2"));
    until("f tried again", || log.count("f run") == 6);
    quit.stop(Why::Clean);
    assert_eq!(h.join().unwrap(), 0);
}

#[test]
fn a_panic_stops_every_part_with_its_failure_stop_and_ends_the_service_with_an_error() {
    let (d, log) = (TempDir::new(), Arc::new(Log::default()));
    let (_quit, h) = spawn(service(&d, vec![fake("a", Kind::Loop, &log), fake("p", Kind::Panic, &log)]));
    switch(&d, "a", Some("t1"));
    until("a running", || state(d.path(), "a").0 == "running");
    switch(&d, "p", Some("t1"));
    assert_eq!(h.join().unwrap(), 1, "systemd starts it again");
    assert_eq!(log.count("p failure stop"), 1, "the part's own failure stop, on its thread");
    assert_eq!(log.count("a failure stop"), 1, "and the others' failure stops");
    assert_eq!(log.count("a clean stop"), 0);
}

#[test]
fn a_loop_standing_still_gets_its_safe_state_from_the_supervisor_and_the_others_their_stops() {
    let (d, log) = (TempDir::new(), Arc::new(Log::default()));
    let mut svc = service(&d, vec![fake("a", Kind::Loop, &log), fake("s", Kind::Stall, &log)]);
    switch(&d, "a", Some("t1"));
    switch(&d, "s", Some("t1"));
    let code = svc.run();
    assert_eq!(code, 1);
    assert!(log.count("s safe state from the supervisor") >= 1);
    assert_eq!(log.count("a failure stop"), 1, "the part that was fine made its failure stop before the service ended");
    assert!(svc.stuck(), "the stuck thread is left behind (the process exits without it)");
    assert_eq!((log.count("s clean stop"), log.count("s failure stop")), (0, 0), "its own stop never came: the supervisor made it");
    log.let_go();
}

#[test]
fn a_part_that_ends_by_itself_stays_ended_until_switched_on_anew() {
    let (d, log) = (TempDir::new(), Arc::new(Log::default()));
    let (quit, h) = spawn(service(&d, vec![fake("e", Kind::End, &log)]));
    switch(&d, "e", Some("t1"));
    until("e ended", || state(d.path(), "e").0 == "ended");
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(log.lines(), ["e run", "e clean stop"], "not started again: it ended with exit code 0 (gpufan on auto)");
    switch(&d, "e", Some("t2"));
    until("e again", || log.count("e run") == 2);
    switch(&d, "e", None);
    until("e off", || state(d.path(), "e").0 == "off");
    quit.stop(Why::Clean);
    assert_eq!(h.join().unwrap(), 0);
}

#[test]
fn a_dry_run_runs_the_named_parts_their_rounds_once() {
    let (d, log) = (TempDir::new(), Arc::new(Log::default()));
    let mut b = base(&d);
    (b.dry, b.steps) = (true, Some(3));
    let mut svc = Service::new(
        vec![fake("a", Kind::Loop, &log), fake("f", Kind::Fail, &log), fake("x", Kind::Loop, &log)],
        b,
        Switches::Fixed(vec!["a".into(), "f".into()]),
        None,
    )
    .unwrap();
    svc.tick = 0.05;
    assert_eq!(svc.run(), 0);
    assert_eq!(log.lines().len(), 4);
    assert_eq!(
        (log.count("a run"), log.count("a clean stop"), log.count("f run"), log.count("f failure stop"), log.count("x run")),
        (1, 1, 1, 1, 0)
    );
}

#[test]
fn on_and_off_wait_for_the_service() {
    let (d, log) = (TempDir::new(), Arc::new(Log::default()));
    let c = ctx(d.path());
    let switches = c.path(SWITCHES);
    fs::create_dir_all(&switches).unwrap();
    let mut svc = Service::new(vec![fake("tune", Kind::Loop, &log)], base(&d), Switches::Dir(switches), Some(c.path(STATE_FILE))).unwrap();
    svc.tick = 0.05;
    let (quit, h) = spawn(svc);
    // the service counts as running for `off` (it waits for the part's stop only then)
    d.files("", &[("commands.json", r#"{"systemctl is-active --quiet yggdrasil-power.service": ""}"#)]);
    assert_eq!(cmd_on(&c, "tune"), 0);
    assert_eq!(state(&c.path(RUN_DIR), "tune").0, "running");
    assert_eq!(cmd_off(&c, "tune"), 0);
    assert_eq!(log.lines(), ["tune run", "tune clean stop"], "off returned after the clean stop");
    assert_eq!(cmd_on(&c, "fans"), 2);
    quit.stop(Why::Clean);
    assert_eq!(h.join().unwrap(), 0);
}

#[test]
fn adopt_restarts_only_the_units_that_still_run_an_old_helper() {
    assert!(old_process("890107\n") && !old_process("0\n") && !old_process("") && !old_process("Running in chroot"));
    let d = TempDir::new();
    let c = ctx(d.path());
    d.files("", &[("commands.json", r#"{"systemctl show -p MainPID --value yggdrasil-casefan": "4242\n", "systemctl show -p MainPID --value yggdrasil-gpufan": null, "systemctl show -p MainPID --value": "0\n"}"#)]);
    assert_eq!(cmd_adopt(&c), 0);
    let ran: Vec<String> = c.recorded.borrow().as_ref().unwrap().iter().filter(|l| l.contains("restart")).cloned().collect();
    assert_eq!(ran, ["systemctl restart yggdrasil-casefan.service"]);
}

#[test]
fn stopped_makes_the_stop_of_each_part_still_switched_on() {
    let d = TempDir::new();
    d.files(
        "sys/class/hwmon/hwmon5",
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
    d.files(
        "etc/yggdrasil",
        &[("case-fans.json", r#"{"ports": {"1": {"name": "pump", "role": "pump", "fixed": 100}, "6": {"role": "fan"}}}"#)],
    );
    let c = ctx(d.path());
    let pwm = |n: u8| fs::read_to_string(d.path().join(format!("sys/class/hwmon/hwmon5/pwm{n}"))).unwrap();
    stopped(&c, true);
    assert_eq!((pwm(1), pwm(6)), (String::new(), String::new()), "casefan not switched on: its ports are left alone");
    d.files("run/yggdrasil/power/on", &[("casefan", "t1")]);
    stopped(&c, false);
    assert_eq!((pwm(1), pwm(6)), ("255".into(), "153".into()), "a clean stop: the pump 100 %, fans 60 %");
    stopped(&c, true);
    assert_eq!((pwm(1), pwm(6)), ("255".into(), "255".into()), "after a failure: everything 100 %");
}
