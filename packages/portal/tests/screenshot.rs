//! The Screenshot portal on a D-Bus of the test's own: the portal's build with stand-ins for grim, yggdrasil-shot and
//! yggdrasil-colorpicker first on its PATH (each notes how it was called and answers as the test sets it). Checked: a
//! screenshot of the whole desktop (grim, the file in the portal's own folder), an interactive one (yggdrasil-shot in its
//! portal mode), one the person cancels (no file left), only the newest ten kept, a picked colour, a cancelled pick, and a
//! pick the program closes while the picker is open (the picker stopped). Skipped where dbus-daemon is not installed, except
//! under YGGDRASIL_TEST_PORTAL_REQUIRED (CI), where that fails.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const NAME: &str = "org.freedesktop.impl.portal.desktop.yggdrasil";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SHOT: &str = "org.freedesktop.impl.portal.Screenshot";

type Results = HashMap<String, OwnedValue>;

struct World {
    dir: PathBuf,
    children: Vec<Child>,
}

impl Drop for World {
    fn drop(&mut self) {
        for c in self.children.iter_mut().rev() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn command(dir: &Path, program: &str) -> Command {
    let mut c = Command::new(program);
    // SAFETY: only an async-signal-safe prctl between fork and exec: the child dies with the test
    unsafe {
        c.pre_exec(|| rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL)).map_err(Into::into));
    }
    c.env_clear().env("HOME", dir).env("XDG_RUNTIME_DIR", dir).stdin(Stdio::null());
    c
}

fn request(n: u32) -> String {
    format!("/org/freedesktop/portal/desktop/request/1_1/r{n}")
}

fn call(proxy: &zbus::blocking::Proxy, method: &str, n: u32, interactive: bool) -> (u32, Results) {
    let mut opts: HashMap<&str, Value> = HashMap::new();
    opts.insert("interactive", Value::Bool(interactive));
    proxy.call(method, &(ObjectPath::try_from(request(n)).unwrap(), "org.example.Draw", "", opts)).unwrap()
}

fn file_of(res: &Results) -> PathBuf {
    let uri = String::try_from(res["uri"].clone()).unwrap();
    PathBuf::from(uri.strip_prefix("file://").expect("a file URI"))
}

fn last_line(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default().lines().last().unwrap_or_default().to_string()
}

