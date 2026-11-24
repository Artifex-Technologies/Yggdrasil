//! What the tests share: a short folder of their own with a PipeWire of its own (a minimal configuration: no devices,
//! nothing of the desktop's audio) and the headless compositor, and child processes that end with the test (a parent-death
//! signal, even when the test is killed). Nothing of the running desktop is used: every child gets a cleared environment.

#![allow(dead_code)] // each test file uses its own part

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const PW_CONF: &str = "context.properties = { core.daemon = true core.name = pipewire-0 support.dbus = false mem.allow-mlock = false }
context.spa-libs = { support.* = support/libspa-support }
context.modules = [
  { name = libpipewire-module-protocol-native }
  { name = libpipewire-module-access }
  { name = libpipewire-module-client-node }
  { name = libpipewire-module-adapter }
  { name = libpipewire-module-metadata }
  { name = libpipewire-module-link-factory }
]
";

/// CI sets YGGDRASIL_TEST_SCREENCAST_REQUIRED: there a test may not skip for want of PipeWire or a build.
pub fn required() -> bool {
    std::env::var_os("YGGDRASIL_TEST_SCREENCAST_REQUIRED").is_some()
}

/// Skips the test (or fails it, when required) for want of `what`.
pub fn skip(what: &str) {
    assert!(!required(), "{what}");
    eprintln!("skipped: {what}");
}

pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// A build of this repository's, from the variable `var`, else target/debug/`name`.
pub fn build_of(var: &str, name: &str) -> Option<PathBuf> {
    let p = std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug").join(name));
    p.is_file().then_some(p)
}

/// The first line of a child's output that starts with `prefix` (the rest of it), within `secs`.
pub fn line_with(child: &mut Child, prefix: &str, secs: u64) -> Option<String> {
    let out = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    let prefix = prefix.to_string();
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(v) = l.strip_prefix(&prefix) {
                let _ = tx.send(v.to_string());
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

/// Waits up to `secs` for `done`.
pub fn wait_for(secs: u64, mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..secs * 20 {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

pub struct World {
    pub dir: PathBuf,
    children: Vec<Child>,
}

impl Drop for World {
    fn drop(&mut self) {
        // the last started first: what uses the compositor and PipeWire before them
        for c in self.children.iter_mut().rev() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl World {
    /// A world in /tmp/`name`-PID (short: the sockets' paths must fit), or None, the test skipped, when PipeWire or the
    /// compositor's build (YGGDRASIL_COMPOSITOR, else target/debug/yggdrasil-compositor) is not there.
    pub fn new(name: &str) -> Option<World> {
        if build_of("YGGDRASIL_COMPOSITOR", "yggdrasil-compositor").is_none() {
            skip("no compositor build (YGGDRASIL_COMPOSITOR, else target/debug/yggdrasil-compositor)");
            return None;
        }
        if !on_path("pipewire") {
            skip("PipeWire is not installed");
            return None;
        }
        let dir = PathBuf::from(format!("/tmp/{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("conf")).unwrap();
        Some(World { dir, children: vec![] })
    }

    /// A command in this world: a cleared environment with this world's runtime folder, home and PipeWire, ended with the
    /// test.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut c = Command::new(program);
        // SAFETY: only an async-signal-safe prctl between fork and exec
        unsafe {
            c.pre_exec(|| rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL)).map_err(Into::into));
        }
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.dir)
            .env("XDG_RUNTIME_DIR", &self.dir)
            .env("PIPEWIRE_RUNTIME_DIR", &self.dir)
            .stdin(Stdio::null());
        c
    }

    /// Ended with the world (the last kept, the first ended).
    pub fn keep(&mut self, c: Child) {
        self.children.push(c);
    }

    /// The world's PipeWire; its socket's path once it is there.
    pub fn start_pipewire(&mut self) -> PathBuf {
        std::fs::write(self.dir.join("conf/pipewire.conf"), PW_CONF).unwrap();
        let mut c = self.command("pipewire");
        c.env("PIPEWIRE_CONFIG_DIR", self.dir.join("conf")).stdout(Stdio::null()).stderr(Stdio::null());
        let child = c.spawn().expect("pipewire starts");
        self.keep(child);
        let sock = self.dir.join("pipewire-0");
        assert!(wait_for(10, || sock.exists()), "PipeWire's socket");
        sock
    }

    /// The headless compositor with `args` (its screens); its Wayland display and its control socket.
    pub fn start_compositor(&mut self, args: &[&str]) -> (String, PathBuf) {
        let comp = build_of("YGGDRASIL_COMPOSITOR", "yggdrasil-compositor").expect("the compositor's build");
        let mut c = self.command(comp);
        c.arg("--headless").args(args).env("YGGDRASIL_COMPOSITOR_XWAYLAND", "0").stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = c.spawn().expect("the compositor starts");
        let display = line_with(&mut child, "WAYLAND_DISPLAY=", 20).expect("the compositor's display");
        self.keep(child);
        let ctl = self.dir.join(format!("yggdrasil-compositor-{display}.ctl"));
        assert!(wait_for(10, || ctl.exists()), "the compositor's control socket");
        (display, ctl)
    }
}

/// One line to the compositor's control socket; its answer.
pub fn ctl(sock: &Path, line: &str) -> String {
    let mut s = std::os::unix::net::UnixStream::connect(sock).expect("the control socket");
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(format!("{line}\n").as_bytes()).unwrap();
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}
