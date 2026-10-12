//! Sidebar contents: favorites, cloud providers, and mounted drives.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
}

/// Where a cloud provider's files appear on disk, and whether that is usable.
///
/// A provider is shown in the sidebar like OneDrive in Windows Explorer: as a
/// normal folder, kept populated by a separate sync/mount process. The explorer
/// never speaks a provider's protocol itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudState {
    /// The folder exists and can be browsed.
    Ready,
    /// The provider is configured but its folder is missing (not mounted/synced yet).
    NotMounted,
}

#[derive(Debug, Clone)]
pub struct CloudPlace {
    pub label: String,
    pub path: PathBuf,
    pub state: CloudState,
}

/// Candidate folders for Proton Drive, first existing one wins; if none exist the
/// first candidate is reported as `NotMounted`.
/// `LABYRINTH_PROTON_DRIVE` overrides everything.
pub fn proton_drive_candidates() -> Vec<PathBuf> {
    if let Ok(p) = std::env::var("LABYRINTH_PROTON_DRIVE") {
        if !p.is_empty() {
            return vec![PathBuf::from(p)];
        }
    }
    let Some(home) = dirs::home_dir() else { return vec![] };
    // "Proton Drive" matches the official Windows client's folder; "ProtonDrive"
    // is what a Linux mount will use.
    vec![home.join("ProtonDrive"), home.join("Proton Drive")]
}

pub fn proton_drive() -> Option<CloudPlace> {
    let cands = proton_drive_candidates();
    let first = cands.first()?.clone();
    let found = cands.into_iter().find(|p| p.is_dir());
    Some(CloudPlace {
        label: "Proton Drive".into(),
        state: if found.is_some() { CloudState::Ready } else { CloudState::NotMounted },
        path: found.unwrap_or(first),
    })
}

pub fn favorites() -> Vec<Place> {
    let items: [(&str, Option<PathBuf>); 7] = [
        ("Home", dirs::home_dir()),
        ("Desktop", dirs::desktop_dir()),
        ("Documents", dirs::document_dir()),
        ("Downloads", dirs::download_dir()),
        ("Music", dirs::audio_dir()),
        ("Pictures", dirs::picture_dir()),
        ("Videos", dirs::video_dir()),
    ];
    items
        .into_iter()
        .filter_map(|(label, p)| p.filter(|p| p.is_dir()).map(|path| Place { label: label.into(), path }))
        .collect()
}

/// Sidebar label for a path. The well-known user folders get fixed English names
/// even when the folder on disk is localized (e.g. "Documentos"); anything else
/// uses its own folder name.
pub fn label_for(path: &Path) -> String {
    let known: [(&str, Option<PathBuf>); 7] = [
        ("Home", dirs::home_dir()),
        ("Desktop", dirs::desktop_dir()),
        ("Documents", dirs::document_dir()),
        ("Downloads", dirs::download_dir()),
        ("Music", dirs::audio_dir()),
        ("Pictures", dirs::picture_dir()),
        ("Videos", dirs::video_dir()),
    ];
    label_with(path, &known)
}

