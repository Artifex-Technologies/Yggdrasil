//! yggdrasil-session: Yggdrasil's own desktop session, on Hyprland (the compositor chosen 2026-10-23, docs/COMPOSITOR.md).
//! The login screen starts it (the "Yggdrasil" entry), or a text console: sign in on Ctrl+Alt+F3 and type yggdrasil-session.
//!
//! Hyprland draws the windows; everything you see and use is Yggdrasil's: the panel, dock and Fylgja, the lock screen, the
//! notifications, the password boxes, Settings, Minos. Your own Hyprland additions go in ~/.config/yggdrasil/hyprland.lua.
//! The environment reaches systemd and D-Bus, and the session target (the autostart folder) starts, from Hyprland's own
//! configuration (packages/session/hyprland/yggdrasil.lua, its last `exec_cmd`), once Hyprland's display is there.
//!
//! A bash script until 2026-11-13 (packages/session/tests/fixtures/yggdrasil-session.sh, kept for the differential in
//! tools/parity-shell-ports.py until 2026-11-20); this does what it did, in the same order, with the same calls, files, log
//! lines and exit code, in one process: the script's log copy loop and monitor log were background shells that started `ls`,
//! `head`, `grep` and `sleep` every five seconds for the whole session; here they are two threads that start nothing.

