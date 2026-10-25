//! Where each video was left, so it can go on from there (as a streaming service does): ~/.local/state/yggdrasil/player.json.
//! A video watched to (nearly) the end is forgotten again, so it starts from the beginning next time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Less than this far in is not worth asking about.
const MIN_MS: i64 = 60_000;
/// Within this much of the end counts as watched.
const END_MS: i64 = 180_000;
/// At most this many videos are remembered (the oldest go first).
const KEEP: usize = 500;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Spot {
    pub at_ms: i64,
    pub length_ms: i64,
    /// Seconds since 1970, when it was last watched.
    pub when: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    pub spots: HashMap<String, Spot>,
}

/// Is this a place worth going back to? (Not the first minute, not the closing credits.)
pub fn worth_resuming(at_ms: i64, length_ms: i64) -> bool {
    at_ms >= MIN_MS && (length_ms <= 0 || at_ms < length_ms - END_MS.min(length_ms / 10))
}

fn path() -> PathBuf {
    yggdrasil_conf::dirs::state().join("player.json")
}

fn key(video: &Path) -> String {
    video.canonicalize().unwrap_or_else(|_| video.to_path_buf()).to_string_lossy().into_owned()
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Store {
    pub fn load() -> Store {
        std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let p = path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(text) = serde_json::to_vec(self) {
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(tmp, p);
            }
        }
    }

    /// Where to offer to continue this video from, if anywhere.
    pub fn spot(&self, video: &Path) -> Option<&Spot> {
        self.spots.get(&key(video)).filter(|s| worth_resuming(s.at_ms, s.length_ms))
    }

    /// Remembers how far it got (or forgets it, near the end or in the first minute).
    pub fn note(&mut self, video: &Path, at_ms: i64, length_ms: i64) {
        let k = key(video);
        if worth_resuming(at_ms, length_ms) {
            self.spots.insert(k, Spot { at_ms, length_ms, when: now() });
            if self.spots.len() > KEEP {
                if let Some(oldest) = self.spots.iter().min_by_key(|(_, s)| s.when).map(|(k, _)| k.clone()) {
                    self.spots.remove(&oldest);
                }
            }
        } else if length_ms > 0 && at_ms >= length_ms - END_MS.min(length_ms / 10) {
            self.spots.remove(&k);
        }
    }

    /// The videos to go on with, the latest first (only those still on the disk).
    pub fn recent(&self, n: usize) -> Vec<(PathBuf, Spot)> {
        let mut v: Vec<(PathBuf, Spot)> = self.spots.iter().filter(|(k, s)| worth_resuming(s.at_ms, s.length_ms) && Path::new(k).exists()).map(|(k, s)| (PathBuf::from(k), s.clone())).collect();
        v.sort_by(|a, b| b.1.when.cmp(&a.1.when));
        v.truncate(n);
        v
    }
}

/// 1:02:03, or 2:03 under an hour.
pub fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_minute_and_the_credits_are_not_worth_resuming() {
        let film = 2 * 3600 * 1000;
        assert!(!worth_resuming(30_000, film));
        assert!(worth_resuming(3_600_000, film));
        assert!(!worth_resuming(film - 60_000, film), "credits");
        // a short clip: the last tenth counts as the end, not the last three minutes
        assert!(worth_resuming(200_000, 600_000));
        assert!(!worth_resuming(560_000, 600_000));
    }

    #[test]
    fn watching_to_the_end_forgets_the_spot() {
        let mut s = Store::default();
        let v = Path::new("/nonexistent/film.mkv");
        s.note(v, 3_600_000, 7_200_000);
        assert_eq!(s.spot(v).map(|x| x.at_ms), Some(3_600_000));
        s.note(v, 7_190_000, 7_200_000);
        assert!(s.spot(v).is_none());
    }

    #[test]
    fn times_read_like_a_clock() {
        assert_eq!(clock(3_723_000), "1:02:03");
        assert_eq!(clock(123_000), "2:03");
        assert_eq!(clock(-5), "0:00");
    }
}
