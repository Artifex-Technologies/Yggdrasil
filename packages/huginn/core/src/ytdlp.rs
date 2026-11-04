//! The desktop extractor: yt-dlp (https://github.com/yt-dlp/yt-dlp, public domain), run as a program with its JSON output. It is
//! the part that has to keep up with YouTube, and yt-dlp's people do that; Huginn only reads its answers. Ids are checked before they
//! go on a command line, and every URL Huginn builds starts with https://www.youtube.com, so nothing a page says becomes an option.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::model::{valid_channel_id, valid_video_id, Channel, Chapter, Details, Streams, Video};
use crate::{Error, Extractor};

pub struct YtDlp {
    pub program: String,
    pub timeout: Duration,
}

impl Default for YtDlp {
    fn default() -> Self {
        YtDlp { program: "yt-dlp".into(), timeout: Duration::from_secs(60) }
    }
}

impl YtDlp {
    pub fn available(&self) -> bool {
        Command::new(&self.program).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    }

    fn json(&self, args: &[&str]) -> Result<Value, Error> {
        let mut child = Command::new(&self.program)
            .args(["--no-warnings", "--ignore-config", "-J"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| Error::Missing("yt-dlp is not installed".into()))?;
        let start = Instant::now();
        // read the output while waiting, so a big answer does not fill the pipe and stall the program
        let mut out = child.stdout.take().ok_or_else(|| Error::Failed("no output".into()))?;
        let mut err = child.stderr.take().ok_or_else(|| Error::Failed("no output".into()))?;
        let reader = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = std::io::Read::read_to_end(&mut out, &mut s);
            s
        });
        let err_reader = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = std::io::Read::read_to_end(&mut err, &mut s);
            s
        });
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if start.elapsed() > self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Failed("yt-dlp took too long".into()));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(Error::Failed(e.to_string())),
            }
        }
        let status = child.wait().map_err(|e| Error::Failed(e.to_string()))?;
        let stdout = reader.join().unwrap_or_default();
        let stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).to_string();
        if !status.success() {
            let line = stderr.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("yt-dlp failed").trim();
            return Err(Error::Failed(line.trim_start_matches("ERROR:").trim().chars().take(300).collect()));
        }
        serde_json::from_slice(&stdout).map_err(|e| Error::Failed(format!("yt-dlp's answer could not be read: {e}")))
    }
}

/// The yt-dlp format choice for a picture of at most `height` lines: H.264 with AAC first (the 2080 Ti and most cards decode it
/// in hardware), then any codec, then a single file with both.
pub fn format_for(height: u32) -> String {
    format!("bv*[height<={height}][vcodec^=avc1]+ba[ext=m4a]/bv*[height<={height}]+ba/b[height<={height}]/b")
}

impl Extractor for YtDlp {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<Video>, Error> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }
        // the search prefix means the argument never starts with '-'
        let arg = format!("ytsearch{}:{q}", limit.clamp(1, 60));
        let v = self.json(&["--flat-playlist", "--", &arg])?;
        Ok(parse_list(&v))
    }

    fn channel(&self, id: &str, limit: usize) -> Result<(Channel, Vec<Video>), Error> {
        if !valid_channel_id(id) {
            return Err(Error::Failed(format!("not a channel id: {id}")));
        }
        let url = format!("https://www.youtube.com/channel/{id}/videos");
        let end = limit.clamp(1, 100).to_string();
        let v = self.json(&["--flat-playlist", "--playlist-end", &end, "--", &url])?;
        Ok(parse_channel(&v, id))
    }

    fn details(&self, id: &str, max_height: u32, audio_only: bool) -> Result<Details, Error> {
        if !valid_video_id(id) {
            return Err(Error::Failed(format!("not a video id: {id}")));
        }
        let url = format!("https://www.youtube.com/watch?v={id}");
        let fmt = if audio_only { "ba[ext=m4a]/ba".to_string() } else { format_for(max_height) };
        let v = self.json(&["--no-playlist", "-f", &fmt, "--", &url])?;
        Ok(parse_details(&v))
    }
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)))
}

