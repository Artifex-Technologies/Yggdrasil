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
    let index =
        |v: &[LsItem]| -> BTreeMap<String, LsItem> { v.iter().filter(|i| !i.is_dir).map(|i| (i.path.clone(), i.clone())).collect() };
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
        let common = li.hashes.iter().find(|(k, v)| ri.hashes.get(*k).is_some_and(|rv| !rv.is_empty() && !v.is_empty()));
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
    if let Ok(p) = std::env::var("MINOS_RCLONE_SOCKET") {
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

/// Ask rclone to retry a failed upload now (for a folder: every one inside it). Returns how many were retried.
pub fn retry_sync(drive_root: &Path, path: &Path) -> Result<usize, String> {
    let rel = path.strip_prefix(drive_root).map_err(|_| "not inside Proton Drive".to_string())?.to_string_lossy().to_string();
    let q = rc_call("vfs/queue", "{}")
        .map(|b| parse_queue(&b))
        .ok_or("the Proton Drive mount has no control socket: restart it (systemctl --user restart proton-drive)")?;
    let prefix = format!("{rel}/");
    let items: Vec<&QueueItem> =
        q.iter().filter(|i| i.name == rel || i.name.starts_with(&prefix)).filter(|i| queue_state(i) == Avail::Failed).collect();
    if items.is_empty() {
        return Err("nothing is waiting to upload there any more".into());
    }
    for item in &items {
        rc_call("vfs/queue-set-expiry", &format!(r#"{{"id":{},"expiry":0}}"#, item.id))
            .ok_or_else(|| "rclone did not accept the retry".to_string())?;
        if let Ok(mut r) = RETRYING.lock() {
            r.get_or_insert_with(Default::default).insert(item.name.clone(), (std::time::Instant::now(), item.tries));
        }
    }
    Ok(items.len())
}

/// Uploads retried on request: shown as syncing until rclone has made one more attempt (its try count went up) or 30 seconds passed.
static RETRYING: std::sync::Mutex<Option<std::collections::HashMap<String, (std::time::Instant, u64)>>> = std::sync::Mutex::new(None);

/// A queued upload's state, counting a retry that is still under way as syncing.
fn queue_state_now(i: &QueueItem) -> Avail {
    if let Ok(mut g) = RETRYING.lock() {
        let map = g.get_or_insert_with(Default::default);
        if let Some((t, tries)) = map.get(&i.name).copied() {
            if t.elapsed() < std::time::Duration::from_secs(30) && i.tries <= tries {
                return Avail::Syncing;
            }
            map.remove(&i.name);
        }
    }
    queue_state(i)
}

/// What a folder's files add up to: any failed upload makes it failed, else any upload makes it syncing; otherwise on this computer
/// when every file is, in the cloud when none is, partly when some are.
pub fn combine(states: impl IntoIterator<Item = Avail>) -> Avail {
    let (mut any, mut local, mut all_local, mut syncing, mut failed) = (false, false, true, false, false);
    for a in states {
        any = true;
        match a {
            Avail::Failed => failed = true,
            Avail::Syncing => syncing = true,
            Avail::Local => local = true,
            Avail::Partial => {
                local = true;
                all_local = false;
            }
            Avail::Cloud => all_local = false,
        }
    }
    if failed {
        Avail::Failed
    } else if syncing {
        Avail::Syncing
    } else if !any || all_local {
        Avail::Local
    } else if local {
        Avail::Partial
    } else {
        Avail::Cloud
    }
}

/// Folders bigger than this get no status (working it out would take too long).
const FOLDER_LIMIT: usize = 5000;

/// The files under `dir` (through the mount; rclone keeps the listings), with their sizes; None past the limit.
fn files_under(dir: &Path) -> Option<Vec<(PathBuf, u64)>> {
    let mut out = vec![];
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                todo.push(e.path());
            } else if ft.is_file() {
                out.push((e.path(), e.metadata().map(|m| m.len()).unwrap_or(0)));
                if out.len() > FOLDER_LIMIT {
                    return None;
                }
            }
        }
    }
    Some(out)
}