use crate::run;
use std::collections::HashSet;
use std::ffi::{CString, OsStr, OsString};
use std::io::{BufRead, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

const ME: &str = "yggdrasil-session";
const CONFIG: &str = "/usr/share/yggdrasil/hyprland/yggdrasil.lua";
const NVIDIA: &str = "/proc/driver/nvidia/version";
/// The monitor log keeps this many lines from earlier sessions.
const EVENTS_KEPT: usize = 2000;
/// Three crashes within this many seconds (the settings themselves, most likely) and the next start is in safe mode.
const CRASH_WINDOW: u64 = 600;
const CRASHES_FOR_SAFE_MODE: usize = 3;

pub fn main() -> ! {
    // The environment first, before any thread or child: what the script exported, in its order.
    set("XDG_CURRENT_DESKTOP", "Yggdrasil");
    set("XDG_SESSION_DESKTOP", "Yggdrasil");
    set("XDG_SESSION_TYPE", "wayland");
    // toolkits: Wayland first, X11 (Xwayland) for the rest
    set("QT_QPA_PLATFORM", "wayland;xcb");
    set("GDK_BACKEND", "wayland,x11,*");
    set("SDL_VIDEODRIVER", "wayland,x11");
    set("CLUTTER_BACKEND", "wayland");
    set("MOZ_ENABLE_WAYLAND", "1");
    set("ELECTRON_OZONE_PLATFORM_HINT", "auto");
    set("_JAVA_AWT_WM_NONREPARENTING", "1");
    set("QT_WAYLAND_DISABLE_WINDOWDECORATION", "0");
    // Hyprland offers to decorate windows but draws no title bar, so Qt programs that accept (VLC, Qt's tools, KDE programs)
    // would have none: they are told not to see that offer and draw their own (title, minimize, maximize, close)
    set_default("QT_WAYLAND_DISABLED_INTERFACES", "zxdg_decoration_manager_v1");
    // Qt programs follow Yggdrasil's dark or light and accent through the desktop portal (Qt's own, no KDE theme plugin
    // needed); without it they are plain light grey
    set_default("QT_QPA_PLATFORMTHEME", "xdgdesktopportal");
    if Path::new(NVIDIA).exists() {
        set("LIBVA_DRIVER_NAME", "nvidia");
        set("__GLX_VENDOR_LIBRARY_NAME", "nvidia");
        set("NVD_BACKEND", "direct");
    }

    // Game stats (MangoHud, for a game started with MANGOHUD=1) in Yggdrasil's look, unless you have your own
    let config_home = xdg_or_home("XDG_CONFIG_HOME", "/.config");
    if !Path::new(&cat(&config_home, "/MangoHud/MangoHud.conf")).exists() && var("MANGOHUD_CONFIGFILE").is_none() {
        let out = mangohud_conf(&config_home);
        std::env::set_var("MANGOHUD_CONFIGFILE", out);
    }
    let conf = var("YGGDRASIL_HYPRLAND_CONFIG").unwrap_or_else(|| CONFIG.into());

    // The user's own session bus (the one systemd runs, which the autostart programs use). A private bus would split the
    // session in two: programs started by Hyprland on one bus, the panel and the rest on the other, and the environment never
    // reaching systemd.
    let runtime = runtime();
    let bus = cat(&runtime, "/bus");
    if std::fs::metadata(&bus).is_ok_and(|m| std::os::unix::fs::FileTypeExt::is_socket(&m.file_type())) {
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", cat(OsStr::new("unix:path="), bus.as_bytes()));
    }
    let mut line = b"starting Hyprland (".to_vec();
    line.extend(hyprland_version());
    line.extend(b") with ");
    line.extend(conf.as_bytes());
    line.extend(b", bus ");
    line.extend(var("DBUS_SESSION_BUS_ADDRESS").map(|b| b.into_vec()).unwrap_or_else(|| b"none".to_vec()));
    log(&line);

    // the console this session starts on shows nothing (no text from before, no cursor) until Hyprland draws
    // SAFETY: isatty on a descriptor number
    if unsafe { libc::isatty(1) } == 1 {
        let mut o = std::io::stdout();
        let _ = o.write_all(b"\x1b[2J\x1b[3J\x1b[H\x1b[?25l");
        let _ = o.flush();
    }
    let state = cat(&xdg_or_home("XDG_STATE_HOME", "/.local/state"), "/yggdrasil");
    mkdir_p(&state);
    // what Hyprland and its launcher print goes to a file, not onto the screen: started by greetd, the session's output is the
    // console itself, and its start-up messages flashed up between the login screen and the desktop
    redirect_output(&cat(&state, "/session-output.log"));

    // Hyprland keeps its log in the runtime folder, which goes when the session does (or with a restart): a copy is kept up to
    // date in ~/.local/state/yggdrasil/hyprland-last.log while the session runs, so a problem can always be looked at afterwards
    let before = names(&runtime);
    let keeper = keeper(runtime.clone(), state.clone(), before);

    // Monitors coming and going, with the time, straight from the kernel (so it is written even if Hyprland is the one in
    // trouble): 2026-11-05 Hyprland 0.57.1 died when a monitor that had been switched off came back. Trimmed to its last 2000
    // lines each login.
    let events = cat(&state, "/monitor-events.log");
    trim_events(&events);
    append(&events, "session start");
    let monitor = run::which("udevadm").map(|_| watch_monitors(&events));

    // Our own small watchdog in place of start-hyprland: after a crash, Hyprland's launcher started it again in SAFE MODE (no
    // Yggdrasil settings, so no panel, no shortcuts, no Wi-Fi menu; 2026-11-05). Here a crash keeps that session's log, then
    // Hyprland starts again with Yggdrasil's settings. Only when it keeps crashing (3 times in 10 minutes, which points at the
    // settings themselves) does the next start use safe mode. The "not started by start-hyprland" warning is turned off in
    // yggdrasil.lua (misc.disable_watchdog_warning).
    let logs = cat(&state, "/hyprland-logs");
    mkdir_p(&logs);
    let mut crashes: Vec<u64> = vec![];
    let mut safe = false;
    let mut run = 0u32;
    let code = loop {
        run += 1;
        let mut c = Command::new("Hyprland");
        if safe {
            c.arg("--safe-mode");
        } else {
            c.arg("--config").arg(&conf);
        }
        let code = status(&mut c);
        // a normal sign-out, or Hyprland told to stop (SIGTERM, SIGINT): not a crash
        if matches!(code, 0 | 143 | 130) {
            break code;
        }
        // a crash (or a fatal error): keep this instance's log before the next one starts and its folder is cleared
        if let Some(log) = newest_log(&runtime) {
            let name = format!("/{}-run{run}-crashed-exit{code}.log", run::local_time("%Y-%m-%d-%H%M%S"));
            cp(&log, &cat(&logs, &name));
        }
        append(&events, &format!("Hyprland ended with exit {code}"));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        crashes = recent(&crashes, now);
        if safe {
            break code; // safe mode itself failed: give up, back to the login screen
        }
        if crashes.len() >= CRASHES_FOR_SAFE_MODE {
            safe = true;
            log(format!("Hyprland crashed {} times in 10 minutes (exit {code}); starting it in safe mode", crashes.len()).as_bytes());
        } else {
            let mut m = format!("Hyprland crashed (exit {code}); starting it again with Yggdrasil's settings. Log kept in ").into_bytes();
            m.extend(logs.as_bytes());
            log(&m);
            // say so once the panel is back (it serves notifications)
            notify_later();
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    drop(keeper);
    if let Some(m) = monitor {
        m.stop();
    }
    append(&events, "session end");
    if let Some(log) = newest_log(&runtime) {
        cp(&log, &cat(&state, "/hyprland-last.log"));
    }
    // the session's programs (the panel and the rest) end with it
    let _ = Command::new("systemctl")
        .args(["--user", "stop", "yggdrasil-session.target", "graphical-session.target"])
        .stderr(Stdio::null())
        .status();
    let mut m = format!("Hyprland ended (exit {code}); its log is in ").into_bytes();
    m.extend(state.as_bytes());
    m.extend(b"/hyprland-last.log");
    log(&m);
    // a restart or shut-down asked for from the session (yggdrasil-logout) happens now, after Hyprland has let go of the screen
    // and the graphics card, as Plasma does it: rebooting under a running compositor could hang the machine at the very end
    // (NVIDIA)
    let after = PathBuf::from(cat(&runtime, "/yggdrasil-after-session"));
    if after.is_file() {
        let mut what = std::fs::read(&after).unwrap_or_default();
        what.retain(|&b| b != 0);
        while what.last() == Some(&b'\n') {
            what.pop();
        }
        if let Err(e) = std::fs::remove_file(&after) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("rm: cannot remove '{}': {}", after.display(), strerror(&e));
            }
        }
        if what == b"reboot" || what == b"poweroff" {
            let what = String::from_utf8_lossy(&what).into_owned();
            log(format!("{what}, now that the session has ended").as_bytes());
            let _ = status(Command::new("systemctl").arg(&what));
        }
    }
    std::process::exit(code)
}

// ---------------------------------------------------------------- the environment

fn set(name: &str, value: &str) {
    std::env::set_var(name, value);
}

/// export NAME=${NAME:-VALUE}
fn set_default(name: &str, value: &str) {
    if var(name).is_none() {
        set(name, value);
    }
}

/// A variable that is set and not empty (the script's ${NAME:-...} and -n tests).
fn var(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|v| !v.is_empty())
}

