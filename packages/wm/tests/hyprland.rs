//! The Hyprland backend against a stand-in Hyprland socket that answers with recorded answers and writes down every request:
//! each tool asks and does exactly what it did through hyprctl before (the same requests, in the same order).
//!
//! The answers (tests/fixtures/hyprland-0.57.1) are as `hyprctl -j` printed them on Hyprland 0.57.1 (2026-11-12, read-only
//! queries): every key, its order and Hyprland's formatting as recorded, the values (titles, classes, addresses, process ids,
//! screens) made up. Two screens: DP-1 (2560x1440, scale 1, focused, the bar and dock reserving 28 and 70) and HDMI-A-1
//! (1920x1080 at 1.25, to its right); four windows, one on the hidden "minimized" workspace. `monitors-minimized-open.json` has that
//! workspace open over DP-1; `getoption-hidden.json` is the pointer timeout yggdrasil-idle sets while the screen is dimmed;
//! `binds.json`, `layers.json` (the panel on DP-1) and `configerrors.txt` are the shortcut list, the panels and two config mistakes.
//! `devices.json` (Settings > Peripherals) is a mouse in three parts (a pointer twice and a keyboard for its extra buttons), a
//! touchpad and a keyboard, with made-up names.
//! `wire.txt` is what hyprctl writes to the socket for each command line, recorded against a stand-in socket in a sandbox.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use yggdrasil_wm::{Desk, Desktops, MonitorRef};

fn fixture(name: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hyprland-0.57.1").join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// A stand-in Hyprland: a runtime folder of its own with hypr/SIG/.socket.sock, answering `j/...` from the fixtures (or as
/// told), "ok" to anything else, and keeping every request.
struct FakeHyprland {
    run: PathBuf,
    asked: Arc<Mutex<Vec<String>>>,
}

/// After a window was asked to close, the stand-in lists no windows (every program closed at once).
const CLOSE_EMPTIES: &str = "close-empties";

impl FakeHyprland {
    fn start(name: &str, answers: &[(&str, &str)]) -> FakeHyprland {
        let run = std::env::temp_dir().join(format!("ygg-wm-hypr-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&run);
        std::fs::create_dir_all(run.join("hypr/testsig")).unwrap();
        let listener = UnixListener::bind(run.join("hypr/testsig/.socket.sock")).unwrap();
        let mut table: HashMap<String, String> = HashMap::new();
        for (q, file) in [
            ("j/clients", "clients.json"),
            ("j/monitors", "monitors.json"),
            ("j/workspaces", "workspaces.json"),
            ("j/activeworkspace", "activeworkspace.json"),
            ("j/activewindow", "activewindow.json"),
            ("j/cursorpos", "cursorpos.json"),
            ("j/getoption cursor:inactive_timeout", "getoption.json"),
            ("j/monitors all", "monitors.json"),
            ("j/binds", "binds.json"),
            ("j/layers", "layers.json"),
            ("/configerrors", "configerrors.txt"),
            ("j/devices", "devices.json"),
        ] {
            table.insert(q.into(), fixture(file));
        }
        let empties = answers.iter().any(|(q, _)| *q == CLOSE_EMPTIES);
        for (q, file) in answers.iter().filter(|(q, _)| *q != CLOSE_EMPTIES) {
            table.insert(q.to_string(), fixture(file));
        }
        let asked = Arc::new(Mutex::new(vec![]));
        let (log, runtime) = (asked.clone(), run.clone());
        std::thread::spawn(move || {
            for c in listener.incoming() {
                let Ok(mut c) = c else { break };
                let _ = c.set_read_timeout(Some(Duration::from_millis(100)));
                let mut req = vec![];
                let mut b = [0u8; 4096];
                // a request is one write; read until the asker waits for the answer
                while let Ok(n) = c.read(&mut b) {
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&b[..n]);
                }
                let req = String::from_utf8_lossy(&req).into_owned();
                // the zones' Super question: Hyprland's Lua writes its answer into the runtime folder
                if req.starts_with("/eval local f = io.open(") {
                    let _ = std::fs::write(runtime.join("yggdrasil-zones-super"), "true");
                }
                if empties && req.contains("hl.dsp.window.close(") {
                    table.insert("j/clients".into(), "[]".into());
                }
                let answer = table.get(&req).cloned().unwrap_or_else(|| "ok".into());
                log.lock().unwrap().push(req);
                let _ = c.write_all(answer.as_bytes());
            }
        });
        FakeHyprland { run, asked }
    }

    fn env(&self) -> impl Fn(&str) -> Option<std::ffi::OsString> + '_ {
        move |k| match k {
            "XDG_RUNTIME_DIR" => Some(self.run.clone().into_os_string()),
            "HYPRLAND_INSTANCE_SIGNATURE" => Some("testsig".into()),
            _ => None,
        }
    }

    fn desk(&self) -> Desk {
        let d = Desk::from_env(self.env()).expect("the stand-in Hyprland is found");
        assert!(d.is_hyprland());
        d
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.asked.lock().unwrap())
    }

    /// A program of this package run against the stand-in, with a config folder of its own.
    fn run_tool(&self, name: &str, arg: Option<&str>, config: &Path) -> i32 {
        let exe = match name {
            "yggdrasil-snap" => env!("CARGO_BIN_EXE_yggdrasil-snap"),
            "yggdrasil-logout" => env!("CARGO_BIN_EXE_yggdrasil-logout"),
            _ => env!("CARGO_BIN_EXE_yggdrasil-close"),
        };
        let mut c = Command::new(exe);
        c.env_clear()
            .env("XDG_RUNTIME_DIR", &self.run)
            .env("HYPRLAND_INSTANCE_SIGNATURE", "testsig")
            .env("XDG_CONFIG_HOME", config)
            .env("HOME", config)
            // only the stand-ins of the system's programs (logout's): never the real loginctl or systemctl
            .env("PATH", self.run.join("bin"));
        if let Some(a) = arg {
            c.arg(a);
        }
        c.status().unwrap().code().unwrap_or(-1)
    }
}

