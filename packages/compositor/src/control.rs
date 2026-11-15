//! The control socket, on every backend but the nested window: `$XDG_RUNTIME_DIR/yggdrasil-compositor-wayland-N.ctl` (mode
//! 0600, printed as YGGDRASIL_COMPOSITOR_CTL=...), one command per connection, answered with lines and then "ok" or "error: ...".
//! `yggdrasil-compositor ctl COMMAND...` sends one.
//!
//! What every backend answers (here): the desktop commands Yggdrasil's own tools use through yggdrasil-wm (wmctl.rs), and
//!   outputs                      one line per output: NAME WxH scale=S at=X,Y logical=WxH, then " power=off" for one that is
//!                                off (wlr-output-power) and " disabled" for one switched off (Settings, wlr-output-management)
//!   scale NAME S                 change an output's scale
//!   power NAME on|off            switch an output on or off (as wlr-output-power does)
//!   pointer-look, windows, focus, free, workspace, lock   what is shown and who has the keyboard (headless.rs lists them)
//!   watch                        the connection stays open and gets "desktops" whenever the desktops change (the panel)
//!   quit                         end the compositor
//! The headless backend adds its test commands (sizes, screens plugged in, screenshots, input made inside the compositor, the
//! clock: headless.rs). The hardware backend makes no input from here: only the devices give input there.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::state::{self, App};

/// A backend's own commands: None when it has no such command (the common ones are tried next).
pub type Extra<'a> = &'a mut dyn FnMut(&[&str], &mut App) -> Option<Result<Vec<String>, String>>;

/// The connections that `watch` the desktops (the panel), and the desktops they were last told of.
#[derive(Default)]
pub struct Control {
    watchers: Vec<UnixStream>,
    desktops_seen: Option<(usize, [usize; crate::state::WORKSPACES])>,
}

impl Control {
    /// After every pass of the loop: the watchers hear "desktops" when the desktops changed (the one shown, or how many windows
    /// one holds). Nothing is written while they stay as they were; a watcher that has gone (or stopped reading) is dropped.
    pub fn tell_watchers(&mut self, app: &App) {
        if self.watchers.is_empty() {
            return;
        }
        let now = app.desktops_key();
        if self.desktops_seen == Some(now) {
            return;
        }
        self.desktops_seen = Some(now);
        self.watchers.retain(|mut w| w.write_all(b"desktops\n").is_ok());
    }

    /// One command from the control socket, answered on it: the backend's own (`extra`) first, then the common ones.
    pub fn serve(&mut self, stream: UnixStream, app: &mut App, extra: Extra, usage: &str) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let mut line = String::new();
        let mut reader = BufReader::new(&stream).take(4096);
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        // the connection stays open, and gets a line "desktops" now and whenever they change (the panel's desktops wait on it)
        if words == ["watch"] {
            let mut s = &stream;
            if stream.set_nonblocking(true).is_ok() && s.write_all(b"desktops\n").is_ok() {
                if self.watchers.is_empty() {
                    self.desktops_seen = Some(app.desktops_key());
                }
                self.watchers.push(stream);
            }
            return;
        }
        let answer = match command(&words, app, extra, usage) {
            Ok(lines) => lines.into_iter().map(|l| l + "\n").collect::<String>() + "ok\n",
            Err(e) => format!("error: {e}\n"),
        };
        let mut s = &stream;
        let _ = s.write_all(answer.as_bytes());
    }
}

