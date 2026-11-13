//! device-offers-online: NetworkManager's dispatcher runs it (as root, `INTERFACE ACTION`, with CONNECTIVITY_STATE in its
//! environment) on every network event. When the computer has come online ("up", or "connectivity-change" to FULL) it starts
//! yggdrasil-online.target in the service manager of each signed-in user, as that user, and returns at once (--no-block). That
//! target starts yggdrasil-device-offers there, and whatever the operating system hooks onto it (Labyrinth: the optional parts
//! still to install from the installer). On any other event it does nothing.
//!
//! For tests only, and only when not run as root: DEVICE_OFFERS_RUN_USER (another /run/user), DEVICE_OFFERS_SYSTEMCTL (another
//! systemctl), DEVICE_OFFERS_MIN_UID (another lowest uid of a person's account than 1000).

use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const TARGET: &str = "yggdrasil-online.target";

/// The events after which the computer may be online.
fn online_event(action: &str, connectivity: Option<&str>) -> bool {
    action == "up" || (action == "connectivity-change" && connectivity == Some("FULL"))
}

/// A signed-in user's service manager: (uid, gid, runtime folder).
#[derive(Debug, PartialEq)]
struct Manager {
    uid: u32,
    gid: u32,
    dir: PathBuf,
}

/// The service managers under `run_user`: folders named by a uid of a person's account (min_uid and up), owned by that uid (not
/// a link), with the manager's socket in them.
fn managers(run_user: &Path, min_uid: u32) -> Vec<Manager> {
    let Ok(rd) = std::fs::read_dir(run_user) else { return vec![] };
    let mut out: Vec<Manager> = rd
        .flatten()
        .filter_map(|e| {
            let uid: u32 = e.file_name().to_str()?.parse().ok()?;
            let m = std::fs::symlink_metadata(e.path()).ok()?;
            (uid >= min_uid && m.is_dir() && m.uid() == uid && e.path().join("systemd/private").exists()).then(|| Manager {
                uid,
                gid: m.gid(),
                dir: e.path(),
            })
        })
        .collect();
    out.sort_by_key(|m| m.uid);
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let action = args.get(1).map(String::as_str).unwrap_or("");
    if !online_event(action, std::env::var("CONNECTIVITY_STATE").ok().as_deref()) {
        return;
    }
    // SAFETY: geteuid has no preconditions
    let root = unsafe { libc::geteuid() } == 0;
    let test = |k: &str| if root { None } else { std::env::var_os(k) };
    let run_user = test("DEVICE_OFFERS_RUN_USER").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/run/user"));
    let systemctl = test("DEVICE_OFFERS_SYSTEMCTL").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/usr/bin/systemctl"));
    let min_uid = test("DEVICE_OFFERS_MIN_UID").and_then(|v| v.to_str()?.parse().ok()).unwrap_or(1000);
    for m in managers(&run_user, min_uid) {
        let mut c = Command::new(&systemctl);
        c.args(["--user", "start", "--no-block", TARGET])
            .env_clear()
            .env("XDG_RUNTIME_DIR", &m.dir)
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if root {
            // as that user (std drops the supplementary groups first)
            c.uid(m.uid).gid(m.gid);
        }
        let _ = c.status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_coming_online_counts() {
        assert!(online_event("up", None));
        assert!(online_event("connectivity-change", Some("FULL")));
        for (a, c) in
            [("connectivity-change", Some("LIMITED")), ("connectivity-change", None), ("down", None), ("dhcp4-change", None), ("", None)]
        {
            assert!(!online_event(a, c), "{a} {c:?}");
        }
    }

    #[test]
    fn signed_in_users_are_found_by_their_runtime_folders() {
        let d = std::env::temp_dir().join(format!("ygg-device-offers-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        // SAFETY: getuid has no preconditions
        let me = unsafe { libc::getuid() };
        std::fs::create_dir_all(d.join(format!("{me}/systemd"))).unwrap();
        std::fs::write(d.join(format!("{me}/systemd/private")), "").unwrap();
        std::fs::create_dir_all(d.join(format!("{}/systemd", me + 1))).unwrap(); // named after another uid than its owner
        std::fs::write(d.join(format!("{}/systemd/private", me + 1)), "").unwrap();
        std::fs::create_dir_all(d.join("not-a-uid/systemd")).unwrap();
        std::fs::create_dir_all(d.join("4242424")).unwrap();
        let found = managers(&d, 0);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].uid, found[0].dir.clone()), (me, d.join(me.to_string())));
        assert!(managers(&d, me + 1).is_empty(), "below the lowest uid of a person's account");
        assert!(managers(&d.join("missing"), 0).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
