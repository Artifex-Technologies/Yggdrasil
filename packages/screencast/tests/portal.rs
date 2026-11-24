//! Screen sharing through yggdrasil-portal, end to end but for the picker's window: a D-Bus of the test's own, its PipeWire
//! and headless compositor (two screens), the portal's build (YGGDRASIL_PORTAL, else target/debug/yggdrasil-portal) and
//! yggdrasil-screencast. A stand-in picker answers for the person (the screen named in the world's `pick` file, or a cancel)
//! and notes what it was asked; a stand-in worker notes its process and runs the real one in its place.
//!
//! Five sessions, as the portal frontend drives them: one shared and closed by the program (its worker ends; the bar's item
//! says Sharing, then off); one whose screen is unplugged (its worker ends, the session says Closed); one the person cancels
//! (no worker); one ended from the bar (`yggdrasil-portal --stop-sharing`: the worker ends, the session says Closed, the
//! item goes off); and one whose portal dies (its worker dies with it: a screen is never left shared by a worker nobody
//! owns).

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use zbus::zvariant::{ObjectPath, OwnedValue, Value};

mod common;
use common::{build_of, ctl, line_with, on_path, skip, wait_for, World};

const NAME: &str = "org.freedesktop.impl.portal.desktop.yggdrasil";
const PATH: &str = "/org/freedesktop/portal/desktop";
const CAST: &str = "org.freedesktop.impl.portal.ScreenCast";
const SESSION: &str = "org.freedesktop.impl.portal.Session";

type Results = HashMap<String, OwnedValue>;
type Streams = Vec<(u32, HashMap<String, OwnedValue>)>;

fn path(p: &str) -> ObjectPath<'_> {
    ObjectPath::try_from(p).unwrap()
}

fn session(n: u32) -> String {
    format!("/org/freedesktop/portal/desktop/session/1_1/s{n}")
}

fn request(n: u32) -> String {
    format!("/org/freedesktop/portal/desktop/request/1_1/r{n}")
}

/// The processes the stand-in worker noted, in order.
fn workers(dir: &Path) -> Vec<u32> {
    std::fs::read_to_string(dir.join("workers")).unwrap_or_default().lines().filter_map(|l| l.parse().ok()).collect()
}

/// The bar item's state and its lines' titles and values, as the portal last wrote them.
fn bar(dir: &Path) -> (String, Vec<(String, String)>) {
    let text = std::fs::read_to_string(dir.join(".local/state/yggdrasil/screen-sharing.json")).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("the bar item's JSON: {e}: {text}"));
    let lines = v["lines"]
        .as_array()
        .map(|l| l.iter().map(|x| (x["title"].as_str().unwrap_or("").into(), x["value"].as_str().unwrap_or("").into())).collect())
        .unwrap_or_default();
    (v["state"].as_str().unwrap_or("").to_string(), lines)
}

/// Waits for the session `path` to say Closed (a thread listening from now on): the receiver hears it.
fn closed_signal(conn: &zbus::blocking::Connection, path: &str) -> mpsc::Receiver<()> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(SESSION)
        .unwrap()
        .member("Closed")
        .unwrap()
        .path(path.to_string())
        .unwrap()
        .build();
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, conn, None).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if signals.into_iter().next().is_some() {
            let _ = tx.send(());
        }
    });
    rx
}

fn alive(pid: u32) -> bool {
    // a process ended but not reaped yet is a zombie: gone as far as sharing goes
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|s| s.rsplit(')').next().is_some_and(|r| !r.trim_start().starts_with('Z')))
}