impl Drop for FakeHyprland {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.run);
    }
}

fn dispatches(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_requests_are_the_bytes_hyprctl_sends() {
    let h = FakeHyprland::start("wire", &[]);
    let Desk::Hyprland(hy) = h.desk() else { unreachable!() };
    for line in fixture("wire.txt").lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let (cmd, bytes) = line.split_once('\t').unwrap();
        let words: Vec<&str> = cmd.splitn(2, ' ').collect();
        match words.as_slice() {
            ["-j", what] => drop(hy.query(what)),
            [what, "-j"] => drop(hy.query(what)),
            ["dispatch", lua] => drop(hy.dispatch(lua)),
            ["eval", lua] => drop(hy.eval(lua)),
            _ => drop(hy.command(cmd)),
        }
        assert_eq!(h.take(), [bytes.to_string()], "{cmd}");
    }
}

#[test]
fn the_panels_desktops() {
    let h = FakeHyprland::start("desktops", &[]);
    let d = h.desk();
    assert_eq!(d.desktops(), Desktops { ids: vec![2, 3], at: 0 }, "the hidden workspace is not a desktop");
    assert_eq!(h.take(), ["j/workspaces", "j/activeworkspace"]);
    d.show_desktop("3");
    d.show_desktop("special:minimized");
    assert_eq!(h.take(), dispatches(&[r#"/dispatch hl.dsp.focus({ workspace = "3" })"#]), "only numbered desktops are shown");
    d.send_to_desktop("0x5a0000004000", "2");
    d.send_to_desktop("0x5a\" })", "2");
    assert_eq!(
        h.take(),
        dispatches(&[r#"/dispatch hl.dsp.window.move({ workspace = "2", follow = false, window = "address:0x5a0000004000" })"#]),
        "an address is hex digits or nothing"
    );
}

#[test]
fn the_docks_minimize_and_restore() {
    let h = FakeHyprland::start("dock", &[]);
    let d = h.desk();
    assert!(d.minimize_app("org.example.Terminal", "Terminal 1"));
    assert_eq!(
        h.take(),
        dispatches(&[
            "j/clients",
            r#"/dispatch hl.dsp.window.move({ workspace = "special:minimized", follow = false, window = "address:0x5a0000003000" })"#
        ])
    );
    // a window in front: focused and raised
    assert!(d.restore_app("org.example.Editor", "notes.txt - Editor"));
    assert_eq!(
        h.take(),
        dispatches(&[
            "j/clients",
            r#"/dispatch hl.dsp.focus({ window = "address:0x5a0000001000" })"#,
            "/dispatch hl.dsp.window.bring_to_top()"
        ])
    );
    // a minimized one: back to the focused screen's workspace (2)
    assert!(d.restore_app("org.example.Player", "A Song - Player"));
    assert_eq!(
        h.take(),
        dispatches(&[
            "j/clients",
            "j/monitors",
            r#"/dispatch hl.dsp.window.move({ workspace = "2", follow = false, window = "address:0x5a0000002000" })"#,
            r#"/dispatch hl.dsp.focus({ window = "address:0x5a0000002000" })"#,
            "/dispatch hl.dsp.window.bring_to_top()"
        ])
    );
    // two of a class and a title that matches neither: not found, nothing done (the dock then asks the usual way)
    assert!(!d.restore_app("org.example.Terminal", "Terminal 9"));
    assert_eq!(h.take(), ["j/clients"]);
    // the only window of its class is found whatever its title now is
    assert!(d.minimize_app("org.example.Editor", "renamed"));
    assert_eq!(h.take().len(), 2);
}

#[test]
fn a_minimized_window_comes_back_with_the_hidden_workspace_closed() {
    let h = FakeHyprland::start("dock-open", &[("j/monitors", "monitors-minimized-open.json")]);
    assert!(h.desk().restore_app("org.example.Player", "A Song - Player"));
    assert_eq!(
        h.take(),
        dispatches(&[
            "j/clients",
            "j/monitors",
            r#"/dispatch hl.dsp.workspace.toggle_special("minimized")"#,
            r#"/dispatch hl.dsp.window.move({ workspace = "2", follow = false, window = "address:0x5a0000002000" })"#,
            r#"/dispatch hl.dsp.focus({ window = "address:0x5a0000002000" })"#,
            "/dispatch hl.dsp.window.bring_to_top()"
        ])
    );
}

#[test]
fn window_zones() {
    let h = FakeHyprland::start("zones", &[]);
    let d = h.desk();
    let w = d.active_window(true).unwrap();
    assert_eq!((w.id.as_str(), w.at, w.floating, w.fullscreen), ("0x5a0000001000", Some((40.0, 128.0)), Some(true), false));
    assert_eq!(w.monitor, Some(MonitorRef::Id(0)));
    assert_eq!(d.cursor(), Some((1270.0, 14.0)));
    let screens = d.screens();
    assert_eq!(screens.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["DP-1", "HDMI-A-1"]);
    assert!(screens[0].is(&MonitorRef::Id(0)) && screens[0].focused && !screens[1].focused);
    assert_eq!(screens[0].monitor().free_area(), (0, 28, 2560, 1342));
    assert_eq!(screens[1].monitor().logical(), (2560, 0, 1536, 864));
    assert_eq!(d.screens_quick(), screens, "the same, asked while dragging");
    assert_eq!(h.take(), ["j/activewindow", "j/cursorpos", "j/monitors", "j/monitors"]);
    // a tiled window is floated, then sized, then placed
    let answers = d.place("0x5a0000003000", false, (0, 28, 1280, 671));
    assert_eq!(answers.len(), 3);
    assert!(answers.iter().all(|(_, a)| a.as_deref() == Some("ok")));
    assert_eq!(
        h.take(),
        dispatches(&[
            r#"/dispatch hl.dsp.window.float({ action = "enable", window = "address:0x5a0000003000" })"#,
            r#"/dispatch hl.dsp.window.resize({ x = 1280, y = 671, window = "address:0x5a0000003000" })"#,
            r#"/dispatch hl.dsp.window.move({ x = 0, y = 28, window = "address:0x5a0000003000" })"#
        ])
    );
    // Super held: asked through Hyprland's Lua, answered in a file in the runtime folder
    assert_eq!(d.super_held(), Some(true));
    let out = h.run.join("yggdrasil-zones-super");
    assert_eq!(
        h.take(),
        [format!(
            "/eval local f = io.open('{}', 'w') if f then f:write(tostring(hl.is_key_down('Super_L') or hl.is_key_down('Super_R'))) f:close() end",
            out.display()
        )]
    );
}

#[test]
fn an_unreadable_window_list_is_not_an_empty_one() {
    let h = FakeHyprland::start("unreadable", &[("j/clients", "configerrors.txt")]);
    assert_eq!(h.desk().try_windows(), None, "browser-warm then restarts nothing");
    assert!(h.desk().windows().is_empty());
    let h = FakeHyprland::start("readable", &[]);
    assert_eq!(h.desk().try_windows().map(|w| w.len()), Some(4));
}

#[test]
fn no_window_in_front() {
    let h = FakeHyprland::start("none", &[("j/activewindow", "activewindow-none.json")]);
    assert_eq!(h.desk().active_window(false), None);
}

#[test]
fn the_window_list_reads_hyprlands_fields() {
    let h = FakeHyprland::start("list", &[]);
    let w = h.desk().windows();
    assert_eq!(w.len(), 4);
    assert!(w[1].minimized && w[1].special && w[1].workspace == Some(-98));
    assert_eq!((w[2].floating, w[2].focus_order, w[2].pid), (Some(false), 1, Some(4103)));
    assert_eq!(w[3].monitor, Some(MonitorRef::Id(1)));
}

#[test]
fn idle_hides_the_pointer_and_puts_it_back() {
    let h = FakeHyprland::start("idle", &[]);
    let d = h.desk();
    assert_eq!(d.pointer_timeout(), Some(0.0));
    d.set_pointer_timeout(0.25);
    d.set_pointer_timeout(0.0);
    assert_eq!(
        h.take(),
        dispatches(&[
            "j/getoption cursor:inactive_timeout",
            "/eval hl.config({ cursor = { inactive_timeout = 0.25 } })",
            "/eval hl.config({ cursor = { inactive_timeout = 0 } })"
        ])
    );
    let h = FakeHyprland::start("idle-hidden", &[("j/getoption cursor:inactive_timeout", "getoption-hidden.json")]);
    assert_eq!(h.desk().pointer_timeout(), Some(0.25), "an earlier run left it hidden");
}

/// yggdrasil-snap as the shell script did it (its Python worked out the free room of the focused screen).
#[test]
fn snap_on_hyprland() {
    let h = FakeHyprland::start("snap", &[]);
    let conf = h.run.join("conf");
    std::fs::create_dir_all(conf.join("yggdrasil")).unwrap();
    for (side, want) in [
        (
            "left",
            vec!["j/monitors", "/dispatch hl.dsp.window.resize({ x = 1280, y = 1342 })", "/dispatch hl.dsp.window.move({ x = 0, y = 28 })"],
        ),
        (
            "right",
            vec![
                "j/monitors",
                "/dispatch hl.dsp.window.resize({ x = 1280, y = 1342 })",
                "/dispatch hl.dsp.window.move({ x = 1280, y = 28 })",
            ],
        ),
        (
            "top",
            vec!["j/monitors", "/dispatch hl.dsp.window.resize({ x = 2560, y = 671 })", "/dispatch hl.dsp.window.move({ x = 0, y = 28 })"],
        ),
        (
            "bottom",
            vec!["j/monitors", "/dispatch hl.dsp.window.resize({ x = 2560, y = 671 })", "/dispatch hl.dsp.window.move({ x = 0, y = 699 })"],
        ),
        ("up", vec!["j/monitors", r#"/dispatch hl.dsp.window.fullscreen({ mode = "maximized" })"#]),
        ("down", vec!["j/monitors", r#"/dispatch hl.dsp.window.move({ workspace = "special:minimized", follow = false })"#]),
    ] {
        assert_eq!(h.run_tool("yggdrasil-snap", Some(side), &conf), 0, "{side}");
        assert_eq!(h.take(), dispatches(&want), "{side}");
    }
    // a tiling screen: the arrows move the focus instead; top and bottom still snap
    std::fs::write(conf.join("yggdrasil/tiling"), "# tiling\nHDMI-A-1, DP-1\n").unwrap();
    assert_eq!(h.run_tool("yggdrasil-snap", Some("left"), &conf), 0);
    assert_eq!(h.take(), dispatches(&["j/monitors", r#"/dispatch hl.dsp.focus({ direction = "l" })"#]));
    assert_eq!(h.run_tool("yggdrasil-snap", Some("down"), &conf), 0);
    assert_eq!(h.take(), dispatches(&["j/monitors", r#"/dispatch hl.dsp.focus({ direction = "d" })"#]));
    assert_eq!(h.run_tool("yggdrasil-snap", Some("top"), &conf), 0);
    assert_eq!(h.take().len(), 3);
    assert_eq!(h.run_tool("yggdrasil-snap", Some("sideways"), &conf), 2);
    assert!(h.take().is_empty());
}

#[test]
fn close_on_hyprland() {
    let h = FakeHyprland::start("close", &[]);
    let conf = h.run.join("conf");
    std::fs::create_dir_all(conf.join("yggdrasil")).unwrap();
    // the editor is in front and does not keep running
    assert_eq!(h.run_tool("yggdrasil-close", None, &conf), 0);
    assert_eq!(h.take(), dispatches(&["j/activewindow", "/dispatch hl.dsp.window.close()"]));
    // listed (in another case): hidden instead
    std::fs::write(conf.join("yggdrasil/keep-running"), "# keep\nORG.EXAMPLE.EDITOR\n").unwrap();
    assert_eq!(h.run_tool("yggdrasil-close", None, &conf), 0);
    assert_eq!(
        h.take(),
        dispatches(&["j/activewindow", r#"/dispatch hl.dsp.window.move({ workspace = "special:minimized", follow = false })"#])
    );
}

#[test]
fn settings_peripherals_on_hyprland() {
    use yggdrasil_wm::input::{self, Accel, DeviceKind, InputConf, Pointer};
    let h = FakeHyprland::start("input", &[]);
    let d = h.desk();
    let devices = d.input_devices().unwrap();
    assert_eq!(h.take(), ["j/devices"]);
    assert_eq!(devices.iter().filter(|x| x.kind == DeviceKind::Keyboard).count(), 2);
    assert_eq!(input::mice(&devices), ["example-gaming-mouse"], "one mouse, its parts together; the touchpad has its own settings");

    // every mouse: Hyprland's Lua at once, everything not set back to its default
    let before = InputConf::default();
    let mut now = InputConf { repeat_delay: Some(300), ..Default::default() };
    now.all = Pointer { sensitivity: Some(-0.5), accel: Some(Accel::Flat), ..Default::default() };
    assert_eq!(d.apply_input(&before, &now), Ok(()));
    assert_eq!(
        h.take(),
        [
            r#"/eval hl.config({ input = { sensitivity = -0.5, accel_profile = "flat", scroll_factor = 1, natural_scroll = false, left_handed = false, repeat_rate = 25, repeat_delay = 300 } })"#
        ]
    );
    // one mouse's own settings: Hyprland reads its config again (the session reads input.conf), so a rule taken away goes too
    let mut one = now.clone();
    one.devices.insert("example-gaming-mouse".into(), Pointer { left_handed: Some(true), ..Default::default() });
    assert_eq!(d.apply_input(&now, &one), Ok(()));
    assert_eq!(h.take(), ["/reload"]);
    assert_eq!(d.apply_input(&one, &now), Ok(()));
    assert_eq!(h.take(), ["/reload"]);
}

#[test]
fn without_hyprland_the_tools_do_nothing() {
    let run = std::env::temp_dir().join(format!("ygg-wm-nohypr-{}", std::process::id()));
    std::fs::create_dir_all(&run).unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_yggdrasil-snap"))
        .arg("left")
        .env_clear()
        .env("XDG_RUNTIME_DIR", &run)
        .env("HYPRLAND_INSTANCE_SIGNATURE", "gone")
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(1));
    let _ = std::fs::remove_dir_all(&run);
}

#[test]
fn the_task_managers_requests_are_its_own_as_before() {
    let h = FakeHyprland::start("taskmanager", &[]);
    let d = h.desk();
    let w = d.windows();
    assert_eq!(w.iter().map(|w| (w.id.as_str(), w.pid)).collect::<Vec<_>>()[0], ("0x5a0000001000", Some(4101)));
    assert!(d.close_window("0x5a0000001000") && d.focus_window("0x5a0000003000"));
    assert!(!d.close_window("0x12'); os.execute('x"), "nothing but hex reaches the Lua line");
    assert_eq!(
        h.take(),
        dispatches(&[
            "j/clients",
            "dispatch hl.dsp.window.close({ window = 'address:0x5a0000001000' })",
            "dispatch hl.dsp.focus({ window = 'address:0x5a0000003000' })"
        ])
    );
}

#[test]
fn paste_settings_and_the_session() {
    let h = FakeHyprland::start("misc", &[]);
    let d = h.desk();
    assert!(d.paste(true) && d.paste(false));
    assert!(d.focus_class("org.example.Settings") && !d.focus_class("x\" }) or (\""));
    assert_eq!(d.reload(), Ok(()));
    assert!(d.set_cursor("Yggdrasil", 24) && !d.set_cursor("../x", 24));
    assert!(d.ask_to_close("0x5a0000002000") && d.exit());
    assert!(d.set_keyboard("us", ""));
    assert_eq!(
        h.take(),
        dispatches(&[
            "/dispatch sendshortcut CTRL SHIFT, V, activewindow",
            "/dispatch sendshortcut CTRL, V, activewindow",
            r#"/dispatch hl.dsp.focus({ window = "class:org.example.Settings" })"#,
            "/reload",
            "/setcursor Yggdrasil 24",
            r#"/dispatch hl.dsp.window.close({ window = "address:0x5a0000002000" })"#,
            "/dispatch hl.dsp.exit()",
            r#"/eval hl.config({ input = { kb_layout = "us", kb_variant = "" } })"#,
        ])
    );
    // the screens with their modes (Settings > Displays), the shortcuts, the config's mistakes, the layers
    let all = d.screens_all();
    assert_eq!((all[0].refresh, all[0].modes.len(), all[1].modes.as_slice()), (Some(59.951), 2, &["1920x1080@60.00Hz".to_string()][..]));
    let binds = d.binds().unwrap();
    assert_eq!(binds.len(), 5);
    assert_eq!((binds[0].modmask, binds[0].key.as_str()), (64, "Q"));
    assert_eq!(binds[2].key, "code:10", "a key given by its code");
    assert!(binds[3].release);
    let errors = d.config_errors().unwrap();
    assert_eq!(errors.lines().count(), 2);
    assert_eq!(d.has_layer("yggdrasil-panel"), Some(true));
    assert_eq!(d.has_layer("yggdrasil-lock"), Some(false));
    assert_eq!(h.take(), ["j/monitors all", "j/binds", "/configerrors", "j/layers", "j/layers"]);
    let h = FakeHyprland::start("nopanel", &[("j/layers", "layers-no-panel.json")]);
    assert_eq!(h.desk().has_layer("yggdrasil-panel"), Some(false));
}

/// Stand-ins for the programs yggdrasil-logout runs (they write down how they were called).
fn stand_ins(h: &FakeHyprland) -> PathBuf {
    let bin = h.run.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = h.run.join("calls");
    for t in ["systemd-cat", "notify-send", "loginctl", "systemctl"] {
        let p = bin.join(t);
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\nprintf '%s' '{t}' >> '{}'\nfor a in \"$@\"; do printf ' [%s]' \"$a\" >> '{}'; done\necho >> '{}'\n",
                log.display(),
                log.display(),
                log.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    log
}

/// yggdrasil-logout as the shell script did it: every window asked to close, then the session ends (or, with a window left,
/// the screen locks and a notification names it).
#[test]
fn logout_on_hyprland() {
    let h = FakeHyprland::start("logout", &[(CLOSE_EMPTIES, "clients.json")]);
    let calls = stand_ins(&h);
    let conf = h.run.join("conf");
    assert_eq!(h.run_tool("yggdrasil-logout", Some("reboot"), &conf), 0);
    let mut want = vec!["j/clients".to_string()];
    for a in ["0x5a0000001000", "0x5a0000002000", "0x5a0000003000", "0x5a0000004000"] {
        want.push(format!(r#"/dispatch hl.dsp.window.close({{ window = "address:{a}" }})"#));
    }
    // one look while waiting (none left), one for the names of those left (none)
    want.extend(["j/clients".to_string(), "j/clients".to_string(), "/dispatch hl.dsp.exit()".to_string()]);
    assert_eq!(h.take(), want);
    assert_eq!(
        std::fs::read_to_string(&calls).unwrap(),
        "systemd-cat [-t] [yggdrasil-session] [echo] [reboot: every window closed]\nsystemctl [--user] [stop] [xdg-desktop-portal-hyprland.service]\n"
    );
    assert_eq!(std::fs::read_to_string(h.run.join("yggdrasil-after-session")).unwrap(), "reboot\n");
    assert_eq!(h.run_tool("yggdrasil-logout", Some("sideways"), &conf), 2);
}

#[test]
fn logout_with_a_window_left_locks_instead() {
    let h = FakeHyprland::start("logout-left", &[]);
    let calls = stand_ins(&h);
    let start = std::time::Instant::now();
    assert_eq!(h.run_tool("yggdrasil-logout", None, &h.run.join("conf")), 1);
    assert!(start.elapsed() >= Duration::from_secs(9), "it waits ten seconds for the programs");
    let asked = h.take();
    assert_eq!(asked.iter().filter(|r| *r == "j/clients").count(), 1 + 40 + 1, "a look before, forty while waiting, one for the names");
    assert!(!asked.iter().any(|r| r.contains("exit")));
    let c = std::fs::read_to_string(&calls).unwrap();
    assert!(c.starts_with("systemd-cat [-t] [yggdrasil-session] [echo] [logout stopped: still open: notes.txt - Editor, A Song - Player, Terminal 1 and others]\n"), "{c}");
    assert!(c.contains("notify-send [-a] [Desktop] [-u] [critical] [-i] [dialog-warning] [Still signed in] [notes.txt - Editor, A Song - Player, Terminal 1 and others did not close"));
    assert!(c.ends_with("loginctl [lock-session]\n"));
    assert!(!h.run.join("yggdrasil-after-session").exists());
}
