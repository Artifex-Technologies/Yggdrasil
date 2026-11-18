//! What the package repositories can update right now, for Agora's Updates and Settings > General > Software Update (one
//! checker, so both say the same).
//!
//! The repositories are asked afresh without root: a private copy of pacman's sync databases is made in the session's runtime
//! folder (`$XDG_RUNTIME_DIR/yggdrasil-updates-PID`, removed afterwards), refreshed there with `fakeroot pacman -Sy --dbpath`
//! (as checkupdates does), and `pacman -Qu` reads it. pacman's own databases in /var/lib/pacman are never written, so a
//! terminal's `pacman -Syu` sees nothing changed and nothing is ever locked.

pub mod history;
pub mod pause;

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// One package that can be updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub name: String,
    pub old: String,
    pub new: String,
    /// From the AUR (Agora's yay check), not the repositories.
    pub aur: bool,
    /// Bytes to download (0: already in pacman's cache, or not asked for) and the repository it comes from ("" when not asked
    /// for); filled by `check_repos(true)`.
    pub size: u64,
    pub repo: String,
}

/// What one check of the repositories found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    pub updates: Vec<Update>,
    /// Bytes still to download for them (what pacman's cache already holds counts nothing); 0 when not asked for.
    pub download: u64,
    /// The update servers answered. When they did not, `updates` is what the last refresh of pacman's own databases knew.
    pub refreshed: bool,
}

/// `pacman -Qu` (and `yay -Qua`) lines: "name old -> new", anything after them ignored ("[ignored]"); other lines are skipped.
pub fn parse_updates(text: &str, aur: bool) -> Vec<Update> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (name, old, arrow, new) = (it.next()?, it.next()?, it.next()?, it.next()?);
            (arrow == "->").then(|| Update {
                name: name.to_string(),
                old: old.to_string(),
                new: new.to_string(),
                aur,
                size: 0,
                repo: String::new(),
            })
        })
        .collect()
}

/// One package of `pacman -Sup --print-format '%n %r %s'`: its name, repository and bytes to download (0 for a package already
/// in pacman's cache). Lines that are not three words ending in a number (a warning) are skipped.
pub fn parse_download(text: &str) -> Vec<(String, String, u64)> {
    text.lines()
        .filter_map(|l| match l.split_whitespace().collect::<Vec<_>>().as_slice() {
            [n, r, s] => Some((n.to_string(), r.to_string(), s.parse().ok()?)),
            _ => None,
        })
        .collect()
}

/// Fills each update's size and repository from `parse_download`'s list, and returns the bytes to download in all (a new
/// dependency the update pulls in counts too).
pub fn add_sizes(updates: &mut [Update], download: &[(String, String, u64)]) -> u64 {
    for u in updates.iter_mut() {
        if let Some((_, r, s)) = download.iter().find(|(n, ..)| *n == u.name) {
            u.size = *s;
            u.repo = r.clone();
        }
    }
    download.iter().map(|(.., s)| s).sum()
}

/// pacman.conf for the private copy: no signature checks. Without root the keyring cannot be read, so repositories that require
/// signed databases (CachyOS's) would fail; this only lists names and versions, and the real update still checks everything.
pub fn unsigned_conf(conf: &str) -> String {
    conf.lines()
        .filter(|l| !l.trim_start().starts_with("SigLevel") && !l.trim_start().starts_with("LocalFileSigLevel"))
        .map(|l| if l.trim() == "[options]" { "[options]\nSigLevel = Never".to_string() } else { l.to_string() })
        .collect::<Vec<_>>()
        .join("\n")
}

fn output(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).stderr(Stdio::null()).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// Asks the repositories what can be updated (needs the network; a few seconds), with the download size when `size` is set.
/// Without a private copy (no runtime folder to write) it reads pacman's own databases as they are.
pub fn check_repos(size: bool) -> Check {
    let run_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    let tmp = run_dir.join(format!("yggdrasil-updates-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let made =
        std::fs::create_dir_all(tmp.join("sync")).is_ok() && std::os::unix::fs::symlink("/var/lib/pacman/local", tmp.join("local")).is_ok();
    let mut check = Check::default();
    if made {
        if let Ok(rd) = std::fs::read_dir("/var/lib/pacman/sync") {
            for e in rd.flatten() {
                let _ = std::fs::copy(e.path(), tmp.join("sync").join(e.file_name()));
            }
        }
        let db = tmp.to_string_lossy().into_owned();
        let conf_path = tmp.join("pacman.conf");
        let _ = std::fs::write(&conf_path, unsigned_conf(&std::fs::read_to_string("/etc/pacman.conf").unwrap_or_default()));
        let cf = conf_path.to_string_lossy().into_owned();
        check.refreshed = Command::new("fakeroot")
            .args(["--", "pacman", "-Sy", "--config", &cf, "--dbpath", &db, "--logfile", "/dev/null"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        check.updates = parse_updates(&output("pacman", &["-Qu", "--config", &cf, "--dbpath", &db]), false);
        if size && !check.updates.is_empty() {
            let d = parse_download(&output("pacman", &["-Sup", "--print-format", "%n %r %s", "--config", &cf, "--dbpath", &db]));
            check.download = add_sizes(&mut check.updates, &d);
        }
    } else {
        check.updates = parse_updates(&output("pacman", &["-Qu"]), false);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    check.updates.sort_by(|a, b| a.name.cmp(&b.name));
    check
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_lines_are_read() {
        let u = parse_updates("krita 5.3.4-1 -> 5.3.5-1\nvivaldi 7.0-1 -> 7.1-1 [ignored]\nnoise\nx 1 2 3\n", false);
        assert_eq!(u.len(), 2);
        assert_eq!((u[0].name.as_str(), u[0].old.as_str(), u[0].new.as_str(), u[0].aur), ("krita", "5.3.4-1", "5.3.5-1", false));
        assert_eq!(u[1].new, "7.1-1");
        assert!(parse_updates("a 1 -> 2", true)[0].aur);
    }

    #[test]
    fn the_download_size_adds_up() {
        let d = parse_download("linux core 152000000\nbash core 0\nwarning: something odd\nnew-dep extra 120000\nx y z\n");
        assert_eq!(d.len(), 3);
        let mut u = parse_updates("linux 6.17.7-1 -> 6.17.8-1\nbash 5.3-1 -> 5.3-2\nkrita 5-1 -> 6-1\n", false);
        assert_eq!(add_sizes(&mut u, &d), 152_120_000, "the new dependency counts");
        assert_eq!((u[0].size, u[0].repo.as_str()), (152_000_000, "core"));
        assert_eq!((u[2].size, u[2].repo.as_str()), (0, ""), "not in the list");
        assert!(parse_download("").is_empty());
    }

    #[test]
    fn the_private_conf_checks_no_signatures() {
        let c = unsigned_conf(
            "[options]\nSigLevel = Required DatabaseOptional\nLocalFileSigLevel = Optional\n[core]\n  SigLevel = Required\nInclude = x",
        );
        assert_eq!(c, "[options]\nSigLevel = Never\n[core]\nInclude = x");
    }
}
