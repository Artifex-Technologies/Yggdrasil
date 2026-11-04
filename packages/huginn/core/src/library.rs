//! The user's library, kept on this device only: subscriptions (no account), the feed made from them, watch history and saved
//! videos. Plain JSON files in one folder the app names (on the desktop ~/.local/share/yggdrasil/huginn).
//!
//! The feed: a channel listing does not say when a video came out, so each video is stamped with when Huginn first saw it, and the
//! feed is newest-first by that (with the channel's own order inside one refresh). From the second refresh on, new uploads are on
//! top within the refresh interval; a video's own page gives its real date.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{Channel, Video};
use crate::{Error, Extractor};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Watched {
    pub video: Video,
    /// Unix seconds.
    pub at: u64,
    /// Where playback stopped, in seconds, when known.
    #[serde(default)]
    pub position: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct FeedItem {
    video: Video,
    /// (first seen, place in its channel's listing): the feed's order
    seen: u64,
    place: u32,
}

#[derive(Debug, Default)]
pub struct Library {
    dir: PathBuf,
    pub subscriptions: Vec<Channel>,
    pub history: Vec<Watched>,
    pub saved: Vec<Video>,
    feed: Vec<FeedItem>,
    /// When the feed was last refreshed (unix seconds).
    pub refreshed: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct FeedFile {
    refreshed: u64,
    items: Vec<FeedItem>,
}

fn load<T: for<'de> Deserialize<'de> + Default>(p: &Path) -> T {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save<T: Serialize>(p: &Path, v: &T) -> Result<(), Error> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| Error::Failed(e.to_string()))?;
    }
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(v).map_err(|e| Error::Failed(e.to_string()))?).map_err(|e| Error::Failed(e.to_string()))?;
    std::fs::rename(&tmp, p).map_err(|e| Error::Failed(e.to_string()))
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// How many entries the history keeps.
pub const HISTORY_MAX: usize = 2000;
/// How many videos the feed keeps.
pub const FEED_MAX: usize = 600;

impl Library {
    pub fn open(dir: &Path) -> Library {
        let feed: FeedFile = load(&dir.join("feed.json"));
        Library {
            dir: dir.to_path_buf(),
            subscriptions: load(&dir.join("subscriptions.json")),
            history: load(&dir.join("history.json")),
            saved: load(&dir.join("saved.json")),
            feed: feed.items,
            refreshed: feed.refreshed,
        }
    }

    pub fn save(&self) -> Result<(), Error> {
        save(&self.dir.join("subscriptions.json"), &self.subscriptions)?;
        save(&self.dir.join("history.json"), &self.history)?;
        save(&self.dir.join("saved.json"), &self.saved)?;
        save(&self.dir.join("feed.json"), &FeedFile { refreshed: self.refreshed, items: self.feed.clone() })
    }

    pub fn subscribed(&self, channel_id: &str) -> bool {
        self.subscriptions.iter().any(|c| c.id == channel_id)
    }

    /// Adds a subscription (or refreshes its name and picture); returns whether it is new.
    pub fn subscribe(&mut self, c: Channel) -> bool {
        if let Some(old) = self.subscriptions.iter_mut().find(|x| x.id == c.id) {
            if !c.name.is_empty() {
                old.name = c.name;
            }
            if c.avatar.is_some() {
                old.avatar = c.avatar;
            }
            if c.followers.is_some() {
                old.followers = c.followers;
            }
            return false;
        }
        self.subscriptions.push(c);
        self.subscriptions.sort_by_key(|c| c.name.to_lowercase());
        true
    }

    pub fn unsubscribe(&mut self, channel_id: &str) {
        self.subscriptions.retain(|c| c.id != channel_id);
        self.feed.retain(|f| f.video.channel_id != channel_id);
    }

    /// The feed, newest first.
    pub fn feed(&self) -> Vec<&Video> {
        let mut f: Vec<&FeedItem> = self.feed.iter().collect();
        f.sort_by(|a, b| b.seen.cmp(&a.seen).then(a.place.cmp(&b.place)));
        f.into_iter().map(|x| &x.video).collect()
    }

    /// Merges one channel's latest videos into the feed; returns how many were new.
    pub fn merge_channel(&mut self, channel: &Channel, videos: Vec<Video>, at: u64) -> usize {
        let mut new = 0;
        for (place, v) in videos.into_iter().enumerate() {
            if let Some(old) = self.feed.iter_mut().find(|f| f.video.id == v.id) {
                // keep when it was first seen; take the newer facts (title changes, view counts)
                let published = old.video.published.clone();
                old.video = v;
                if old.video.published.is_none() {
                    old.video.published = published;
                }
            } else {
                self.feed.push(FeedItem { video: v, seen: at, place: place as u32 });
                new += 1;
            }
        }
        self.subscribe(channel.clone());
        // oldest out first when the feed is full
        if self.feed.len() > FEED_MAX {
            self.feed.sort_by(|a, b| b.seen.cmp(&a.seen).then(a.place.cmp(&b.place)));
            self.feed.truncate(FEED_MAX);
        }
        new
    }