/// CreateSession, SelectSources and Start for session `n`: Start's answer.
fn share(cast: &zbus::blocking::Proxy, n: u32, cursor: bool) -> (u32, Results) {
    let s = session(n);
    let (code, made): (u32, Results) =
        cast.call("CreateSession", &(path(&request(n * 10)), path(&s), "org.example.Call", HashMap::<&str, Value>::new())).unwrap();
    assert_eq!(code, 0, "CreateSession");
    assert_eq!(String::try_from(made["session_id"].clone()).unwrap(), s);
    let mut opts: HashMap<&str, Value> = HashMap::new();
    opts.insert("types", Value::U32(1));
    opts.insert("multiple", Value::Bool(false));
    opts.insert("cursor_mode", Value::U32(if cursor { 2 } else { 1 }));
    let (code, _): (u32, Results) = cast.call("SelectSources", &(path(&request(n * 10 + 1)), path(&s), "org.example.Call", opts)).unwrap();
    assert_eq!(code, 0, "SelectSources");
    cast.call("Start", &(path(&request(n * 10 + 2)), path(&s), "org.example.Call", "", HashMap::<&str, Value>::new())).unwrap()
}

#[test]
fn a_program_shares_the_screen_it_was_given_until_it_stops() {
    let Some(mut w) = World::new("ygg-sc-portal") else { return };
    let Some(portal) = build_of("YGGDRASIL_PORTAL", "yggdrasil-portal") else {
        return skip("no portal build (YGGDRASIL_PORTAL, else target/debug/yggdrasil-portal)");
    };
    if !on_path("dbus-daemon") {
        return skip("dbus-daemon is not installed");
    }
    let dir = w.dir.clone();
    w.start_pipewire();
    let (display, ctl_sock) = w.start_compositor(&["--output", "320x240", "--output", "200x150"]);

    // the stand-ins, first on the portal's PATH
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let d = dir.display();
    let picker = format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {d}/picker-args\n[ -f {d}/pick ] || exit 1\ncat {d}/pick\n");
    let worker = format!(
        "#!/bin/sh\necho $$ >> {d}/workers\nprintf '%s\\n' \"$*\" >> {d}/worker-args\nexec {} \"$@\"\n",
        env!("CARGO_BIN_EXE_yggdrasil-screencast")
    );
    // the program's menu entry: the bar names it
    std::fs::create_dir_all(dir.join(".local/share/applications")).unwrap();
    std::fs::write(
        dir.join(".local/share/applications/org.example.Call.desktop"),
        "[Desktop Entry]\nType=Application\nName=Example Call\n",
    )
    .unwrap();
    for (name, text) in [("yggdrasil-share-picker", picker), ("yggdrasil-screencast", worker)] {
        std::fs::write(bin.join(name), text).unwrap();
        std::process::Command::new("chmod").arg("+x").arg(bin.join(name)).status().unwrap();
    }

    // the test's own D-Bus
    let mut bus = w.command("dbus-daemon");
    bus.args(["--session", "--nofork", "--print-address=1"])
        .arg(format!("--address=unix:path={}", dir.join("bus").display()))
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut bus = bus.spawn().expect("dbus-daemon starts");
    let address = format!("unix:{}", line_with(&mut bus, "unix:", 10).expect("the bus's address"));
    w.keep(bus);

    let path_var = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let mut p = w.command(&portal);
    p.env("DBUS_SESSION_BUS_ADDRESS", &address).env("WAYLAND_DISPLAY", &display).env("PATH", &path_var).stdout(Stdio::null());
    let mut portal_child = p.spawn().expect("the portal starts");

    let conn = zbus::blocking::connection::Builder::address(address.as_str()).unwrap().build().unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&conn).unwrap();
    assert!(wait_for(10, || dbus.name_has_owner(NAME.try_into().unwrap()).unwrap_or(false)), "the portal on the bus");
    assert_eq!(bar(&dir).0, "off", "nothing shared: the bar item is off from the start");
    let cast = zbus::blocking::Proxy::new(&conn, NAME, PATH, CAST).unwrap();
    assert_eq!(cast.get_property::<u32>("AvailableSourceTypes").unwrap(), 1, "screens only");
    assert_eq!(cast.get_property::<u32>("AvailableCursorModes").unwrap(), 3, "hidden or painted in");

    // 1. shared, then closed by the program: its worker ends
    std::fs::write(dir.join("pick"), "HEADLESS-1\n").unwrap();
    let (code, res) = share(&cast, 1, true);
    assert_eq!(code, 0, "Start");
    let streams = Streams::try_from(res["streams"].clone()).unwrap();
    assert_eq!(streams.len(), 1);
    let (node, props) = &streams[0];
    assert!(*node > 0 && *node != u32::MAX, "a PipeWire node: {node}");
    assert_eq!(<(i32, i32)>::try_from(props["position"].clone()).unwrap(), (0, 0));
    assert_eq!(<(i32, i32)>::try_from(props["size"].clone()).unwrap(), (320, 240));
    assert_eq!(u32::try_from(&props["source_type"]).unwrap(), 1);
    let asked = std::fs::read_to_string(dir.join("picker-args")).unwrap();
    assert_eq!(asked.lines().last(), Some("--app org.example.Call"), "the picker is told who asks, and that one screen is wanted");
    let ran = std::fs::read_to_string(dir.join("worker-args")).unwrap();
    assert_eq!(ran.lines().last(), Some("--output HEADLESS-1 --cursor"), "the chosen screen, with the pointer as asked");
    let first = workers(&dir)[0];
    assert!(alive(first));
    assert_eq!(bar(&dir), ("alert".into(), vec![("Example Call".into(), "HEADLESS-1".into())]), "the bar says who sees what");
    let s1 = zbus::blocking::Proxy::new(&conn, NAME, session(1), SESSION).unwrap();
    let () = s1.call("Close", &()).unwrap();
    assert!(wait_for(5, || !alive(first)), "the worker ends with its session");
    assert_eq!(bar(&dir).0, "off");

    // 2. its screen unplugged: the worker ends and the session says Closed
    std::fs::write(dir.join("pick"), "HEADLESS-2\n").unwrap();
    let rx = closed_signal(&conn, &session(2));
    let (code, res) = share(&cast, 2, false);
    assert_eq!(code, 0);
    let streams = Streams::try_from(res["streams"].clone()).unwrap();
    assert_eq!(<(i32, i32)>::try_from(streams[0].1["position"].clone()).unwrap(), (320, 0), "the second screen's place");
    assert_eq!(std::fs::read_to_string(dir.join("worker-args")).unwrap().lines().last(), Some("--output HEADLESS-2"));
    let second = workers(&dir)[1];
    ctl(&ctl_sock, "unplug HEADLESS-2");
    assert!(wait_for(5, || !alive(second)), "the worker ends with its screen");
    rx.recv_timeout(Duration::from_secs(5)).expect("the session says Closed");
    assert!(wait_for(5, || bar(&dir).0 == "off"), "the bar item goes off with the screen");

    // 3. the person cancels: no worker
    std::fs::remove_file(dir.join("pick")).unwrap();
    let (code, res) = share(&cast, 3, false);
    assert_eq!(code, 1, "cancelled");
    assert!(!res.contains_key("streams"));
    assert_eq!(workers(&dir).len(), 2, "nothing started");

    // 4. ended from the bar: Stop sharing runs `yggdrasil-portal --stop-sharing stop` (the item's act with the button's id)
    std::fs::write(dir.join("pick"), "HEADLESS-1\n").unwrap();
    let rx = closed_signal(&conn, &session(4));
    assert_eq!(share(&cast, 4, false).0, 0);
    let stopped = workers(&dir)[2];
    assert_eq!(bar(&dir).0, "alert");
    let mut stop = w.command(&portal);
    stop.args(["--stop-sharing", "stop"]).env("DBUS_SESSION_BUS_ADDRESS", &address);
    assert!(stop.status().unwrap().success(), "yggdrasil-portal --stop-sharing");
    assert_eq!(bar(&dir).0, "off", "the bar item goes off at once");
    assert!(wait_for(5, || !alive(stopped)), "the worker ends");
    rx.recv_timeout(Duration::from_secs(5)).expect("the session says Closed, so the program knows");

    // 5. the portal dies: its worker dies with it
    let (code, _) = share(&cast, 5, false);
    assert_eq!(code, 0);
    let fourth = workers(&dir)[3];
    assert!(alive(fourth));
    let _ = portal_child.kill();
    let _ = portal_child.wait();
    assert!(wait_for(5, || !alive(fourth)), "no worker outlives the portal");
    drop(w);
}
