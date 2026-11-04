//! Work off the window's thread: pictures (a few at a time, from YouTube's picture hosts only, into the cache), the extractor's
//! answers, playback in yggdrasil-player and downloads through yt-dlp. Each sends a `Msg` back and wakes the window.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};

use eframe::egui;
use huginn_core::model::{valid_video_id, Channel, Details, Video};

pub enum Msg {
    Image(String, Option<PathBuf>),
    Search(String, Result<Vec<Video>, String>),
    Details(String, Result<Details, String>),
    Channel(String, Result<(Channel, Vec<Video>), String>),
    /// One subscription's latest videos during a refresh, then (done, total)
    FeedChannel(Channel, Vec<Video>, usize, usize),
    FeedFailed(String, usize, usize),
    FeedDone,
    /// Playback started, or why not
    Played(Result<String, String>),
}

/// Where pictures may come from: YouTube's own picture hosts (thumbnails and channel pictures), over https.
pub fn picture_host_ok(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split('/').next().unwrap_or("");
    matches!(host, "i.ytimg.com" | "i1.ytimg.com" | "i2.ytimg.com" | "i3.ytimg.com" | "i4.ytimg.com" | "i9.ytimg.com" | "yt3.ggpht.com" | "yt3.googleusercontent.com")
}

pub fn cache_dir() -> PathBuf {
    yggdrasil_conf::dirs::cache().join("huginn")
}