    /// Asks the extractor for every subscription's latest videos. `progress` is told (done, total) after each channel; failures are
    /// collected, not fatal (one channel gone does not stop the rest).
    pub fn refresh(&mut self, x: &dyn Extractor, per_channel: usize, mut progress: impl FnMut(usize, usize)) -> (usize, Vec<String>) {
        let subs = self.subscriptions.clone();
        let at = now();
        let (mut new, mut errors) = (0, vec![]);
        for (i, c) in subs.iter().enumerate() {
            match x.channel(&c.id, per_channel) {
                Ok((ch, vids)) => new += self.merge_channel(&ch, vids, at),
                Err(e) => errors.push(format!("{}: {e}", c.name)),
            }
            progress(i + 1, subs.len());
        }
        self.refreshed = at;
        (new, errors)
    }

    pub fn watched(&mut self, video: Video, position: Option<u64>) {
        self.history.retain(|w| w.video.id != video.id);
        self.history.insert(0, Watched { video, at: now(), position });
        self.history.truncate(HISTORY_MAX);
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    pub fn toggle_saved(&mut self, v: &Video) -> bool {
        if let Some(i) = self.saved.iter().position(|x| x.id == v.id) {
            self.saved.remove(i);
            false
        } else {
            self.saved.insert(0, v.clone());
            true
        }
    }

    pub fn is_saved(&self, id: &str) -> bool {
        self.saved.iter().any(|v| v.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Details;

    fn vid(id: &str, ch: &str) -> Video {
        Video { id: id.into(), title: format!("t-{id}"), channel_id: ch.into(), ..Default::default() }
    }

    struct Fake;
    impl Extractor for Fake {
        fn search(&self, _: &str, _: usize) -> Result<Vec<Video>, Error> {
            Ok(vec![])
        }
        fn channel(&self, id: &str, _: usize) -> Result<(Channel, Vec<Video>), Error> {
            if id == "UCgonegonegonegonegone00" {
                return Err(Error::Failed("gone".into()));
            }
            Ok((Channel { id: id.into(), name: "Fresh name".into(), ..Default::default() }, vec![vid("aaaaaaaaaa1", id), vid("aaaaaaaaaa2", id)]))
        }
        fn details(&self, _: &str, _: u32, _: bool) -> Result<Details, Error> {
            Err(Error::Failed("no".into()))
        }
    }

    #[test]
    fn feed_order_refresh_and_files() {
        let d = std::env::temp_dir().join(format!("huginn-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mut l = Library::open(&d);
        let ch = Channel { id: "UC0vBXGSyV14uvJ4hECDOl0Q".into(), name: "Old".into(), ..Default::default() };
        assert!(l.subscribe(ch.clone()) && !l.subscribe(ch.clone()));
        l.merge_channel(&ch, vec![vid("old00000001", &ch.id), vid("old00000002", &ch.id)], 100);
        assert_eq!(l.merge_channel(&ch, vec![vid("new00000001", &ch.id), vid("old00000001", &ch.id)], 200), 1);
        assert_eq!(l.feed().iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["new00000001", "old00000001", "old00000002"]);
        l.subscribe(Channel { id: "UCgonegonegonegonegone00".into(), name: "Gone".into(), ..Default::default() });
        let (new, errs) = l.refresh(&Fake, 15, |_, _| {});
        assert_eq!((new, errs.len()), (2, 1), "one channel failing does not stop the others");
        assert_eq!(l.subscriptions.iter().find(|c| c.id == ch.id).unwrap().name, "Fresh name");
        l.watched(vid("new00000001", &ch.id), Some(42));
        l.watched(vid("new00000001", &ch.id), Some(50));
        assert_eq!(l.history.len(), 1, "watching again moves it to the top");
        assert!(l.toggle_saved(&vid("sav00000001", &ch.id)) && l.is_saved("sav00000001"));
        l.save().unwrap();
        let again = Library::open(&d);
        assert_eq!((again.subscriptions.len(), again.feed().len(), again.history[0].position, again.saved.len()), (2, 5, Some(50), 1));
        l.unsubscribe(&ch.id);
        assert!(l.feed().iter().all(|v| v.channel_id != ch.id));
        let _ = std::fs::remove_dir_all(&d);
    }
}
