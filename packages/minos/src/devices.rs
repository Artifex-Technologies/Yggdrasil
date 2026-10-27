//! Removable drives: noticing them come and go while Minos is open, opening (mounting), ejecting and formatting them through
//! udisks2 (the session's own rights: no password for a USB drive), and starting the tools for partitioning and for making a
//! bootable drive. Nothing here touches a disk the system lives on.

use crate::places::Disk;
use std::path::PathBuf;
use std::process::Command;

/// What changes when a drive is plugged in, unplugged, opened or ejected: the block devices and their sizes, and the mount table.
/// Cheap enough to read every couple of seconds (a small folder and one file in /proc).
pub fn signature() -> String {
    let mut s = String::new();
    if let Ok(rd) = std::fs::read_dir("/sys/block") {
        let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with("loop") && !n.starts_with("zram") && !n.starts_with("ram")).collect();
        names.sort();
        for n in names {
            let size = std::fs::read_to_string(format!("/sys/block/{n}/size")).unwrap_or_default();
            s.push_str(&format!("{n}:{} ", size.trim()));
        }
    }
    if let Ok(m) = std::fs::read_to_string("/proc/self/mounts") {
        for l in m.lines().filter(|l| l.starts_with("/dev/")) {
            s.push_str(l.split(' ').take(2).collect::<Vec<_>>().join(" ").as_str());
            s.push('\n');
        }
    }
    s
}

fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd).args(args).output().map_err(|e| format!("could not run {cmd}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim();
        Err(err.trim_start_matches("Error ").to_string())
    }
}

/// Mounts a partition (its device name, "sda1") and returns where it is now.
pub fn mount(part: &str) -> Result<PathBuf, String> {
    let out = run("udisksctl", &["mount", "--no-user-interaction", "-b", &format!("/dev/{part}")])?;
    // "Mounted /dev/sda1 at /run/media/me/STICK"
    out.trim().rsplit_once(" at ").map(|(_, p)| PathBuf::from(p.trim_end_matches('.'))).ok_or_else(|| format!("udisksctl said: {}", out.trim()))
}

/// The partitions of a removable disk that have a filesystem and are not mounted: opened when it is plugged in.
pub fn unmounted_parts(d: &Disk) -> Vec<String> {
    d.parts.iter().filter(|p| p.mounts.is_empty() && !p.fs.is_empty() && p.fs != "swap" && !p.fs.contains("LVM") && !p.fs.contains("crypto")).map(|p| p.name.clone()).collect()
}

fn refuse_system(d: &Disk) -> Result<(), String> {
    if d.holds_system() || !d.removable {
        return Err(format!("{} is not a removable drive; Minos only changes those", d.display_name()));
    }
    Ok(())
}

fn unmount_all(d: &Disk) -> Result<(), String> {
    for p in d.parts.iter().filter(|p| !p.mounts.is_empty()) {
        run("udisksctl", &["unmount", "--no-user-interaction", "-b", &format!("/dev/{}", p.name)]).map_err(|e| {
            if e.contains("busy") || e.contains("target is busy") { format!("{} is still in use (a file open from it, or a terminal in it); close that first", p.label) } else { e }
        })?;
    }
    Ok(())
}

/// Unmounts everything on the drive and powers it off, so it can be pulled out.
pub fn eject(d: &Disk) -> Result<(), String> {
    refuse_system(d)?;
    unmount_all(d)?;
    // a card reader cannot power off; unmounted is safe enough then
    let _ = run("udisksctl", &["power-off", "--no-user-interaction", "-b", &format!("/dev/{}", d.name)]);
    Ok(())
}

/// File systems a drive can be formatted as: (shown name, udisks type, MBR partition type, longest label, what it is good for).
pub const FORMATS: [(&str, &str, &str, usize, &str); 4] = [
    ("exFAT", "exfat", "0x07", 15, "Works on Windows, macOS and Linux, and holds files over 4 GB. Best for most USB drives."),
    ("FAT32", "vfat", "0x0c", 11, "Works almost everywhere (TVs, cars, cameras, firmware updates); no file over 4 GB."),
    ("NTFS", "ntfs", "0x07", 32, "Windows' own; read and written by Linux too, read-only on macOS."),
    ("ext4", "ext4", "0x83", 16, "Linux only; keeps Linux permissions."),
];

/// The label as the file system allows it: FAT32 keeps 11 upper-case characters, the others their own lengths.
pub fn clean_label(fs: usize, label: &str) -> String {
    let (_, kind, _, max, _) = FORMATS[fs];
    let mut l: String = label.chars().filter(|c| !c.is_control() && !"\"*/:<>?\\|".contains(*c)).take(max).collect();
    if kind == "vfat" {
        l = l.to_uppercase();
    }
    l.trim().to_string()
}

