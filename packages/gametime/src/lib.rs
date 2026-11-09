//! Game time: while a game runs, background chores (definition updates, clean-ups, catalogues) wait and catch up afterwards;
//! security that watches (the leak watch, the downloads virus scan, Heimdall) keeps running. Nothing stays in memory: systemd
//! asks `yggdrasil-gametime no-game UNIT` before a chore starts (`ExecCondition=`), and a timer runs `catch-up` every 5 minutes.
//!
//! A game is running when one of these is found in /proc (no program is started to find out):
//! - Steam's launcher, `reaper SteamLaunch AppId=N -- ...`: the game's whole process tree lives under it;
//! - gamescope;
//! - Lutris's `lutris-wrapper`, which runs every Lutris game;
//! - a program named in /etc/yggdrasil/gametime.conf (`games = name, name`).
//!
//! Each rule looks at the program itself (its argv[0] or its kernel name), never at its arguments, so a grep, a shell or an
//! editor whose command line only mentions these words is never taken for a game. Sarina's GameRunning (pgrep -f) was fooled
//! by its own pgrep that way. The calling process is skipped too.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The settings file: `games = name, name` adds programs that count as a game.
pub const CONFIG: &str = "/etc/yggdrasil/gametime.conf";
/// Where the system manager's waiting chores are noted (a user manager's are in $XDG_RUNTIME_DIR/yggdrasil-gametime).
pub const SYSTEM_DIR: &str = "/run/yggdrasil-gametime";
/// The list of waiting units, one per line, in that folder.
pub const LIST: &str = "deferred";
/// The list a catch-up has taken and is starting (kept apart so chores deferred meanwhile are not lost).
pub const TAKEN: &str = "deferred.taken";
/// Only the start of a command line is needed; a huge one (a JVM's classpath) is not read whole.
const CMDLINE_CAP: u64 = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Steam,
    Gamescope,
    Lutris,
    Named,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Steam => "steam",
            Kind::Gamescope => "gamescope",
            Kind::Lutris => "lutris",
            Kind::Named => "named",
        }
    }
}

/// The game that was found: which rule, which process, and the AppId, Lutris title or configured name ("" when none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub kind: Kind,
    pub pid: u32,
    pub detail: String,
}

impl Game {
    /// Why this counts as a game, for a log line or Settings.
    pub fn reason(&self) -> String {
        match self.kind {
            Kind::Steam => format!("Steam is running a game (AppId {}, process {})", self.detail, self.pid),
            Kind::Gamescope => format!("gamescope is running (process {})", self.pid),
            Kind::Lutris if self.detail.is_empty() => format!("Lutris is running a game (process {})", self.pid),
            Kind::Lutris => format!("Lutris is running {} (process {})", self.detail, self.pid),
            Kind::Named => format!("{} is running (process {}; a game named in {CONFIG})", self.detail, self.pid),
        }
    }
}

/// argv from /proc/PID/cmdline: NUL-separated. The NUL at the end, and the NUL padding a renamed process
/// (setproctitle) leaves behind, are dropped.
pub fn parse_cmdline(raw: &[u8]) -> Vec<String> {
    let mut argv: Vec<String> = raw.split(|&b| b == 0).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
    while argv.last().is_some_and(|a| a.is_empty()) {
        argv.pop();
    }
    argv
}

/// The last part of a path, Unix or Windows (a Wine program's argv[0] is often `C:\Games\x\game.exe`).
pub fn base(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn named<'a>(prog: &str, names: &'a [String]) -> Option<&'a String> {
    names.iter().find(|n| !n.is_empty() && prog.eq_ignore_ascii_case(n))
}