/// The best thumbnail of an entry (the widest), or the standard one for its id.
fn thumbnail(v: &Value, id: &str) -> Option<String> {
    let best = v.get("thumbnails").and_then(Value::as_array).and_then(|t| {
        t.iter().filter(|x| x.get("url").and_then(Value::as_str).is_some_and(|u| u.starts_with("https://")))
            .max_by_key(|x| u(x, "width").unwrap_or(0))
            .and_then(|x| x.get("url").and_then(Value::as_str).map(str::to_string))
    });
    best.or_else(|| valid_video_id(id).then(|| format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")))
}

pub fn parse_video(e: &Value) -> Option<Video> {
    let id = s(e, "id");
    if !valid_video_id(&id) {
        return None; // a playlist or a channel among the results
    }
    Some(Video {
        title: s(e, "title"),
        channel: { let c = s(e, "channel"); if c.is_empty() { s(e, "uploader") } else { c } },
        channel_id: s(e, "channel_id"),
        duration: u(e, "duration"),
        views: u(e, "view_count"),
        published: e.get("upload_date").and_then(Value::as_str).map(str::to_string),
        thumbnail: thumbnail(e, &id),
        live: matches!(e.get("live_status").and_then(Value::as_str), Some("is_live")),
        id,
    })
}

pub fn parse_list(v: &Value) -> Vec<Video> {
    v.get("entries").and_then(Value::as_array).map(|a| a.iter().filter_map(parse_video).collect()).unwrap_or_default()
}

pub fn parse_channel(v: &Value, id: &str) -> (Channel, Vec<Video>) {
    // the avatar is the square picture among the channel's thumbnails
    let avatar = v.get("thumbnails").and_then(Value::as_array).and_then(|t| {
        t.iter().filter(|x| u(x, "width").is_some() && u(x, "width") == u(x, "height")).max_by_key(|x| u(x, "width").unwrap_or(0))
            .and_then(|x| x.get("url").and_then(Value::as_str).map(str::to_string))
    });
    let name = { let c = s(v, "channel"); if c.is_empty() { s(v, "uploader") } else { c } };
    let mut videos = parse_list(v);
    for vid in &mut videos {
        if vid.channel.is_empty() {
            vid.channel = name.clone();
        }
        if vid.channel_id.is_empty() {
            vid.channel_id = id.to_string();
        }
    }
    (Channel { id: id.to_string(), name, avatar, followers: u(v, "channel_follower_count") }, videos)
}

pub fn parse_details(v: &Value) -> Details {
    let video = parse_video(v).unwrap_or_default();
    let mut streams = Streams::default();
    if let Some(parts) = v.get("requested_formats").and_then(Value::as_array) {
        for f in parts {
            let url = s(f, "url");
            if s(f, "vcodec") != "none" && !s(f, "vcodec").is_empty() {
                streams.video = Some(url);
                streams.height = u(f, "height").map(|h| h as u32);
            } else {
                streams.audio = Some(url);
            }
        }
    } else if !s(v, "url").is_empty() {
        // one file with both, or sound only
        if s(v, "vcodec") == "none" {
            streams.audio = Some(s(v, "url"));
        } else {
            streams.video = Some(s(v, "url"));
            streams.height = u(v, "height").map(|h| h as u32);
        }
    }
    let chapters = v.get("chapters").and_then(Value::as_array).map(|c| {
        c.iter().map(|x| Chapter { start: x.get("start_time").and_then(Value::as_f64).unwrap_or(0.0), title: s(x, "title") }).collect()
    }).unwrap_or_default();
    Details { video, description: s(v, "description"), likes: u(v, "like_count"), followers: u(v, "channel_follower_count"), chapters, streams }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_results_keep_videos_only() {
        let v: Value = serde_json::from_str(r#"{"entries":[
            {"id":"3lJ5oT_JviI","title":"Why Gaming on Linux Suddenly Matters","channel":"Techquickie","channel_id":"UC0vBXGSyV14uvJ4hECDOl0Q",
             "duration":474,"view_count":756582,"thumbnails":[{"url":"https://i.ytimg.com/a.jpg","width":360},{"url":"https://i.ytimg.com/b.jpg","width":720}]},
            {"id":"PLxxxxxxxxxxxxxxxxxx","title":"a playlist"},
            {"id":"--exec=boom","title":"hostile"}]}"#).unwrap();
        let l = parse_list(&v);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].thumbnail.as_deref(), Some("https://i.ytimg.com/b.jpg"), "the widest picture");
        assert_eq!(l[0].views, Some(756582));
    }

    #[test]
    fn details_with_separate_picture_and_sound() {
        let v: Value = serde_json::from_str(r#"{"id":"3lJ5oT_JviI","title":"T","channel":"C","channel_id":"UC0vBXGSyV14uvJ4hECDOl0Q",
            "description":"About","like_count":25193,"upload_date":"20260514",
            "chapters":[{"start_time":0.0,"title":"Intro"},{"start_time":61.5,"title":"Part two"}],
            "requested_formats":[{"format_id":"137","vcodec":"avc1.640028","acodec":"none","height":1080,"url":"https://v"},
                                 {"format_id":"140","vcodec":"none","acodec":"mp4a.40.2","url":"https://a"}]}"#).unwrap();
        let d = parse_details(&v);
        assert_eq!(d.streams, Streams { video: Some("https://v".into()), audio: Some("https://a".into()), height: Some(1080) });
        assert_eq!(d.chapters[1].title, "Part two");
        assert_eq!(d.video.published.as_deref(), Some("20260514"));
    }

    #[test]
    fn a_single_file_and_sound_only() {
        let v: Value = serde_json::from_str(r#"{"id":"3lJ5oT_JviI","vcodec":"none","url":"https://a"}"#).unwrap();
        assert_eq!(parse_details(&v).streams.audio.as_deref(), Some("https://a"));
        let v: Value = serde_json::from_str(r#"{"id":"3lJ5oT_JviI","vcodec":"avc1","height":360,"url":"https://b"}"#).unwrap();
        assert_eq!(parse_details(&v).streams.video.as_deref(), Some("https://b"));
    }

    #[test]
    fn channel_listing_fills_the_channel_in() {
        let v: Value = serde_json::from_str(r#"{"channel":"Techquickie","channel_follower_count":4300000,
            "thumbnails":[{"url":"https://banner","width":1060,"height":175},{"url":"https://avatar","width":900,"height":900}],
            "entries":[{"id":"Nm8iwVQ9O-A","title":"Why Is Your Monitor Set to 59.94 Hz?","duration":483}]}"#).unwrap();
        let (c, vids) = parse_channel(&v, "UC0vBXGSyV14uvJ4hECDOl0Q");
        assert_eq!((c.name.as_str(), c.avatar.as_deref(), c.followers), ("Techquickie", Some("https://avatar"), Some(4300000)));
        assert_eq!((vids[0].channel.as_str(), vids[0].channel_id.as_str()), ("Techquickie", "UC0vBXGSyV14uvJ4hECDOl0Q"));
    }

    #[test]
    fn bad_ids_never_reach_the_program() {
        let y = YtDlp { program: "/nonexistent/yt-dlp".into(), ..Default::default() };
        assert!(matches!(y.details("-o/etc/x", 1080, false), Err(Error::Failed(_))));
        assert!(matches!(y.channel("--exec", 10), Err(Error::Failed(_))));
    }
}
