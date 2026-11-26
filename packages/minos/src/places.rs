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
/// `MINOS_PROTON_DRIVE` overrides everything.
pub fn proton_drive_candidates() -> Vec<PathBuf> {
    if let Ok(p) = std::env::var("MINOS_PROTON_DRIVE") {
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
    // the mount's folder stays behind empty when the mount is down (a lapsed sign-in, no network): that is not a working Proton Drive,
    // and showing it as one hid the way to sign in again. Usable = a mount, or a folder with files in it (a synced copy).
    let found =
        cands.into_iter().find(|p| p.is_dir() && (is_mount(p) || std::fs::read_dir(p).map(|mut d| d.next().is_some()).unwrap_or(false)));
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
    items.into_iter().filter_map(|(label, p)| p.filter(|p| p.is_dir()).map(|path| Place { label: label.into(), path })).collect()
}

/// The well-known folders that get their own picture in the sidebar (as Windows draws Desktop, Documents, Downloads ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    Home,
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Videos,
    Objects3D,
}

/// Which well-known folder `path` is, if any: the user's XDG folders, home, and a "3D Objects" folder in home.
pub fn special(path: &Path) -> Option<Special> {
    let known = [
        (Special::Desktop, dirs::desktop_dir()),
        (Special::Documents, dirs::document_dir()),
        (Special::Downloads, dirs::download_dir()),
        (Special::Music, dirs::audio_dir()),
        (Special::Pictures, dirs::picture_dir()),
        (Special::Videos, dirs::video_dir()),
        (Special::Home, dirs::home_dir()),
        (Special::Objects3D, dirs::home_dir().map(|h| h.join("3D Objects"))),
    ];
    known.into_iter().find(|(_, p)| p.as_deref() == Some(path)).map(|(s, _)| s)
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
/// live session carries ("archiso", "localhost", or the operating system's own ID from /etc/os-release, with or without "-os") do
/// not count as a name.
pub fn pc_name() -> String {
    let host = std::fs::read_to_string("/etc/hostname").ok().or_else(|| std::env::var("HOSTNAME").ok()).unwrap_or_default();
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let id = os.lines().find_map(|l| l.strip_prefix("ID=")).map(|v| v.trim().trim_matches('"').to_lowercase()).unwrap_or_default();
    display_name(host.trim(), &id)
}

fn display_name(host: &str, os_id: &str) -> String {
    let h = host.to_lowercase();
    let generic = ["", "localhost", "localhost.localdomain", "archiso"];
    let os_default = !os_id.is_empty() && (h == os_id || h == format!("{os_id}-os"));
    if generic.contains(&h.as_str()) || os_default {
        THIS_PC.to_string()
    } else {
        host.to_string()
    }
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
    const PSEUDO_FS: &[&str] =
        &["tmpfs", "devtmpfs", "squashfs", "overlay", "efivarfs", "proc", "sysfs", "cgroup2", "ramfs", "fuse.portal"];
    const SKIP_PREFIX: &[&str] = &[
        "/boot",
        "/snap",
        "/run/user",
        "/var/lib/docker",
        "/var/lib/containers",
        "/nix/store",
        "/sys",
        "/proc",
        "/dev",
        "/mnt/wsl",
        "/usr/lib/wsl",
    ];
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
            if vol.is_empty() {
                format!("Local Disk ({shown}\\)")
            } else {
                format!("{vol} ({shown}\\)")
            }
        } else if !vol.is_empty() && !vol.starts_with("/dev/") {
            vol.to_string()
        } else {
            d.mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| m.clone())
        };
        out.push(Drive { label, path: d.mount, total: d.total, free: d.free, removable: d.removable });
    }
    // Root first, then by mount path (so C:, D:, E: stay in letter order).
    out.sort_by(|a, b| {
        (a.path != Path::new("/"))
            .cmp(&(b.path != Path::new("/")))
            .then(a.path.to_string_lossy().to_lowercase().cmp(&b.path.to_string_lossy().to_lowercase()))
    });
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
    /// The filesystem's own name (its volume label), "" when it has none.
    pub volume: String,
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
    /// A USB stick, card or other drive that can be unplugged.
    pub removable: bool,
    pub model: String,
}