/// The rules that look at a process's command line. Only argv[0] says what the program is; the other arguments are read
/// only once argv[0] is Steam's reaper or a Python running lutris-wrapper.
pub fn classify_argv(argv: &[String], names: &[String]) -> Option<(Kind, String)> {
    let a0 = argv.first()?;
    let prog = base(a0);
    if prog == "reaper" {
        // reaper's own options come before "--"; after it is the game's command line
        let (mut launch, mut app) = (false, None);
        for a in argv[1..].iter().take_while(|a| a.as_str() != "--") {
            if a == "SteamLaunch" {
                launch = true;
            } else if let Some(id) = a.strip_prefix("AppId=") {
                if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                    app = Some(id.to_string());
                }
            }
        }
        if let (true, Some(id)) = (launch, app) {
            return Some((Kind::Steam, id));
        }
    }
    if prog == "gamescope" {
        return Some((Kind::Gamescope, String::new()));
    }
    // lutris-wrapper renames itself "lutris-wrapper: TITLE" when setproctitle is installed; otherwise it is a Python script
    if let Some(title) = a0.strip_prefix("lutris-wrapper:") {
        return Some((Kind::Lutris, title.trim().to_string()));
    }
    if prog == "lutris-wrapper" {
        return Some((Kind::Lutris, argv.get(1).cloned().unwrap_or_default()));
    }
    if prog.starts_with("python") && argv.get(1).is_some_and(|a| base(a) == "lutris-wrapper") {
        return Some((Kind::Lutris, argv.get(2).cloned().unwrap_or_default()));
    }
    named(prog, names).map(|n| (Kind::Named, n.clone()))
}

/// The rules that look at the kernel's name for a process (/proc/PID/comm, at most 15 bytes), for programs whose argv[0]
/// says something else. Nobody can give a grep this name by quoting a word.
pub fn classify_comm(comm: &str, names: &[String]) -> Option<(Kind, String)> {
    if comm == "gamescope" {
        return Some((Kind::Gamescope, String::new()));
    }
    if comm.starts_with("lutris-wrapper") {
        return Some((Kind::Lutris, String::new()));
    }
    if let Some(n) = named(comm, names) {
        return Some((Kind::Named, n.clone()));
    }
    // the kernel keeps 15 bytes of a name: "Cyberpunk2077.exe" is "Cyberpunk2077.e"
    if comm.len() == 15 {
        if let Some(n) = names.iter().find(|n| n.len() > 15 && n.is_char_boundary(15) && n[..15].eq_ignore_ascii_case(comm)) {
            return Some((Kind::Named, n.clone()));
        }
    }
    None
}

fn read_capped(path: &Path, cap: u64) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    fs::File::open(path).ok()?.take(cap).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Looks through a /proc tree for a running game. `skip_pid` is the caller's own process. Processes with no command line
/// (kernel threads, and zombies whose program has already ended) are passed over. Lowest process number first.
pub fn find_game(proc_root: &Path, names: &[String], skip_pid: u32) -> Option<Game> {
    let mut pids: Vec<u32> = fs::read_dir(proc_root)
        .ok()?
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    pids.sort_unstable();
    for pid in pids {
        if pid == skip_pid {
            continue;
        }
        let dir = proc_root.join(pid.to_string());
        let Some(raw) = read_capped(&dir.join("cmdline"), CMDLINE_CAP) else { continue };
        let argv = parse_cmdline(&raw);
        if argv.is_empty() {
            continue;
        }
        let found = classify_argv(&argv, names).or_else(|| {
            let comm = fs::read_to_string(dir.join("comm")).ok()?;
            classify_comm(comm.trim_end_matches('\n'), names)
        });
        if let Some((kind, detail)) = found {
            return Some(Game { kind, pid, detail });
        }
    }
    None
}

/// Programs that count as a game, from `games = a, b` lines of the settings file (`#` starts a comment).
pub fn game_names(conf: &str) -> Vec<String> {
    conf.lines()
        .filter_map(|l| l.split('#').next()?.split_once('='))
        .filter(|(k, _)| k.trim() == "games")
        .flat_map(|(_, v)| v.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect::<Vec<_>>())
        .collect()
}

/// A unit name as systemd gives it for %n: letters, digits and `:-_.\@`, ending in .service (ExecCondition= exists only
/// in services). Anything else is never written down or passed to systemctl.
pub fn valid_unit(unit: &str) -> bool {
    unit.len() > ".service".len()
        && unit.len() <= 255
        && unit.ends_with(".service")
        && !unit.starts_with(['-', '.'])
        && unit.bytes().all(|b| b.is_ascii_alphanumeric() || b":-_.\\@".contains(&b))
}

fn parse_list(text: &str, into: &mut Vec<String>) {
    for l in text.lines().map(str::trim) {
        if valid_unit(l) && !into.iter().any(|u| u == l) {
            into.push(l.to_string());
        }
    }
}

