//! The watcher and the fingerprint calls against stand-ins: a private D-Bus daemon of the test's own (never the system's or the
//! session's bus) carrying a pretend iio-sensor-proxy and a pretend fprintd, a pretend Hyprland socket in a temporary folder,
//! a pretend /sys, and /usr/bin/sleep standing in for the on-screen keyboard. No screen, sensor, reader or PAM file is touched.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use laptop::tablet::{Accel, Conf, Event, Orientation, Osk, State, Watcher};
use zbus::zvariant::{OwnedObjectPath, Value};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ygg-laptop-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A D-Bus daemon on a socket in a temporary folder, ended when dropped.
struct Bus {
    child: Child,
    address: String,
    dir: PathBuf,
}

impl Bus {
    fn start(name: &str) -> Option<Bus> {
        let daemon = Path::new("/usr/bin/dbus-daemon");
        if !daemon.exists() {
            eprintln!("skipped: no dbus-daemon");
            return None;
        }
        let dir = temp(name);
        let conf = dir.join("bus.conf");
        std::fs::write(
            &conf,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path={}/bus</listen><auth>EXTERNAL</auth>
<policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>"#,
                dir.display()
            ),
        )
        .unwrap();
        let mut cmd = Command::new(daemon);
        // the daemon ends with the test even when the test is killed
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let mut child = cmd
            .arg(format!("--config-file={}", conf.display()))
            .args(["--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut()?).read_line(&mut line).ok()?;
        Some(Bus { child, address: line.trim().to_string(), dir })
    }

    fn connect(&self) -> zbus::blocking::Connection {
        zbus::blocking::connection::Builder::address(self.address.as_str()).unwrap().build().unwrap()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// iio-sensor-proxy, as far as the watcher uses it.
struct Sensor {
    orientation: String,
    calls: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "net.hadess.SensorProxy")]
impl Sensor {
    fn claim_accelerometer(&self) {
        self.calls.lock().unwrap().push("claim".into());
    }
    fn release_accelerometer(&self) {
        self.calls.lock().unwrap().push("release".into());
    }
    #[zbus(property)]
    fn has_accelerometer(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn accelerometer_orientation(&self) -> String {
        self.orientation.clone()
    }
}

/// A Hyprland control socket that answers `monitors all` and `devices` and keeps every `eval`.
fn hyprland(dir: &Path) -> (yggdrasil_wm::hyprland::Hyprland, Arc<Mutex<Vec<String>>>) {
    let socket = dir.join(".socket.sock");
    let l = UnixListener::bind(&socket).unwrap();
    let evals: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = evals.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { return };
            let mut buf = [0u8; 8192];
            let n = c.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let answer = if req == "j/monitors all" {
                r#"[{"id":0,"name":"eDP-1","width":2880,"height":1800,"refreshRate":60.0,"x":0,"y":0,"scale":1.75,"transform":0,
                    "focused":true,"disabled":false,"reserved":[0,0,0,0],"activeWorkspace":{"id":1,"name":"1"},"specialWorkspace":{"id":0,"name":""}},
                   {"id":1,"name":"DP-2","width":3440,"height":1440,"refreshRate":100.0,"x":1646,"y":0,"scale":1,"transform":0,
                    "focused":false,"disabled":false,"reserved":[0,0,0,0],"activeWorkspace":{"id":2,"name":"2"},"specialWorkspace":{"id":0,"name":""}}]"#
                    .to_string()
            } else if req == "j/devices" {
                r#"{"mice":[],"keyboards":[],"touch":[{"address":"0x1","name":"wacom-hid-5349-finger"}],
                    "tablets":[{"address":"0x2","name":"wacom-hid-5349-pen"}],"switches":[{"address":"0x3","name":"thinkpad-extra-buttons"}]}"#
                    .to_string()
            } else if let Some(lua) = req.strip_prefix("/eval ") {
                log.lock().unwrap().push(lua.to_string());
                "ok".to_string()
            } else {
                "unknown request".to_string()
            };
            let _ = c.write_all(answer.as_bytes());
        }
    });
    (yggdrasil_wm::hyprland::Hyprland { socket, runtime: dir.to_path_buf() }, evals)
}

fn transforms(evals: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    evals
        .lock()
        .unwrap()
        .iter()
        .map(|l| {
            assert!(l.starts_with("hl.monitor({ output = \"eDP-1\", mode = \"2880x1800@60\", position = \"0x0\", scale = 1.75"), "{l}");
            assert!(
                l.contains("hl.device({ name = \"wacom-hid-5349-finger\"") && l.contains("hl.device({ name = \"wacom-hid-5349-pen\""),
                "{l}"
            );
            l.split("transform = ").nth(1).unwrap_or("").chars().take(1).collect()
        })
        .collect()
}