/// A path as the script wrote it, A followed by B: "$state/hyprland-logs", with no tidying of the slashes, as it is also
/// what the log lines say.
fn cat(a: &OsStr, b: impl AsRef<[u8]>) -> OsString {
    let mut v = a.as_bytes().to_vec();
    v.extend_from_slice(b.as_ref());
    OsString::from_vec(v)
}

/// ${NAME:-$HOME/REST}. The script ran with `set -u`: no HOME (when it is needed) ends it there, exit 1.
fn xdg_or_home(name: &str, rest: &str) -> OsString {
    var(name).unwrap_or_else(|| match std::env::var_os("HOME") {
        Some(home) => cat(&home, rest),
        None => {
            eprintln!("{ME}: HOME: unbound variable");
            std::process::exit(1)
        }
    })
}

/// ${XDG_RUNTIME_DIR:-/run/user/$UID}, as written.
fn runtime() -> OsString {
    // SAFETY: getuid cannot fail
    var("XDG_RUNTIME_DIR").unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }).into())
}

// ---------------------------------------------------------------- MangoHud

/// Game stats (MangoHud, for a game started with MANGOHUD=1): one slim line across the top centre in Yggdrasil's look, the
/// accent from Settings > Appearance and the desktop's font. Drawn by the game itself, so it costs next to nothing and does
/// not stop the game's frames going straight to the monitor. Your own ~/.config/MangoHud/MangoHud.conf wins when there is
/// one. Returns the file's path (also when it could not be written, as the script did).
fn mangohud_conf(config_home: &OsStr) -> OsString {
    let out = cat(&runtime(), "/yggdrasil-mangohud.conf");
    let accent = std::fs::read(cat(config_home, "/yggdrasil/appearance.conf"))
        .ok()
        .and_then(|data| data.split(|&b| b == b'\n').find_map(accent_numbers).map(|rgb| rgb.iter().map(|n| hex02(n)).collect::<String>()))
        .unwrap_or_else(|| "7c54d6".into());
    let font = run::output(Command::new("fc-match").args(["-f", "%{file}", "sans-serif"]));
    let mut conf = b"# Written by yggdrasil-session at sign-in; your own ~/.config/MangoHud/MangoHud.conf is used instead when it exists.
horizontal
hud_compact
position=top-center
offset_y=6
round_corners=10
background_color=0b0c10
background_alpha=0.8
text_color=e8e9ee
text_outline=0
"
    .to_vec();
    if !font.is_empty() {
        conf.extend(b"font_file=");
        conf.extend(&font);
    }
    conf.extend(
        format!(
            "
font_size=16
fps
frametime
frame_timing=0
throttling_status=0
cpu_stats
cpu_temp
gpu_stats
gpu_temp
cpu_color={accent}
gpu_color={accent}
engine_color={accent}
frametime_color={accent}
horizontal_separator_color={accent}
toggle_hud=Shift_R+F12
"
        )
        .as_bytes(),
    );
    if let Err(e) = std::fs::write(&out, conf) {
        eprintln!("{ME}: {}: {}", Path::new(&out).display(), strerror(&e));
    }
    out
}

