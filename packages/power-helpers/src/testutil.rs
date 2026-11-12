//! For the tests: a temporary folder that goes away, and a context on it that runs no command and counts as root.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::sys::Ctx;

pub struct TempDir(pub PathBuf);

impl TempDir {
    // new() makes a folder on disk: a Default doing that would surprise
    #[allow(clippy::new_without_default)]
    pub fn new() -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("power-helpers-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Writes files (name, text) into a folder under it, making the folder.
    pub fn files(&self, dir: &str, files: &[(&str, &str)]) -> PathBuf {
        let d = self.0.join(dir);
        fs::create_dir_all(&d).unwrap();
        for (name, text) in files {
            fs::write(d.join(name), text).unwrap();
        }
        d
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A context whose files are under `root`, that runs no command (it records them) and counts as root.
pub fn ctx(root: &Path) -> Ctx {
    Ctx::for_tests(root.to_path_buf())
}
