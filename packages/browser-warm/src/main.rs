//! yggdrasil-browser-warm: keeps the default browser started with no windows, so a new window or a link opens almost at
//! once (the launcher hands it to the running browser over its singleton socket instead of starting a new one).
//!
//! Only Chromium-based browsers can do this: `--no-startup-window` starts one with no window, and `--keep-alive-for-test`
//! keeps it running (without it, Chromium quits once its extensions have loaded, as nothing holds it). Firefox-based
//! browsers have no way to run with no window, so for them this does nothing.
//!
//! The cost is memory: the browser stays loaded, about 0.5-0.8 GB, even with every window closed.
//!
//! The background browser is a systemd user unit of its own (`yggdrasil-browser-warm-instance`). When the desktop has an
//! app launcher with per-app rules (the `apps` integration), it runs in that app's slice, so the app's network rule
//! covers it as it covers a browser started from the dock. An app the launcher runs in a sandbox is never warmed: the
//! launcher cannot start it with extra arguments, and a warm copy started outside the sandbox would serve every later
//! window, sandbox or not.
//!
//! `run` (the service) checks every minute. It starts the browser when no copy of it is running, and restarts the
//! background copy only when the browser was updated (its program on disk was replaced) or the default browser changed,
//! and then only when it has no windows open and pacman is not working.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

const INSTANCE: &str = "yggdrasil-browser-warm-instance";
const PACMAN_LOCK: &str = "/var/lib/pacman/db.lck";
const POLL: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Family {
    /// Chromium-based, with the folder its profile lives in (under ~/.config).
    Chromium(&'static str),
    Firefox,
    Unknown,
}

/// What kind of browser a program is, from its file name.
fn family(program: &str) -> Family {
    let name = program.rsplit('/').next().unwrap_or(program);
    let chromium: &[(&[&str], &str)] = &[
        (&["chromium", "chromium-browser"], "chromium"),
        (&["google-chrome", "google-chrome-stable"], "google-chrome"),
        (&["google-chrome-beta"], "google-chrome-beta"),
        (&["google-chrome-unstable"], "google-chrome-unstable"),
        (&["brave", "brave-browser", "brave-browser-stable"], "BraveSoftware/Brave-Browser"),
        (&["brave-origin"], "BraveSoftware/Brave-Origin"),
        (&["microsoft-edge", "microsoft-edge-stable"], "microsoft-edge"),
        (&["vivaldi", "vivaldi-stable"], "vivaldi"),
        (&["helium", "helium-browser"], "net.imput.helium"),
        (&["thorium", "thorium-browser"], "thorium"),
        (&["ungoogled-chromium"], "chromium"),
    ];
    for (names, profile) in chromium {
        if names.contains(&name) {
            return Family::Chromium(profile);
        }
    }
    const FIREFOX: &[&str] = &["firefox", "firefox-esr", "firefox-developer-edition", "librewolf", "zen", "zen-browser", "floorp", "waterfox", "mullvad-browser", "torbrowser-launcher", "tor-browser"];
    if FIREFOX.contains(&name) {
        return Family::Firefox;
    }
    Family::Unknown
}

/// The words of a desktop file's Exec line (quoting as the Desktop Entry spec has it), without field codes.
fn exec_words(line: &str) -> Vec<String> {
    let (mut words, mut cur, mut quoted, mut any) = (vec![], String::new(), false, false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        words.push(cur);
    }
    words.retain(|w| !(w.len() == 2 && w.starts_with('%')));
    words
}

/// The program an Exec line starts: the first word, after an `env VAR=value ...` prefix.
fn exec_program(line: &str) -> Option<String> {
    let words = exec_words(line);
    let mut it = words.into_iter().peekable();
    if it.peek().map(|w| w == "env" || w.ends_with("/env")).unwrap_or(false) {
        it.next();
        while it.peek().map(|w| w.contains('=') && !w.starts_with('/')).unwrap_or(false) {
            it.next();
        }
    }
    it.next()
}

/// The main Exec line of a desktop file (the one in [Desktop Entry], not an action's).
fn desktop_exec(text: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
        } else if inside {
            if let Some(v) = line.strip_prefix("Exec=") {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn config_home() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".config"))
}

fn data_dirs() -> Vec<PathBuf> {
    let mut out = vec![std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".local/share"))];
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    out.extend(sys.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    out
}

fn find_desktop(id: &str) -> Option<PathBuf> {
    data_dirs().into_iter().map(|d| d.join("applications").join(id)).find(|p| p.is_file())
}