/// The script's `sed -n 's/^[[:space:]]*accent[[:space:]]*=[[:space:]]*\([0-9]\+\)[[:space:]]*,...,\([0-9]\+\).*/\1 \2 \3/p'`
/// on one line: the three numbers of `accent = R, G, B`, as written (leading zeros and all).
fn accent_numbers(line: &[u8]) -> Option<[&[u8]; 3]> {
    fn spaces(s: &[u8]) -> &[u8] {
        let n = s.iter().take_while(|b| matches!(b, b' ' | b'\t' | b'\x0b' | b'\x0c' | b'\r')).count();
        &s[n..]
    }
    fn digits(s: &[u8]) -> Option<(&[u8], &[u8])> {
        let n = s.iter().take_while(|b| b.is_ascii_digit()).count();
        (n > 0).then(|| s.split_at(n))
    }
    let s = spaces(line).strip_prefix(b"accent")?;
    let s = spaces(spaces(s).strip_prefix(b"=")?);
    let (r, s) = digits(s)?;
    let s = spaces(spaces(s).strip_prefix(b",")?);
    let (g, s) = digits(s)?;
    let s = spaces(spaces(s).strip_prefix(b",")?);
    let (b, _) = digits(s)?;
    Some([r, g, b])
}

/// bash's `printf '%02x' NUMBER` for a run of digits: a leading 0 makes it octal (08 is an error and counts as 0), too big
/// is the largest number there is; each error said on standard error as bash says it.
fn hex02(n: &[u8]) -> String {
    let octal = n.len() > 1 && n[0] == b'0';
    let base: u64 = if octal { 8 } else { 10 };
    let (mut v, mut over, mut used) = (0u64, false, 0);
    for &d in n {
        let d = u64::from(d - b'0');
        if d >= base {
            break;
        }
        match v.checked_mul(base).and_then(|x| x.checked_add(d)) {
            Some(x) => v = x,
            None => over = true,
        }
        used += 1;
    }
    if over {
        v = u64::MAX;
    }
    let shown = String::from_utf8_lossy(n);
    if used < n.len() {
        eprintln!("{ME}: printf: {shown}: invalid octal number");
    } else if over {
        eprintln!("{ME}: printf: {shown}: Numerical result out of range");
    }
    format!("{v:02x}")
}

