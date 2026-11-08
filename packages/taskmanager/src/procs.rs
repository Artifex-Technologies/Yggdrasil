//! Processes, read from /proc: what each one is, and what it used between two looks (processor, memory, disk).
//!
//! Memory is the process's own (resident minus shared, i.e. RssAnon: what it alone holds, like Windows' private working set), so the memory of an app's
//! helper processes adds up without counting shared libraries twice. Disk is what it read and wrote through the block layer
//! (/proc/PID/io), which Linux shows only for your own processes and to root.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    pub name: String,
    pub state: char,
    pub ppid: u32,
    /// utime + stime, in clock ticks.
    pub ticks: u64,
    /// cutime + cstime: children it has waited for.
    pub child_ticks: u64,
    pub nice: i64,
    pub threads: u32,
    /// When it started, in clock ticks after boot.
    pub start: u64,
}

/// One /proc/PID/stat line. The name sits in parentheses and may hold spaces and parentheses itself, so the fields after it
/// are counted from the last ')'.
pub fn parse_stat(s: &str) -> Option<Stat> {
    let open = s.find('(')?;
    let close = s.rfind(')')?;
    let name = s.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = s.get(close + 1..)?.split_whitespace().collect();
    // rest[0] is field 3 (the state), so field n is rest[n - 3]
    let f = |n: usize| rest.get(n - 3).and_then(|v| v.parse::<i64>().ok());
    Some(Stat {
        name,
        state: rest.first()?.chars().next()?,
        ppid: f(4)? as u32,
        ticks: (f(14)? + f(15)?).max(0) as u64,
        child_ticks: (f(16)? + f(17)?).max(0) as u64,
        nice: f(19)?,
        threads: f(20)?.max(0) as u32,
        start: f(22)?.max(0) as u64,
    })
}

/// Pages a process holds on its own, from /proc/PID/statm: resident minus shared (shared = file-backed and shared memory),
/// which is RssAnon, what `status` would say, without reading the whole status file.
pub fn parse_statm(s: &str) -> Option<u64> {
    let f: Vec<u64> = s.split_whitespace().take(3).filter_map(|x| x.parse().ok()).collect();
    Some(f.get(1)?.saturating_sub(*f.get(2)?))
}

/// (bytes read, bytes written) from /proc/PID/io.
pub fn parse_io(s: &str) -> Option<(u64, u64)> {
    let mut r = None;
    let mut w = None;
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("read_bytes:") {
            r = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("write_bytes:") {
            w = v.trim().parse().ok();
        }
    }
    Some((r?, w?))
}

/// Account names by uid, from /etc/passwd.
pub fn parse_passwd(s: &str) -> HashMap<u32, String> {
    s.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            Some((f.get(2)?.parse().ok()?, f.first()?.to_string()))
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// What it is called: the kernel's short name, or the program's file name when that was cut at 15 letters.
    pub name: String,
    pub cmd: String,
    pub exe: String,
    pub uid: u32,
    pub user: String,
    pub state: char,
    pub threads: u32,
    pub nice: i64,
    /// Seconds after boot it started.
    pub started: f64,
    /// Share of the whole processor (all cores) since the last look, 0-100.
    pub cpu: f32,
    /// Its own memory, bytes.
    pub mem: u64,
    /// Disk read + written since the last look, bytes per second (0 when Linux does not show it).
    pub disk: f64,
    /// Processor time so far, seconds (with the children it waited for).
    pub cpu_time: f64,
    pub read: u64,
    pub written: u64,
    /// A kernel thread (no program of its own).
    pub kernel: bool,
}

impl Proc {
    pub fn state_text(&self) -> &'static str {
        match self.state {
            'R' => "Running",
            'S' => "Sleeping",
            'D' => "Waiting for the disk",
            'T' | 't' => "Suspended",
            'Z' => "Ended (zombie)",
            'I' => "Idle",
            _ => "Other",
        }
    }
}