/// A file's state from rclone's cache alone (no upload queue).
fn cached_state(rel: &Path, size: u64) -> Avail {
    let root = cache_root();
    let meta = root.as_ref().and_then(|c| std::fs::read_to_string(c.join("vfsMeta").join("proton").join(rel)).ok());
    let on_disk = root.as_ref().and_then(|c| {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(c.join("vfs").join("proton").join(rel)).ok().filter(|m| m.is_file()).map(|m| m.blocks() * 512)
    });
    avail_from_meta(meta.as_deref(), size, on_disk)
}

/// Folder states worked out in the background: the result and when, or None while it is being worked out.
type FolderStates = std::collections::HashMap<PathBuf, Option<(std::time::Instant, Option<Avail>)>>;
static FOLDERS: std::sync::Mutex<Option<FolderStates>> = std::sync::Mutex::new(None);

/// Folders downloaded with "Download to this computer" and how many files they had then:
/// ~/.local/state/yggdrasil/minos-downloaded.json, {"/home/me/ProtonDrive/Photos": 120, ...}.
fn downloaded_path() -> Option<PathBuf> {
    Some(dirs::state_dir().or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))?.join("yggdrasil").join("minos-downloaded.json"))
}

fn downloaded() -> std::collections::BTreeMap<String, usize> {
    downloaded_path().and_then(|p| crate::compat::read_to_string(&p)).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn remember_downloaded(dir: &Path, files: usize) {
    let Some(p) = downloaded_path() else { return };
    let mut m = downloaded();
    m.insert(dir.to_string_lossy().into_owned(), files);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, serde_json::to_vec_pretty(&m).unwrap_or_default());
}

/// How many files under `dir` (inside the cache) rclone holds on this computer: a walk of the local cache only, never of the
/// cloud (walking the mount fetches every folder from Proton, a minute for a big one, and held up everything else).
fn cached_files(cache_dir: &Path) -> usize {
    let mut n = 0;
    let mut todo = vec![cache_dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => todo.push(e.path()),
                Ok(t) if t.is_file() => n += 1,
                _ => {}
            }
        }
        if n > 100_000 {
            break;
        }
    }
    n
}

/// A folder's state from what is on this computer: on it when it was downloaded whole and its files are all still in the
/// cache, partly when some of it is, else only in the cloud.
pub fn folder_state(downloaded_files: Option<usize>, cached: usize) -> Avail {
    match downloaded_files {
        Some(want) if cached >= want => Avail::Local,
        _ if cached > 0 => Avail::Partial,
        _ => Avail::Cloud,
    }
}

/// A folder under the Proton Drive mount: failed or syncing at once from the upload queue; otherwise from rclone's cache on this
/// computer (worked out in the background, None until then) and kept for 10 seconds.
pub fn folder_availability(drive_root: &Path, dir: &Path) -> Option<Avail> {
    use std::time::Duration;
    let rel = dir.strip_prefix(drive_root).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    let prefix = format!("{}/", rel.to_string_lossy());
    if let Some(q) = queue() {
        let mine: Vec<&QueueItem> = q.iter().filter(|i| i.name.starts_with(&prefix)).collect();
        if !mine.is_empty() {
            return Some(combine(mine.into_iter().map(queue_state_now)));
        }
    }
    let mut g = FOLDERS.lock().ok()?;
    let map = g.get_or_insert_with(Default::default);
    match map.get(dir) {
        Some(Some((t, a))) if t.elapsed() < Duration::from_secs(10) => return *a,
        Some(None) => return None, // being worked out
        Some(Some((_, a))) => {
            let a = *a;
            map.insert(dir.to_path_buf(), None);
            spawn_folder_check(rel.to_path_buf(), dir.to_path_buf());
            return a; // the old answer until the new one is there
        }
        None => {}
    }
    if map.len() > 2000 {
        map.clear();
    }
    map.insert(dir.to_path_buf(), None);
    drop(g);
    spawn_folder_check(rel.to_path_buf(), dir.to_path_buf());
    None
}

