//! The two programs on a made-up computer: a /sys tree with USB devices, an OS's packages integration and lists, and stand-ins for
//! nmcli, pacman, notify-send (which "clicks" what ANSWER says), the OS's component command, Agora's password box and systemctl.
//! Every call is written to a calls file. No real network, notification, package or service is touched; the no-network case
//! also runs in a network namespace of its own (unshare -rn) when the system allows one.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct World {
    dir: PathBuf,
}

const RECORD: &str = "line=${0##*/}; for a in \"$@\"; do line=\"$line [$a]\"; done; printf '%s\\n' \"$line\" >> \"$WORLD/calls\"\n";

impl World {
    fn new(name: &str) -> World {
        let dir = std::env::temp_dir().join(format!("ygg-device-offers-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let w = World { dir };
        w.write(
            "share/integrations/os-packages.conf",
            &format!(
                "kind = packages\nname = Example\ncomponents = {0}/os/components.json\ncomponent = {0}/bin/os-apps\ndevices = {0}/os/devices.json\nfirst_login = {0}/os/first-login.json\n",
                w.dir.display()
            ),
        );
        w.write(
            "os/devices.json",
            r#"{"extras": [
                {"id": "razer", "name": "Razer", "note": "OpenRazer.", "components": ["razer"], "chosen_if_usb": ["1532"]},
                {"id": "ckb-next", "name": "Corsair keyboards and mice", "components": ["ckb-next"], "chosen_if_usb": ["1b1c"], "usb_kind": "input"},
                {"id": "sarina", "name": "Sarina", "components": ["sarina"], "chosen_if_exists": "/usr/bin/sarina"}]}"#,
        );
        w.write(
            "os/components.json",
            r#"{"components": [{"id": "razer", "packages": ["openrazer-daemon"]}, {"id": "ckb-next", "packages": ["ckb-next"]}, {"id": "sarina", "packages": ["sarina-git"]}]}"#,
        );
        w.tool("nmcli", "echo \"${NM:-full}\"");
        w.tool("pacman", "printf '%s\\n' ${INSTALLED:-base}");
        w.tool("notify-send", "case \"$*\" in *-A*) [ -n \"$NO_SERVER\" ] && exit 1; echo \"$ANSWER\";; esac");
        w.tool("os-apps", "echo \"AGORA_SUDO=$AGORA_SUDO\" >> \"$WORLD/calls\"; exit ${APPS_RC:-0}");
        w.tool("sudo", "");
        // the dispatcher starts systemctl with a clean environment: this one knows its world by itself
        w.write(
            "bin/systemctl",
            &format!("#!/bin/sh\nWORLD='{}'\n{RECORD}echo \"XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR\" >> \"$WORLD/calls\"\n", w.dir.display()),
        );
        std::fs::set_permissions(w.dir.join("bin/systemctl"), std::fs::Permissions::from_mode(0o755)).unwrap();
        for d in ["run", "home", "sys"] {
            std::fs::create_dir_all(w.dir.join(d)).unwrap();
        }
        w
    }

    fn write(&self, rest: &str, text: &str) {
        let f = self.dir.join(rest);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    }