/// The default browser's desktop id: from xdg-settings, else from mimeapps.list.
fn default_browser_id() -> Option<String> {
    if let Ok(o) = Command::new("xdg-settings").args(["get", "default-web-browser"]).stderr(Stdio::null()).output() {
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if o.status.success() && s.ends_with(".desktop") {
            return Some(s);
        }
    }
    let text = std::fs::read_to_string(config_home().join("mimeapps.list")).ok()?;
    mimeapps_default(&text, "x-scheme-handler/https")
}

fn mimeapps_default(text: &str, mime: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[Default Applications]";
        } else if inside {
            if let Some(v) = line.strip_prefix(mime).and_then(|r| r.trim_start().strip_prefix('=')) {
                return v.split(';').map(str::trim).find(|s| !s.is_empty()).map(String::from);
            }
        }
    }
    None
}

fn which(prog: &str) -> Option<PathBuf> {
    if prog.contains('/') {
        return Path::new(prog).is_file().then(|| PathBuf::from(prog));
    }
    std::env::var("PATH").ok()?.split(':').map(|d| Path::new(d).join(prog)).find(|p| p.is_file())
}

/// The browser to keep warm.
#[derive(Clone, Debug, PartialEq)]
struct Target {
    desktop_id: String,
    program: PathBuf,
    profile: &'static str,
}

/// Why there is nothing to warm, as a line for the log.
fn target() -> Result<Target, String> {
    let id = default_browser_id().ok_or("no default browser is set")?;
    let file = find_desktop(&id).ok_or_else(|| format!("the default browser's desktop file {id} was not found"))?;
    let exec = std::fs::read_to_string(&file).ok().and_then(|t| desktop_exec(&t)).ok_or_else(|| format!("{id} has no Exec line"))?;
    let prog = exec_program(&exec).ok_or_else(|| format!("{id} has an empty Exec line"))?;
    if prog == "flatpak" || prog.ends_with("/flatpak") {
        return Err(format!("{id} is a Flatpak: not supported, nothing to do"));
    }
    let program = which(&prog).ok_or_else(|| format!("{prog} (from {id}) is not installed"))?;
    match family(&prog) {
        Family::Chromium(profile) => Ok(Target { desktop_id: id, program, profile }),
        Family::Firefox => Err(format!("{prog} is Firefox-based: it cannot run with no window, so there is nothing to do")),
        Family::Unknown => Err(format!("{prog} is not a Chromium-based browser this knows: nothing to do")),
    }
}

/// The desktop's app launcher (the `apps` integration): (where per-app settings live, slice prefix).
fn launcher() -> Option<(PathBuf, String)> {
    let dirs = [config_home().join("yggdrasil/integrations"), PathBuf::from("/etc/yggdrasil/integrations"), PathBuf::from("/usr/share/yggdrasil/integrations")];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "conf")).collect();
        files.sort();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            let kv = |k: &str| {
                text.lines().map(|l| l.split('#').next().unwrap_or("").trim()).find_map(|l| {
                    let (a, b) = l.split_once('=')?;
                    (a.trim() == k).then(|| b.trim().to_string())
                })
            };
            if kv("kind").as_deref() == Some("apps") && kv("launch").is_some() {
                let settings = kv("settings").map(|s| PathBuf::from(s.replace("%h", &home().to_string_lossy())));
                return Some((settings.unwrap_or_default(), kv("unit_prefix").unwrap_or_default()));
            }
        }
    }
    None
}

fn app_id(desktop_id: &str) -> &str {
    desktop_id.strip_suffix(".desktop").unwrap_or(desktop_id)
}

/// The launcher's slice for an app (the launcher and its firewall helper build the same name: no dash in it).
fn slice_for(prefix: &str, desktop_id: &str) -> String {
    let id: String = app_id(desktop_id).chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    format!("{prefix}{id}.slice")
}

/// Whether the launcher runs this app in a sandbox (then it is never warmed).
fn sandboxed(settings_dir: &Path, desktop_id: &str) -> bool {
    if settings_dir.as_os_str().is_empty() {
        return false;
    }
    let p = settings_dir.join(format!("{}.json", app_id(desktop_id)));
    match std::fs::read_to_string(&p) {
        Ok(t) => match serde_json::from_str::<serde_json::Value>(&t) {
            Ok(v) => v["sandbox"].as_bool().unwrap_or(false),
            // a settings file that cannot be read: assume the strict reading
            Err(_) => true,
        },
        Err(_) => false,
    }
}