fn spawn_folder_check(rel: PathBuf, dir: PathBuf) {
    std::thread::spawn(move || {
        let cached = cache_root().map(|c| cached_files(&c.join("vfs").join("proton").join(&rel))).unwrap_or(0);
        let a = Some(folder_state(downloaded().get(dir.to_string_lossy().as_ref()).copied(), cached));
        if let Ok(mut g) = FOLDERS.lock() {
            g.get_or_insert_with(Default::default).insert(dir, Some((std::time::Instant::now(), a)));
        }
    });
}

/// Downloads files and folders under the mount to this computer (rclone keeps what was read in its cache): every file is read
/// once, from start to end. `progress` gets (files done, files in all); returns how many could not be read.
pub fn download(paths: &[PathBuf], progress: impl Fn(usize, usize)) -> usize {
    use std::io::Read;
    let mut files: Vec<PathBuf> = vec![];
    let mut folders: Vec<(PathBuf, usize)> = vec![];
    for p in paths {
        if p.is_dir() {
            let under: Vec<PathBuf> = files_under(p).unwrap_or_default().into_iter().map(|(f, _)| f).collect();
            folders.push((p.clone(), under.len()));
            files.extend(under);
        } else {
            files.push(p.clone());
        }
    }
    let (total, mut failed) = (files.len(), 0);
    let mut buf = vec![0u8; 1 << 20];
    for (i, f) in files.iter().enumerate() {
        progress(i, total);
        let ok = std::fs::File::open(f).map(|mut h| loop {
            match h.read(&mut buf) {
                Ok(0) => break true,
                Ok(_) => {}
                Err(_) => break false,
            }
        });
        if !matches!(ok, Ok(true)) {
            failed += 1;
        }
    }
    // a folder downloaded whole shows as on this computer from now on (while its files stay in the cache)
    if failed == 0 {
        for (dir, n) in &folders {
            remember_downloaded(dir, *n);
        }
    }
    if let Ok(mut g) = FOLDERS.lock() {
        *g = None; // folder states are worked out again
    }
    progress(total, total);
    failed
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
        return if size == 0 || on_disk >= size {
            Avail::Local
        } else if on_disk > 0 {
            Avail::Partial
        } else {
            Avail::Cloud
        };
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
    let retrying = RETRYING.lock().ok().and_then(|g| g.as_ref().map(|m| m.contains_key(&*rel.to_string_lossy()))).unwrap_or(false);
    if let Some((t, a)) = map.get(path) {
        if t.elapsed() < Duration::from_secs(4) && !retrying {
            return Some(*a);
        }
    }
    let mut a = cached_state(rel, size);
    if let Some(item) = queue().and_then(|q| q.into_iter().find(|i| Path::new(&i.name) == rel)) {
        a = queue_state_now(&item);
    }
    if map.len() > 5000 {
        map.clear();
    }
    map.insert(path.to_path_buf(), (Instant::now(), a));
    Some(a)
}

/// Whether rclone already has a Proton Drive remote called `proton`.
pub fn proton_configured() -> bool {
    Command::new("rclone")
        .arg("listremotes")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == "proton:"))
        .unwrap_or(false)
}