impl Disk {
    /// The name shown for the drive, as Windows does: the volume label, else "USB Drive" or "Local Disk".
    pub fn display_name(&self) -> String {
        // the label of a partition with files on it (not swap), the mounted one first
        let named = |p: &&Partition| !p.volume.is_empty() && p.fs != "swap";
        if let Some(p) = self.parts.iter().filter(named).find(|p| !p.mounts.is_empty()).or_else(|| self.parts.iter().find(named)) {
            return p.volume.clone();
        }
        if self.removable {
            "USB Drive".into()
        } else {
            "Local Disk".into()
        }
    }

    /// (total, free) of what is mounted on it, or None when nothing is.
    pub fn mounted_usage(&self) -> Option<(u64, u64)> {
        let u: Vec<(u64, u64)> = self.parts.iter().filter_map(|p| p.usage).collect();
        (!u.is_empty()).then(|| u.iter().fold((0, 0), |(t, f), (a, b)| (t + a, f + b)))
    }

    /// The system lives on it (/, /boot or /home): never formatted or ejected from here.
    pub fn holds_system(&self) -> bool {
        self.parts
            .iter()
            .flat_map(|p| &p.mounts)
            .any(|m| m == Path::new("/") || m.starts_with("/boot") || m == Path::new("/home") || m.starts_with("/usr"))
    }

    /// Where to go when it is clicked: the mount point of its one mounted partition, else the disk's own view.
    pub fn open_path(&self) -> PathBuf {
        let mounted: Vec<&PathBuf> = self.parts.iter().filter_map(|p| p.mounts.first()).collect();
        if self.removable && mounted.len() == 1 {
            mounted[0].clone()
        } else {
            disk_path(&self.name)
        }
    }

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
    for m in v["mountpoints"].as_array().into_iter().flatten().chain(v["mountpoint"].as_str().map(|_| &v["mountpoint"])) {
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
        if str_of(&d["type"]) != "disk"
            || d["name"].as_str().is_none_or(|n| n.starts_with("zram") || n.starts_with("loop") || n.starts_with("sr"))
        {
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
            let plabel = [str_of(&c["label"]), str_of(&c["partlabel"])]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or_else(|| "Partition".to_string());
            parts.push(Partition {
                name: pname.clone(),
                label: format!("{plabel} ({pname})"),
                volume: str_of(&c["label"]),
                fs: str_of(&c["fstype"]),
                size: num_of(&c["size"]),
                mounts,
                usage,
            });
        }
        let removable = d["rm"].as_bool().unwrap_or(false)
            || d["hotplug"].as_bool().unwrap_or(false)
            || str_of(&d["tran"]) == "usb"
            || num_of(&d["rm"]) == 1;
        disks.push(Disk { name, label, size, parts, removable, model });
    }
    disks
}

/// The disks in this computer, with their partitions and how full the mounted ones are. Only the disks' own mount points are
/// asked for their size: asking every mount (as sysinfo's list does, FUSE mounts too) froze the window for seconds while
/// Proton Drive's mount answered over a slow line (2026-11-22).
pub fn disks() -> Vec<Disk> {
    let out = std::process::Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,TYPE,SIZE,MODEL,FSTYPE,LABEL,PARTLABEL,MOUNTPOINTS,RM,HOTPLUG,TRAN"])
        .output();
    let Ok(out) = out else { return vec![] };
    parse_lsblk(&String::from_utf8_lossy(&out.stdout), &space)
}

/// A mounted file system's size and free space in bytes (statvfs), as sysinfo counts them.
fn space(m: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(m.as_os_str().as_bytes()).ok()?;
    let mut s = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a valid C string and `s` is written by statvfs before it is read (only when it returns 0).
    let s = unsafe {
        if libc::statvfs(path.as_ptr(), s.as_mut_ptr()) != 0 {
            return None;
        }
        s.assume_init()
    };
    let block = s.f_frsize as u64;
    Some((s.f_blocks as u64 * block, s.f_bavail as u64 * block))
}

