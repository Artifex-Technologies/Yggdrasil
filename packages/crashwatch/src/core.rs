//! What a stored core says about a crash, without opening it: its file name (whose crash) and the extended attributes
//! systemd-coredump sets on it (`user.coredump.exe`, `.signal`, `.uid`, `.comm`). The crashing user may read those through the
//! ACL systemd-coredump gives them on their own cores; another user's cores are not readable, and are skipped by name anyway.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Where systemd-coredump stores cores (Storage=external, the default).
pub const CORE_DIR: &str = "/var/lib/systemd/coredump";

/// What a core file's name says: `core.COMM.UID.BOOT_ID.PID.USEC` plus `.zst`, `.xz` or `.lz4` when compressed. systemd escapes
/// COMM (a dot becomes `\x2e`); the fields are counted from the end all the same.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreName {
    /// The program's short name (the kernel's comm, at most 15 bytes), unescaped.
    pub comm: String,
    pub uid: u32,
    pub pid: u32,
}

pub fn parse_core_name(name: &str) -> Option<CoreName> {
    let rest = name.strip_prefix("core.")?;
    let rest = [".zst", ".xz", ".lz4"].iter().find_map(|x| rest.strip_suffix(x)).unwrap_or(rest);
    let mut f = rest.rsplitn(5, '.');
    let (usec, pid, boot, uid, comm) = (f.next()?, f.next()?, f.next()?, f.next()?, f.next()?);
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if comm.is_empty() || !digits(usec) || !digits(pid) || !digits(uid) || boot.len() != 32 || !boot.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(CoreName { comm: unescape(comm), uid: uid.parse().ok()?, pid: pid.parse().ok()? })
}

/// systemd's `\xHH` escapes back to bytes.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' {
            if let Some(v) = std::str::from_utf8(&b[i + 2..i + 4]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One extended attribute of a file, as text (None when it is missing or not readable).
pub fn read_xattr(path: &Path, name: &str) -> Option<String> {
    let p = CString::new(path.as_os_str().as_bytes()).ok()?;
    let n = CString::new(name).ok()?;
    let mut buf = [0u8; 4096];
    // SAFETY: the buffer is as long as said; getxattr writes at most that much
    let r = unsafe { libc::getxattr(p.as_ptr(), n.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    if r < 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[..r as usize]).trim_end_matches('\0').to_string())
}

/// What a notice says about a crash: the program (its path when known) and the signal's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crash {
    pub exe: String,
    pub signal: String,
}

/// The crash a core records, if it is `uid`'s own. `attr` reads one extended attribute of the core (read_xattr on the real
/// file). When the file is already gone (systemd-coredump removes a core over its size limits right after storing it) the
/// name still tells whose crash it was and the program's short name.
pub fn crash_of(core: &CoreName, uid: u32, attr: impl Fn(&str) -> Option<String>) -> Option<Crash> {
    if core.uid != uid {
        return None;
    }
    if attr("user.coredump.uid").is_some_and(|u| u.trim() != uid.to_string()) {
        return None;
    }
    let exe = attr("user.coredump.exe")
        .map(|e| clean_exe(e.trim_end_matches(" (deleted)")))
        .filter(|e| !e.is_empty())
        .or_else(|| attr("user.coredump.comm").map(|c| clean_exe(&c)).filter(|c| !c.is_empty()))
        .or_else(|| Some(clean_exe(&core.comm)).filter(|c| !c.is_empty()))
        .unwrap_or_else(|| "a program".to_string());
    let signal = attr("user.coredump.signal").map(|s| signal_name(&s)).unwrap_or_else(|| "a signal".to_string());
    Some(Crash { exe, signal })
}

