//! What Huginn shows: videos, channels, a video's details and the streams to play.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Video {
    /// YouTube's 11-character video id.
    pub id: String,
    pub title: String,
    pub channel: String,
    pub channel_id: String,
    /// Seconds; None for a live stream or when not known.
    pub duration: Option<u64>,
    pub views: Option<u64>,
    /// Upload date as YYYYMMDD, when known (a channel listing does not say).
    pub published: Option<String>,
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub live: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    /// The channel id (UC + 22 characters).
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub followers: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub start: f64,
    pub title: String,
}

/// What to play: a picture stream and, when YouTube sends them apart (above 720p), a sound stream.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Streams {
    pub video: Option<String>,
    pub audio: Option<String>,
    pub height: Option<u32>,
    /// The User-Agent and Referer yt-dlp reports for these streams: YouTube issued them for that client, and a player that asks with
    /// other headers can be refused (403). Every yt-dlp front end passes them on (mpv does).
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub referer: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Details {
    pub video: Video,
    pub description: String,
    pub likes: Option<u64>,
    pub followers: Option<u64>,
    pub chapters: Vec<Chapter>,
    pub streams: Streams,
}

/// "12:05", "1:02:33", "LIVE".
pub fn duration_text(v: &Video) -> String {
    if v.live {
        return "LIVE".into();
    }
    match v.duration {
        Some(s) if s >= 3600 => format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
        Some(s) => format!("{}:{:02}", s / 60, s % 60),
        None => String::new(),
    }
}

/// "756K views", "4.3M", "1,204".
pub fn count_text(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.1}B", n as f64 / 1e9).replace(".0B", "B"),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6).replace(".0M", "M"),
        n if n >= 10_000 => format!("{}K", n / 1000),
        n if n >= 1_000 => format!("{},{:03}", n / 1000, n % 1000),
        n => n.to_string(),
    }
}

/// A plain YouTube video id: 11 characters of letters, digits, `-` and `_`. Anything else is never passed on.
pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A channel id: `UC` and 22 characters of the same set.
pub fn valid_channel_id(id: &str) -> bool {
    id.len() == 24 && id.starts_with("UC") && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        let mut v = Video { duration: Some(474), ..Default::default() };
        assert_eq!(duration_text(&v), "7:54");
        v.duration = Some(3753);
        assert_eq!(duration_text(&v), "1:02:33");
        v.live = true;
        assert_eq!(duration_text(&v), "LIVE");
        assert_eq!(count_text(756_582), "756K");
        assert_eq!(count_text(4_300_000), "4.3M");
        assert_eq!(count_text(2_000_000), "2M");
        assert_eq!(count_text(1_204), "1,204");
    }

    #[test]
    fn ids() {
        assert!(valid_video_id("3lJ5oT_JviI"));
        assert!(!valid_video_id("--exec=evil") && !valid_video_id("3lJ5oT_Jvi") && !valid_video_id("3lJ5oT JviI"));
        assert!(valid_channel_id("UC0vBXGSyV14uvJ4hECDOl0Q"));
        assert!(!valid_channel_id("UC0vBXGSyV14uvJ4hECDOl0") && !valid_channel_id("XX0vBXGSyV14uvJ4hECDOl0Q"));
    }
}