// ---------------------------------------------------------------- Hyprland

/// `Hyprland --version 2>/dev/null | head -n1 | cut -d' ' -f1-2`: "Hyprland 0.57.1".
fn hyprland_version() -> Vec<u8> {
    let out = run::output(Command::new("Hyprland").arg("--version"));
    let first = out.split(|&b| b == b'\n').next().unwrap_or_default();
    let mut words = first.splitn(3, |&b| b == b' ');
    match (words.next(), words.next()) {
        (Some(a), Some(b)) => [a, b" ", b].concat(),
        _ => first.to_vec(),
    }
}

/// A program's exit status as the shell gives it (128 + a signal that ended it); one that is not there is 127, said on
/// standard error as bash says it.
fn status(c: &mut Command) -> i32 {
    match c.status() {
        Ok(s) => run::code(s),
        Err(e) => {
            let prog = c.get_program().to_string_lossy().into_owned();
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!("{ME}: {prog}: command not found");
                127
            } else {
                eprintln!("{ME}: {prog}: {}", strerror(&e));
                126
            }
        }
    }
}

/// The crash times of the last ten minutes, and this one.
fn recent(crashes: &[u64], now: u64) -> Vec<u64> {
    let mut v: Vec<u64> = crashes.iter().copied().filter(|&t| now.saturating_sub(t) < CRASH_WINDOW).collect();
    v.push(now);
    v
}

/// `(sleep 10; notify-send ... 2>/dev/null) &`: the notice comes ten seconds later, also when the session has ended by then.
/// A process of its own (forked twice, so this one has nothing to wait for), as the script's background shell was.
fn notify_later() {
    let args: Vec<CString> = [
        "notify-send",
        "-a",
        "Yggdrasil",
        "-i",
        "dialog-warning",
        "The desktop restarted",
        "Hyprland stopped unexpectedly and was started again. Open programs had to close; its log is kept in ~/.local/state/yggdrasil/hyprland-logs.",
    ]
    .iter()
    .map(|a| CString::new(*a).unwrap_or_default())
    .collect();
    let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    let null = c"/dev/null";
    // SAFETY: after fork the children only make async-signal-safe calls (fork, signal, open, dup2, nanosleep, execvp, _exit)
    // on memory prepared before it; the parent waits for the first child, which exits at once
    unsafe {
        let pid = libc::fork();
        if pid == 0 {
            if libc::fork() != 0 {
                libc::_exit(0);
            }
            // as a shell's background job: Ctrl+C and Ctrl+\ do not reach it, its input is /dev/null; errors go nowhere
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            libc::signal(libc::SIGQUIT, libc::SIG_IGN);
            let fd = libc::open(null.as_ptr(), libc::O_RDWR);
            if fd >= 0 {
                libc::dup2(fd, 0);
                libc::dup2(fd, 2);
            }
            let mut t = libc::timespec { tv_sec: 10, tv_nsec: 0 };
            let mut rest = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            while libc::nanosleep(&t, &mut rest) == -1 {
                t = rest;
            }
            libc::execvp(argv[0], argv.as_ptr());
            libc::_exit(127);
        }
        if pid > 0 {
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
    }
}

// ---------------------------------------------------------------- the log copies

/// The names in $runtime/hypr when the session started (`ls`: not those starting with a dot).
fn names(runtime: &OsStr) -> HashSet<OsString> {
    std::fs::read_dir(PathBuf::from(cat(runtime, "/hypr")))
        .map(|d| d.flatten().map(|e| e.file_name()).filter(|n| !n.as_bytes().starts_with(b".")).collect())
        .unwrap_or_default()
}

/// The newest Hyprland instance folder (`ls -td $runtime/hypr/*/ | head -n1`: by modification time, then by name) and its
/// hyprland.log when there is one.
fn newest(runtime: &OsStr) -> Option<(OsString, PathBuf)> {
    let mut v: Vec<(i64, i64, OsString)> = std::fs::read_dir(PathBuf::from(cat(runtime, "/hypr")))
        .ok()?
        .flatten()
        .filter(|e| !e.file_name().as_bytes().starts_with(b"."))
        .filter_map(|e| {
            let m = std::fs::metadata(e.path()).ok().filter(|m| m.is_dir())?;
            Some((m.mtime(), m.mtime_nsec(), e.file_name()))
        })
        .collect();
    v.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)).then_with(|| a.2.cmp(&b.2)));
    let (_, _, name) = v.into_iter().next()?;
    let log = PathBuf::from(cat(runtime, "/hypr/")).join(&name).join("hyprland.log");
    Some((name, log))
}

