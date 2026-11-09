//! The notices on a real D-Bus, with a stand-in notification server: never on the desktop's own bus. Run it on a private one:
//!
//!   dbus-run-session -- env CRASHWATCH_LIVE_DBUS=1 cargo test --release --test live_dbus -- --ignored
//!
//! It checks the whole way: a core appears, the watcher starts a notice, the notice reaches the server with its "Ask" action
//! (only with an assistant installed), a click asks the stand-in assistant, and its answer replaces the "looking" notice.

use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zbus::zvariant::OwnedValue;

#[derive(Clone, Debug)]
struct Call {
    app: String,
    replaces: u32,
    icon: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    timeout: i32,
    id: u32,
}

struct Server {
    calls: Arc<Mutex<Vec<Call>>>,
    next: AtomicU32,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    #[allow(clippy::too_many_arguments)]
    fn notify(&self, app: String, replaces: u32, icon: String, summary: String, body: String, actions: Vec<String>, _hints: HashMap<String, OwnedValue>, timeout: i32) -> u32 {
        let id = if replaces != 0 { replaces } else { self.next.fetch_add(1, Ordering::SeqCst) };
        self.calls.lock().unwrap().push(Call { app, replaces, icon, summary, body, actions, timeout, id });
        id
    }
    fn close_notification(&self, _id: u32) {}
    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".into(), "actions".into()]
    }
    fn get_server_information(&self) -> (String, String, String, String) {
        ("stand-in".into(), "test".into(), "1".into(), "1.2".into())
    }
}

fn private_bus() -> bool {
    let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    // SAFETY: getuid cannot fail
    let desktop = format!("/run/user/{}/bus", unsafe { libc::getuid() });
    std::env::var("CRASHWATCH_LIVE_DBUS").as_deref() == Ok("1") && !addr.is_empty() && !addr.contains(&desktop)
}