fn systemctl(args: &[&str]) -> Option<String> {
    let o = Command::new("systemctl").arg("--user").args(args).stderr(Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn instance_active() -> bool {
    Command::new("systemctl").args(["--user", "-q", "is-active", INSTANCE]).status().map(|s| s.success()).unwrap_or(false)
}

fn instance_main_pid() -> Option<u32> {
    systemctl(&["show", "-p", "MainPID", "--value", INSTANCE])?.parse().ok().filter(|p| *p > 0)
}

/// The program the background browser was started with.
fn instance_program() -> Option<PathBuf> {
    let s = systemctl(&["show", "-p", "ExecStart", "--value", INSTANCE])?;
    let rest = &s[s.find("path=")? + 5..];
    Some(PathBuf::from(rest.split([' ', ';']).next()?))
}

/// The browser's own process: the one holding its profile's singleton lock (a link to "HOST-PID"), if it is alive.
fn browser_pid(profile: &str) -> Option<u32> {
    let link = std::fs::read_link(config_home().join(profile).join("SingletonLock")).ok()?;
    let pid: u32 = link.to_string_lossy().rsplit('-').next()?.parse().ok()?;
    Path::new(&format!("/proc/{pid}")).exists().then_some(pid)
}

fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // the name in parentheses may hold spaces: the fields after the last ')' are state, ppid, ...
    stat[stat.rfind(')')? + 1..].split_whitespace().nth(1)?.parse().ok()
}

/// Whether `pid` descends from `ancestor` (Chromium moves itself into a scope of its own, out of the unit's cgroup, so
/// parentage is what still ties it to the unit).
fn descends_from(mut pid: u32, ancestor: u32) -> bool {
    for _ in 0..64 {
        if pid == ancestor {
            return true;
        }
        match parent_of(pid) {
            Some(p) if p > 1 => pid = p,
            _ => return false,
        }
    }
    false
}

/// Whether the browser has windows open. None when that cannot be told (then nothing is restarted).
fn has_windows(pid: u32) -> Option<bool> {
    let o = Command::new("hyprctl").args(["clients", "-j"]).stderr(Stdio::null()).output().ok()?;
    if !o.status.success() {
        return None;
    }
    windows_of(&String::from_utf8_lossy(&o.stdout), |w| w == pid || descends_from(w, pid))
}

fn windows_of(json: &str, ours: impl Fn(u32) -> bool) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(v.as_array()?.iter().filter_map(|c| c["pid"].as_u64()).any(|p| ours(p as u32)))
}

/// Whether the running program was replaced on disk (a package update).
fn program_replaced(pid: u32) -> bool {
    std::fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.to_string_lossy().ends_with(" (deleted)")).unwrap_or(false)
}

fn extra_flags() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(config_home().join("yggdrasil/browser-warm-flags.conf")) else { return vec![] };
    text.lines().flat_map(|l| l.split('#').next().unwrap_or("").split_whitespace().map(String::from).collect::<Vec<_>>()).collect()
}

/// The systemd-run command that starts the background browser.
fn start_command(t: &Target, slice: Option<&str>, flags: &[String]) -> Vec<String> {
    let mut c: Vec<String> = ["systemd-run", "--user", "--quiet", "--collect", &format!("--unit={INSTANCE}")].iter().map(|s| s.to_string()).collect();
    c.push(format!("--slice={}", slice.unwrap_or("app-graphical.slice")));
    c.push(format!("--description=Background {} (yggdrasil-browser-warm)", app_id(&t.desktop_id)));
    c.extend(["--property=KillMode=mixed", "--property=Restart=on-failure"].map(String::from));
    // the user manager may lack the session's display variables
    for k in ["WAYLAND_DISPLAY", "DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE", "HYPRLAND_INSTANCE_SIGNATURE", "DBUS_SESSION_BUS_ADDRESS"] {
        if let Ok(v) = std::env::var(k) {
            c.push(format!("--setenv={k}={v}"));
        }
    }
    c.push(t.program.to_string_lossy().into());
    c.extend(["--no-startup-window", "--keep-alive-for-test"].map(String::from));
    c.extend(flags.iter().cloned());
    c
}

/// Why the target may not be warmed (the sandbox rule), or the slice to run it in.
fn placement(t: &Target) -> Result<Option<String>, String> {
    match launcher() {
        Some((settings, prefix)) => {
            if sandboxed(&settings, &t.desktop_id) {
                Err(format!("{} runs in a sandbox: a background copy would run outside it, so it is not warmed", app_id(&t.desktop_id)))
            } else {
                Ok((!prefix.is_empty()).then(|| slice_for(&prefix, &t.desktop_id)))
            }
        }
        None => Ok(None),
    }
}