/// rclone's scrambled form of a secret (`rclone obscure -`): the secret goes in on rclone's standard input, never on its command line.
/// The scrambling can be undone by anyone, so the result is as secret as the input: it only ever goes into rclone's config file.
fn obscure(secret: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new("rclone")
        .args(["obscure", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run rclone (is it installed?): {e}"))?;
    // the pipe closes when the handle is dropped at the end of this statement, so rclone sees the end of its input
    child.stdin.take().ok_or("no stdin")?.write_all(secret.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let scrambled = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // rclone answers in base64 (the URL alphabet): nothing, or anything else, means it failed, and that must not be saved as the password
    if !out.status.success() || scrambled.is_empty() || !scrambled.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(format!("rclone could not scramble the password: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(scrambled)
}

/// rclone's config file: the one `rclone config file` names (RCLONE_CONFIG when that is set, otherwise rclone's own default place).
fn rclone_config_path() -> Result<PathBuf, String> {
    let out = Command::new("rclone")
        .args(["config", "file"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run rclone (is it installed?): {e}"))?;
    // "Configuration file is stored at:" (or "...doesn't exist, but rclone will use this path:"), then the path on its own line
    let text = String::from_utf8_lossy(&out.stdout);
    match text.lines().map(str::trim).rfind(|l| !l.is_empty()).map(PathBuf::from) {
        Some(p) if out.status.success() && p.is_absolute() => Ok(p),
        _ => Err(format!("rclone did not say where its config is: {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}

/// Whether a value can go into rclone's config as it is: on its own line, read back unchanged by rclone's parser (no quotes, comment
/// marks, spaces or line breaks). Letters, digits and `@ . _ + -`: what user names, e-mail addresses, codes and scrambled values are.
pub fn conf_value_ok(v: &str) -> bool {
    v.chars().all(|c| c.is_alphanumeric() || "@._+-".contains(c))
}

/// Whether rclone's config is encrypted (`rclone config` > set a password): then only rclone can change it.
pub fn conf_encrypted(conf: &str) -> bool {
    conf.lines().any(|l| l.trim_start().starts_with("RCLONE_ENCRYPT_V0:"))
}

/// Whether rclone's config has a section for `remote`.
pub fn conf_has_remote(conf: &str, remote: &str) -> bool {
    let header = format!("[{remote}]");
    conf.lines().any(|l| l.trim() == header)
}

/// rclone's config text with one remote's section changed: each `set` key gets its value (where the key was, otherwise added after
/// the section's last line), each `drop` key goes, and a missing section is added at the end. Every other line, including every other
/// remote's section and every comment, stays byte for byte.
pub fn edit_remote(conf: &str, remote: &str, set: &[(&str, &str)], drop: &[&str]) -> String {
    let header = format!("[{remote}]");
    let is_header = |l: &str| {
        let t = l.trim();
        t.starts_with('[') && t.ends_with(']')
    };
    let key_of = |l: &str| -> Option<String> {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            return None;
        }
        // rclone's parser takes `key = value` and `key: value`
        t.find(['=', ':']).map(|i| t[..i].trim().to_string())
    };
    let mut written = vec![false; set.len()];
    let mut out = String::with_capacity(conf.len() + 256);
    let lines: Vec<&str> = conf.split_inclusive('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        out.push_str(lines[i]);
        if lines[i].trim() != header {
            i += 1;
            continue;
        }
        // the section's lines, up to the next header
        let start = i + 1;
        let mut end = start;
        while end < lines.len() && !is_header(lines[end]) {
            end += 1;
        }
        // keys that are missing go after the section's last non-blank line, so the blank line before the next section stays
        let last = (start..end).rev().find(|&j| !lines[j].trim().is_empty()).map_or(start, |j| j + 1);
        for (j, line) in lines.iter().enumerate().take(end).skip(start) {
            if j == last {
                push_missing(&mut out, set, &mut written);
            }
            match key_of(line) {
                Some(k) if drop.contains(&k.as_str()) => {}
                Some(k) if set.iter().any(|(s, _)| *s == k) => {
                    let n = set.iter().position(|(s, _)| *s == k).unwrap_or(0);
                    if !written[n] {
                        written[n] = true;
                        out.push_str(&format!("{} = {}\n", set[n].0, set[n].1));
                    }
                }
                _ => out.push_str(line),
            }
        }
        if last == end {
            push_missing(&mut out, set, &mut written);
        }
        i = end;
    }
    if !conf_has_remote(conf, remote) {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(&header);
        out.push('\n');
        push_missing(&mut out, set, &mut written);
        out.push('\n');
    }
    out
}

fn push_missing(out: &mut String, set: &[(&str, &str)], written: &mut [bool]) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for (n, (k, v)) in set.iter().enumerate() {
        if !written[n] {
            written[n] = true;
            out.push_str(&format!("{k} = {v}\n"));
        }
    }
}

/// Writes `text` to `path` as a file only its owner can read and write (0600), all at once: a new file next to it, flushed to disk,
/// then renamed over the old one, so rclone never reads half a config. A config that is a symlink is written where the link points.
pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(std::io::Error::other("not a file path"));
    };
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let tmp = dir.join(format!(".{}.minos-{}", name.to_string_lossy(), std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    // create_new: never through a link someone left at that name
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    let done = f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).and_then(|_| std::fs::rename(&tmp, &path));
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done?;
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Saves a Proton sign-in in rclone's `proton` remote (rclone's `protondrive` backend): made when missing, renewed in place when the
/// sign-in lapsed. No secret is ever on a command line, where every program on the computer could read it (/proc/PID/cmdline): the
/// passwords reach `rclone obscure -` on its standard input, and Minos writes the remote's section into rclone's config itself (0600,
/// in one rename; the other remotes stay byte for byte). `mailbox` and `twofa` may be empty.
pub fn save_proton_signin(user: &str, password: &str, mailbox: &str, twofa: &str) -> Result<(), String> {
    let (user, twofa) = (user.trim(), twofa.trim());
    if !conf_value_ok(user) {
        return Err("That user name has characters rclone's config cannot hold (letters, digits and @ . _ + - only)".into());
    }
    if !twofa.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("The two-factor code is the digits your authenticator app shows".into());
    }
    let path = rclone_config_path()?;
    let conf = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read rclone's config: {e}")),
    };
    if conf_encrypted(&conf) {
        return Err(
            "rclone's config is encrypted, so Minos cannot add the sign-in to it: connect with `rclone config` in a terminal".into()
        );
    }
    // a lapsed sign-in is renewed in place; a first one is created
    let renew = conf_has_remote(&conf, "proton");
    let password = obscure(password)?;
    let mailbox = if mailbox.is_empty() { None } else { Some(obscure(mailbox)?) };
    let mut set: Vec<(&str, &str)> = Vec::new();
    if !renew {
        set.push(("type", "protondrive"));
    }
    set.push(("username", user));
    set.push(("password", &password));
    if let Some(m) = &mailbox {
        set.push(("mailbox_password", m));
    }
    // The two-factor code is good for one sign-in only (about 30 seconds): it is handed over for this sign-in and removed again once
    // the drive is mounted (rclone keeps a session instead). Signing in again also drops the old session, so the new code is used.
    let mut drop: Vec<&str> = Vec::new();
    if twofa.is_empty() {
        drop.push("2fa");
    } else {
        set.push(("2fa", twofa));
    }
    if renew {
        drop.extend(SESSION_KEYS);
    }
    write_private(&path, &edit_remote(&conf, "proton", &set, &drop)).map_err(|e| format!("cannot save the sign-in in rclone's config: {e}"))
}

/// Sign in to Proton Drive (rclone's `protondrive` backend, remote `proton`) and start the mount at
/// `~/ProtonDrive`, so the Cloud entry in the sidebar fills in. `mailbox` and `twofa` may be empty.
pub fn connect_proton(user: &str, password: &str, mailbox: &str, twofa: &str) -> Result<(), String> {
    save_proton_signin(user, password, mailbox, twofa)?;
    let mounted = start_proton_mount().and_then(|_| wait_mounted(20).then_some(()).ok_or_else(String::new));
    forget_twofa();
    mounted.map_err(|_| {
        // a refused sign-in must not be retried by the service: repeated failed sign-ins can lock a Proton account for a while
        stop_proton_mount();
        "Proton did not accept that sign-in (check the password and the two-factor code, which lasts about 30 seconds)".to_string()
    })
}

/// rclone's saved Proton session (made at the first sign-in, renewed by itself).
const SESSION_KEYS: [&str; 4] = ["client_uid", "client_access_token", "client_refresh_token", "client_salted_key_pass"];

/// Removes the two-factor code from rclone's saved settings: it was for one sign-in, and a later start that sent it again would be
/// refused (and keep being refused while the service retried).
pub fn forget_twofa() {
    let _ = Command::new("rclone").args(["config", "update", "proton", "2fa", ""]).output();
}

/// Stops the mount and its retries (after a sign-in Proton refused).
pub fn stop_proton_mount() {
    let _ = Command::new("systemctl").args(["--user", "stop", "proton-drive"]).status();
    let _ = Command::new("systemctl").args(["--user", "reset-failed", "proton-drive"]).status();
}

/// Marks an error that means "ask for the sign-in again".
pub const SIGN_IN_AGAIN: &str = "SIGNIN:";

/// The user name saved in rclone's `proton` remote, to prefill the form when the sign-in has lapsed.
pub fn proton_username() -> String {
    Command::new("rclone")
        .args(["config", "show", "proton"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find_map(|l| l.strip_prefix("username = ").map(|v| v.trim().to_string()))
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Waits for the Proton Drive folder to be a mount (the service can start and then fail at its login): whether it became one.
pub fn wait_mounted(seconds: u64) -> bool {
    let Some(home) = dirs::home_dir() else { return false };
    let dir = home.join("ProtonDrive");
    for _ in 0..seconds * 2 {
        if Command::new("mountpoint").arg("-q").arg(&dir).status().map(|s| s.success()).unwrap_or(false) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    false
}

/// Start the mount service (Minos's package ships it as a user unit; a copy in ~/.config/systemd/user wins, as systemd does it).
pub fn start_proton_mount() -> Result<(), String> {
    let home = dirs::home_dir().ok_or("no home folder")?;
    let own = home.join(".config/systemd/user/proton-drive.service");
    if !own.is_file() && !std::path::Path::new("/usr/lib/systemd/user/proton-drive.service").is_file() {
        return Err("the Proton Drive mount service is not installed (it comes with Minos)".into());
    }
    // a mount that kept failing (a lapsed sign-in) may have hit its retry limit: clear that, so this start is a fresh one
    let _ = Command::new("systemctl").args(["--user", "reset-failed", "proton-drive"]).status();
    let st = Command::new("systemctl").args(["--user", "enable", "--now", "proton-drive"]).output().map_err(|e| e.to_string())?;
    if !st.status.success() {
        return Err(format!("could not start the mount: {}", String::from_utf8_lossy(&st.stderr).trim()));
    }
    Ok(())
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
        let b = |p: &str, r: Option<Report>| {
            badge_for(Path::new(p), &links, &r.map(|r| BTreeMap::from([("n".to_string(), r)])).unwrap_or_default())
        };
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
    fn a_folder_is_judged_by_what_is_on_this_computer() {
        assert_eq!(folder_state(None, 0), Avail::Cloud);
        assert_eq!(folder_state(None, 3), Avail::Partial, "some files downloaded");
        assert_eq!(folder_state(Some(10), 10), Avail::Local, "downloaded whole and all still here");
        assert_eq!(folder_state(Some(10), 7), Avail::Partial, "the cache let some go");
    }

    #[test]
    fn a_folder_adds_up_its_files() {
        use Avail::*;
        assert_eq!(combine([]), Local, "an empty folder has nothing to download");
        assert_eq!(combine([Local, Local]), Local);
        assert_eq!(combine([Cloud, Cloud]), Cloud);
        assert_eq!(combine([Local, Cloud]), Partial);
        assert_eq!(combine([Partial]), Partial);
        assert_eq!(combine([Local, Syncing, Cloud]), Syncing);
        assert_eq!(combine([Syncing, Failed]), Failed);
    }

    #[test]
    fn the_upload_queue_says_syncing_or_failed() {
        let q = parse_queue(
            r#"{"queue":[{"name":"a.txt","id":1,"size":3,"expiry":57.9,"tries":0,"delay":60,"uploading":false},{"name":"d/b.txt","id":2,"size":2,"expiry":107.9,"tries":1,"delay":120,"uploading":false},{"name":"c","id":3,"tries":2,"uploading":true}]}"#,
        );
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

    const CONF: &str = "# kept as it is\n[backup]\ntype = sftp\nhost = example.invalid\npass = c2VjcmV0\n\n[proton]\ntype = protondrive\nusername = old@example.invalid\npassword = T0xE\n2fa = 111111\nclient_uid = u\nclient_access_token = a\nclient_refresh_token = r\nclient_salted_key_pass = s\nreplace_existing_draft = true\n\n[zz]\ntype = local\n";

    #[test]
    fn a_renewed_sign_in_changes_only_the_proton_section() {
        let set = [("username", "new@example.invalid"), ("password", "TkVX"), ("2fa", "222222")];
        let out = edit_remote(CONF, "proton", &set, &SESSION_KEYS);
        assert_eq!(
            out,
            "# kept as it is\n[backup]\ntype = sftp\nhost = example.invalid\npass = c2VjcmV0\n\n[proton]\ntype = protondrive\nusername = new@example.invalid\npassword = TkVX\n2fa = 222222\nreplace_existing_draft = true\n\n[zz]\ntype = local\n"
        );
        // the other sections are byte for byte what they were
        let (before, after) = (CONF.split("[proton]").collect::<Vec<_>>(), out.split("[proton]").collect::<Vec<_>>());
        assert_eq!(before[0], after[0]);
        assert_eq!(CONF[CONF.find("[zz]").unwrap()..], out[out.find("[zz]").unwrap()..]);
    }

    #[test]
    fn a_missing_key_is_added_at_the_end_of_its_section_and_a_dropped_one_goes() {
        let out = edit_remote(CONF, "proton", &[("mailbox_password", "TUI")], &["2fa"]);
        assert!(out.contains("replace_existing_draft = true\nmailbox_password = TUI\n\n[zz]"), "{out}");
        assert!(!out.contains("2fa"));
        assert_eq!(edit_remote(CONF, "proton", &[], &[]), CONF);
    }

    #[test]
    fn a_first_sign_in_adds_a_section_after_the_others() {
        let set = [("type", "protondrive"), ("username", "me@example.invalid"), ("password", "UFc")];
        let base = "[backup]\ntype = sftp\n";
        assert_eq!(
            edit_remote(base, "proton", &set, &["2fa"]),
            "[backup]\ntype = sftp\n\n[proton]\ntype = protondrive\nusername = me@example.invalid\npassword = UFc\n\n"
        );
        // no trailing line break in the old file, and no file at all
        assert!(edit_remote("[backup]\ntype = sftp", "proton", &set, &[]).starts_with("[backup]\ntype = sftp\n\n[proton]\n"));
        assert_eq!(edit_remote("", "proton", &set, &[]), "[proton]\ntype = protondrive\nusername = me@example.invalid\npassword = UFc\n\n");
        // a remote whose name only starts the same is another remote
        assert!(edit_remote("[proton2]\ntype = x\n", "proton", &set, &[]).starts_with("[proton2]\ntype = x\n\n[proton]\n"));
    }

    #[test]
    fn values_that_could_break_out_of_their_line_are_refused() {
        assert!(conf_value_ok("someone.else+drive@example.invalid"));
        assert!(conf_value_ok("123456"));
        for bad in ["a\nclient_uid = x", "a b", "a#b", "a;b", "\"a\"", "`a`", "[x]", "a=b"] {
            assert!(!conf_value_ok(bad), "{bad:?}");
        }
        assert!(conf_encrypted("# Encrypted rclone configuration File\n\nRCLONE_ENCRYPT_V0:\nabc\n"));
        assert!(!conf_encrypted(CONF));
        assert!(conf_has_remote(CONF, "proton") && !conf_has_remote(CONF, "prot"));
    }

    #[test]
    fn the_config_is_written_whole_and_only_for_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("minos-conf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("rclone").join("rclone.conf");
        write_private(&path, "[a]\ntype = local\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, CONF).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CONF);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // nothing left beside it
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        // a config that is a link is written where the link points, and stays a link
        let real = dir.join("real.conf");
        std::fs::write(&real, "x").unwrap();
        let link = dir.join("link.conf");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_private(&link, CONF).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), CONF);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
