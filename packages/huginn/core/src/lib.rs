//! Huginn's core: everything that is not the window. Videos and channels (`model`), the user's library on this device (`library`:
//! subscriptions without an account, the feed, history, saved videos), bringing subscriptions from NewPipe, FreeTube, OpenTubeX and
//! Google Takeout (`import`), and the extractors behind one small interface (`Extractor`).
//!
//! Extractors: `ytdlp` runs yt-dlp (the desktop). The interface is all the app uses, so another one (Invidious's API, or a native
//! one) can take its place without the app changing; iOS cannot run yt-dlp and will need one of those.
//!
//! Never here: a UI, a desktop-only path, an account, anything sent anywhere but to the extractor's own source.

pub mod import;
pub mod library;
pub mod model;
pub mod ytdlp;

use model::{Channel, Details, Video};

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// What is needed is not there (yt-dlp not installed).
    Missing(String),
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Missing(s) | Error::Failed(s) => f.write_str(s),
        }
    }
}

/// Where videos come from.
pub trait Extractor: Send + Sync {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<Video>, Error>;
    /// A channel and its latest videos (newest first).
    fn channel(&self, id: &str, limit: usize) -> Result<(Channel, Vec<Video>), Error>;
    /// A video's page: description, chapters, and the streams to play (picture at most `max_height` lines, or sound only).
    fn details(&self, id: &str, max_height: u32, audio_only: bool) -> Result<Details, Error>;
}