fn label_with(path: &Path, known: &[(&str, Option<PathBuf>)]) -> String {
    for (name, p) in known {
        if p.as_deref() == Some(path) {
            return (*name).to_string();
        }
    }
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The virtual "This PC" location (a list of drives), not a folder on disk.
pub const THIS_PC: &str = "This PC";

/// What "This PC" is called on screen: the computer's name once it has been given one, else "This PC".
/// (The location itself stays `THIS_PC`; only the label changes.) The names a fresh install or the
/// live session carries ("labyrinth", "archiso", "localhost") do not count as a name.
pub fn pc_name() -> String {
    let host = std::fs::read_to_string("/etc/hostname").ok().or_else(|| std::env::var("HOSTNAME").ok()).unwrap_or_default();
    display_name(host.trim())
}

fn display_name(host: &str) -> String {
    let generic = ["", "localhost", "localhost.localdomain", "archiso", "labyrinth", "labyrinth-os"];
    if generic.contains(&host.to_lowercase().as_str()) { THIS_PC.to_string() } else { host.to_string() }
}

pub fn this_pc() -> PathBuf {
    PathBuf::from(THIS_PC)
}

pub fn is_this_pc(p: &Path) -> bool {
    p == Path::new(THIS_PC)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Drive {
    pub label: String,
    pub path: PathBuf,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
}

/// One disk as the OS reports it, before filtering.
#[derive(Debug, Clone)]
pub struct RawDisk {
    pub name: String,
    pub mount: PathBuf,
    pub fs: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
}

/// Keep real, user-relevant drives; drop pseudo filesystems and system plumbing mounts.
pub fn filter_drives(raw: Vec<RawDisk>) -> Vec<Drive> {
    const PSEUDO_FS: &[&str] = &["tmpfs", "devtmpfs", "squashfs", "overlay", "efivarfs", "proc", "sysfs", "cgroup2", "ramfs", "fuse.portal"];
    const SKIP_PREFIX: &[&str] = &["/boot", "/snap", "/run/user", "/var/lib/docker", "/var/lib/containers", "/nix/store", "/sys", "/proc", "/dev", "/mnt/wsl", "/usr/lib/wsl"];
    let mut out: Vec<Drive> = Vec::new();
    for d in raw {
        let m = d.mount.to_string_lossy().replace('\\', "/");
        if d.total == 0 || PSEUDO_FS.contains(&d.fs.as_str()) || SKIP_PREFIX.iter().any(|p| m == *p || m.starts_with(&format!("{p}/"))) {
            continue;
        }
        if out.iter().any(|x| x.path == d.mount) {
            continue;
        }
        let vol = d.name.trim();
        let windows_style = m.len() >= 2 && m.as_bytes()[1] == b':';
        let label = if m == "/" {
            "System (/)".to_string()
        } else if windows_style {
            let shown = m.trim_end_matches('/');
            if vol.is_empty() { format!("Local Disk ({shown}\\)") } else { format!("{vol} ({shown}\\)") }
        } else if !vol.is_empty() && !vol.starts_with("/dev/") {
            vol.to_string()
        } else {
            d.mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| m.clone())
        };
        out.push(Drive { label, path: d.mount, total: d.total, free: d.free, removable: d.removable });
    }
    // Root first, then by mount path (so C:, D:, E: stay in letter order).
    out.sort_by(|a, b| (a.path != Path::new("/")).cmp(&(b.path != Path::new("/"))).then(a.path.to_string_lossy().to_lowercase().cmp(&b.path.to_string_lossy().to_lowercase())));
    out
}

/// Drives mounted right now (sizes and free space included).
pub fn drives() -> Vec<Drive> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    filter_drives(
        disks
            .list()
            .iter()
            .map(|d| RawDisk {
                name: d.name().to_string_lossy().into_owned(),
                mount: d.mount_point().to_path_buf(),
                fs: d.file_system().to_string_lossy().into_owned(),
                total: d.total_space(),
                free: d.available_space(),
                removable: d.is_removable(),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localized_known_folders_get_english_labels() {
        let known = [("Documents", Some(PathBuf::from("/u/Documentos"))), ("Home", Some(PathBuf::from("/u")))];
        assert_eq!(label_with(Path::new("/u/Documentos"), &known), "Documents");
        assert_eq!(label_with(Path::new("/u"), &known), "Home");
        assert_eq!(label_with(Path::new("/u/Projects"), &known), "Projects");
        assert_eq!(label_with(Path::new("/"), &known), "/");
    }

    fn raw(name: &str, mount: &str, fs: &str, total: u64) -> RawDisk {
        RawDisk { name: name.into(), mount: mount.into(), fs: fs.into(), total, free: total / 4, removable: false }
    }

    #[test]
    fn drive_filter_keeps_real_drives_only() {
        let d = filter_drives(vec![
            raw("/dev/nvme0n1p2", "/", "btrfs", 1000),
            raw("tmpfs", "/run/user/1000", "tmpfs", 50),
            raw("/dev/nvme0n1p1", "/boot/efi", "vfat", 1000),
            raw("BACKUP", "/run/media/me/BACKUP", "exfat", 500),
            raw("x", "/snap/core", "squashfs", 100),
            raw("empty", "/mnt/empty", "ext4", 0),
            raw("dup", "/", "btrfs", 1000),
        ]);
        let labels: Vec<_> = d.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(labels, ["System (/)", "BACKUP"]);
    }

    #[test]
    fn windows_style_labels_match_eden() {
        let d = filter_drives(vec![raw("OS", "C:\\", "NTFS", 100), raw("", "E:\\", "NTFS", 100)]);
        assert_eq!(d[0].label, "OS (C:\\)");
        assert_eq!(d[1].label, "Local Disk (E:\\)");
    }

    #[test]
    fn the_computer_name_replaces_this_pc_only_once_it_is_set() {
        assert_eq!(display_name("OptiPlex"), "OptiPlex");
        assert_eq!(display_name("Matthews-PC"), "Matthews-PC");
        for unnamed in ["", "labyrinth", "Labyrinth", "archiso", "localhost"] {
            assert_eq!(display_name(unnamed), "This PC");
        }
    }

    #[test]
    fn this_pc_is_virtual() {
        assert!(is_this_pc(&this_pc()));
        assert!(!is_this_pc(Path::new("/")));
    }
}
