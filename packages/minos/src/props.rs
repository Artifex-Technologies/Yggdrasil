//! Properties (the command bar's Properties, Alt+Enter): what a file or folder is, how big, where, when it was made and changed,
//! and who may read and change it. A folder's size is counted on a thread by the caller ([`count`]), never on a drive that may
//! stall (`places::may_stall`: network and cloud mounts), where every file would be fetched or asked for over the line.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// What the dialog shows about one item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub name: String,
    /// "Folder", "PNG file", "Link to a folder", ...
    pub kind: String,
    pub is_dir: bool,
    /// A file's size (a folder's is counted separately).
    pub size: u64,
    pub created: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    /// `rwxr-xr-x`.
    pub mode: String,
    /// Who may do what, in words.
    pub access: String,
    pub owner: String,
}

/// `rwxr-xr-x` for a mode's permission bits.
pub fn mode_string(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let b = (mode >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// The permission bits in words: "You: read and write; group: read; others: read".
pub fn access_words(mode: u32, is_dir: bool) -> String {
    let words = |b: u32| -> &'static str {
        match (b & 4 != 0, b & 2 != 0, b & 1 != 0 && is_dir) {
            (true, true, _) => "read and change",
            (true, false, _) => "read only",
            (false, true, _) => "change only",
            (false, false, true) => "pass through only",
            (false, false, false) => "no access",
        }
    };
    format!("Owner: {}; group: {}; others: {}", words(mode >> 6), words(mode >> 3), words(mode))
}

/// The account name of a user id (from the system's user list), or the number.
fn user_name(uid: u32) -> String {
    let mut buf = vec![0u8; 4096];
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwuid_r writes into pw and buf, both ours and big enough as given
    let rc = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut out) };
    if rc != 0 || out.is_null() || pw.pw_name.is_null() {
        return uid.to_string();
    }
    // SAFETY: pw_name points into buf, NUL-terminated by getpwuid_r
    unsafe { std::ffi::CStr::from_ptr(pw.pw_name) }.to_string_lossy().into_owned()
}

/// Reads one item's facts (a link is described as the link, not followed).
pub fn facts(path: &Path) -> std::io::Result<Facts> {
    let link = std::fs::symlink_metadata(path)?;
    let meta = if link.file_type().is_symlink() { std::fs::metadata(path).unwrap_or(link.clone()) } else { link.clone() };
    let is_dir = meta.is_dir();
    let mut kind = if is_dir {
        "Folder".to_string()
    } else {
        match path.extension().map(|x| x.to_string_lossy().to_uppercase()) {
            Some(x) if !x.is_empty() && x.len() <= 8 => format!("{x} file"),
            _ => "File".into(),
        }
    };
    if link.file_type().is_symlink() {
        kind = format!("Link to a {}", if is_dir { "folder" } else { "file" });
    }
    let mode = meta.permissions().mode();
    Ok(Facts {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned()),
        kind,
        is_dir,
        size: if is_dir { 0 } else { meta.len() },
        created: meta.created().ok(),
        modified: meta.modified().ok(),
        mode: mode_string(mode),
        access: access_words(mode, is_dir),
        owner: user_name(meta.uid()),
    })
}

/// What counting found so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Count {
    pub bytes: u64,
    pub files: u64,
    pub folders: u64,
    /// Some folders could not be read (no permission).
    pub unreadable: u64,
}

/// Adds up everything under `paths` (folders walked, links not followed, other drives mounted inside not entered), calling
/// `progress` now and then; stops early when `stop` is set. The total.
pub fn count(paths: &[std::path::PathBuf], stop: &AtomicBool, progress: &mut dyn FnMut(Count)) -> Count {
    let mut c = Count::default();
    let mut stack: Vec<(std::path::PathBuf, u64)> = Vec::new();
    for p in paths {
        let Ok(m) = std::fs::symlink_metadata(p) else { continue };
        if m.is_dir() {
            stack.push((p.clone(), m.dev()));
        } else {
            c.files += 1;
            c.bytes += m.len();
        }
    }
    let mut seen = 0u32;
    while let Some((dir, dev)) = stack.pop() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            c.unreadable += 1;
            continue;
        };
        for item in rd.flatten() {
            let Ok(m) = item.metadata() else { continue };
            if m.is_dir() {
                c.folders += 1;
                if m.dev() == dev {
                    stack.push((item.path(), dev));
                }
            } else {
                c.files += 1;
                c.bytes += m.len();
            }
            seen += 1;
            if seen.is_multiple_of(2000) {
                progress(c);
            }
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_as_letters_and_words() {
        assert_eq!(mode_string(0o755), "rwxr-xr-x");
        assert_eq!(mode_string(0o640), "rw-r-----");
        assert_eq!(mode_string(0o100600), "rw-------", "only the permission bits");
        assert_eq!(access_words(0o644, false), "Owner: read and change; group: read only; others: read only");
        assert_eq!(access_words(0o711, true), "Owner: read and change; group: pass through only; others: pass through only");
        assert_eq!(access_words(0o600, false), "Owner: read and change; group: no access; others: no access");
    }

    #[test]
    fn a_folder_is_counted_and_a_file_described() {
        let d = std::env::temp_dir().join(format!("minos-props-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub/deeper")).unwrap();
        std::fs::write(d.join("a.txt"), "hello").unwrap();
        std::fs::write(d.join("sub/b.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(d.join("sub/deeper/c"), "xyz").unwrap();
        std::os::unix::fs::symlink(d.join("sub"), d.join("link")).unwrap();
        let stop = AtomicBool::new(false);
        let c = count(std::slice::from_ref(&d), &stop, &mut |_| {});
        assert_eq!((c.files, c.folders), (4, 2), "three files and the link (not followed), two folders: {c:?}");
        let link_len = std::fs::symlink_metadata(d.join("link")).unwrap().len();
        assert_eq!(c.bytes, 1008 + link_len);
        let f = facts(&d.join("a.txt")).unwrap();
        assert_eq!((f.kind.as_str(), f.size, f.is_dir), ("TXT file", 5, false));
        assert!(f.modified.is_some() && !f.owner.is_empty());
        assert_eq!(facts(&d.join("link")).unwrap().kind, "Link to a folder");
        assert_eq!(facts(&d).unwrap().kind, "Folder");
        stop.store(true, Ordering::Relaxed);
        assert_eq!(count(std::slice::from_ref(&d), &stop, &mut |_| {}).files, 0, "stopped before it began");
        let _ = std::fs::remove_dir_all(&d);
    }
}