/// The busctl arguments for the format options a{sv}: the label, and for FAT32 "-F 32" to mkfs (it would pick FAT16 for a small
/// drive by itself).
fn format_options(kind: &str, label: &str) -> Vec<String> {
    let mut entries: Vec<Vec<String>> = vec![];
    if !label.is_empty() {
        entries.push(["label", "s", label].map(String::from).to_vec());
    }
    if kind == "vfat" {
        entries.push(["mkfs-args", "as", "2", "-F", "32"].map(String::from).to_vec());
    }
    std::iter::once(entries.len().to_string()).chain(entries.into_iter().flatten()).collect()
}

/// Erases the whole drive and makes it one partition of the chosen file system (an MBR table, which every computer reads).
pub fn format(d: &Disk, fs: usize, label: &str) -> Result<(), String> {
    refuse_system(d)?;
    let (_, kind, mbr, _, _) = FORMATS[fs];
    let tool = match kind {
        "exfat" => "mkfs.exfat",
        "vfat" => "mkfs.vfat",
        "ntfs" => "mkfs.ntfs",
        _ => "mkfs.ext4",
    };
    if !["/usr/bin", "/usr/sbin", "/sbin"].iter().any(|d| std::path::Path::new(d).join(tool).exists()) {
        let pkg = match kind {
            "exfat" => "exfatprogs",
            "vfat" => "dosfstools",
            "ntfs" => "ntfs-3g",
            _ => "e2fsprogs",
        };
        return Err(format!("formatting as {} needs the {pkg} package", FORMATS[fs].0));
    }
    unmount_all(d)?;
    let obj = format!("/org/freedesktop/UDisks2/block_devices/{}", d.name);
    let label = clean_label(fs, label);
    run("busctl", &["call", "--timeout=120", "org.freedesktop.UDisks2", &obj, "org.freedesktop.UDisks2.Block", "Format", "sa{sv}", "dos", "0"])?;
    // "--": the label and mkfs arguments may start with "-"
    let mut args: Vec<String> = ["call", "--timeout=600", "--", "org.freedesktop.UDisks2", &obj, "org.freedesktop.UDisks2.PartitionTable", "CreatePartitionAndFormat", "ttssa{sv}sa{sv}", "1048576", "0", mbr, "", "0", kind]
        .map(String::from)
        .to_vec();
    args.extend(format_options(kind, &label));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run("busctl", &args)?;
    Ok(())
}

/// A tool Minos can start for a drive: (shown name, program, package, what it does).
pub const BOOTABLE: [(&str, &str, &str, &str); 2] = [
    ("Write an ISO (Impression)", "impression", "impression", "Writes one Linux or other ISO image to the drive, like Rufus' default mode."),
    ("Multi-ISO boot drive (Ventoy)", "ventoygui", "ventoy-bin", "Installs Ventoy once; then copy any number of ISOs (Windows ones too) onto the drive and pick one at boot."),
];
pub const PARTITIONER: (&str, &str) = ("gparted", "gparted");

pub fn installed(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// Starts a tool, detached. `Err` names the package to install when it is missing.
pub fn launch(program: &str, package: &str) -> Result<(), String> {
    if !installed(program) {
        return Err(package.to_string());
    }
    Command::new(program).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(|_| ()).map_err(|e| format!("could not start {program}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::places::Partition;

    fn disk(removable: bool, mount: &str) -> Disk {
        let mounts = if mount.is_empty() { vec![] } else { vec![PathBuf::from(mount)] };
        Disk { name: "sda".into(), label: "UDisk (sda)".into(), size: 4 << 30, removable, model: "UDisk".into(), parts: vec![Partition { name: "sda1".into(), label: "Partition (sda1)".into(), volume: String::new(), fs: "vfat".into(), size: 4 << 30, mounts, usage: None }] }
    }

    #[test]
    fn only_removable_drives_without_the_system_are_changed() {
        assert!(refuse_system(&disk(true, "")).is_ok());
        assert!(refuse_system(&disk(false, "")).is_err());
        assert!(refuse_system(&disk(true, "/")).is_err());
        assert!(refuse_system(&disk(true, "/boot/efi")).is_err());
    }

    #[test]
    fn labels_fit_the_file_system() {
        assert_eq!(clean_label(1, "my stick 2026 extra"), "MY STICK 20");
        assert_eq!(clean_label(0, "Photos: \"old\""), "Photos old");
        assert_eq!(format_options("exfat", ""), vec!["0"]);
        assert_eq!(format_options("vfat", "STICK"), vec!["2", "label", "s", "STICK", "mkfs-args", "as", "2", "-F", "32"]);
    }

    #[test]
    fn an_unplugged_drive_changes_the_signature_and_the_names_read() {
        let d = disk(true, "");
        assert_eq!(d.display_name(), "USB Drive");
        assert_eq!(unmounted_parts(&d), vec!["sda1"]);
        assert!(!signature().is_empty());
    }
}