#[test]
fn folding_into_a_tablet_turns_the_screen_with_the_sensor_and_shows_the_keyboard() {
    let Some(bus) = Bus::start("tablet") else { return };
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let server = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("net.hadess.SensorProxy")
        .unwrap()
        .serve_at("/net/hadess/SensorProxy", Sensor { orientation: "left-up".into(), calls: calls.clone() })
        .unwrap()
        .build()
        .unwrap();
    let client = bus.connect();
    let accel = Accel::new(&client).unwrap();
    assert!(accel.present());
    let (tx, rx) = mpsc::channel();
    accel.watch(tx.clone());

    let dir = temp("tablet-files");
    let (h, evals) = hyprland(&dir);
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    let conf_path = dir.join("config/tablet.conf");
    let state = State { has_switch: true, tablet: false, orientation: None, conf: Conf::default(), plugged_keyboard: false, rotation: 0 };
    let osk = Osk::new(vec!["/usr/bin/sleep".into(), "60".into()]);
    let mut w = Watcher::new(state, Some(h), Some(accel), osk, root.clone(), conf_path.clone());
    assert_eq!(w.applied, Some(0), "the screen's turn, as Hyprland reports it");

    w.step();
    assert!(evals.lock().unwrap().is_empty() && calls.lock().unwrap().is_empty(), "a laptop: nothing to do, the sensor left asleep");
    assert!(!w.osk.shown());

    w.handle(Event::Mode(true));
    w.step();
    assert_eq!(*calls.lock().unwrap(), ["claim"]);
    assert_eq!(transforms(&evals), ["1"], "folded: turned as the sensor reads, touch and pen with it");
    assert!(w.osk.shown(), "folded with no keyboard plugged in: the on-screen keyboard");

    // the sensor reads another way up and says so as iio-sensor-proxy does (PropertiesChanged)
    server.object_server().interface::<_, Sensor>("/net/hadess/SensorProxy").unwrap().get_mut().orientation = "bottom-up".into();
    let changed: HashMap<&str, Value> = HashMap::from([("AccelerometerOrientation", Value::from("bottom-up"))]);
    server
        .emit_signal(
            None::<&str>,
            "/net/hadess/SensorProxy",
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &("net.hadess.SensorProxy", changed, Vec::<&str>::new()),
        )
        .unwrap();
    // (zbus hands over the value it had first, then each change)
    loop {
        let e = rx.recv_timeout(Duration::from_secs(5)).expect("the reading arrives");
        let last = e == Event::Orientation(Some(Orientation::BottomUp));
        assert!(matches!(e, Event::Orientation(Some(_))), "{e:?}");
        w.handle(e);
        if last {
            break;
        }
    }
    w.step();
    assert_eq!(transforms(&evals), ["1", "2"]);

    Conf { rotation_lock: true, keyboard: true }.save(&conf_path).unwrap();
    w.handle(Event::Conf);
    w.step();
    assert_eq!(*calls.lock().unwrap(), ["claim", "release"], "locked: the sensor is let go");
    assert_eq!(transforms(&evals), ["1", "2"], "and the screen stays");

    // a Bluetooth keyboard with every letter key arrives
    let kb = root.join("sys/class/input/input30");
    std::fs::create_dir_all(kb.join("id")).unwrap();
    std::fs::create_dir_all(kb.join("capabilities")).unwrap();
    std::fs::write(kb.join("id/bustype"), "0005\n").unwrap();
    std::fs::write(kb.join("capabilities/key"), format!("{:x} {:x}\n", 0u64, (1u64 << 16) | (1 << 25) | (1 << 30) | (1 << 44) | (1 << 57)))
        .unwrap();
    w.handle(Event::Inputs);
    w.step();
    assert!(!w.osk.shown(), "a keyboard is plugged in: none on the screen");

    w.handle(Event::Mode(false));
    w.step();
    assert_eq!(transforms(&evals), ["1", "2", "0"], "opened as a laptop: upright again, lock or not");
    w.handle(Event::Reapply);
    w.step();
    assert_eq!(transforms(&evals), ["1", "2", "0", "0"], "Hyprland reread its config: turned again");
    drop(w);
    let _ = std::fs::remove_dir_all(&dir);
}

/// fprintd, as far as Settings uses it: one reader, fingers per user, enrolment in three touches.
struct Manager;