fn newest_log(runtime: &OsStr) -> Option<PathBuf> {
    newest(runtime).map(|(_, log)| log).filter(|l| l.is_file())
}

/// Every five seconds: the log of a Hyprland started by this session (one whose folder was not there before) is copied to
/// hyprland-last.log. Ends when the returned sender is dropped.
fn keeper(runtime: OsString, state: OsString, before: HashSet<OsString>) -> mpsc::Sender<()> {
    let (tx, rx) = mpsc::channel::<()>();
    let last = cat(&state, "/hyprland-last.log");
    let _ = std::thread::Builder::new().stack_size(64 * 1024).spawn(move || {
        while let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(Duration::from_secs(5)) {
            if let Some((name, log)) = newest(&runtime) {
                if log.is_file() && !before.contains(&name) {
                    cp(&log, &last);
                }
            }
        }
    });
    tx
}

/// `cp -f SRC DST`: into DST/NAME when DST is a folder; an existing file is written over (keeping its mode) or, when it cannot
/// be opened, removed and made again; a new one gets SRC's mode less the umask. Errors said as cp says them.
fn cp(src: &Path, dst: &OsStr) {
    let mut dst = PathBuf::from(dst);
    match std::fs::metadata(&dst) {
        Ok(m) if m.is_dir() => dst = dst.join(src.file_name().unwrap_or_default()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return eprintln!("cp: cannot stat '{}': {}", dst.display(), strerror(&e)),
    }
    let mut input = match std::fs::File::open(src) {
        Ok(f) => f,
        Err(e) => return eprintln!("cp: cannot open '{}' for reading: {}", src.display(), strerror(&e)),
    };
    let mode = input.metadata().map(|m| m.mode() & 0o777).unwrap_or(0o644);
    let open = |p: &Path| std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(p);
    let mut out = match open(&dst) {
        Ok(f) => f,
        Err(_) => {
            let _ = std::fs::remove_file(&dst);
            match open(&dst) {
                Ok(f) => f,
                Err(e) => return eprintln!("cp: cannot create regular file '{}': {}", dst.display(), strerror(&e)),
            }
        }
    };
    if let Err(e) = std::io::copy(&mut input, &mut out) {
        eprintln!("cp: error writing '{}': {}", dst.display(), strerror(&e));
    }
}

// ---------------------------------------------------------------- the monitor log

/// `tail -n 2000 $events >$events.tmp 2>/dev/null && mv -f $events.tmp $events`, when the log is a file.
fn trim_events(events: &OsStr) {
    if !Path::new(events).is_file() {
        return;
    }
    let tmp = cat(events, ".tmp");
    let mut f = match std::fs::File::create(&tmp) {
        Ok(f) => f,
        Err(e) => return eprintln!("{ME}: {}: {}", Path::new(&tmp).display(), strerror(&e)),
    };
    let Ok(data) = std::fs::read(events) else { return };
    if f.write_all(last_lines(&data, EVENTS_KEPT)).is_err() {
        return;
    }
    drop(f);
    if let Err(e) = std::fs::rename(&tmp, events) {
        eprintln!("mv: cannot move '{}' to '{}': {}", Path::new(&tmp).display(), Path::new(events).display(), strerror(&e));
    }
}

/// `tail -n N`: the last N lines (one without a newline at the end counts).
fn last_lines(data: &[u8], n: usize) -> &[u8] {
    let body = data.strip_suffix(b"\n").unwrap_or(data);
    let start = body.iter().enumerate().rev().filter(|(_, &b)| b == b'\n').nth(n - 1).map_or(0, |(i, _)| i + 1);
    &data[start..]
}

/// `echo "$(date '+%F %T') WHAT" >>$events`
fn append(events: &OsStr, what: &str) {
    let line = format!("{} {what}\n", run::local_time("%F %T"));
    if let Err(e) = std::fs::OpenOptions::new().append(true).create(true).open(events).and_then(|mut f| f.write_all(line.as_bytes())) {
        eprintln!("{ME}: {}: {}", Path::new(events).display(), strerror(&e));
    }
}

struct Monitor {
    pid: Option<u32>,
    stop: Arc<AtomicBool>,
}

impl Monitor {
    /// The script's `pkill -P` and `kill`: udevadm is told to stop (SIGTERM) and nothing more is written.
    fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(pid) = self.pid {
            // SAFETY: kill with a pid of our own child and a signal number
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
    }
}

/// `udevadm monitor --kernel --subsystem-match=drm 2>/dev/null | while read line; [[ $line == KERNEL* ]] && echo "$(date) $line"`
/// into the monitor log, in the background (input from /dev/null, Ctrl+C and Ctrl+\ ignored, as a shell's background job).
fn watch_monitors(events: &OsStr) -> Monitor {
    let stop = Arc::new(AtomicBool::new(false));
    let log = match std::fs::OpenOptions::new().append(true).create(true).open(events) {
        Ok(f) => Some(f),
        Err(e) => {
            eprintln!("{ME}: {}: {}", Path::new(events).display(), strerror(&e));
            None
        }
    };
    let mut c = Command::new("udevadm");
    c.args(["monitor", "--kernel", "--subsystem-match=drm"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    // SAFETY: signal is async-signal-safe
    unsafe {
        c.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            libc::signal(libc::SIGQUIT, libc::SIG_IGN);
            Ok(())
        });
    }
    let Ok(mut child) = c.spawn() else { return Monitor { pid: None, stop } };
    let pid = child.id();
    let out = child.stdout.take();
    let stopped = stop.clone();
    let _ = std::thread::Builder::new().stack_size(64 * 1024).spawn(move || {
        // without the log there is no reader: udevadm ends at its first line, as it did behind the script's failed `>>`
        let (Some(out), Some(mut f)) = (out, log) else {
            let _ = child.wait();
            return;
        };
        let mut r = std::io::BufReader::new(out);
        let mut line = vec![];
        // only whole lines, as `read` gives them
        while matches!(r.read_until(b'\n', &mut line), Ok(n) if n > 0) && line.ends_with(b"\n") && !stopped.load(Ordering::SeqCst) {
            if line.starts_with(b"KERNEL") {
                let mut out = format!("{} ", run::local_time("%F %T")).into_bytes();
                out.extend(line.iter().filter(|&&b| b != 0));
                let _ = f.write_all(&out);
            }
            line.clear();
        }
        let _ = child.wait();
    });
    Monitor { pid: Some(pid), stop }
}

// ---------------------------------------------------------------- the small things

/// `systemd-cat -t yggdrasil-session echo MESSAGE 2>/dev/null || echo "yggdrasil-session: MESSAGE" >&2`
fn log(msg: &[u8]) {
    let ok = Command::new("systemd-cat")
        .args(["-t", ME, "echo"])
        .arg(OsStr::from_bytes(msg))
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        let mut e = std::io::stderr().lock();
        let _ = e.write_all(&[ME.as_bytes(), b": ", msg, b"\n"].concat());
    }
}