#[test]
fn programs_get_screenshots_and_colours_without_touching_the_desktop() {
    let on_path = |n: &str| std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(n).is_file()));
    if !on_path("dbus-daemon") {
        assert!(std::env::var_os("YGGDRASIL_TEST_PORTAL_REQUIRED").is_none(), "dbus-daemon is not installed");
        eprintln!("skipped: dbus-daemon is not installed");
        return;
    }
    let dir = PathBuf::from(format!("/tmp/ygg-portal-shot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut w = World { dir: dir.clone(), children: vec![] };
    let d = dir.display();
    // grim PATH: a picture; yggdrasil-shot --portal PATH: one unless told to cancel; the picker: the colour in `colour`, a
    // cancel without it, a long wait with `slow`
    let tools = [
        ("grim", format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {d}/grim-args\nprintf 'PNG' > \"$1\"\n")),
        (
            "yggdrasil-shot",
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {d}/shot-args\n[ -f {d}/cancel ] && exit 1\nprintf 'PNG' > \"$2\"\n"),
        ),
        (
            "yggdrasil-colorpicker",
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {d}/picker-args\n[ -f {d}/slow ] && exec sleep 30\n[ -f {d}/colour ] || exit 1\ncat {d}/colour\n"
            ),
        ),
    ];
    for (name, text) in tools {
        std::fs::write(bin.join(name), text).unwrap();
        Command::new("chmod").arg("+x").arg(bin.join(name)).status().unwrap();
    }

    let mut bus = command(&dir, "dbus-daemon");
    bus.env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .args(["--session", "--nofork", "--print-address=1"])
        .arg(format!("--address=unix:path={}", dir.join("bus").display()))
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut bus = bus.spawn().expect("dbus-daemon starts");
    let out = bus.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Some(Ok(l)) = BufReader::new(out).lines().next() {
            let _ = tx.send(l);
        }
    });
    let address = rx.recv_timeout(Duration::from_secs(10)).expect("the bus's address");
    w.children.push(bus);

    let mut portal = command(&dir, env!("CARGO_BIN_EXE_yggdrasil-portal"));
    portal
        .env("DBUS_SESSION_BUS_ADDRESS", &address)
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()))
        .stdout(Stdio::null());
    w.children.push(portal.spawn().expect("the portal starts"));

    let conn = zbus::blocking::connection::Builder::address(address.as_str()).unwrap().build().unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&conn).unwrap();
    let up = (0..200).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        dbus.name_has_owner(NAME.try_into().unwrap()).unwrap_or(false)
    });
    assert!(up, "the portal on the bus");
    let shot = zbus::blocking::Proxy::new(&conn, NAME, PATH, SHOT).unwrap();
    let shots = dir.join("yggdrasil-portal/screenshots");

    // the whole desktop, through grim: a file of the portal's own, given straight to grim
    let (code, res) = call(&shot, "Screenshot", 1, false);
    assert_eq!(code, 0);
    let first = file_of(&res);
    assert_eq!(first.parent(), Some(shots.as_path()), "in the portal's own folder: {}", first.display());
    assert_eq!(std::fs::read(&first).unwrap(), b"PNG");
    assert_eq!(last_line(&dir.join("grim-args")), first.display().to_string(), "grim with the path alone");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(dir.join("yggdrasil-portal")).unwrap().permissions().mode() & 0o777, 0o700, "this user's only");

    // interactive: yggdrasil-shot in its portal mode
    let (code, res) = call(&shot, "Screenshot", 2, true);
    assert_eq!(code, 0);
    let chosen = file_of(&res);
    assert_eq!(last_line(&dir.join("shot-args")), format!("--portal {}", chosen.display()));

    // the person cancels: no answer, no file
    std::fs::write(dir.join("cancel"), "").unwrap();
    let (code, res) = call(&shot, "Screenshot", 3, true);
    assert_eq!(code, 1);
    assert!(!res.contains_key("uri"));
    assert_eq!(std::fs::read_dir(&shots).unwrap().count(), 2, "nothing left behind");

    // only the newest ten are kept
    for n in 4..16 {
        assert_eq!(call(&shot, "Screenshot", n, false).0, 0);
    }
    assert_eq!(std::fs::read_dir(&shots).unwrap().count(), 10);
    assert!(!first.exists(), "the oldest went first");

    // a colour, as three fractions; the picker in its portal mode
    std::fs::write(dir.join("colour"), "1 0 0.2\n").unwrap();
    let (code, res) = call(&shot, "PickColor", 20, false);
    assert_eq!(code, 0);
    assert_eq!(<(f64, f64, f64)>::try_from(res["color"].clone()).unwrap(), (1.0, 0.0, 0.2));
    assert_eq!(last_line(&dir.join("picker-args")), "--portal");

    // cancelled: no colour
    std::fs::remove_file(dir.join("colour")).unwrap();
    let (code, res) = call(&shot, "PickColor", 21, false);
    assert_eq!(code, 1);
    assert!(!res.contains_key("color"));

    // the program gives up while the picker is open: Request.Close stops the picker, and the call answers at once
    std::fs::write(dir.join("slow"), "").unwrap();
    let (tx, rx) = mpsc::channel();
    let shot2 = shot.clone();
    std::thread::spawn(move || {
        let _ = tx.send(call(&shot2, "PickColor", 22, false).0);
    });
    let req = zbus::blocking::Proxy::new(&conn, NAME, request(22), "org.freedesktop.impl.portal.Request").unwrap();
    let closed = (0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        req.call::<_, _, ()>("Close", &()).is_ok()
    });
    assert!(closed, "the request is there to close while the picker is open");
    let code = rx.recv_timeout(Duration::from_secs(5)).expect("the call answers once the picker is stopped");
    assert_eq!(code, 1);
    drop(w);
}
