//! Directory listing, sorting and filtering (no GUI types).

use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub hidden: bool,
    /// For a drive: (total bytes, free bytes). None for ordinary files and folders.
    pub usage: Option<(u64, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
}

#[derive(Debug, Clone, Copy)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self { key: SortKey::Name, descending: false }
    }
}

/// Read one directory. Entries that cannot be stat'ed are skipped, not fatal.
pub fn read_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let Ok(meta) = item.metadata() else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        out.push(Entry {
            hidden: name.starts_with('.'),
            name,
            path: item.path(),
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified: meta.modified().ok(),
            usage: None,
        });
    }
    Ok(out)
}

/// Folders first, then by `sort`. Name comparison is case-insensitive.
pub fn sort_entries(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| {
        let dirs = b.is_dir.cmp(&a.is_dir);
        if dirs != Ordering::Equal {
            return dirs;
        }
        let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
        let ord = match sort.key {
            SortKey::Name => by_name,
            SortKey::Size => a.size.cmp(&b.size).then(by_name),
            SortKey::Modified => a.modified.cmp(&b.modified).then(by_name),
        };
        if sort.descending { ord.reverse() } else { ord }
    });
}

/// Indices of entries that match `query` (case-insensitive substring) and the
/// hidden-files setting.
pub fn visible(entries: &[Entry], query: &str, show_hidden: bool) -> Vec<usize> {
    let q = query.to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| (show_hidden || !e.hidden) && (q.is_empty() || e.name.to_lowercase().contains(&q)))
        .map(|(i, _)| i)
        .collect()
}

/// "Folder", "PNG file", "File": the Type column.
pub fn type_name(e: &Entry) -> String {
    if e.usage.is_some() {
        return "Drive".into();
    }
    if e.is_dir {
        return "Folder".into();
    }
    match e.path.extension().map(|x| x.to_string_lossy().to_uppercase()) {
        Some(x) if !x.is_empty() && x.len() <= 8 => format!("{x} file"),
        _ => "File".into(),
    }
}

/// The "This PC" listing: one entry per drive, carrying its usage.
pub fn drive_entries(drives: &[crate::places::Drive]) -> Vec<Entry> {
    drives
        .iter()
        .map(|d| Entry {
            name: d.label.clone(),
            path: d.path.clone(),
            is_dir: true,
            size: d.total,
            modified: None,
            hidden: false,
            usage: Some((d.total, d.free)),
        })
        .collect()
}

/// The "This PC" listing: one entry per physical disk, carrying the usage of what is mounted on it.
pub fn disk_entries(disks: &[crate::places::Disk]) -> Vec<Entry> {
    disks
        .iter()
        .map(|d| Entry {
            name: d.label.clone(),
            path: crate::places::disk_path(&d.name),
            is_dir: true,
            size: d.size,
            modified: None,
            hidden: false,
            usage: Some(d.usage()),
        })
        .collect()
}

/// A disk's partitions. A mounted one opens at its mount point; one that is not mounted is listed but cannot be opened.
pub fn partition_entries(disk: &crate::places::Disk) -> Vec<Entry> {
    disk.parts
        .iter()
        .map(|p| Entry {
            name: p.label.clone(),
            path: p.mounts.first().cloned().unwrap_or_else(|| std::path::PathBuf::from(format!("{}{}", crate::places::PART_PREFIX, p.name))),
            is_dir: true,
            size: p.size,
            modified: None,
            hidden: false,
            usage: p.usage,
        })
        .collect()
}

/// Used fraction 0..=1 of a drive.
pub fn used_fraction(total: u64, free: u64) -> f32 {
    if total == 0 { 0.0 } else { (total.saturating_sub(free) as f64 / total as f64).clamp(0.0, 1.0) as f32 }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, is_dir: bool, size: u64) -> Entry {
        Entry { name: name.into(), path: name.into(), is_dir, size, modified: None, hidden: name.starts_with('.'), usage: None }
    }

    #[test]
    fn folders_first_then_case_insensitive_name() {
        let mut v = vec![e("b.txt", false, 1), e("Zdir", true, 0), e("A.txt", false, 5), e("adir", true, 0)];
        sort_entries(&mut v, Sort::default());
        let names: Vec<_> = v.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["adir", "Zdir", "A.txt", "b.txt"]);
    }

    #[test]
    fn size_descending_keeps_folders_first() {
        let mut v = vec![e("s", false, 1), e("big", false, 9), e("d", true, 0)];
        sort_entries(&mut v, Sort { key: SortKey::Size, descending: true });
        let names: Vec<_> = v.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["d", "big", "s"]);
    }

    #[test]
    fn filter_and_hidden() {
        let v = vec![e(".git", true, 0), e("Song.flac", false, 1), e("song.txt", false, 1), e("pic.png", false, 1)];
        assert_eq!(visible(&v, "song", false), vec![1, 2]);
        assert_eq!(visible(&v, "", false), vec![1, 2, 3]);
        assert_eq!(visible(&v, "", true), vec![0, 1, 2, 3]);
    }

    #[test]
    fn type_names_and_usage() {
        assert_eq!(type_name(&e("a.png", false, 1)), "PNG file");
        assert_eq!(type_name(&e("noext", false, 1)), "File");
        assert_eq!(type_name(&e("dir", true, 0)), "Folder");
        assert_eq!(type_name(&e("archive.verylongextension", false, 1)), "File");
        let mut d = e("C", true, 0);
        d.usage = Some((100, 8));
        assert_eq!(type_name(&d), "Drive");
        assert!((used_fraction(100, 8) - 0.92).abs() < 1e-6);
        assert_eq!(used_fraction(0, 0), 0.0);
        assert_eq!(used_fraction(100, 500), 0.0);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(10), "10 B");
        assert_eq!(human_size(1536), "1.5 KB");
    }
}