/// Downloads a picture into the cache (size-limited); its path. The same picture is fetched once.
pub fn fetch_picture(url: &str) -> Option<PathBuf> {
    if !picture_host_ok(url) {
        return None;
    }
    let mut h: u64 = 1469598103934665603;
    for b in url.bytes() {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    let dir = cache_dir().join("img");
    let _ = std::fs::create_dir_all(&dir);
    let ext = if url.contains(".webp") { "webp" } else if url.contains(".png") { "png" } else { "jpg" };
    let path = dir.join(format!("{h:016x}.{ext}"));
    if path.is_file() {
        return Some(path);
    }
    let tmp = dir.join(format!("{h:016x}.part{}", std::process::id()));
    let ok = std::process::Command::new("curl")
        .args(["-fsSL", "--proto", "=https", "--max-time", "30", "--max-filesize", "4000000", "-o"])
        .arg(&tmp)
        .arg("--")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok && std::fs::rename(&tmp, &path).is_ok() {
        Some(path)
    } else {
        let _ = std::fs::remove_file(&tmp);
        None
    }
}

/// Pictures to fetch, newest wish first (what is on screen now), by a few threads.
#[derive(Clone)]
pub struct Pictures {
    queue: Arc<(Mutex<VecDeque<String>>, Condvar)>,
}

impl Pictures {
    pub fn start(tx: Sender<Msg>, ctx: egui::Context, threads: usize) -> Pictures {
        let p = Pictures { queue: Arc::new((Mutex::new(VecDeque::new()), Condvar::new())) };
        for _ in 0..threads {
            let (q, tx, ctx) = (p.queue.clone(), tx.clone(), ctx.clone());
            std::thread::spawn(move || loop {
                let url = {
                    let (lock, cv) = &*q;
                    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while g.is_empty() {
                        g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                    }
                    g.pop_front()
                };
                if let Some(url) = url {
                    let path = fetch_picture(&url);
                    let _ = tx.send(Msg::Image(url, path));
                    ctx.request_repaint();
                }
            });
        }
        p
    }

    pub fn want(&self, url: &str) {
        let (lock, cv) = &*self.queue;
        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !g.iter().any(|u| u == url) {
            g.push_front(url.to_string());
            // pictures scrolled past long ago are not worth fetching first
            g.truncate(200);
        }
        cv.notify_one();
    }
}

/// Runs `f` on its own thread and sends its message.
pub fn spawn(tx: &Sender<Msg>, ctx: &egui::Context, f: impl FnOnce() -> Msg + Send + 'static) {
    let (tx, ctx) = (tx.clone(), ctx.clone());
    std::thread::spawn(move || {
        let _ = tx.send(f());
        ctx.request_repaint();
    });
}

/// Starts yggdrasil-player on the video's streams (from `start` seconds).
pub fn play(d: &Details, start: Option<f64>) -> Result<String, String> {
    let video = d.streams.video.clone().or_else(|| d.streams.audio.clone()).ok_or("no stream to play")?;
    let mut cmd = std::process::Command::new("yggdrasil-player");
    cmd.arg("--title").arg(&d.video.title);
    if let (Some(a), true) = (&d.streams.audio, d.streams.video.is_some()) {
        cmd.arg("--audio").arg(a);
    }
    if let Some(s) = start.filter(|s| *s > 0.0) {
        cmd.arg("--start").arg(format!("{s:.1}"));
    }
    if let Some(ua) = &d.streams.user_agent {
        cmd.arg("--user-agent").arg(ua);
    }
    if let Some(r) = &d.streams.referer {
        cmd.arg("--referrer").arg(r);
    }
    cmd.arg("--").arg(video).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.spawn().map(|_| d.video.title.clone()).map_err(|_| "yggdrasil-player is not installed".to_string())
}

#[derive(Clone, Debug, PartialEq)]
pub enum DlState {
    Running,
    Done,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Download {
    pub video: Video,
    pub audio_only: bool,
    pub progress: f32,
    pub state: DlState,
}

/// A download through yt-dlp into `folder` (a picture of at most `height` lines with its sound, or the sound only).
pub fn download(v: &Video, audio_only: bool, height: u32, folder: &Path, ctx: &egui::Context) -> Arc<Mutex<Download>> {
    let d = Arc::new(Mutex::new(Download { video: v.clone(), audio_only, progress: 0.0, state: DlState::Running }));
    if !valid_video_id(&v.id) {
        d.lock().unwrap().state = DlState::Failed("not a video id".into());
        return d;
    }
    let (job, ctx, folder, id) = (d.clone(), ctx.clone(), folder.to_path_buf(), v.id.clone());
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(&folder);
        let fmt = if audio_only { "ba[ext=m4a]/ba".to_string() } else { huginn_core::ytdlp::format_for(height) };
        let out = folder.join("%(title).150B [%(id)s].%(ext)s");
        let child = std::process::Command::new("yt-dlp")
            .args(["--no-warnings", "--ignore-config", "--newline", "--no-playlist", "-f", &fmt, "-o"])
            .arg(&out)
            .arg("--")
            .arg(format!("https://www.youtube.com/watch?v={id}"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(_) => {
                job.lock().unwrap().state = DlState::Failed("yt-dlp is not installed".into());
                ctx.request_repaint();
                return;
            }
        };
        if let Some(out) = child.stdout.take() {
            use std::io::BufRead;
            // "[download]  42.1% of 120.00MiB at ..." per line; a picture and its sound are two downloads, then a merge
            let (mut part, mut last) = (0, 0.0f32);
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(p) = line.strip_prefix("[download]").and_then(|r| r.trim().split('%').next()).and_then(|n| n.trim().parse::<f32>().ok()) {
                    if p < last - 50.0 {
                        part += 1;
                    }
                    last = p;
                    let parts = if audio_only { 1.0 } else { 2.0 };
                    job.lock().unwrap().progress = ((part as f32 + p / 100.0) / parts).min(0.99);
                    ctx.request_repaint();
                }
            }
        }
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut err);
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        let mut j = job.lock().unwrap();
        if ok {
            j.progress = 1.0;
            j.state = DlState::Done;
        } else {
            j.state = DlState::Failed(err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("the download stopped").trim_start_matches("ERROR:").trim().chars().take(200).collect());
        }
        ctx.request_repaint();
    });
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_only_from_youtube_hosts() {
        assert!(picture_host_ok("https://i.ytimg.com/vi/3lJ5oT_JviI/hq720.jpg"));
        assert!(picture_host_ok("https://yt3.ggpht.com/abc=s176"));
        assert!(!picture_host_ok("http://i.ytimg.com/vi/x.jpg"), "https only");
        assert!(!picture_host_ok("https://i.ytimg.com.evil.example/x.jpg"));
        assert!(!picture_host_ok("https://evil.example/i.ytimg.com/x.jpg"));
        assert!(fetch_picture("https://evil.example/x.jpg").is_none());
    }
}
