//! File operations: copy, move, rename, new folder, trash.

use std::io;
use std::path::{Path, PathBuf};

/// `name`, or `name (2)`, `name (3)` ... if something already exists in `dir`.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

fn copy_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    if src.is_dir() {
        // Refuse to copy a folder into itself: it would recurse forever.
        if dst.starts_with(src) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy a folder into itself"));
        }
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_recursive(&e.path(), &dst.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

/// A drive or filesystem root ("/", "C:\\") has no parent and must never be moved, copied or renamed:
/// moving one would copy a whole disk and then delete it.
fn refuse_root(p: &Path) -> io::Result<()> {
    if p.parent().is_none() || p.as_os_str().is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "refusing to operate on a drive or filesystem root"));
    }
    Ok(())
}

/// Copy `src` into directory `dest_dir`, never overwriting. Returns the new path.
pub fn copy_into(src: &Path, dest_dir: &Path) -> io::Result<PathBuf> {
    refuse_root(src)?;
    let name = src.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let dst = unique_path(dest_dir, &name.to_string_lossy());
    copy_recursive(src, &dst)?;
    Ok(dst)
}

/// Move `src` into `dest_dir`, never overwriting. Falls back to copy + delete
/// across filesystems. Moving onto its own folder is a no-op returning `src`.
pub fn move_into(src: &Path, dest_dir: &Path) -> io::Result<PathBuf> {
    refuse_root(src)?;
    if src.parent() == Some(dest_dir) {
        return Ok(src.to_path_buf());
    }
    if dest_dir.starts_with(src) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot move a folder into itself"));
    }
    let name = src.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let dst = unique_path(dest_dir, &name.to_string_lossy());
    if std::fs::rename(src, &dst).is_ok() {
        return Ok(dst);
    }
    copy_recursive(src, &dst)?;
    if src.is_dir() { std::fs::remove_dir_all(src)? } else { std::fs::remove_file(src)? }
    Ok(dst)
}

pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    refuse_root(path)?;
    if new_name.is_empty() || new_name.contains('/') || new_name.contains('\\') || new_name == "." || new_name == ".." {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid name"));
    }
    let dst = path.with_file_name(new_name);
    if dst.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a file with that name exists"));
    }
    std::fs::rename(path, &dst)?;
    Ok(dst)
}

pub fn new_folder(dir: &Path) -> io::Result<PathBuf> {
    let p = unique_path(dir, "New folder");
    std::fs::create_dir(&p)?;
    Ok(p)
}

pub fn new_file(dir: &Path) -> io::Result<PathBuf> {
    let p = unique_path(dir, "New file");
    std::fs::File::create_new(&p)?;
    Ok(p)
}

/// Send to the freedesktop trash (recoverable). Never deletes permanently.
pub fn trash(path: &Path) -> Result<(), String> {
    // A drive or filesystem root ("/", "C:\") has no parent. Never send one to the trash.
    if path.parent().is_none() || path.as_os_str().is_empty() {
        return Err("refusing to trash a drive or filesystem root".into());
    }
    trash::delete(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("lf-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn unique_names() {
        let d = tmp("unique");
        std::fs::write(d.join("a.txt"), "x").unwrap();
        assert_eq!(unique_path(&d, "a.txt"), d.join("a (2).txt"));
        std::fs::write(d.join("a (2).txt"), "x").unwrap();
        assert_eq!(unique_path(&d, "a.txt"), d.join("a (3).txt"));
        assert_eq!(unique_path(&d, "b"), d.join("b"));
    }

    #[test]
    fn copy_move_rename() {
        let d = tmp("ops");
        std::fs::create_dir_all(d.join("src/sub")).unwrap();
        std::fs::write(d.join("src/sub/f.txt"), "hi").unwrap();
        std::fs::create_dir(d.join("dest")).unwrap();

        let c = copy_into(&d.join("src"), &d.join("dest")).unwrap();
        assert_eq!(std::fs::read_to_string(c.join("sub/f.txt")).unwrap(), "hi");
        assert!(d.join("src/sub/f.txt").exists());

        let c2 = copy_into(&d.join("src"), &d.join("dest")).unwrap();
        assert_eq!(c2, d.join("dest/src (2)"));

        let m = move_into(&d.join("src"), &d.join("dest/src (2)")).unwrap();
        assert!(m.join("sub/f.txt").exists());
        assert!(!d.join("src").exists());

        let r = rename(&m, "renamed").unwrap();
        assert!(r.ends_with("renamed") && r.exists());
        assert!(rename(&r, "a/b").is_err());
        // "sub" now sits next to nothing, but renaming onto an existing sibling must fail.
        std::fs::create_dir(r.with_file_name("taken")).unwrap();
        assert!(rename(&r, "taken").is_err());
    }

    #[test]
    fn refuses_folder_into_itself() {
        let d = tmp("self");
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        assert!(copy_into(&d.join("a"), &d.join("a/b")).is_err());
        assert!(move_into(&d.join("a"), &d.join("a/b")).is_err());
        assert!(d.join("a/b").exists());
    }

    #[test]
    fn roots_are_never_trashed_and_new_file_does_not_overwrite() {
        assert!(trash(Path::new("/")).is_err());
        assert!(trash(Path::new("")).is_err());
        let d = tmp("nfile");
        assert_eq!(new_file(&d).unwrap(), d.join("New file"));
        assert_eq!(new_file(&d).unwrap(), d.join("New file (2)"));
    }

    #[test]
    fn roots_cannot_be_moved_copied_or_renamed() {
        let d = tmp("rootops");
        assert!(move_into(Path::new("/"), &d).is_err());
        assert!(copy_into(Path::new("/"), &d).is_err());
        assert!(rename(Path::new("/"), "x").is_err());
    }

    #[test]
    fn new_folder_does_not_overwrite() {
        let d = tmp("nf");
        assert_eq!(new_folder(&d).unwrap(), d.join("New folder"));
        assert_eq!(new_folder(&d).unwrap(), d.join("New folder (2)"));
    }
}