/// `mkdir -p DIR`, its errors as GNU mkdir words them (the part of the path that failed).
fn mkdir_p(dir: &OsStr) {
    let bytes = dir.as_bytes();
    let mut ends: Vec<usize> =
        bytes.iter().enumerate().filter(|&(i, &b)| b == b'/' && i > 0 && bytes[i - 1] != b'/').map(|(i, _)| i).collect();
    if !bytes.ends_with(b"/") {
        ends.push(bytes.len());
    }
    let last = ends.len().saturating_sub(1);
    for (k, &end) in ends.iter().enumerate() {
        let part = Path::new(OsStr::from_bytes(&bytes[..end]));
        let err = match std::fs::create_dir(part) {
            Ok(()) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => match std::fs::metadata(part) {
                Ok(m) if m.is_dir() => continue,
                Ok(_) if k < last => std::io::Error::from_raw_os_error(libc::ENOTDIR),
                _ => e,
            },
            Err(e) => e,
        };
        eprintln!("mkdir: cannot create directory '{}': {}", part.display(), strerror(&err));
        return;
    }
}

/// `exec >FILE 2>&1`; when FILE cannot be opened, said, and the output stays where it was.
fn redirect_output(file: &OsStr) {
    match std::fs::File::create(file) {
        Ok(f) => {
            use std::os::fd::AsRawFd;
            let _ = std::io::stdout().flush();
            // SAFETY: dup2 onto the standard descriptors from a file that is open
            unsafe {
                libc::dup2(f.as_raw_fd(), 1);
                libc::dup2(f.as_raw_fd(), 2);
            }
        }
        Err(e) => eprintln!("{ME}: {}: {}", Path::new(file).display(), strerror(&e)),
    }
}

