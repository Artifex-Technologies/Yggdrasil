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

// ---------------------------------------------------------------- physical disks

/// The virtual location of one physical disk (its partitions), `Disk:vda`.
const DISK_PREFIX: &str = "Disk:";
/// An entry for a partition that is not mounted; it cannot be opened.
pub const PART_PREFIX: &str = "Part:";

pub fn disk_path(name: &str) -> PathBuf {
    PathBuf::from(format!("{DISK_PREFIX}{name}"))
}

/// The disk's device name if `p` is a disk location.
pub fn disk_name(p: &Path) -> Option<String> {
    p.to_str().and_then(|s| s.strip_prefix(DISK_PREFIX)).map(str::to_string)
}

/// A place that is not a folder on disk: "This PC" or a disk.
pub fn is_virtual(p: &Path) -> bool {
    is_this_pc(p) || disk_name(p).is_some()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Partition {
    pub name: String,
    pub label: String,
    pub fs: String,
    /// Size of the partition in bytes.
    pub size: u64,
    /// Where it is mounted, shortest path first.
    pub mounts: Vec<PathBuf>,
    /// (total, free) of the mounted filesystem.
    pub usage: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Disk {
    pub name: String,
    pub label: String,
    pub size: u64,
    pub parts: Vec<Partition>,
}

impl Disk {
    /// (total, free) for the whole disk: its size, and what the mounted partitions leave free.
    pub fn usage(&self) -> (u64, u64) {
        let used: u64 = self.parts.iter().filter_map(|p| p.usage).map(|(t, f)| t.saturating_sub(f)).sum();
        (self.size, self.size.saturating_sub(used))
    }
}

fn str_of(v: &serde_json::Value) -> String {
    v.as_str().unwrap_or("").trim().to_string()
}

fn num_of(v: &serde_json::Value) -> u64 {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0)
}

fn mounts_of(v: &serde_json::Value, out: &mut Vec<PathBuf>) {
    for m in v["mountpoints"].as_array().into_iter().flatten().chain(v["mountpoint"].as_str().map(|_| &v["mountpoint"]).into_iter()) {
        if let Some(s) = m.as_str() {
            if !s.is_empty() && !s.starts_with('[') {
                out.push(PathBuf::from(s));
            }
        }
    }
    for c in v["children"].as_array().into_iter().flatten() {
        mounts_of(c, out);
    }
}

/// Parse `lsblk -J -b` output into disks with their partitions. `usage` gives (total, free) by mount point.
pub fn parse_lsblk(json: &str, usage: &dyn Fn(&Path) -> Option<(u64, u64)>) -> Vec<Disk> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    let mut disks = vec![];
    for d in v["blockdevices"].as_array().into_iter().flatten() {
        // Real disks only: no loop devices, optical drives or compressed RAM.
        if str_of(&d["type"]) != "disk" || d["name"].as_str().map_or(true, |n| n.starts_with("zram") || n.starts_with("loop") || n.starts_with("sr")) {
            continue;
        }
        let name = str_of(&d["name"]);
        let size = num_of(&d["size"]);
        let model = str_of(&d["model"]);
        let label = if model.is_empty() { format!("Disk ({name})") } else { format!("{model} ({name})") };
        let mut parts = vec![];
        let mut nodes: Vec<&serde_json::Value> = d["children"].as_array().into_iter().flatten().collect();
        if nodes.is_empty() {
            nodes.push(d); // a disk with a filesystem straight on it
        }
        for c in nodes {
            let pname = str_of(&c["name"]);
            let mut mounts = vec![];
            mounts_of(c, &mut mounts);
            mounts.sort_by_key(|m| (m.components().count(), m.clone()));
            mounts.dedup();
            let usage = mounts.iter().find_map(|m| usage(m));
            let plabel = [str_of(&c["label"]), str_of(&c["partlabel"])].into_iter().find(|s| !s.is_empty()).unwrap_or_else(|| "Partition".to_string());
            parts.push(Partition { name: pname.clone(), label: format!("{plabel} ({pname})"), fs: str_of(&c["fstype"]), size: num_of(&c["size"]), mounts, usage });
        }
        disks.push(Disk { name, label, size, parts });
    }
    disks
}

/// The disks in this computer, with their partitions and how full the mounted ones are.
pub fn disks() -> Vec<Disk> {
    let out = std::process::Command::new("lsblk").args(["-J", "-b", "-o", "NAME,TYPE,SIZE,MODEL,FSTYPE,LABEL,PARTLABEL,MOUNTPOINTS"]).output();
    let Ok(out) = out else { return vec![] };
    let sys = sysinfo::Disks::new_with_refreshed_list();
    let usage = |m: &Path| sys.list().iter().find(|d| d.mount_point() == m).map(|d| (d.total_space(), d.available_space()));
    parse_lsblk(&String::from_utf8_lossy(&out.stdout), &usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSBLK: &str = r#"{"blockdevices":[
      {"name":"vda","type":"disk","size":85899345920,"model":null,"children":[
        {"name":"vda1","type":"part","size":1073741824,"fstype":"vfat","label":null,"partlabel":"EFI system","mountpoints":["/boot/efi"]},
        {"name":"vda2","type":"part","size":84825604096,"fstype":"btrfs","label":"Labyrinth","mountpoints":["/home","/","/var/log"]}]},
      {"name":"zram0","type":"disk","size":8000000000,"mountpoints":["[SWAP]"]},
      {"name":"sr0","type":"rom","size":1000}]}"#;

    #[test]
    fn disks_list_the_device_with_its_partitions() {
        let usage = |m: &Path| match m.to_str() { Some("/") => Some((1000, 400)), Some("/boot/efi") => Some((100, 90)), _ => None };
        let d = parse_lsblk(LSBLK, &usage);
        assert_eq!(d.len(), 1, "zram and optical drives are not disks");
        assert_eq!(d[0].name, "vda");
        assert_eq!(d[0].label, "Disk (vda)");
        assert_eq!(d[0].parts.len(), 2);
        assert_eq!(d[0].parts[0].label, "EFI system (vda1)");
        assert_eq!(d[0].parts[1].label, "Labyrinth (vda2)");
        assert_eq!(d[0].parts[1].mounts[0], PathBuf::from("/"), "the shortest mount comes first");
        assert_eq!(d[0].parts[1].usage, Some((1000, 400)));
        // 600 + 10 used of the 80 GiB disk
        assert_eq!(d[0].usage().1, 85899345920 - 610);
    }

    #[test]
    fn disk_locations_are_virtual() {
        assert_eq!(disk_name(&disk_path("nvme0n1")).as_deref(), Some("nvme0n1"));
        assert!(is_virtual(&disk_path("vda")) && is_virtual(&this_pc()));
        assert!(!is_virtual(Path::new("/home")));
    }

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
