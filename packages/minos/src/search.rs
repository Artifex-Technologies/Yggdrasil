//! Recursive search that runs on a background thread and streams results.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

pub struct Search {
    pub rx: Receiver<PathBuf>,
    cancel: Arc<AtomicBool>,
}

impl Search {
    /// Start walking `root`, sending every path whose file name contains `query`
    /// (case-insensitive). Stops after `limit` hits. Does not follow symlinks.
    pub fn start(root: PathBuf, query: String, show_hidden: bool, limit: usize, on_hit: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            let q = query.to_lowercase();
            let mut stack = vec![root];
            let mut hits = 0;
            while let Some(dir) = stack.pop() {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !show_hidden && name.starts_with('.') {
                        continue;
                    }
                    let Ok(ft) = e.file_type() else { continue };
                    if name.to_lowercase().contains(&q) {
                        if tx.send(e.path()).is_err() {
                            return;
                        }
                        on_hit();
                        hits += 1;
                        if hits >= limit {
                            return;
                        }
                    }
                    if ft.is_dir() {
                        stack.push(e.path());
                    }
                }
            }
        });
        Self { rx, cancel }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn display_relative(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn finds_nested_matches_and_respects_hidden() {
        let d = std::env::temp_dir().join(format!("lf-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::create_dir_all(d.join(".hid")).unwrap();
        std::fs::write(d.join("a/b/Song One.flac"), "").unwrap();
        std::fs::write(d.join(".hid/song two.flac"), "").unwrap();
        std::fs::write(d.join("other.txt"), "").unwrap();

        let collect = |hidden| {
            let s = Search::start(d.clone(), "song".into(), hidden, 100, || {});
            let mut v = vec![];
            while let Ok(p) = s.rx.recv_timeout(Duration::from_secs(2)) {
                v.push(p);
            }
            v.sort();
            v
        };
        assert_eq!(collect(false), vec![d.join("a/b/Song One.flac")]);
        assert_eq!(collect(true).len(), 2);
    }
}
