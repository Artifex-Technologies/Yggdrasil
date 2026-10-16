//! Linking a local folder to a cloud folder, and verifying the two match.
//!
//! Listings come from `rclone lsjson -R --hash` run on *both* sides (a local path
//! and a `remote:path`), so this module only compares two listings and never
//! hashes anything itself. Verification is read-only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LsItem {
    pub path: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub is_dir: bool,
    #[serde(default)]
    pub hashes: BTreeMap<String, String>,
}

pub fn parse_lsjson(text: &str) -> Result<Vec<LsItem>, String> {
    serde_json::from_str(text).map_err(|e| format!("cannot parse rclone listing: {e}"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Problem {
    /// On the remote but not locally.
    MissingLocal(String),
    /// Local but not on the remote (not uploaded yet).
    MissingRemote(String),
    SizeDiffers(String),
    HashDiffers(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Report {
    pub files_compared: usize,
    /// Files matched by size and by a hash both sides provided.
    pub verified_by_hash: usize,
    /// Files matched by size only because no common hash type was available.
    pub size_only: usize,
    pub problems: Vec<Problem>,
    /// Unix seconds when the verification ran (0 = unknown / older report).
    #[serde(default)]
    pub checked_at: u64,
}

impl Report {
    /// Clean means nothing wrong *and* every file was actually checked by hash.
    pub fn is_fully_verified(&self) -> bool {
        self.problems.is_empty() && self.size_only == 0 && self.files_compared > 0
    }

    pub fn summary(&self) -> String {
        if self.problems.is_empty() {
            if self.size_only == 0 && self.files_compared > 0 {
                format!("Verified: {} files match by hash", self.files_compared)
            } else if self.files_compared == 0 {
                "Nothing to compare".into()
            } else {
                format!(
                    "No differences, but only {} of {} files could be hash-checked (rest by size)",
                    self.verified_by_hash, self.files_compared
                )
            }
        } else {
            format!("{} problem(s) in {} files", self.problems.len(), self.files_compared)
        }
    }
}

/// Compare two listings (directories ignored).
pub fn verify(local: &[LsItem], remote: &[LsItem]) -> Report {
    let index = |v: &[LsItem]| -> BTreeMap<String, LsItem> {
        v.iter().filter(|i| !i.is_dir).map(|i| (i.path.clone(), i.clone())).collect()
    };
    let (l, r) = (index(local), index(remote));
    let mut rep = Report::default();
    for (path, li) in &l {
        let Some(ri) = r.get(path) else {
            rep.problems.push(Problem::MissingRemote(path.clone()));
            continue;
        };
        rep.files_compared += 1;
        if li.size != ri.size {
            rep.problems.push(Problem::SizeDiffers(path.clone()));
            continue;
        }
        let common = li
            .hashes
            .iter()
            .find(|(k, v)| ri.hashes.get(*k).is_some_and(|rv| !rv.is_empty() && !v.is_empty()));
        match common {
            Some((k, v)) if ri.hashes.get(k).map(|s| s.eq_ignore_ascii_case(v)) == Some(true) => rep.verified_by_hash += 1,
            Some(_) => rep.problems.push(Problem::HashDiffers(path.clone())),
            None => rep.size_only += 1,
        }
    }
    for path in r.keys() {
        if !l.contains_key(path) {
            rep.problems.push(Problem::MissingLocal(path.clone()));
        }
    }
    rep
}

/// A user-defined pairing, e.g. `~/Documents/Github` <-> `proton:My files/Documents/Github`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    pub local: PathBuf,
    pub remote: String,
}

pub fn links_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("minos").join("cloud-links.json"))
}

pub fn report_path(link_name: &str) -> Option<PathBuf> {
    let safe: String = link_name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dirs::config_dir().map(|d| d.join("minos").join(format!("verify-{safe}.json")))
}

pub fn load_links(path: &Path) -> Vec<Link> {
    crate::compat::read_to_string(path).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_links(path: &Path, links: &[Link]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(links).map_err(std::io::Error::other)?)
}

fn rclone_list(target: &str) -> Result<Vec<LsItem>, String> {
    let out = Command::new("rclone")
        .args(["lsjson", "-R", "--hash", "--no-mimetype", "--no-modtime", target])
        .output()
        .map_err(|e| format!("cannot run rclone (is it installed?): {e}"))?;
    if !out.status.success() {
        return Err(format!("rclone failed on {target}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    parse_lsjson(&String::from_utf8_lossy(&out.stdout))
}

/// Run the full verification for a link and store the report for the explorer.
pub fn verify_link(link: &Link) -> Result<Report, String> {
    let local = rclone_list(&link.local.to_string_lossy())?;
    let remote = rclone_list(&link.remote)?;
    let mut rep = verify(&local, &remote);
    rep.checked_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Some(p) = report_path(&link.name) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, serde_json::to_vec_pretty(&rep).unwrap_or_default());
    }
    Ok(rep)
}

pub fn last_report(link_name: &str) -> Option<Report> {
    let p = report_path(link_name)?;
    serde_json::from_str(&crate::compat::read_to_string(&p)?).ok()
}

/// What to draw next to a file or folder inside (or above) a linked folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    /// Linked, but never verified.
    Unverified,
    /// Last check matched every file by hash.
    Synced,
    /// Last check found no differences, but only by size (no common hash).
    SizeOnly,
    /// This file, or something inside this folder, differed in the last check.
    Problem,
    /// A folder that contains a linked folder further down.
    ContainsLink,
}

fn rel_slash(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
}

fn problem_path(p: &Problem) -> &str {
    match p {
        Problem::MissingLocal(f) | Problem::MissingRemote(f) | Problem::SizeDiffers(f) | Problem::HashDiffers(f) => f,
    }
}

/// Badge for `path`, from the links and each link's last report. This reflects the
/// last *verification*, not live transfer state.
pub fn badge_for(path: &Path, links: &[Link], reports: &BTreeMap<String, Report>) -> Option<Badge> {
    for l in links {
        let Some(rel) = rel_slash(path, &l.local) else { continue };
        let Some(rep) = reports.get(&l.name) else { return Some(Badge::Unverified) };
        let prefix = format!("{rel}/");
        let hit = rep.problems.iter().map(problem_path).any(|f| rel.is_empty() || f == rel || f.starts_with(&prefix));
        return Some(if hit {
            Badge::Problem
        } else if rep.size_only == 0 && rep.files_compared > 0 {
            Badge::Synced
        } else if rep.size_only > 0 {
            Badge::SizeOnly
        } else {
            Badge::Unverified
        });
    }
    if links.iter().any(|l| l.local != path && l.local.starts_with(path)) {
        return Some(Badge::ContainsLink);
    }
    None
}

/// Where a Proton Drive file's bytes are: rclone's mount keeps downloaded files in its cache, everything else is fetched on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avail {
    /// Fully on this computer (also a file waiting to upload).
    Local,
    /// Some of it downloaded.
    Partial,
    /// Only in the cloud; opening it downloads it.
    Cloud,
    /// Being uploaded, or waiting its turn.
    Syncing,
    /// The upload failed; rclone tries again later, or now on request.
    Failed,
}

/// One entry of rclone's upload queue (`vfs/queue`).
#[derive(Debug, Clone, PartialEq)]
pub struct QueueItem {
    pub id: u64,
    /// Path inside the mount, e.g. "Documents/a.txt".
    pub name: String,
    pub tries: u64,
    pub uploading: bool,
}

pub fn parse_queue(json: &str) -> Vec<QueueItem> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    v.get("queue")
        .and_then(|q| q.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    Some(QueueItem {
                        id: i.get("id")?.as_u64()?,
                        name: i.get("name")?.as_str()?.to_string(),
                        tries: i.get("tries").and_then(|t| t.as_u64()).unwrap_or(0),
                        uploading: i.get("uploading").and_then(|t| t.as_bool()).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A file in the queue is syncing until an upload has failed at least once (and is not running right now).
pub fn queue_state(i: &QueueItem) -> Avail {
    if i.tries > 0 && !i.uploading {
        Avail::Failed
    } else {
        Avail::Syncing
    }
}

/// The mount's control socket (the unit starts rclone with `--rc --rc-addr unix://$XDG_RUNTIME_DIR/rclone-proton.sock`).
fn rc_socket() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LABYRINTH_RCLONE_SOCKET") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    Some(PathBuf::from(std::env::var("XDG_RUNTIME_DIR").ok()?).join("rclone-proton.sock"))
}

fn rc_call(cmd: &str, body: &str) -> Option<String> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    let mut s = UnixStream::connect(rc_socket()?).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    s.set_write_timeout(Some(Duration::from_secs(3))).ok()?;
    let req = format!("POST /{cmd} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    s.write_all(req.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    let status_ok = out.starts_with("HTTP/1.1 200");
    let body = out.split_once("\r\n\r\n")?.1;
    status_ok.then(|| body.to_string())
}

/// The upload queue, asked for at most every 2 seconds. None when the mount has no control socket (an older unit): then a changed-but-not-yet-uploaded file just counts as syncing.
fn queue() -> Option<Vec<QueueItem>> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LAST: Mutex<Option<(Instant, Option<Vec<QueueItem>>)>> = Mutex::new(None);
    let mut g = LAST.lock().ok()?;
    if let Some((t, q)) = g.as_ref() {
        if t.elapsed() < Duration::from_secs(2) {
            return q.clone();
        }
    }
    let q = rc_call("vfs/queue", "{}").map(|b| parse_queue(&b));
    *g = Some((Instant::now(), q.clone()));
    q
}

/// Ask rclone to retry a failed upload now.
pub fn retry_sync(drive_root: &Path, path: &Path) -> Result<(), String> {
    let rel = path.strip_prefix(drive_root).map_err(|_| "not inside Proton Drive".to_string())?.to_string_lossy().to_string();
    let q = rc_call("vfs/queue", "{}").map(|b| parse_queue(&b)).ok_or("the Proton Drive mount has no control socket: restart it (systemctl --user restart proton-drive)")?;
    let item = q.iter().find(|i| i.name == rel).ok_or("it is not waiting to upload any more")?;
    rc_call("vfs/queue-set-expiry", &format!(r#"{{"id":{},"expiry":0}}"#, item.id)).map(|_| ()).ok_or_else(|| "rclone did not accept the retry".to_string())
}

/// rclone's cache for one file: `meta` is its `vfsMeta` record, `on_disk` the bytes the cached data file really occupies (None = no cached file).
/// `Rs` lists the downloaded ranges when rclone has written them; otherwise a cached file counts as complete when it is not sparse.
pub fn avail_from_meta(meta: Option<&str>, size: u64, on_disk: Option<u64>) -> Avail {
    let v: serde_json::Value = meta.and_then(|m| serde_json::from_str(m).ok()).unwrap_or(serde_json::Value::Null);
    if v.get("Dirty").and_then(|d| d.as_bool()) == Some(true) {
        return Avail::Syncing;
    }
    let Some(on_disk) = on_disk else { return Avail::Cloud };
    let mut ranges: Vec<(u64, u64)> = v
        .get("Rs")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|r| Some((r.get("Pos")?.as_u64()?, r.get("Size")?.as_u64()?))).collect())
        .unwrap_or_default();
    if ranges.is_empty() {
        // No range list: judge by how much of the file is really on disk (a sparse, partly downloaded file takes less).
        return if size == 0 || on_disk >= size { Avail::Local } else if on_disk > 0 { Avail::Partial } else { Avail::Cloud };
    }
    ranges.sort();
    let mut covered = 0u64;
    for (pos, len) in ranges {
        if len > 0 && pos <= covered {
            covered = covered.max(pos + len);
        }
    }
    if covered >= size {
        Avail::Local
    } else {
        Avail::Partial
    }
}

fn cache_root() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("RCLONE_CACHE_DIR") {
        if !d.is_empty() {
            return Some(PathBuf::from(d));
        }
    }
    Some(dirs::cache_dir()?.join("rclone"))
}

/// Availability of a file under the Proton Drive mount, cached for a few seconds (this is asked for every visible file on every redraw).
pub fn availability(drive_root: &Path, path: &Path, size: u64) -> Option<Avail> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<HashMap<PathBuf, (Instant, Avail)>>> = Mutex::new(None);
    let rel = path.strip_prefix(drive_root).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    let mut guard = CACHE.lock().ok()?;
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some((t, a)) = map.get(path) {
        if t.elapsed() < Duration::from_secs(4) {
            return Some(*a);
        }
    }
    let root = cache_root();
    let meta = root.as_ref().and_then(|c| std::fs::read_to_string(c.join("vfsMeta").join("proton").join(rel)).ok());
    let on_disk = root.as_ref().and_then(|c| {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(c.join("vfs").join("proton").join(rel)).ok().filter(|m| m.is_file()).map(|m| m.blocks() * 512)
    });
    let mut a = avail_from_meta(meta.as_deref(), size, on_disk);
    if let Some(item) = queue().and_then(|q| q.into_iter().find(|i| Path::new(&i.name) == rel)) {
        a = queue_state(&item);
    }
    if map.len() > 5000 {
        map.clear();
    }
    map.insert(path.to_path_buf(), (Instant::now(), a));
    Some(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, size: i64, sha1: Option<&str>) -> LsItem {
        LsItem {
            path: path.into(),
            size,
            is_dir: false,
            hashes: sha1.map(|h| BTreeMap::from([("sha1".to_string(), h.to_string())])).unwrap_or_default(),
        }
    }

    #[test]
    fn parses_rclone_json() {
        let j = r#"[{"Path":"a.txt","Name":"a.txt","Size":3,"IsDir":false,"Hashes":{"sha1":"AB"}},
                    {"Path":"d","Name":"d","Size":-1,"IsDir":true}]"#;
        let v = parse_lsjson(j).unwrap();
        assert_eq!(v.len(), 2);
        assert!(v[1].is_dir);
        assert_eq!(v[0].hashes["sha1"], "AB");
        assert!(parse_lsjson("nope").is_err());
    }

    #[test]
    fn clean_match_is_fully_verified() {
        let a = vec![item("x", 3, Some("aa")), item("d/y", 5, Some("bb"))];
        let b = vec![item("x", 3, Some("AA")), item("d/y", 5, Some("bb"))];
        let r = verify(&a, &b);
        assert!(r.is_fully_verified());
        assert_eq!(r.verified_by_hash, 2);
    }

    #[test]
    fn detects_every_kind_of_problem() {
        let local = vec![item("same", 1, Some("1")), item("only-local", 1, None), item("size", 1, None), item("hash", 2, Some("x"))];
        let remote = vec![item("same", 1, Some("1")), item("only-remote", 1, None), item("size", 9, None), item("hash", 2, Some("y"))];
        let r = verify(&local, &remote);
        assert!(r.problems.contains(&Problem::MissingRemote("only-local".into())));
        assert!(r.problems.contains(&Problem::MissingLocal("only-remote".into())));
        assert!(r.problems.contains(&Problem::SizeDiffers("size".into())));
        assert!(r.problems.contains(&Problem::HashDiffers("hash".into())));
        assert!(!r.is_fully_verified());
    }

    #[test]
    fn no_common_hash_is_not_called_verified() {
        // Same names and sizes but no hashes: must not claim a hash-verified match.
        let r = verify(&[item("a", 4, None)], &[item("a", 4, None)]);
        assert!(r.problems.is_empty());
        assert_eq!(r.size_only, 1);
        assert!(!r.is_fully_verified());
        assert!(r.summary().contains("size"));
    }

    #[test]
    fn empty_is_not_verified() {
        assert!(!verify(&[], &[]).is_fully_verified());
    }

    #[test]
    fn badges() {
        let link = Link { name: "n".into(), local: PathBuf::from("/h/docs"), remote: "r:".into() };
        let links = vec![link];
        let bad = Report {
            files_compared: 3,
            verified_by_hash: 2,
            problems: vec![Problem::HashDiffers("sub/y.txt".into())],
            ..Default::default()
        };
        let good = Report { files_compared: 3, verified_by_hash: 3, ..Default::default() };
        let b = |p: &str, r: Option<Report>| badge_for(Path::new(p), &links, &r.map(|r| BTreeMap::from([("n".to_string(), r)])).unwrap_or_default());
        assert_eq!(b("/h/docs/sub/y.txt", Some(bad.clone())), Some(Badge::Problem));
        assert_eq!(b("/h/docs/sub", Some(bad.clone())), Some(Badge::Problem));
        assert_eq!(b("/h/docs/other.txt", Some(bad.clone())), Some(Badge::Synced));
        assert_eq!(b("/h/docs/other.txt", Some(good.clone())), Some(Badge::Synced));
        assert_eq!(b("/h/docs", Some(bad)), Some(Badge::Problem));
        assert_eq!(b("/h/docs/x", None), Some(Badge::Unverified));
        assert_eq!(b("/h", Some(good.clone())), Some(Badge::ContainsLink));
        assert_eq!(b("/elsewhere/x", Some(good)), None);
        // "sub" must not match a sibling named "sub2".
        let r = Report { files_compared: 1, problems: vec![Problem::SizeDiffers("sub/a".into())], ..Default::default() };
        assert_ne!(b("/h/docs/sub2", Some(r)), Some(Badge::Problem));
    }

    #[test]
    fn the_upload_queue_says_syncing_or_failed() {
        let q = parse_queue(r#"{"queue":[{"name":"a.txt","id":1,"size":3,"expiry":57.9,"tries":0,"delay":60,"uploading":false},{"name":"d/b.txt","id":2,"size":2,"expiry":107.9,"tries":1,"delay":120,"uploading":false},{"name":"c","id":3,"tries":2,"uploading":true}]}"#);
        assert_eq!(q.len(), 3);
        assert_eq!(queue_state(&q[0]), Avail::Syncing);
        assert_eq!(queue_state(&q[1]), Avail::Failed);
        assert_eq!(queue_state(&q[2]), Avail::Syncing);
        assert!(parse_queue("nonsense").is_empty());
    }

    #[test]
    fn availability_from_rclone_cache() {
        let m = |rs: &str, dirty: bool| format!(r#"{{"Size":100,"Rs":{rs},"Dirty":{dirty}}}"#);
        // nothing cached
        assert_eq!(avail_from_meta(None, 100, None), Avail::Cloud);
        assert_eq!(avail_from_meta(Some(&m("null", false)), 100, None), Avail::Cloud);
        // rclone wrote no range list (what a fully downloaded file looked like on the real mount): judged by the data file
        assert_eq!(avail_from_meta(Some(&m("null", false)), 100, Some(4096)), Avail::Local);
        assert_eq!(avail_from_meta(Some(&m("null", false)), 10_000, Some(4096)), Avail::Partial);
        assert_eq!(avail_from_meta(Some(&m("null", false)), 10_000, Some(0)), Avail::Cloud);
        // range lists
        assert_eq!(avail_from_meta(Some(&m(r#"[{"Pos":0,"Size":40},{"Pos":40,"Size":60}]"#, false)), 100, Some(100)), Avail::Local);
        assert_eq!(avail_from_meta(Some(&m(r#"[{"Pos":0,"Size":40},{"Pos":60,"Size":40}]"#, false)), 100, Some(100)), Avail::Partial);
        // waiting to upload
        assert_eq!(avail_from_meta(Some(&m("null", true)), 100, None), Avail::Syncing);
        assert_eq!(avail_from_meta(Some("garbage"), 100, None), Avail::Cloud);
    }
}

/// Whether rclone already has a Proton Drive remote called `proton`.
pub fn proton_configured() -> bool {
    Command::new("rclone").arg("listremotes").output().map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == "proton:")).unwrap_or(false)
}

fn obscure(secret: &str) -> Result<String, String> {
    use std::io::Write;
    let mut child = Command::new("rclone").args(["obscure", "-"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().map_err(|e| format!("cannot run rclone (is it installed?): {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(secret.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Sign in to Proton Drive (rclone's `protondrive` backend, remote `proton`) and start the mount at
/// `~/ProtonDrive`, so the Cloud entry in the sidebar fills in. `mailbox` and `twofa` may be empty.
pub fn connect_proton(user: &str, password: &str, mailbox: &str, twofa: &str) -> Result<(), String> {
    let mut args: Vec<String> = ["config", "create", "proton", "protondrive", "username"].map(String::from).to_vec();
    args.push(user.trim().to_string());
    args.push("password".into());
    args.push(obscure(password)?);
    if !mailbox.is_empty() {
        args.push("mailbox_password".into());
        args.push(obscure(mailbox)?);
    }
    if !twofa.trim().is_empty() {
        args.push("2fa".into());
        args.push(twofa.trim().to_string());
    }
    let out = Command::new("rclone").args(&args).output().map_err(|e| format!("cannot run rclone: {e}"))?;
    if !out.status.success() {
        let _ = Command::new("rclone").args(["config", "delete", "proton"]).status();
        return Err(format!("Proton refused the sign-in: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("").trim()));
    }
    start_proton_mount()
}

/// Put the mount service in place (once) and start it.
pub fn start_proton_mount() -> Result<(), String> {
    let home = dirs::home_dir().ok_or("no home folder")?;
    let unit = home.join(".config/systemd/user/proton-drive.service");
    if !unit.is_file() {
        std::fs::create_dir_all(unit.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::copy("/usr/share/labyrinth/proton-drive.service", &unit).map_err(|e| format!("cannot install the mount service: {e}"))?;
        let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    }
    let st = Command::new("systemctl").args(["--user", "enable", "--now", "proton-drive"]).output().map_err(|e| e.to_string())?;
    if !st.status.success() {
        return Err(format!("could not start the mount: {}", String::from_utf8_lossy(&st.stderr).trim()));
    }
    Ok(())
}