/// The display name: the kernel keeps 15 letters, so a longer one comes from the program's path.
pub fn display_name(comm: &str, cmdline: &str, exe: &str) -> String {
    if comm.len() < 15 {
        return comm.to_string();
    }
    let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let arg0 = cmdline.split('\0').next().unwrap_or("");
    for cand in [base(arg0.split(' ').next().unwrap_or("")), base(exe)] {
        if cand.starts_with(comm) {
            return cand;
        }
    }
    comm.to_string()
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

pub struct Sampler {
    /// pid -> (start, ticks, read + written) at the last look.
    prev: HashMap<u32, (u64, u64, u64)>,
    at: Option<Instant>,
    /// (pid, start) -> (command line, program): read once per process.
    fixed: HashMap<(u32, u64), (String, String)>,
    users: HashMap<u32, String>,
    tck: f64,
    ncpu: f64,
    page: u64,
    me: u32,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Sampler {
        let tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let ncpu = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
        Sampler {
            prev: HashMap::new(),
            at: None,
            fixed: HashMap::new(),
            users: read("/etc/passwd").map(|s| parse_passwd(&s)).unwrap_or_default(),
            tck: if tck > 0 { tck as f64 } else { 100.0 },
            ncpu: if ncpu > 0 { ncpu as f64 } else { 1.0 },
            page: (unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).max(4096) as u64,
            me: unsafe { libc::getuid() },
        }
    }

    pub fn user(&self, uid: u32) -> String {
        self.users.get(&uid).cloned().unwrap_or_else(|| uid.to_string())
    }

    /// Every process now, with what it used since the last call.
    pub fn sample(&mut self) -> Vec<Proc> {
        let now = Instant::now();
        let dt = self.at.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
        let mut out = Vec::with_capacity(512);
        let mut prev = HashMap::with_capacity(self.prev.len());
        let mut live = HashSet::new();
        let Ok(dir) = std::fs::read_dir("/proc") else { return out };
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            let Some(st) = read(&format!("/proc/{pid}/stat")).and_then(|s| parse_stat(&s)) else { continue };
            let kernel = pid == 2 || st.ppid == 2;
            // the owner from the folder itself and memory from statm: much cheaper than reading the whole status file
            let uid = std::os::unix::fs::MetadataExt::uid(&match e.metadata() { Ok(m) => m, Err(_) => continue });
            let mem = if kernel { 0 } else { read(&format!("/proc/{pid}/statm")).and_then(|s| parse_statm(&s)).unwrap_or(0) * self.page };
            let key = (pid, st.start);
            live.insert(key);
            let (cmd, exe) = self
                .fixed
                .entry(key)
                .or_insert_with(|| {
                    let cmd = read(&format!("/proc/{pid}/cmdline")).unwrap_or_default();
                    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.to_string_lossy().trim_end_matches(" (deleted)").to_string()).unwrap_or_default();
                    (cmd, exe)
                })
                .clone();
            // Linux shows another account's disk use only to root: not asked for
            let (read_b, written) = if kernel || (uid != self.me && self.me != 0) { (0, 0) } else { read(&format!("/proc/{pid}/io")).and_then(|s| parse_io(&s)).unwrap_or((0, 0)) };
            let io = read_b + written;
            let (cpu, disk) = match self.prev.get(&pid) {
                Some(&(start, ticks, io0)) if start == st.start && dt > 0.0 => {
                    let c = st.ticks.saturating_sub(ticks) as f64 / self.tck / dt / self.ncpu * 100.0;
                    (c.min(100.0) as f32, io.saturating_sub(io0) as f64 / dt)
                }
                _ => (0.0, 0.0),
            };
            prev.insert(pid, (st.start, st.ticks, io));
            out.push(Proc {
                pid,
                ppid: st.ppid,
                name: display_name(&st.name, &cmd, &exe),
                cmd: cmd.split('\0').filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" "),
                exe,
                uid,
                user: self.user(uid),
                state: st.state,
                threads: st.threads,
                nice: st.nice,
                started: st.start as f64 / self.tck,
                cpu,
                mem,
                disk,
                cpu_time: (st.ticks + st.child_ticks) as f64 / self.tck,
                read: read_b,
                written,
                kernel,
            });
        }
        self.fixed.retain(|k, _| live.contains(k));
        self.prev = prev;
        self.at = Some(now);
        out
    }
}

/// Children of each process.
pub fn children(procs: &[Proc]) -> HashMap<u32, Vec<u32>> {
    let mut m: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        if p.pid != p.ppid {
            m.entry(p.ppid).or_default().push(p.pid);
        }
    }
    m
}