/// An error as the C library words it, without Rust's "(os error N)".
fn strerror(e: &std::io::Error) -> String {
    let s = e.to_string();
    s.split(" (os error").next().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_as_sed_read_it() {
        assert_eq!(accent_numbers(b"accent = 124, 84,214"), Some([&b"124"[..], b"84", b"214"]));
        assert_eq!(accent_numbers(b" \taccent=1,2,3 # and more"), Some([&b"1"[..], b"2", b"3"]));
        assert_eq!(accent_numbers(b"accent = 010 , 08,300x"), Some([&b"010"[..], b"08", b"300"]));
        assert_eq!(accent_numbers(b"accent_color = 1,2,3"), None);
        assert_eq!(accent_numbers(b"accent = 1,2"), None);
        assert_eq!(accent_numbers(b"accent = #7c54d6"), None);
        assert_eq!(accent_numbers(b"# accent = 1,2,3"), None);
    }

    #[test]
    fn hex_as_bash_printf_gives_it() {
        assert_eq!(hex02(b"124"), "7c");
        assert_eq!(hex02(b"0"), "00");
        assert_eq!(hex02(b"00"), "00");
        assert_eq!(hex02(b"010"), "08");
        assert_eq!(hex02(b"08"), "00");
        assert_eq!(hex02(b"0778"), "3f");
        assert_eq!(hex02(b"300"), "12c");
        assert_eq!(hex02(b"18446744073709551615"), "ffffffffffffffff");
        assert_eq!(hex02(b"99999999999999999999"), "ffffffffffffffff");
        assert_eq!(hex02(b"018446744073709551616"), "01");
    }

    #[test]
    fn crashes_older_than_ten_minutes_do_not_count() {
        assert_eq!(recent(&[], 1000), [1000]);
        assert_eq!(recent(&[100, 400, 401], 1000), [401, 1000]);
        assert_eq!(recent(&[990, 995], 1000).len(), CRASHES_FOR_SAFE_MODE);
        // a clock set back counts the crash as now
        assert_eq!(recent(&[2000], 1000), [2000, 1000]);
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        assert_eq!(last_lines(b"a\nb\nc\n", 2), b"b\nc\n");
        assert_eq!(last_lines(b"a\nb\nc", 2), b"b\nc");
        assert_eq!(last_lines(b"a\n", 2), b"a\n");
        assert_eq!(last_lines(b"", 2), b"");
        assert_eq!(last_lines(b"\n\n\n", 2), b"\n\n");
    }
}