/// Whether `p` is on a file system that can take seconds to answer, or never when its line is down: FUSE (Proton Drive through
/// rclone, sshfs, the document portal) and network file systems. Read from /proc/self/mountinfo, without touching `p` itself.
pub fn may_stall(p: &Path) -> bool {
    std::fs::read_to_string("/proc/self/mountinfo").is_ok_and(|t| stalls_in(&t, p))
}

fn stalls_in(mountinfo: &str, p: &Path) -> bool {
    // the deepest mount holding `p`; of two at the same place, the later one (it is the one on top)
    let mut best: Option<(usize, bool)> = None;
    for line in mountinfo.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let Some(sep) = f.iter().position(|x| *x == "-") else { continue };
        let (Some(point), Some(fstype)) = (f.get(4), f.get(sep + 1)) else { continue };
        let point = unescape_mount(point);
        if p.starts_with(&point) && best.is_none_or(|(depth, _)| point.len() >= depth) {
            best = Some((point.len(), slow_fs(fstype)));
        }
    }
    best.is_some_and(|(_, slow)| slow)
}

/// [`may_stall`] for a given copy of /proc/self/mountinfo (read once for many paths).
pub fn may_stall_in(mountinfo: &str, p: &Path) -> bool {
    stalls_in(mountinfo, p)
}

/// The mount points a count of a folder's size must not walk into: file systems that may stall (as [`may_stall`]) and the
/// kernel's own (/proc, /sys and the like, whose files are not on any disk).
pub fn count_skips_in(mountinfo: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in mountinfo.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let Some(sep) = f.iter().position(|x| *x == "-") else { continue };
        let (Some(point), Some(fstype)) = (f.get(4), f.get(sep + 1)) else { continue };
        if slow_fs(fstype) || pseudo_fs(fstype) {
            out.push(PathBuf::from(unescape_mount(point)));
        }
    }
    out
}

fn pseudo_fs(fstype: &str) -> bool {
    matches!(
        fstype,
        "proc"
            | "sysfs"
            | "devtmpfs"
            | "devpts"
            | "cgroup"
            | "cgroup2"
            | "debugfs"
            | "tracefs"
            | "securityfs"
            | "pstore"
            | "bpf"
            | "configfs"
            | "efivarfs"
            | "mqueue"
            | "hugetlbfs"
            | "binfmt_misc"
            | "autofs"
            | "fusectl"
            | "nsfs"
    )
}

fn slow_fs(fstype: &str) -> bool {
    // fuseblk is a local disk through FUSE (NTFS with ntfs-3g)
    (fstype.starts_with("fuse") && fstype != "fuseblk" && fstype != "fusectl")
        || matches!(fstype, "nfs" | "nfs4" | "cifs" | "smb3" | "smbfs" | "9p" | "davfs" | "ceph" | "glusterfs" | "afs" | "sshfs")
}

/// mountinfo writes a space, tab, newline and backslash in a path as \040, \011, \012 and \134.
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let octal = b
            .get(i + 1..i + 4)
            .filter(|d| b[i] == b'\\' && d.iter().all(|c| (b'0'..=b'7').contains(c)))
            .and_then(|d| u8::try_from(d.iter().fold(0u32, |n, c| n * 8 + u32::from(c - b'0'))).ok());
        match octal {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
    fn the_well_known_folders_are_recognised() {
        if let Some(d) = dirs::download_dir() {
            assert_eq!(special(&d), Some(Special::Downloads));
        }
        if let Some(h) = dirs::home_dir() {
            assert_eq!(special(&h), Some(Special::Home));
            assert_eq!(special(&h.join("3D Objects")), Some(Special::Objects3D));
            assert_eq!(special(&h.join("some project")), None);
        }
    }

    #[test]
    fn disks_list_the_device_with_its_partitions() {
        let usage = |m: &Path| match m.to_str() {
            Some("/") => Some((1000, 400)),
            Some("/boot/efi") => Some((100, 90)),
            _ => None,
        };
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
        assert_eq!(display_name("OptiPlex", "someos"), "OptiPlex");
        assert_eq!(display_name("Matthews-PC", "someos"), "Matthews-PC");
        for unnamed in ["", "someos", "SomeOS", "someos-os", "archiso", "localhost"] {
            assert_eq!(display_name(unnamed, "someos"), "This PC");
        }
        assert_eq!(display_name("someos", ""), "someos", "with no os-release ID only the generic names count");
    }

    #[test]
    fn this_pc_is_virtual() {
        assert!(is_this_pc(&this_pc()));
        assert!(!is_this_pc(Path::new("/")));
    }
}

