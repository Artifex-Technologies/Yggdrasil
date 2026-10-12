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
}