fn start(t: &Target) -> Result<(), String> {
    if instance_active() {
        return Ok(());
    }
    if let Some(pid) = browser_pid(t.profile) {
        return Err(format!("{} is already running (pid {pid}); it will be warmed once it is closed", app_id(&t.desktop_id)));
    }
    let slice = placement(t)?;
    let cmd = start_command(t, slice.as_deref(), &extra_flags());
    let st = Command::new(&cmd[0]).args(&cmd[1..]).stdin(Stdio::null()).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err(format!("systemd-run failed ({st})"));
    }
    log(&format!("started {} with no window{}", t.program.display(), slice.map(|s| format!(" in {s}")).unwrap_or_default()));
    Ok(())
}

/// Stops the background browser: asks the browser to quit first (it may sit behind a wrapper, and it moved itself out
/// of the unit's cgroup), then stops the unit.
fn stop() {
    if let Some(main) = instance_main_pid() {
        let lock_pid = target().ok().and_then(|t| browser_pid(t.profile)).or_else(|| {
            let p = instance_program()?;
            browser_pid(match family(&p.to_string_lossy()) {
                Family::Chromium(prof) => prof,
                _ => return None,
            })
        });
        if let Some(pid) = lock_pid.filter(|p| *p != main && descends_from(*p, main)) {
            let _ = Command::new("kill").arg(pid.to_string()).status();
            for _ in 0..100 {
                if !Path::new(&format!("/proc/{pid}")).exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    let _ = Command::new("systemctl").args(["--user", "stop", INSTANCE]).status();
}

/// The background browser's process and whether it has windows open (None: cannot tell).
fn instance_state() -> Option<(u32, Option<bool>)> {
    let main = instance_main_pid()?;
    let prog = instance_program()?;
    let pid = match family(&prog.to_string_lossy()) {
        Family::Chromium(profile) => browser_pid(profile).filter(|p| descends_from(*p, main)).unwrap_or(main),
        _ => main,
    };
    Some((pid, has_windows(pid)))
}

fn log(msg: &str) {
    eprintln!("yggdrasil-browser-warm: {msg}");
}

/// One round of `run`. `said` keeps the last log line so a steady state is not repeated every minute.
fn check(said: &mut String) {
    let mut say = |m: String| {
        if *said != m {
            log(&m);
            *said = m;
        }
    };
    if Path::new(PACMAN_LOCK).exists() {
        return;
    }
    let want = target();
    let want_ok = want.as_ref().ok().filter(|t| placement(t).is_ok());
    if instance_active() {
        let Some((pid, windows)) = instance_state() else { return };
        let reason = if program_replaced(pid) {
            Some("the browser was updated".to_string())
        } else if want_ok.map(|t| Some(&t.program) != instance_program().as_ref()).unwrap_or(true) {
            Some(match &want {
                Ok(t) => placement(t).err().unwrap_or_else(|| "the default browser changed".into()),
                Err(e) => e.clone(),
            })
        } else {
            None
        };
        let Some(reason) = reason else { return };
        if windows != Some(false) {
            say(format!("{reason}; waiting until it has no windows open to restart it"));
            return;
        }
        log(&format!("{reason}: restarting the background browser"));
        stop();
    }
    match want {
        Ok(t) => {
            if let Err(e) = start(&t) {
                say(e);
            } else {
                said.clear();
            }
        }
        Err(e) => say(e),
    }
}

fn status() {
    match target() {
        Ok(t) => println!("default browser: {} ({})", t.desktop_id, t.program.display()),
        Err(e) => println!("default browser: {e}"),
    }
    match instance_state() {
        Some((pid, w)) => {
            let exe = std::fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.display().to_string()).unwrap_or_default();
            println!("background browser: running, pid {pid}, {exe}");
            println!("windows: {}", match w {
                Some(true) => "open",
                Some(false) => "none",
                None => "unknown",
            });
        }
        None => println!("background browser: stopped"),
    }
}

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("run") => {
            let mut said = String::new();
            loop {
                check(&mut said);
                std::thread::sleep(POLL);
            }
        }
        Some("start") => match target().and_then(|t| start(&t)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                // nothing to warm (a Firefox-based browser, a sandboxed one, one already open) is not an error
                log(&e);
                ExitCode::SUCCESS
            }
        },
        Some("stop") => {
            stop();
            ExitCode::SUCCESS
        }
        // when the service stops: let the background browser go, unless it has windows open
        Some("release") => {
            if let Some((_, Some(false))) = instance_state() {
                stop();
            }
            ExitCode::SUCCESS
        }
        Some("status") => {
            status();
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: yggdrasil-browser-warm run | start | stop | release | status");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_lines_are_read_like_the_spec_says() {
        assert_eq!(exec_program("/usr/bin/vivaldi-stable %U").as_deref(), Some("/usr/bin/vivaldi-stable"));
        assert_eq!(exec_program("env GDK_BACKEND=wayland BRAVE=1 brave --flag %U").as_deref(), Some("brave"));
        assert_eq!(exec_program("\"/opt/My Browser/chrome\" --x %u").as_deref(), Some("/opt/My Browser/chrome"));
        assert_eq!(exec_words("chromium --a=\"b c\" %F"), vec!["chromium", "--a=b c"]);
        assert_eq!(exec_program(""), None);
        let desktop = "[Desktop Entry]\nName=V\nExec=/usr/bin/vivaldi-stable %U\n[Desktop Action new-window]\nExec=/usr/bin/vivaldi-stable --new-window\n";
        assert_eq!(desktop_exec(desktop).as_deref(), Some("/usr/bin/vivaldi-stable %U"));
        assert_eq!(desktop_exec("[Desktop Action x]\nExec=a\n"), None);
    }

    #[test]
    fn browsers_are_sorted_into_families() {
        assert_eq!(family("/usr/bin/vivaldi-stable"), Family::Chromium("vivaldi"));
        assert_eq!(family("brave"), Family::Chromium("BraveSoftware/Brave-Browser"));
        assert_eq!(family("/usr/bin/google-chrome-stable"), Family::Chromium("google-chrome"));
        assert_eq!(family("microsoft-edge-stable"), Family::Chromium("microsoft-edge"));
        assert_eq!(family("/usr/bin/firefox"), Family::Firefox);
        assert_eq!(family("librewolf"), Family::Firefox);
        assert_eq!(family("zen-browser"), Family::Firefox);
        assert_eq!(family("epiphany"), Family::Unknown);
    }

    #[test]
    fn mimeapps_and_slices() {
        let m = "[Added Associations]\nx-scheme-handler/https=firefox.desktop;\n[Default Applications]\ntext/html=a.desktop\nx-scheme-handler/https=vivaldi-stable.desktop;b.desktop;\n";
        assert_eq!(mimeapps_default(m, "x-scheme-handler/https").as_deref(), Some("vivaldi-stable.desktop"));
        assert_eq!(slice_for("lapp_", "vivaldi-stable.desktop"), "lapp_vivaldi_stable.slice");
        assert_eq!(slice_for("lapp_", "com.brave.Browser.desktop"), "lapp_com_brave_Browser.slice");
    }

    #[test]
    fn a_sandboxed_app_is_never_warmed() {
        let d = std::env::temp_dir().join(format!("bw-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(!sandboxed(&d, "vivaldi-stable.desktop"), "no settings file: no sandbox");
        std::fs::write(d.join("vivaldi-stable.json"), r#"{"sandbox": true}"#).unwrap();
        assert!(sandboxed(&d, "vivaldi-stable.desktop"));
        std::fs::write(d.join("vivaldi-stable.json"), r#"{"sandbox": false, "files": "all"}"#).unwrap();
        assert!(!sandboxed(&d, "vivaldi-stable.desktop"));
        std::fs::write(d.join("vivaldi-stable.json"), "not json").unwrap();
        assert!(sandboxed(&d, "vivaldi-stable.desktop"), "unreadable settings: strict");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn windows_are_matched_by_process() {
        let j = r#"[{"class":"vivaldi-stable","pid":1965},{"class":"foot","pid":42}]"#;
        assert_eq!(windows_of(j, |p| p == 1965), Some(true));
        assert_eq!(windows_of(j, |p| p == 7), Some(false));
        assert_eq!(windows_of("[]", |_| true), Some(false));
        assert_eq!(windows_of("nope", |_| true), None);
    }

    #[test]
    fn the_start_command_runs_in_the_apps_slice() {
        let t = Target { desktop_id: "vivaldi-stable.desktop".into(), program: "/usr/bin/vivaldi-stable".into(), profile: "vivaldi" };
        let c = start_command(&t, Some("lapp_vivaldi_stable.slice"), &["--ozone-platform=wayland".into()]);
        assert!(c.contains(&"--slice=lapp_vivaldi_stable.slice".to_string()));
        assert!(c.contains(&format!("--unit={INSTANCE}")));
        let tail: Vec<&str> = c.iter().rev().take(4).rev().map(String::as_str).collect();
        assert_eq!(tail, ["/usr/bin/vivaldi-stable", "--no-startup-window", "--keep-alive-for-test", "--ozone-platform=wayland"]);
        assert!(start_command(&t, None, &[]).contains(&"--slice=app-graphical.slice".to_string()));
    }
}