/// Whether `p` is a mount point: a different device than its parent folder.
pub fn is_mount(p: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(m), Some(parent)) = (std::fs::metadata(p), p.parent()) else { return false };
    std::fs::metadata(parent).map(|pm| pm.dev() != m.dev()).unwrap_or(false)
}

#[cfg(test)]
mod mount_tests {
    use super::*;

    #[test]
    fn an_empty_left_behind_folder_is_not_a_drive() {
        let d = std::env::temp_dir().join(format!("minos-pd-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::env::set_var("MINOS_PROTON_DRIVE", &d);
        assert_eq!(proton_drive().unwrap().state, CloudState::NotMounted);
        std::fs::write(d.join("file.txt"), "x").unwrap();
        assert_eq!(proton_drive().unwrap().state, CloudState::Ready);
        std::env::remove_var("MINOS_PROTON_DRIVE");
        let _ = std::fs::remove_dir_all(&d);
        assert!(!is_mount(Path::new("/tmp/does-not-exist-here")));
    }

    const MOUNTINFO: &str = "\
22 1 0:21 / / rw,relatime shared:1 - btrfs /dev/nvme0n1p2 rw\n\
40 22 0:40 / /home/u/ProtonDrive rw,nosuid shared:20 - fuse.rclone proton: rw,user_id=1000\n\
41 22 0:41 / /home/u/Proton\\040Drive rw shared:21 - nfs4 server:/x rw\n\
42 22 8:17 / /mnt/windows rw shared:22 - fuseblk /dev/sdb1 rw\n\
43 40 0:43 / /home/u/ProtonDrive/local rw shared:23 - ext4 /dev/sdc1 rw\n";

    #[test]
    fn a_folder_on_fuse_or_the_network_may_stall_and_a_disk_does_not() {
        let stalls = |p: &str| stalls_in(MOUNTINFO, Path::new(p));
        assert!(stalls("/home/u/ProtonDrive"));
        assert!(stalls("/home/u/ProtonDrive/Documents/My Games"));
        assert!(stalls("/home/u/Proton Drive/x"), "a space in the mount point (\\040)");
        assert!(!stalls("/home/u/ProtonDriveX"), "a longer name is not inside the mount");
        assert!(!stalls("/home/u/Documents"));
        assert!(!stalls("/mnt/windows/Games"), "fuseblk is a local disk");
        assert!(!stalls("/home/u/ProtonDrive/local/a"), "a disk mounted inside the FUSE mount");
        assert_eq!(unescape_mount("a\\040b\\134c\\999"), "a b\\c\\999");
    }

    #[test]
    fn a_size_count_skips_network_cloud_and_kernel_mounts() {
        let info = format!("{MOUNTINFO}50 22 0:5 / /proc rw - proc proc rw\n51 22 0:6 / /sys rw - sysfs sysfs rw\n");
        let skips = count_skips_in(&info);
        assert_eq!(
            skips,
            ["/home/u/ProtonDrive", "/home/u/Proton Drive", "/proc", "/sys"].map(PathBuf::from).to_vec(),
            "not the root, not fuseblk, not a local disk inside the FUSE mount"
        );
        assert!(may_stall_in(&info, Path::new("/home/u/ProtonDrive/x")));
        assert!(!may_stall_in(&info, Path::new("/home/u")));
    }

    #[test]
    fn a_disks_size_comes_from_statvfs() {
        let (total, free) = space(Path::new("/")).unwrap();
        assert!(total > 0 && free <= total);
        assert!(space(Path::new("/does/not/exist")).is_none());
    }
}