/// Every process below `root` (not `root` itself), children before grandchildren.
pub fn descendants(root: u32, kids: &HashMap<u32, Vec<u32>>) -> Vec<u32> {
    let mut out = vec![];
    let mut todo = vec![root];
    let mut seen = HashSet::from([root]);
    while let Some(p) = todo.pop() {
        for &c in kids.get(&p).map(|v| v.as_slice()).unwrap_or(&[]) {
            if seen.insert(c) {
                out.push(c);
                todo.push(c);
            }
        }
    }
    out
}

/// A window on the desktop (from Hyprland).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub address: String,
    pub pid: u32,
    pub class: String,
    pub title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// Has a window.
    App,
    /// Yours, without a window.
    Background,
    /// Another account's (root and the system's own).
    System,
}

/// A row of the Processes tab: an app with all its helper processes, or one process (with any children of the same name).
#[derive(Clone, Debug, Default)]
pub struct Group {
    pub kind: Option<Kind>,
    pub name: String,
    /// Window class, for the app's icon and name.
    pub class: String,
    pub root: u32,
    /// The root first, then the rest.
    pub pids: Vec<u32>,
    pub windows: Vec<Window>,
    pub cpu: f32,
    pub mem: u64,
    pub disk: f64,
    pub threads: u32,
    pub cpu_time: f64,
    pub read: u64,
    pub written: u64,
    pub started: f64,
    pub uid: u32,
    pub suspended: bool,
}

impl Group {
    pub fn kind(&self) -> Kind {
        self.kind.unwrap_or(Kind::Background)
    }
}

/// Sorts the processes into apps (a window's process and everything under it), your background processes and the system's.
/// `name_of` turns a window class (with its title and the program) into the app's name.
pub fn group(procs: &[Proc], windows: &[Window], my_uid: u32, name_of: &dyn Fn(&str, &str, &str) -> String) -> Vec<Group> {
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let kids = children(procs);
    // an app's root is a window's process, unless a process above it already has a window (helpers that open their own)
    let window_pids: HashSet<u32> = windows.iter().map(|w| w.pid).filter(|p| by_pid.contains_key(p)).collect();
    let has_app_above = |mut pid: u32| {
        let mut hops = 0;
        while let Some(p) = by_pid.get(&pid) {
            if p.ppid == pid || p.ppid == 0 || hops > 64 {
                return false;
            }
            pid = p.ppid;
            hops += 1;
            if window_pids.contains(&pid) {
                return true;
            }
        }
        false
    };
    let mut roots: Vec<u32> = window_pids.iter().copied().filter(|&p| !has_app_above(p)).collect();
    roots.sort_unstable();
    let mut claimed: HashSet<u32> = HashSet::new();
    let mut out = vec![];
    for &root in &roots {
        let mut pids = vec![root];
        pids.extend(descendants(root, &kids));
        claimed.extend(pids.iter().copied());
        let wins: Vec<Window> = windows.iter().filter(|w| pids.contains(&w.pid)).cloned().collect();
        let class = wins.iter().find(|w| w.pid == root).or(wins.first()).map(|w| w.class.clone()).unwrap_or_default();
        let title = wins.first().map(|w| w.title.clone()).unwrap_or_default();
        let p = by_pid[&root];
        let mut g = Group { kind: Some(Kind::App), name: name_of(&class, &title, &p.exe), class, root, windows: wins, started: p.started, uid: p.uid, ..Default::default() };
        if g.name.is_empty() {
            g.name = p.name.clone();
        }
        g.pids = pids;
        out.push(g);
    }
    // the rest one by one, with children of the same name folded in (a program's own worker processes)
    let mut rest: Vec<&Proc> = procs.iter().filter(|p| !p.kernel && !claimed.contains(&p.pid)).collect();
    rest.sort_by_key(|p| p.pid);
    let mut folded: HashSet<u32> = HashSet::new();
    for p in &rest {
        if folded.contains(&p.pid) {
            continue;
        }
        if let Some(parent) = by_pid.get(&p.ppid) {
            if parent.name == p.name && parent.uid == p.uid && !claimed.contains(&parent.pid) && !parent.kernel {
                continue; // folded into its parent below
            }
        }
        let mut pids = vec![p.pid];
        let mut todo = vec![p.pid];
        while let Some(x) = todo.pop() {
            for &c in kids.get(&x).map(|v| v.as_slice()).unwrap_or(&[]) {
                if let Some(cp) = by_pid.get(&c) {
                    if cp.name == p.name && cp.uid == p.uid && !claimed.contains(&c) && folded.insert(c) {
                        pids.push(c);
                        todo.push(c);
                    }
                }
            }
        }
        let kind = if p.uid == my_uid { Kind::Background } else { Kind::System };
        out.push(Group { kind: Some(kind), name: p.name.clone(), root: p.pid, pids, started: p.started, uid: p.uid, ..Default::default() });
    }
    for g in &mut out {
        let mut all_stopped = true;
        for pid in &g.pids {
            if let Some(p) = by_pid.get(pid) {
                g.cpu += p.cpu;
                g.mem += p.mem;
                g.disk += p.disk;
                g.threads += p.threads;
                g.cpu_time += p.cpu_time;
                g.read += p.read;
                g.written += p.written;
                all_stopped &= matches!(p.state, 'T' | 't');
            }
        }
        g.suspended = all_stopped && !g.pids.is_empty();
    }
    out
}