#[zbus::interface(name = "net.reactivated.Fprint.Manager")]
impl Manager {
    fn get_default_device(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/net/reactivated/Fprint/Device/0").unwrap()
    }
}

struct Reader {
    fingers: Arc<Mutex<Vec<String>>>,
    calls: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "net.reactivated.Fprint.Device")]
impl Reader {
    fn list_enrolled_fingers(&self, user: &str) -> Vec<String> {
        self.calls.lock().unwrap().push(format!("list {user}"));
        self.fingers.lock().unwrap().clone()
    }
    fn claim(&self, user: &str) {
        self.calls.lock().unwrap().push(format!("claim {user}"));
    }
    fn release(&self) {
        self.calls.lock().unwrap().push("release".into());
    }
    async fn enroll_start(&self, finger: &str, #[zbus(signal_emitter)] e: zbus::object_server::SignalEmitter<'_>) {
        self.calls.lock().unwrap().push(format!("enroll {finger}"));
        for (r, done) in [("enroll-stage-passed", false), ("enroll-retry-scan", false), ("enroll-completed", true)] {
            Reader::enroll_status(&e, r, done).await.unwrap();
        }
        self.fingers.lock().unwrap().push(finger.to_string());
    }
    fn enroll_stop(&self) {
        self.calls.lock().unwrap().push("stop".into());
    }
    fn delete_enrolled_fingers2(&self) {
        self.fingers.lock().unwrap().clear();
    }
    #[zbus(signal)]
    async fn enroll_status(e: &zbus::object_server::SignalEmitter<'_>, result: &str, done: bool) -> zbus::Result<()>;
    #[zbus(property, name = "name")]
    fn name(&self) -> String {
        "Synaptics Sensors".into()
    }
}

#[test]
fn the_reader_is_read_and_a_finger_enrolled() {
    use laptop::fingerprint;
    let Some(bus) = Bus::start("fprint") else { return };
    let client = bus.connect();
    let dir = temp("fprint-files");
    let pam = dir.join("sudo");
    std::fs::write(&pam, "#%PAM-1.0\nauth include system-auth\n").unwrap();
    // fprintd is not there: not installed, nothing else claimed
    let none = fingerprint::status(&client, "ada", &pam);
    assert_eq!(none, fingerprint::Status::default());

    let fingers: Arc<Mutex<Vec<String>>> = Arc::default();
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let _server = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("net.reactivated.Fprint")
        .unwrap()
        .serve_at("/net/reactivated/Fprint/Manager", Manager)
        .unwrap()
        .serve_at("/net/reactivated/Fprint/Device/0", Reader { fingers: fingers.clone(), calls: calls.clone() })
        .unwrap()
        .build()
        .unwrap();
    let s = fingerprint::status(&client, "ada", &pam);
    assert_eq!((s.installed, s.reader.as_deref(), s.fingers.len(), s.sudo), (true, Some("Synaptics Sensors"), 0, false));

    let mut steps = vec![];
    fingerprint::enroll(&client, "right-index-finger", |r| steps.push(r.to_string())).unwrap();
    assert_eq!(steps, ["enroll-stage-passed", "enroll-retry-scan", "enroll-completed"]);
    assert_eq!(
        *calls.lock().unwrap(),
        ["list ada", "claim ", "enroll right-index-finger", "stop", "release"],
        "claimed for the caller, let go after"
    );

    std::fs::write(&pam, fingerprint::pam_with("#%PAM-1.0\nauth include system-auth\n").unwrap()).unwrap();
    let s = fingerprint::status(&client, "ada", &pam);
    assert_eq!((s.fingers.clone(), s.sudo), (vec!["right-index-finger".to_string()], true));
    fingerprint::delete_all(&client).unwrap();
    assert!(fingerprint::status(&client, "ada", &pam).fingers.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_switch_and_settings_reach_the_watcher_over_its_socket() {
    let dir = temp("socket");
    assert_eq!(laptop::tablet::tell(&dir, "mode on"), Ok(false), "no watcher running: nothing to tell, no error");
    let (tx, rx) = mpsc::channel();
    laptop::tablet::listen(&laptop::tablet::socket_path(&dir), tx).unwrap();
    assert_eq!(laptop::tablet::tell(&dir, "mode on"), Ok(true));
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Event::Mode(true)));
    assert_eq!(laptop::tablet::tell(&dir, "conf"), Ok(true));
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Event::Conf));
    assert_eq!(laptop::tablet::tell(&dir, "reboot"), Err("unknown command".into()));
    assert!(rx.try_recv().is_err(), "nothing passed on for a line it does not know");
    let _ = std::fs::remove_dir_all(&dir);
}