/// The crash a journal entry of systemd-coredump records (its "process dumped core" message, with the COREDUMP_* fields), if
/// it is `uid`'s own: the crashed process's id (when the entry has one) and what a notice says. `field` reads one field.
pub fn crash_of_entry(uid: u32, field: impl Fn(&str) -> Option<String>) -> Option<(Option<u32>, Crash)> {
    if field("COREDUMP_UID")?.trim() != uid.to_string() {
        return None;
    }
    let pid = field("COREDUMP_PID").and_then(|p| p.trim().parse().ok());
    let exe = field("COREDUMP_EXE")
        .map(|e| clean_exe(e.trim_end_matches(" (deleted)")))
        .filter(|e| !e.is_empty())
        .or_else(|| field("COREDUMP_COMM").map(|c| clean_exe(&c)).filter(|c| !c.is_empty()))
        .unwrap_or_else(|| "a program".to_string());
    let signal = field("COREDUMP_SIGNAL").map(|s| signal_name(&s)).unwrap_or_else(|| "a signal".to_string());
    Some((pid, Crash { exe, signal }))
}

/// A program path made safe to show: letters, digits and `_ . / + -` only, at most 100 characters (as the Python version did).
pub fn clean_exe(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '+' | '-')).take(100).collect()
}

/// The program's name from its path (the path itself when it ends in a slash).
pub fn program_name(exe: &str) -> &str {
    exe.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(exe)
}