fn read_or_empty(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        r => r,
    }
}

/// The chores that are waiting for a game to end: one folder per service manager.
pub struct Waitlist {
    pub dir: PathBuf,
}

impl Waitlist {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Waitlist { dir: dir.into() }
    }

    fn list_file(&self) -> PathBuf {
        self.dir.join(LIST)
    }

    /// Notes a unit (once). True when it was not listed yet.
    pub fn add(&self, unit: &str) -> io::Result<bool> {
        if !valid_unit(unit) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("not a service name: {unit:?}")));
        }
        fs::DirBuilder::new().recursive(true).mode(0o755).create(&self.dir)?;
        let mut listed = Vec::new();
        parse_list(&read_or_empty(&self.list_file())?, &mut listed);
        if listed.iter().any(|u| u == unit) {
            return Ok(false);
        }
        // one short write in append mode: two chores noted at the same moment cannot mix their lines
        let mut f = fs::OpenOptions::new().create(true).append(true).mode(0o644).open(self.list_file())?;
        f.write_all(format!("{unit}\n").as_bytes())?;
        Ok(true)
    }

    /// Whether anything waits: a single stat, so a catch-up with nothing to do costs nothing.
    pub fn has_any(&self) -> bool {
        fs::symlink_metadata(self.list_file()).is_ok()
    }

    /// What waits, in the order it was noted (for `status`).
    pub fn list(&self) -> Vec<String> {
        let mut units = Vec::new();
        for f in [TAKEN, LIST] {
            parse_list(&read_or_empty(&self.dir.join(f)).unwrap_or_default(), &mut units);
        }
        units
    }

    /// Takes the list for starting. It is moved aside first, so a chore deferred while the others start is kept for the next
    /// catch-up; a list left by a catch-up that was interrupted is taken along. Call `done` once they are started.
    pub fn take(&self) -> io::Result<Vec<String>> {
        let taken = self.dir.join(TAKEN);
        let mut units = Vec::new();
        parse_list(&read_or_empty(&taken)?, &mut units);
        match fs::rename(self.list_file(), &taken) {
            Ok(()) => parse_list(&read_or_empty(&taken)?, &mut units),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(units)
    }

    /// Forgets the list `take` returned.
    pub fn done(&self) -> io::Result<()> {
        match fs::remove_file(self.dir.join(TAKEN)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

/// A string as JSON.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_list(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","))
}

/// `status --json`: {"game", "reason", "kind", "pid", "detail", "waiting": {"system": [...], "user": [...]}}.
pub fn status_json(game: Option<&Game>, system: &[String], user: &[String]) -> String {
    let waiting = format!("{{\"system\":{},\"user\":{}}}", json_list(system), json_list(user));
    match game {
        Some(g) => format!(
            "{{\"game\":true,\"reason\":{},\"kind\":{},\"pid\":{},\"detail\":{},\"waiting\":{waiting}}}",
            json_str(&g.reason()),
            json_str(g.kind.as_str()),
            g.pid,
            json_str(&g.detail)
        ),
        None => format!("{{\"game\":false,\"reason\":null,\"kind\":null,\"pid\":null,\"detail\":null,\"waiting\":{waiting}}}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// A /proc tree in a temporary folder: (pid, argv, comm). An empty argv writes an empty cmdline (a kernel thread or a zombie).
    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str, procs: &[(u32, &[&str], &str)]) -> Tree {
            let root = std::env::temp_dir().join(format!("gametime-test-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            for (pid, argv, comm) in procs {
                let d = root.join(pid.to_string());
                fs::create_dir_all(&d).unwrap();
                let mut raw = Vec::new();
                for a in argv.iter() {
                    raw.extend_from_slice(a.as_bytes());
                    raw.push(0);
                }
                fs::write(d.join("cmdline"), raw).unwrap();
                fs::write(d.join("comm"), format!("{comm}\n")).unwrap();
            }
            // what /proc also holds: names that are not processes
            fs::create_dir_all(root.join("self")).unwrap();
            fs::write(root.join("uptime"), "1.0 1.0\n").unwrap();
            Tree(root)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    const NOBODY: u32 = u32::MAX;

    #[test]
    fn cmdline_is_split_on_nul_and_the_padding_dropped() {
        assert_eq!(parse_cmdline(b"/usr/bin/gamescope\0-W\x002560\0"), s(&["/usr/bin/gamescope", "-W", "2560"]));
        assert_eq!(parse_cmdline(b"lutris-wrapper: Diablo\0\0\0\0"), s(&["lutris-wrapper: Diablo"]));
        assert!(parse_cmdline(b"").is_empty());
        assert_eq!(base("/home/player/.local/share/Steam/ubuntu12_32/reaper"), "reaper");
        assert_eq!(base(r"C:\Games\Witcher\witcher3.exe"), "witcher3.exe");
        assert_eq!(base("gamescope"), "gamescope");
    }

    #[test]
    fn steams_reaper_with_an_app_id_is_a_game() {
        let argv = s(&["/home/player/.local/share/Steam/ubuntu12_32/reaper", "SteamLaunch", "AppId=1091500", "--",
            "/home/player/.local/share/Steam/ubuntu12_32/steam-launch-wrapper", "--", "/game/bin"]);
        assert_eq!(classify_argv(&argv, &[]), Some((Kind::Steam, "1091500".into())));
    }

    #[test]
    fn a_reaper_that_launched_nothing_is_not_a_game() {
        for argv in [
            &["reaper", "SteamLaunch"][..],                                   // no AppId
            &["reaper", "AppId=10"][..],                                      // no SteamLaunch
            &["reaper", "SteamLaunch", "AppId=x10"][..],                      // not a number
            &["reaper", "SteamLaunch", "AppId="][..],                         // empty
            &["reaper", "--", "game", "SteamLaunch", "AppId=10"][..],         // only the game's own arguments say so
        ] {
            assert_eq!(classify_argv(&s(argv), &[]), None, "{argv:?}");
        }
    }

    #[test]
    fn a_command_line_that_only_quotes_the_words_is_not_a_game() {
        let names = s(&["eldenring.exe"]);
        for argv in [
            &["pgrep", "-a", "-f", "SteamLaunch AppId="][..],
            &["pgrep", "-af", "reaper SteamLaunch AppId=10"][..],
            &["/bin/bash", "-c", "reaper SteamLaunch AppId=10 -- true"][..],
            &["grep", "-r", "gamescope", "/etc"][..],
            &["vim", "/usr/bin/gamescope"][..],
            &["less", "/usr/share/lutris/bin/lutris-wrapper"][..],
            &["sh", "-c", "lutris-wrapper: Diablo"][..],
            &["nano", "eldenring.exe"][..],
            &["yggdrasil-gametime", "status"][..],
        ] {
            assert_eq!(classify_argv(&s(argv), &names), None, "{argv:?}");
        }
        // what those programs are called by the kernel
        for comm in ["pgrep", "bash", "grep", "vim", "less", "sh", "nano", "yggdrasil-gamet"] {
            assert_eq!(classify_comm(comm, &names), None, "{comm}");
        }
    }

    #[test]
    fn gamescope_by_program_or_kernel_name() {
        assert_eq!(classify_argv(&s(&["/usr/bin/gamescope", "-W", "3440", "--", "%command%"]), &[]), Some((Kind::Gamescope, String::new())));
        assert_eq!(classify_comm("gamescope", &[]), Some((Kind::Gamescope, String::new())));
        // its helper and a renamed copy are something else
        assert_eq!(classify_argv(&s(&["gamescopereaper", "--"]), &[]), None);
        assert_eq!(classify_comm("gamescopereaper", &[]), None);
    }

    #[test]
    fn lutris_wrapper_in_each_of_its_forms() {
        let python = s(&["python3", "/usr/share/lutris/bin/lutris-wrapper", "Diablo II", "0", "0", "wine", "Game.exe"]);
        assert_eq!(classify_argv(&python, &[]), Some((Kind::Lutris, "Diablo II".into())));
        let python_path = s(&["/usr/bin/python3.14", "/app/share/lutris/bin/lutris-wrapper", "Celeste", "0", "0"]);
        assert_eq!(classify_argv(&python_path, &[]), Some((Kind::Lutris, "Celeste".into())));
        assert_eq!(classify_argv(&s(&["lutris-wrapper: Diablo II"]), &[]), Some((Kind::Lutris, "Diablo II".into())));
        assert_eq!(classify_argv(&s(&["/usr/share/lutris/bin/lutris-wrapper", "Hades", "0", "0"]), &[]), Some((Kind::Lutris, "Hades".into())));
        assert_eq!(classify_comm("lutris-wrapper:", &[]), Some((Kind::Lutris, String::new())));
        // Lutris itself, open with no game, is not a game
        assert_eq!(classify_argv(&s(&["python3", "/usr/bin/lutris"]), &[]), None);
        assert_eq!(classify_argv(&s(&["python3", "-c", "import lutris-wrapper"]), &[]), None);
    }

    #[test]
    fn names_from_the_settings_file() {
        let conf = "# programs that count as a game\ngames = eldenring.exe, Cyberpunk2077.exe\ngames=heroic # Heroic's launcher\nother = x\n";
        let names = game_names(conf);
        assert_eq!(names, s(&["eldenring.exe", "Cyberpunk2077.exe", "heroic"]));
        // by argv[0], Unix or Windows path, any case
        assert_eq!(classify_argv(&s(&[r"Z:\games\ELDEN RING\Game\eldenring.exe"]), &names), Some((Kind::Named, "eldenring.exe".into())));
        assert_eq!(classify_argv(&s(&["/opt/Heroic/heroic", "--no-sandbox"]), &names), Some((Kind::Named, "heroic".into())));
        assert_eq!(classify_argv(&s(&["/usr/bin/Heroic"]), &names), Some((Kind::Named, "heroic".into())));
        // by the kernel's name, also when it was cut to 15 bytes
        assert_eq!(classify_comm("eldenring.exe", &names), Some((Kind::Named, "eldenring.exe".into())));
        assert_eq!(classify_comm("Cyberpunk2077.e", &names), Some((Kind::Named, "Cyberpunk2077.exe".into())));
        assert_eq!(classify_comm("Cyberpunk2077", &names), None);
        assert!(game_names("").is_empty());
        assert!(game_names("games =  , ,\n").is_empty());
    }

    #[test]
    fn the_committed_fixtures() {
        let none: Vec<String> = Vec::new();
        let steam = find_game(&fixture("steam"), &none, NOBODY).expect("steam fixture");
        assert_eq!((steam.kind, steam.pid, steam.detail.as_str()), (Kind::Steam, 4210, "1091500"));
        assert_eq!(steam.reason(), "Steam is running a game (AppId 1091500, process 4210)");
        let gs = find_game(&fixture("gamescope"), &none, NOBODY).expect("gamescope fixture");
        assert_eq!((gs.kind, gs.pid), (Kind::Gamescope, 5100));
        let lutris = find_game(&fixture("lutris"), &none, NOBODY).expect("lutris fixture");
        assert_eq!((lutris.kind, lutris.pid, lutris.detail.as_str()), (Kind::Lutris, 6020, "Diablo II"));
        assert_eq!(find_game(&fixture("quoting"), &none, NOBODY), None, "only mentions, no game");
        assert_eq!(find_game(&fixture("idle"), &none, NOBODY), None, "a desktop with no game");
        assert_eq!(find_game(&fixture("does-not-exist"), &none, NOBODY), None);
    }

    #[test]
    fn our_own_process_is_skipped() {
        let t = Tree::new("own", &[(77, &["reaper", "SteamLaunch", "AppId=10", "--", "x"], "reaper")]);
        assert_eq!(find_game(&t.0, &[], 77), None);
        assert_eq!(find_game(&t.0, &[], 78).map(|g| g.pid), Some(77));
    }

    #[test]
    fn kernel_threads_and_zombies_are_passed_over() {
        // a zombie keeps its kernel name but has no command line any more
        let t = Tree::new("zombie", &[(10, &[], "gamescope"), (11, &[], "kworker/0:1"), (12, &["/usr/bin/bash"], "bash")]);
        assert_eq!(find_game(&t.0, &[], NOBODY), None);
    }

    #[test]
    fn a_game_found_by_its_kernel_name_only() {
        // Wine: argv[0] is the loader, the kernel name is the game's
        let t = Tree::new("wine", &[(300, &["/usr/bin/wine64-preloader", "/usr/bin/wine64"], "eldenring.exe"), (301, &["/usr/bin/gamescope"], "gamescope")]);
        let g = find_game(&t.0, &s(&["eldenring.exe"]), NOBODY).unwrap();
        assert_eq!((g.kind, g.pid), (Kind::Named, 300), "lowest process number first");
        assert_eq!(g.reason(), format!("eldenring.exe is running (process 300; a game named in {CONFIG})"));
        // without the name, the gamescope next to it is found
        assert_eq!(find_game(&t.0, &[], NOBODY).map(|g| g.kind), Some(Kind::Gamescope));
    }

    #[test]
    fn unit_names() {
        for ok in ["fstrim.service", "clamav-freshclam-once.service", "user@1000.service", "foo\\x2dbar.service", "a:b.service"] {
            assert!(valid_unit(ok), "{ok}");
        }
        for bad in ["", ".service", "fstrim", "fstrim.timer", "-x.service", "a b.service", "../x.service", "x;rm.service",
            "a\n.service", "x/y.service"] {
            assert!(!valid_unit(bad), "{bad:?}");
        }
        assert!(!valid_unit(&format!("{}.service", "a".repeat(250))));
    }

    #[test]
    fn the_waitlist_notes_once_takes_and_forgets() {
        let dir = std::env::temp_dir().join(format!("gametime-test-{}-waitlist", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let wl = Waitlist::new(dir.join("yggdrasil-gametime"));
        assert!(!wl.has_any());
        assert!(wl.list().is_empty());
        assert_eq!(wl.take().unwrap(), Vec::<String>::new(), "nothing to take is not an error");
        assert!(wl.add("fstrim.service").unwrap());
        assert!(!wl.add("fstrim.service").unwrap(), "noted once");
        assert!(wl.add("heimdall-inventory.service").unwrap());
        assert!(wl.add("bad name").is_err());
        assert!(wl.has_any());
        assert_eq!(wl.list(), s(&["fstrim.service", "heimdall-inventory.service"]));
        let mode = fs::metadata(&wl.dir).unwrap().permissions();
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777, 0o755);

        let taken = wl.take().unwrap();
        assert_eq!(taken, s(&["fstrim.service", "heimdall-inventory.service"]));
        assert!(!wl.has_any(), "the list is moved aside while they start");
        // a chore deferred while the others start waits for the next catch-up
        wl.add("snapper-cleanup.service").unwrap();
        wl.done().unwrap();
        assert_eq!(wl.list(), s(&["snapper-cleanup.service"]));

        // an interrupted catch-up's list is taken along with the new one, without repeats
        let first = wl.take().unwrap();
        assert_eq!(first, s(&["snapper-cleanup.service"]));
        wl.add("fstrim.service").unwrap();
        wl.add("snapper-cleanup.service").unwrap();
        assert_eq!(wl.take().unwrap(), s(&["snapper-cleanup.service", "fstrim.service"]));
        wl.done().unwrap();
        wl.done().unwrap();
        assert!(wl.list().is_empty());

        // lines that are not unit names (a hand-edited list) are never handed on
        fs::write(wl.dir.join(LIST), "fstrim.service\n; rm -rf /\n\n--now.service\nfstrim.service\n").unwrap();
        assert_eq!(wl.take().unwrap(), s(&["fstrim.service"]));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_for_programs() {
        assert_eq!(json_str("a \"b\" \\ c\n\u{1}"), "\"a \\\"b\\\" \\\\ c\\n\\u0001\"");
        let g = Game { kind: Kind::Steam, pid: 42, detail: "570".into() };
        assert_eq!(
            status_json(Some(&g), &s(&["fstrim.service"]), &[]),
            "{\"game\":true,\"reason\":\"Steam is running a game (AppId 570, process 42)\",\"kind\":\"steam\",\"pid\":42,\"detail\":\"570\",\"waiting\":{\"system\":[\"fstrim.service\"],\"user\":[]}}"
        );
        assert_eq!(
            status_json(None, &[], &s(&["a.service", "b.service"])),
            "{\"game\":false,\"reason\":null,\"kind\":null,\"pid\":null,\"detail\":null,\"waiting\":{\"system\":[],\"user\":[\"a.service\",\"b.service\"]}}"
        );
    }
}