fn wait_for(calls: &Arc<Mutex<Vec<Call>>>, n: usize) -> Vec<Call> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let v = calls.lock().unwrap().clone();
        if v.len() >= n || Instant::now() > deadline {
            return v;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn exits(c: &mut Child, within: Duration) -> Option<i32> {
    let deadline = Instant::now() + within;
    loop {
        if let Ok(Some(s)) = c.try_wait() {
            return s.code();
        }
        if Instant::now() > deadline {
            let _ = c.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn notice(integrations: &Path, home: &Path, exe: &str, signal: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_crashwatch-notice"))
        .args([exe, signal])
        .env("CRASHWATCH_INTEGRATIONS", integrations)
        .env("HOME", home)
        .stdin(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
#[ignore = "needs a private session bus: dbus-run-session -- env CRASHWATCH_LIVE_DBUS=1 cargo test --test live_dbus -- --ignored"]
fn notices_on_a_private_bus() {
    if !private_bus() {
        eprintln!("not on a private bus (or CRASHWATCH_LIVE_DBUS is not 1): skipped, so nothing shows on the desktop");
        return;
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let conn = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name("org.freedesktop.Notifications"))
        .and_then(|b| b.serve_at("/org/freedesktop/Notifications", Server { calls: calls.clone(), next: AtomicU32::new(1) }))
        .and_then(|b| b.build())
        .expect("serve the stand-in notification server");
    let emit = |member: &str, body: &(u32, &str)| {
        conn.emit_signal(None::<&str>, "/org/freedesktop/Notifications", "org.freedesktop.Notifications", member, body).unwrap()
    };
    let closed = |id: u32| {
        conn.emit_signal(None::<&str>, "/org/freedesktop/Notifications", "org.freedesktop.Notifications", "NotificationClosed", &(id, 2u32)).unwrap()
    };

    let d = std::env::temp_dir().join(format!("crashwatch-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let (with, without, home): (PathBuf, PathBuf, PathBuf) = (d.join("with"), d.join("without"), d.join("home"));
    for p in [&with, &without, &home] {
        std::fs::create_dir_all(p).unwrap();
    }
    let asked = d.join("asked.txt");
    let ask = d.join("ask");
    script(&ask, &format!("#!/bin/sh\n{{ pwd; for a in \"$@\"; do echo \"$a\"; done; }} > '{}'\necho 'It ran out of memory.'\necho 'Try a smaller texture setting.'\n", asked.display()));
    std::fs::write(
        with.join("sarina.conf"),
        format!("kind = assistant\nname = Sarina\nask = /usr/bin/true\nquestion = {} --voice\ncrash_question = The program {{program}} just crashed ({{signal}}). Look into it.\n", ask.display()),
    )
    .unwrap();

    // 1. no assistant: the program and the signal, no action, and it is done at once
    let mut c = notice(&without, &home, "/usr/bin/game", "SIGSEGV");
    let v = wait_for(&calls, 1);
    assert_eq!(exits(&mut c, Duration::from_secs(5)), Some(0), "nothing to wait for without an action");
    assert_eq!(v.len(), 1);
    assert_eq!((v[0].app.as_str(), v[0].summary.as_str(), v[0].body.as_str()), ("Crash watch", "game crashed", "/usr/bin/game (SIGSEGV)."));
    assert!(v[0].actions.is_empty(), "{:?}", v[0].actions);
    assert_eq!((v[0].icon.as_str(), v[0].timeout), ("dialog-warning", 30_000));

    // 2. with the assistant: the action; closing the notice ends it without asking
    let mut c = notice(&with, &home, "/usr/bin/game", "SIGSEGV");
    let v = wait_for(&calls, 2);
    let first = v[1].clone();
    assert_eq!((first.app.as_str(), first.body.as_str()), ("Sarina", "/usr/bin/game (SIGSEGV). Want Sarina to take a look?"));
    assert_eq!(first.actions, vec!["ask".to_string(), "Ask Sarina".to_string()]);
    closed(first.id);
    assert_eq!(exits(&mut c, Duration::from_secs(5)), Some(0), "a closed notice ends it");
    assert!(!asked.exists(), "nobody clicked Ask");

    // 3. a click on another notice is not ours; then "Ask Sarina" on this one
    let mut c = notice(&with, &home, "/opt/Game/bin/game.x86_64", "SIGABRT");
    let v = wait_for(&calls, 3);
    let id = v[2].id;
    emit("ActionInvoked", &(id + 100, "ask"));
    emit("ActionInvoked", &(id, "default"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(calls.lock().unwrap().len(), 3, "not asked yet");
    emit("ActionInvoked", &(id, "ask"));
    let v = wait_for(&calls, 5);
    assert_eq!(exits(&mut c, Duration::from_secs(10)), Some(0));
    assert_eq!(v.len(), 5, "{v:#?}");
    let (looking, answer) = (&v[3], &v[4]);
    assert_eq!(looking.summary, "Sarina is looking at the game.x86_64 crash");
    assert_eq!(looking.body, "Sarina's answer shows here in a moment.");
    assert_eq!((looking.icon.as_str(), looking.replaces), ("dialog-information", 0));
    assert_eq!(answer.replaces, looking.id, "the answer takes the place of the looking notice");
    assert_eq!(answer.summary, "About the game.x86_64 crash");
    assert_eq!(answer.body, "It ran out of memory.\nTry a smaller texture setting.");
    assert_eq!((answer.app.as_str(), answer.timeout), ("Sarina", 0));
    let got = std::fs::read_to_string(&asked).unwrap();
    let got: Vec<&str> = got.lines().collect();
    assert_eq!(got, vec![home.to_str().unwrap(), "--voice", "The program game.x86_64 just crashed (SIGABRT). Look into it."], "asked in the home folder");

    // 4. the whole way: a core appears, the watcher starts the notice
    let cores = d.join("coredump");
    std::fs::create_dir_all(&cores).unwrap();
    let mut w = Command::new(env!("CARGO_BIN_EXE_yggdrasil-crashwatch"))
        .args(["--dir", cores.to_str().unwrap()])
        .env("CRASHWATCH_NOTICE", env!("CARGO_BIN_EXE_crashwatch-notice"))
        .env("CRASHWATCH_INTEGRATIONS", &without)
        .env("HOME", &home)
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    // SAFETY: getuid cannot fail
    let me = unsafe { libc::getuid() };
    let f = cores.join(format!("core.crashy.{me}.86b6f1565fe940188f8f77e469c8a952.5.1794175822000000.zst"));
    let temp = cores.join(".#temp");
    std::fs::write(&temp, b"x").unwrap();
    let set = |k: &str, v: &str| {
        let p = CString::new(temp.as_os_str().as_bytes()).unwrap();
        let n = CString::new(k).unwrap();
        // SAFETY: plain buffers of the given lengths
        unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), v.as_ptr().cast(), v.len(), 0) == 0 }
    };
    let attrs = set("user.coredump.exe", "/tmp/sarina-evals/crashy/crashy") && set("user.coredump.signal", "11");
    std::fs::rename(&temp, &f).unwrap();
    let v = wait_for(&calls, 6);
    let _ = w.kill();
    let _ = w.wait();
    assert_eq!(v.len(), 6, "the watcher's notice reached the server");
    if attrs {
        assert_eq!(v[5].body, "/tmp/sarina-evals/crashy/crashy (SIGSEGV).");
    }
    assert_eq!(v[5].summary, "crashy crashed");
    let _ = std::fs::remove_dir_all(&d);
}