    fn tool(&self, name: &str, body: &str) {
        self.write(&format!("bin/{name}"), &format!("#!/bin/sh\n{RECORD}{body}\n"));
        std::fs::set_permissions(self.dir.join("bin").join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A USB device with a boot keyboard interface or none.
    fn plug(&self, at: &str, vendor: &str, product: &str, keyboard: bool) {
        self.write(&format!("sys/bus/usb/devices/{at}/idVendor"), &format!("{vendor}\n"));
        self.write(&format!("sys/bus/usb/devices/{at}/idProduct"), &format!("{product}\n"));
        let (c, s, p) = if keyboard { ("03", "01", "01") } else { ("03", "00", "00") };
        self.write(&format!("sys/bus/usb/devices/{at}:1.0/bInterfaceClass"), c);
        self.write(&format!("sys/bus/usb/devices/{at}:1.0/bInterfaceSubClass"), s);
        self.write(&format!("sys/bus/usb/devices/{at}:1.0/bInterfaceProtocol"), p);
    }

    fn command(&self, program: &str, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut c = Command::new(program);
        c.args(args)
            .env_clear()
            .env("PATH", self.dir.join("bin"))
            .env("WORLD", &self.dir)
            .env("USER", "ada")
            .env("HOME", self.dir.join("home"))
            .env("XDG_STATE_HOME", self.dir.join("home/state"))
            .env("XDG_RUNTIME_DIR", self.dir.join("run"))
            .env("DEVICE_OFFERS_ROOT", &self.dir)
            .env("DEVICE_OFFERS_INTEGRATIONS", self.dir.join("share/integrations"))
            .env("DEVICE_OFFERS_AGORA_SUDO", self.dir.join("bin/sudo"));
        for (k, v) in env {
            c.env(k, v);
        }
        c
    }

    /// Runs yggdrasil-device-offers: (exit code, standard output).
    fn offers(&self, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
        let o = self.command(env!("CARGO_BIN_EXE_yggdrasil-device-offers"), args, env).output().unwrap();
        (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned())
    }

    fn calls(&self) -> String {
        let c = std::fs::read_to_string(self.dir.join("calls")).unwrap_or_default();
        let _ = std::fs::remove_file(self.dir.join("calls"));
        c
    }

    /// The notify-send calls that asked something.
    fn questions(calls: &str) -> Vec<&str> {
        calls.lines().filter(|l| l.starts_with("notify-send") && l.contains("[-A]")).collect()
    }

    fn read(&self, rest: &str) -> String {
        std::fs::read_to_string(self.dir.join(rest)).unwrap_or_default()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn offline_nothing_is_asked_or_installed() {
    let w = World::new("offline");
    w.plug("1-1", "1532", "0084", false);
    for state in ["none", "limited", "portal", "unknown"] {
        assert_eq!(w.offers(&[], &[("NM", state)]).0, 0);
        let calls = w.calls();
        assert!(!calls.contains("notify-send") && !calls.contains("os-apps") && !calls.contains("pacman"), "{state}: {calls}");
    }
    assert!(w.read("run/yggdrasil/device-offers-asked").is_empty(), "nothing counts as asked");
    // what would be offered is still known
    assert_eq!(w.offers(&["--print"], &[("NM", "none")]).1, "razer\trazer\n");
}

/// The real thing without a network: a network namespace of its own (only the loopback, no routes) and no NetworkManager, so the
/// program falls back to the kernel's routing table. Skipped where user namespaces are not allowed.
#[test]
fn in_a_network_namespace_without_network_nothing_is_asked() {
    let w = World::new("netns");
    w.plug("1-1", "1532", "0084", false);
    std::fs::remove_file(w.dir.join("bin/nmcli")).unwrap();
    if !Command::new("/usr/bin/unshare").args(["-rn", "true"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipped: unshare -rn is not allowed here");
        return;
    }
    let program = env!("CARGO_BIN_EXE_yggdrasil-device-offers");
    // only the stand-ins on PATH: no nmcli, so no NetworkManager to ask
    let mut c = w.command("/usr/bin/unshare", &["-rn", program], &[]);
    let started = std::time::Instant::now();
    let st = c.status().unwrap();
    assert!(st.success());
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "no waiting for a network");
    let calls = w.calls();
    assert!(!calls.contains("notify-send") && !calls.contains("os-apps"), "{calls}");
}

#[test]
fn online_a_razer_mouse_is_offered_and_installed_with_one_click() {
    let w = World::new("install");
    w.plug("1-1", "1532", "0084", false);
    w.plug("1-2", "1b1c", "0c10", false); // a Corsair fan controller: no keyboard
    assert_eq!(w.offers(&[], &[("ANSWER", "install")]).0, 0);
    let calls = w.calls();
    let q = World::questions(&calls);
    assert_eq!(q.len(), 1, "{calls}");
    assert!(q[0].contains("[install=Install] [-A] [never=Don't ask again] [Software for Razer]"), "{}", q[0]);
    assert!(q[0].contains("OpenRazer."), "the list's note");
    assert!(calls.contains("os-apps [install] [razer]\n"), "{calls}");
    assert!(calls.contains(&format!("AGORA_SUDO={}", w.dir.join("bin/sudo").display())), "root steps through the password box");
    assert!(calls.contains("[Installed] [Ready: Razer.]"), "{calls}");
    assert_eq!(w.read("run/yggdrasil/device-offers-asked"), "razer\n");
    // the next trigger in this session asks nothing, whatever happened
    w.offers(&[], &[("ANSWER", "install")]);
    assert!(World::questions(&w.calls()).is_empty());
}

#[test]
fn a_failed_install_says_where_to_look() {
    let w = World::new("fails");
    w.plug("1-1", "1532", "0084", false);
    w.offers(&[], &[("ANSWER", "install"), ("APPS_RC", "1")]);
    let calls = w.calls();
    assert!(
        calls.contains("[Could not install Razer] [Agora can install it too. Details: ~/.local/state/yggdrasil/device-offers.log]"),
        "{calls}"
    );
}

#[test]
fn several_devices_one_question() {
    let w = World::new("several");
    w.plug("1-1", "1532", "0084", false);
    w.plug("1-2", "1b1c", "1b7d", true);
    w.offers(&[], &[("ANSWER", "install")]);
    let calls = w.calls();
    let q = World::questions(&calls);
    assert_eq!(q.len(), 1, "{calls}");
    assert!(q[0].contains("[Software for your devices] [Found on this computer: Razer, Corsair keyboards and mice."), "{}", q[0]);
    assert!(calls.contains("os-apps [install] [razer] [ckb-next]\n"), "{calls}");
}

#[test]
fn closed_means_not_this_session_and_never_means_never() {
    let w = World::new("answers");
    w.plug("1-1", "1532", "0084", false);
    w.offers(&[], &[("ANSWER", "")]);
    let calls = w.calls();
    assert_eq!(World::questions(&calls).len(), 1);
    assert!(!calls.contains("os-apps"), "closing installs nothing");
    w.offers(&[], &[]);
    assert!(World::questions(&w.calls()).is_empty(), "not again in this session");
    // a new session (the runtime folder goes at sign-out): asked again; this time "Don't ask again"
    std::fs::remove_dir_all(w.dir.join("run/yggdrasil")).unwrap();
    w.offers(&[], &[("ANSWER", "never")]);
    assert_eq!(World::questions(&w.calls()).len(), 1);
    assert_eq!(w.read("home/state/yggdrasil/device-offers-never"), "razer\n");
    std::fs::remove_dir_all(w.dir.join("run/yggdrasil")).unwrap();
    w.offers(&[], &[("ANSWER", "install")]);
    assert!(World::questions(&w.calls()).is_empty(), "never again, in any session");
}

#[test]
fn installed_or_left_to_the_first_login_is_not_offered() {
    let w = World::new("left");
    w.plug("1-1", "1532", "0084", false);
    w.offers(&[], &[("INSTALLED", "base openrazer-daemon")]);
    assert!(World::questions(&w.calls()).is_empty(), "installed already");
    w.write("os/first-login.json", r#"{"user": "ada", "components": ["razer"]}"#);
    w.offers(&[], &[]);
    assert!(World::questions(&w.calls()).is_empty(), "the OS installs it at the first login");
    w.write("os/first-login.json", r#"{"user": "bob", "components": ["razer"]}"#);
    w.offers(&[], &[("ANSWER", "")]);
    assert_eq!(World::questions(&w.calls()).len(), 1, "someone else's list");
}

#[test]
fn without_a_device_list_or_devices_nothing_happens() {
    let w = World::new("nothing");
    assert_eq!(w.offers(&[], &[]).0, 0);
    assert!(!w.calls().contains("notify-send"), "nothing plugged in");
    w.plug("1-1", "1532", "0084", false);
    w.write("share/integrations/os-packages.conf", "kind = packages\nname = Example\n");
    assert_eq!(w.offers(&[], &[]).0, 0);
    assert!(w.calls().is_empty(), "an OS without a device list: not even a look");
    assert_eq!(w.offers(&["--bogus"], &[]).0, 2);
}

#[test]
fn coming_online_starts_the_users_online_target() {
    // SAFETY: getuid has no preconditions
    let me = unsafe { libc::getuid() };
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped as root: the test settings are not read then");
        return;
    }
    let w = World::new("dispatch");
    std::fs::create_dir_all(w.dir.join(format!("run/{me}/systemd"))).unwrap();
    std::fs::write(w.dir.join(format!("run/{me}/systemd/private")), "").unwrap();
    let run = |args: &[&str], env: &[(&str, &str)]| {
        let mut c = w.command(env!("CARGO_BIN_EXE_device-offers-online"), args, env);
        c.env("DEVICE_OFFERS_RUN_USER", w.dir.join("run"))
            .env("DEVICE_OFFERS_SYSTEMCTL", w.dir.join("bin/systemctl"))
            .env("DEVICE_OFFERS_MIN_UID", "0");
        assert!(c.status().unwrap().success());
        w.calls()
    };
    for (args, env) in [
        (vec!["wlan0", "down"], vec![]),
        (vec!["wlan0", "dhcp4-change"], vec![]),
        (vec!["", "connectivity-change"], vec![("CONNECTIVITY_STATE", "LIMITED")]),
        (vec![], vec![]),
    ] {
        assert_eq!(run(&args, &env), "", "{args:?} {env:?}");
    }
    let want = format!(
        "systemctl [--user] [start] [--no-block] [yggdrasil-online.target]\nXDG_RUNTIME_DIR={}\n",
        w.dir.join(format!("run/{me}")).display()
    );
    assert_eq!(run(&["wlan0", "up"], &[]), want);
    assert_eq!(run(&["", "connectivity-change"], &[("CONNECTIVITY_STATE", "FULL")]), want);
}

/// The units and the udev rule as the package installs them: systemd and udev accept them (when their checkers are here).
#[test]
fn the_units_and_the_rule_are_valid() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
    if Command::new("udevadm").arg("--version").output().is_ok() {
        let o = Command::new("udevadm").arg("verify").arg(data.join("71-yggdrasil-device-offers.rules")).output().unwrap();
        assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    }
    let target = std::fs::read_to_string(data.join("yggdrasil-online.target")).unwrap();
    assert!(target.contains("StopWhenUnneeded=yes"), "it stops at once, so the next start starts what it wants again");
    let service = std::fs::read_to_string(data.join("yggdrasil-device-offers.service")).unwrap();
    assert!(service.contains("ExecStart=/usr/lib/yggdrasil/device-offers\n"));
    assert!(service.contains("ConditionEnvironment=WAYLAND_DISPLAY"), "only in a desktop session");
    let rule = std::fs::read_to_string(data.join("71-yggdrasil-device-offers.rules")).unwrap();
    assert!(rule.contains("ENV{SYSTEMD_USER_WANTS}+=\"yggdrasil-device-offers.service\""));
}