/// Make the control socket (only this user may open it) and print its path. Its connections are served by the caller.
pub fn listen(runtime: &Path, socket: &str) -> Result<(UnixListener, PathBuf), String> {
    let ctl = runtime.join(format!("yggdrasil-compositor-{socket}.ctl"));
    let _ = std::fs::remove_file(&ctl);
    let listener = UnixListener::bind(&ctl).map_err(|e| format!("cannot make the control socket {}: {e}", ctl.display()))?;
    std::fs::set_permissions(&ctl, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    println!("YGGDRASIL_COMPOSITOR_CTL={}", ctl.display());
    let _ = std::io::stdout().flush();
    Ok((listener, ctl))
}

/// The answer to one command: the tools' desktop commands (wmctl.rs), the backend's own, then the common ones.
fn command(words: &[&str], app: &mut App, extra: Extra, usage: &str) -> Result<Vec<String>, String> {
    if let Some(r) = crate::wmctl::command(words, app) {
        return r;
    }
    if let Some(r) = extra(words, app) {
        return r;
    }
    let output = |name: &str, app: &App| app.head(name).map(|hd| hd.output.clone()).ok_or_else(|| format!("no output {name}"));
    match words {
        ["outputs"] => Ok(app
            .heads
            .list
            .iter()
            .map(|hd| {
                let o = &hd.output;
                let m = o.current_mode().map(|m| m.size).unwrap_or_default();
                let l = state::logical_size(o);
                let at = o.current_location();
                format!(
                    "{} {}x{} scale={} at={},{} logical={}x{}{}{}",
                    o.name(),
                    m.w,
                    m.h,
                    o.current_scale().fractional_scale(),
                    at.x,
                    at.y,
                    l.w,
                    l.h,
                    if app.power.is_on(o) { "" } else { " power=off" },
                    if hd.enabled { "" } else { " disabled" }
                )
            })
            .collect()),
        ["scale", name, s] => {
            let v = crate::headless::parse_scale(s).ok_or_else(|| format!("{s}: give a number from 0.25 to 8"))?;
            let mut want = app.head(name).ok_or_else(|| format!("no output {name}"))?.now();
            want.scale = v;
            app.configure_outputs(vec![(name.to_string(), want)], false)?;
            Ok(vec![])
        }
        ["pointer-look"] => Ok(vec![app.pointer_look()]),
        ["windows"] => Ok(app
            .window_infos()
            .into_iter()
            .map(|i| {
                let (title, app_id) = state::title_app(&i.window);
                let size = i.window.geometry().size;
                let (title, app_id) = (crate::wmctl::field(&title), crate::wmctl::field(&app_id));
                format!("{app_id}\t{title}\t{},{}\t{}x{}\t{}\t{}", i.at.x, i.at.y, size.w, size.h, i.workspace + 1, i.states.join(","))
            })
            .collect()),
        ["focus"] => Ok(vec![app.focus_info()]),
        ["free"] => Ok(app
            .space
            .outputs()
            .map(|o| {
                let r = app.free_area(o);
                format!("{}\t{},{}\t{}x{}", o.name(), r.loc.x, r.loc.y, r.size.w, r.size.h)
            })
            .collect()),
        ["workspace"] => Ok(vec![(app.workspace() + 1).to_string()]),
        ["lock"] => {
            let mut lines = vec![if app.locked { "locked" } else { "unlocked" }.to_string()];
            for (o, size) in app.lock_infos() {
                let (w, hh) = size.map(|s| (s.w, s.h)).unwrap_or((0, 0));
                lines.push(format!("{}\t{w}x{hh}", o.name()));
            }
            Ok(lines)
        }
        ["power", name, on] => {
            let o = output(name, app)?;
            let on = match *on {
                "on" => true,
                "off" => false,
                v => return Err(format!("{v}: on or off")),
            };
            app.set_output_power(&o, on);
            Ok(vec![])
        }
        ["quit"] => {
            app.quit();
            Ok(vec![])
        }
        [] => Err("no command".into()),
        _ => Err(format!("unknown command {:?} ({usage})", words.join(" "))),
    }
}

/// `yggdrasil-compositor ctl [--socket PATH] COMMAND...`: send one command to a running compositor and print the answer. The
/// socket is --socket, else $YGGDRASIL_COMPOSITOR_CTL, else the only one in $XDG_RUNTIME_DIR. Exit 0 on "ok", 1 on an error,
/// 2 when no compositor was found.
pub fn ctl(args: &[String]) -> i32 {
    let (path, words): (Option<PathBuf>, &[String]) = match args {
        [flag, p, rest @ ..] if flag == "--socket" => (Some(PathBuf::from(p)), rest),
        _ => (None, args),
    };
    let path = path.or_else(|| std::env::var_os("YGGDRASIL_COMPOSITOR_CTL").map(PathBuf::from)).or_else(|| {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
        let found: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("yggdrasil-compositor-") && n.ends_with(".ctl"))
            })
            .collect();
        (found.len() == 1).then(|| found[0].clone())
    });
    let Some(path) = path else {
        eprintln!("yggdrasil-compositor ctl: no compositor found (give --socket PATH or YGGDRASIL_COMPOSITOR_CTL)");
        return 2;
    };
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("yggdrasil-compositor ctl: {}: {e}", path.display());
            return 2;
        }
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    if stream.write_all(format!("{}\n", words.join(" ")).as_bytes()).is_err() {
        return 2;
    }
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    let mut lines: Vec<&str> = answer.lines().collect();
    match lines.pop() {
        Some("ok") => {
            for l in lines {
                println!("{l}");
            }
            0
        }
        Some(e) => {
            eprintln!("yggdrasil-compositor ctl: {}", e.trim_start_matches("error: "));
            1
        }
        None => {
            eprintln!("yggdrasil-compositor ctl: no answer");
            1
        }
    }
}