/// Sends `sig` to each process; the ones it could not reach (another account's) are returned.
pub fn signal(pids: &[u32], sig: i32) -> Vec<u32> {
    pids.iter().copied().filter(|&p| p > 1 && unsafe { libc::kill(p as libc::pid_t, sig) } != 0).collect()
}

/// Names that end the desktop session or stop the whole system when they end: asked about first.
pub const CRITICAL: [&str; 12] = ["systemd", "Hyprland", "Xwayland", "dbus-broker", "dbus-daemon", "dbus-broker-lau", "pipewire", "wireplumber", "greetd", "yggdrasil-panel", "start-hyprland", "yggdrasil-sessi"];

pub fn is_critical(p: &Proc) -> bool {
    p.pid == 1 || CRITICAL.iter().any(|c| p.name == *c || p.name.starts_with("yggdrasil-session"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_lines_with_odd_names() {
        let s = "1234 (Web Content (x)) S 1000 1234 1234 0 -1 4194560 100 0 0 0 250 50 3 2 20 0 7 0 98765 1000000 300 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0";
        let st = parse_stat(s).unwrap();
        assert_eq!(st.name, "Web Content (x)");
        assert_eq!((st.state, st.ppid, st.ticks, st.child_ticks, st.nice, st.threads, st.start), ('S', 1000, 300, 5, 0, 7, 98765));
        assert!(parse_stat("12 (x").is_none());
        assert!(parse_stat("").is_none());
    }

    #[test]
    fn statm_io_and_passwd() {
        assert_eq!(parse_io("rchar: 5\nwchar: 6\nread_bytes: 4096\nwrite_bytes: 8192\n"), Some((4096, 8192)));
        assert_eq!(parse_statm("12000 5000 1200 10 0 3000 0\n"), Some(3800));
        assert_eq!(parse_statm("1 2"), None);
        assert_eq!(parse_io("rchar: 5\n"), None);
        let users = parse_passwd("root:x:0:0::/root:/bin/bash\nme:x:1000:1000::/home/me:/bin/bash\nbroken\n");
        assert_eq!((users[&0].as_str(), users[&1000].as_str()), ("root", "me"));
    }

    #[test]
    fn long_names_come_from_the_program() {
        assert_eq!(display_name("bash", "/usr/bin/bash\0", "/usr/bin/bash"), "bash");
        assert_eq!(display_name("yggdrasil-panel", "/usr/bin/yggdrasil-panel\0", "/usr/bin/yggdrasil-panel"), "yggdrasil-panel");
        assert_eq!(display_name("labyrinth-sarin", "/usr/local/bin/labyrinth-sarina-speak\0x\0", ""), "labyrinth-sarina-speak");
        assert_eq!(display_name("labyrinth-sarin", "", "/usr/bin/python3"), "labyrinth-sarin");
    }

    fn p(pid: u32, ppid: u32, name: &str, uid: u32, cpu: f32, mem: u64) -> Proc {
        Proc { pid, ppid, name: name.into(), uid, cpu, mem, state: 'S', threads: 1, ..Default::default() }
    }

    #[test]
    fn apps_take_their_helpers_and_the_rest_stand_alone() {
        let procs = vec![
            p(1, 0, "systemd", 0, 0.1, 10),
            p(900, 1, "systemd", 1000, 0.0, 20),
            p(1000, 900, "code", 1000, 2.0, 100),
            p(1001, 1000, "code", 1000, 3.0, 200),
            p(1002, 1001, "code", 1000, 1.0, 300),
            p(1003, 1000, "bash", 1000, 0.0, 5), // a terminal inside the editor: part of the app
            p(1100, 900, "ghostty", 1000, 0.5, 50),
            p(1101, 1100, "bash", 1000, 0.0, 5),
            p(1200, 900, "pipewire", 1000, 0.2, 30),
            p(1300, 1, "sshd", 0, 0.0, 7),
            p(1301, 1300, "sshd", 0, 0.0, 3),
            Proc { pid: 2, ppid: 0, name: "kthreadd".into(), kernel: true, ..Default::default() },
        ];
        let wins = vec![
            Window { address: "0xa".into(), pid: 1000, class: "code".into(), title: "x".into() },
            Window { address: "0xb".into(), pid: 1002, class: "code".into(), title: "y".into() },
            Window { address: "0xc".into(), pid: 1100, class: "ghostty".into(), title: "~".into() },
        ];
        let names = |class: &str, _t: &str, _e: &str| match class {
            "code" => "Visual Studio Code".to_string(),
            _ => "Ghostty".to_string(),
        };
        let g = group(&procs, &wins, 1000, &names);
        let code = g.iter().find(|g| g.name == "Visual Studio Code").unwrap();
        assert_eq!(code.kind(), Kind::App);
        assert_eq!(code.pids.len(), 4, "the editor, its helpers and the shell under it");
        assert_eq!(code.windows.len(), 2, "a helper's own window stays with the app");
        assert_eq!((code.cpu, code.mem), (6.0, 605));
        let term = g.iter().find(|g| g.name == "Ghostty").unwrap();
        assert_eq!(term.pids, vec![1100, 1101]);
        assert!(g.iter().any(|g| g.name == "pipewire" && g.kind() == Kind::Background));
        let sshd: Vec<_> = g.iter().filter(|g| g.name == "sshd").collect();
        assert_eq!(sshd.len(), 1, "a child of the same name folds into its parent");
        assert_eq!((sshd[0].kind(), sshd[0].pids.len()), (Kind::System, 2));
        assert!(!g.iter().any(|g| g.name == "kthreadd"), "kernel threads are not listed");
        assert_eq!(g.iter().filter(|g| g.name == "systemd").count(), 2, "systemd and the user's systemd: different accounts");
    }

    #[test]
    fn descendants_go_all_the_way_down_and_survive_loops() {
        let procs = vec![p(10, 1, "a", 0, 0.0, 0), p(11, 10, "b", 0, 0.0, 0), p(12, 11, "c", 0, 0.0, 0), p(13, 13, "self", 0, 0.0, 0)];
        let kids = children(&procs);
        let mut d = descendants(10, &kids);
        d.sort();
        assert_eq!(d, vec![11, 12]);
        assert!(descendants(13, &kids).is_empty());
    }

    #[test]
    fn the_own_process_is_read() {
        let mut s = Sampler::new();
        let first = s.sample();
        let me = first.iter().find(|p| p.pid == std::process::id()).expect("this test's own process");
        assert_eq!(me.uid, unsafe { libc::getuid() });
        assert!(me.mem > 0 && !me.exe.is_empty() && me.threads >= 1);
        assert!(first.iter().all(|p| p.cpu == 0.0), "no CPU share before a second look");
        let _ = s.sample();
    }

    #[test]
    fn critical_ones() {
        assert!(is_critical(&p(1, 0, "systemd", 0, 0.0, 0)));
        assert!(is_critical(&p(500, 1, "Hyprland", 1000, 0.0, 0)));
        assert!(!is_critical(&p(600, 1, "vivaldi-bin", 1000, 0.0, 0)));
    }
}
