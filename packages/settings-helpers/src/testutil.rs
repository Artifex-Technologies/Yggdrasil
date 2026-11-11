//! For the tests: a temporary folder that goes away, and one lock for the tests that set environment variables (they are
//! the process's, shared by the tests running at the same time).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

pub struct TempDir(pub PathBuf);

impl Default for TempDir {
    fn default() -> TempDir {
        TempDir::new()
    }
}

impl TempDir {
    pub fn new() -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("settings-helpers-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    /// A path under it, as text.
    pub fn p(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static ENV: Mutex<()> = Mutex::new(());

/// Held while a test sets and reads environment variables.
pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV.lock().unwrap_or_else(|e| e.into_inner())
}