/// A signal number as its name (`11` is SIGSEGV); any other number as "signal N", anything else as "a signal".
pub fn signal_name(n: &str) -> String {
    const NAMES: [&str; 31] = [
        "SIGHUP",
        "SIGINT",
        "SIGQUIT",
        "SIGILL",
        "SIGTRAP",
        "SIGABRT",
        "SIGBUS",
        "SIGFPE",
        "SIGKILL",
        "SIGUSR1",
        "SIGSEGV",
        "SIGUSR2",
        "SIGPIPE",
        "SIGALRM",
        "SIGTERM",
        "SIGSTKFLT",
        "SIGCHLD",
        "SIGCONT",
        "SIGSTOP",
        "SIGTSTP",
        "SIGTTIN",
        "SIGTTOU",
        "SIGURG",
        "SIGXCPU",
        "SIGXFSZ",
        "SIGVTALRM",
        "SIGPROF",
        "SIGWINCH",
        "SIGIO",
        "SIGPWR",
        "SIGSYS",
    ];
    match n.trim().parse::<usize>() {
        Ok(i) if (1..=31).contains(&i) => NAMES[i - 1].to_string(),
        Ok(i) => format!("signal {i}"),
        Err(_) => "a signal".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const BOOT: &str = "86b6f1565fe940188f8f77e469c8a952";

    #[test]
    fn core_file_names() {
        let n = parse_core_name(&format!("core.crashy.1000.{BOOT}.3994999.1794209190000000.zst")).unwrap();
        assert_eq!(n, CoreName { comm: "crashy".into(), uid: 1000, pid: 3994999 });
        for ext in ["", ".xz", ".lz4"] {
            let n = parse_core_name(&format!("core.cage.1001.{BOOT}.42.1794175822000000{ext}")).unwrap();
            assert_eq!((n.comm.as_str(), n.uid, n.pid), ("cage", 1001, 42), "{ext}");
        }
        // systemd escapes dots (and slashes, spaces) in the short name
        let n = parse_core_name(&format!("core.Web\\x20Content.1000.{BOOT}.7.1794175822000000.zst")).unwrap();
        assert_eq!(n.comm, "Web Content");
        let n = parse_core_name(&format!("core.python3\\x2e14.1000.{BOOT}.7.1794175822000000.zst")).unwrap();
        assert_eq!(n.comm, "python3.14");
        // a short name with an unescaped dot (an older systemd) still counts from the end
        let n = parse_core_name(&format!("core.a.b.1000.{BOOT}.7.1794175822000000.zst")).unwrap();
        assert_eq!((n.comm.as_str(), n.uid), ("a.b", 1000));
    }

    #[test]
    fn other_files_are_not_cores() {
        for bad in [
            format!(".#core.crashy.1000.{BOOT}.1.2.zst"), // systemd's temporary name, renamed when done
            format!("core.crashy.1000.{BOOT}.1"),         // a field missing
            format!("core..1000.{BOOT}.1.2.zst"),         // no program name
            format!("core.crashy.root.{BOOT}.1.2.zst"),   // uid is not a number
            "core.crashy.1000.notabootid.1.2.zst".to_string(),
            format!("core.crashy.1000.{BOOT}.1.x2.zst"),
            "notes.txt".to_string(),
            "core".to_string(),
        ] {
            assert_eq!(parse_core_name(&bad), None, "{bad}");
        }
    }

    fn attrs(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (format!("user.coredump.{k}"), v.to_string())).collect();
        move |k: &str| m.get(k).cloned()
    }

    fn core(uid: u32) -> CoreName {
        CoreName { comm: "game.x86_64".into(), uid, pid: 9 }
    }

    #[test]
    fn only_my_own_crashes() {
        let a = attrs(&[("exe", "/usr/bin/game"), ("signal", "11"), ("uid", "1000")]);
        assert!(crash_of(&core(1000), 1000, &a).is_some());
        assert_eq!(crash_of(&core(1001), 1000, &a), None, "another user's core (by its name)");
        let theirs = attrs(&[("exe", "/usr/bin/game"), ("signal", "11"), ("uid", "0")]);
        assert_eq!(crash_of(&core(1000), 1000, &theirs), None, "the attributes disagree with the name: not shown");
    }

    #[test]
    fn program_and_signal_from_the_attributes() {
        let c = crash_of(&core(1000), 1000, attrs(&[("exe", "/home/me/Games/game.x86_64"), ("signal", "6")])).unwrap();
        assert_eq!(c, Crash { exe: "/home/me/Games/game.x86_64".into(), signal: "SIGABRT".into() });
        // a program replaced by an update while it ran
        let c = crash_of(&core(1000), 1000, attrs(&[("exe", "/usr/bin/vivaldi-bin (deleted)"), ("signal", "11")])).unwrap();
        assert_eq!(c.exe, "/usr/bin/vivaldi-bin");
        // no exe attribute: the short name from the attributes, else from the file name
        let c = crash_of(&core(1000), 1000, attrs(&[("comm", "steamwebhelper"), ("signal", "5")])).unwrap();
        assert_eq!((c.exe.as_str(), c.signal.as_str()), ("steamwebhelper", "SIGTRAP"));
        let c = crash_of(&core(1000), 1000, attrs(&[])).unwrap();
        assert_eq!((c.exe.as_str(), c.signal.as_str()), ("game.x86_64", "a signal"), "the file was already gone");
        let c = crash_of(&CoreName { comm: "  ".into(), uid: 1000, pid: 1 }, 1000, attrs(&[])).unwrap();
        assert_eq!(c.exe, "a program");
    }

    fn fields(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| m.get(k).cloned()
    }

    #[test]
    fn crashes_from_the_journal() {
        // a core that was not stored (Storage=none): the entry has no COREDUMP_FILENAME
        let e = fields(&[
            ("COREDUMP_UID", "1000"),
            ("COREDUMP_PID", "4242"),
            ("COREDUMP_EXE", "/opt/Game/game.x86_64"),
            ("COREDUMP_SIGNAL", "11"),
            ("COREDUMP_COMM", "game.x86_64"),
        ]);
        assert_eq!(crash_of_entry(1000, &e), Some((Some(4242), Crash { exe: "/opt/Game/game.x86_64".into(), signal: "SIGSEGV".into() })));
        assert_eq!(crash_of_entry(1001, &e), None, "another user's crash");
        assert_eq!(crash_of_entry(1000, fields(&[("COREDUMP_PID", "1"), ("COREDUMP_EXE", "/x")])), None, "no owner: not shown");
        // what is missing or odd falls back as for a core file
        assert_eq!(crash_of_entry(1000, fields(&[("COREDUMP_UID", "10000"), ("COREDUMP_EXE", "/x")])), None, "not a prefix match");
        let e = fields(&[("COREDUMP_UID", "1000"), ("COREDUMP_EXE", "/usr/bin/vivaldi-bin (deleted)"), ("COREDUMP_SIGNAL", "6")]);
        assert_eq!(crash_of_entry(1000, &e), Some((None, Crash { exe: "/usr/bin/vivaldi-bin".into(), signal: "SIGABRT".into() })));
        let e = fields(&[("COREDUMP_UID", "1000"), ("COREDUMP_PID", "x"), ("COREDUMP_COMM", "steamwebhelper")]);
        assert_eq!(crash_of_entry(1000, &e), Some((None, Crash { exe: "steamwebhelper".into(), signal: "a signal".into() })));
        let e = fields(&[("COREDUMP_UID", "1000"), ("COREDUMP_EXE", "/usr/bin/a b;rm -rf $HOME")]);
        assert_eq!(crash_of_entry(1000, &e).unwrap().1.exe, "/usr/bin/abrm-rfHOME", "cleaned like a file's attributes");
        assert_eq!(crash_of_entry(1000, fields(&[("COREDUMP_UID", "1000")])).unwrap().1.exe, "a program");
    }

    #[test]
    fn what_is_shown_is_cleaned() {
        assert_eq!(clean_exe("/usr/bin/a b;rm -rf $HOME`x`"), "/usr/bin/abrm-rfHOMEx");
        assert_eq!(clean_exe("/opt/Spiel/Übung_2+1.bin"), "/opt/Spiel/Übung_2+1.bin");
        assert_eq!(clean_exe(&"x".repeat(300)).len(), 100);
        assert_eq!(program_name("/usr/lib/firefox/firefox"), "firefox");
        assert_eq!(program_name("steamwebhelper"), "steamwebhelper");
        assert_eq!(program_name("odd/"), "odd/");
    }

    #[test]
    fn signal_names() {
        assert_eq!(signal_name("11"), "SIGSEGV");
        assert_eq!(signal_name("6\n"), "SIGABRT");
        assert_eq!(signal_name("31"), "SIGSYS");
        assert_eq!(signal_name("3"), "SIGQUIT");
        assert_eq!(signal_name("40"), "signal 40");
        assert_eq!(signal_name(""), "a signal");
        assert_eq!(signal_name("SEGV"), "a signal");
    }

    /// The real thing: attributes set on a file, read back the way the watcher reads a core (tmpfs and most disks keep user.*
    /// attributes; where the file system cannot, the test says so and passes).
    #[test]
    fn reads_attributes_of_a_real_file() {
        let d = std::env::temp_dir().join(format!("crashwatch-xattr-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join(format!("core.game.1000.{BOOT}.9.1794175822000000.zst"));
        std::fs::write(&f, b"not a core").unwrap();
        let set = |k: &str, v: &str| {
            let p = CString::new(f.as_os_str().as_bytes()).unwrap();
            let n = CString::new(k).unwrap();
            // SAFETY: plain buffers of the given lengths
            unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), v.as_ptr().cast(), v.len(), 0) == 0 }
        };
        if !set("user.coredump.exe", "/usr/bin/game") {
            eprintln!("no user.* attributes on {}: skipped", d.display());
            let _ = std::fs::remove_dir_all(&d);
            return;
        }
        assert!(set("user.coredump.signal", "11") && set("user.coredump.uid", "1000"));
        assert_eq!(read_xattr(&f, "user.coredump.exe").as_deref(), Some("/usr/bin/game"));
        assert_eq!(read_xattr(&f, "user.coredump.hostname"), None);
        assert_eq!(read_xattr(&d.join("gone"), "user.coredump.exe"), None);
        let name = parse_core_name(f.file_name().unwrap().to_str().unwrap()).unwrap();
        let c = crash_of(&name, 1000, |k| read_xattr(&f, k)).unwrap();
        assert_eq!(c, Crash { exe: "/usr/bin/game".into(), signal: "SIGSEGV".into() });
        let _ = std::fs::remove_dir_all(&d);
    }
}
