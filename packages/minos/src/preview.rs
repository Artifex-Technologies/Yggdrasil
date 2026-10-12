//! What to show in the preview pane for a file or folder. No GUI types: the result is plain data
//! (text, details, and pixels), built on a background thread and drawn by the app.
//!
//! Design points that come from using other file managers:
//!  * Work for any selection in any folder, never only "when one folder is open".
//!  * Never produce something that has to be scrolled to see: images are scaled to fit, text is a
//!    short excerpt that ends in "...", folders list their first entries and a count.
//!  * Bounded: big files are not read whole (text reads at most 64 KB, images over 150 MB are
//!    skipped), and the external tools used for PDFs and video get a time limit.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{archive, listing};

pub const MAX_IMAGE_FILE: u64 = 150 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
const TEXT_LINES: usize = 40;
const TEXT_CHARS: usize = 6000;

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    None,
    /// RGBA pixels, already scaled down.
    Image { width: u32, height: u32, rgba: Vec<u8> },
    Text { text: String, truncated: bool },
    /// A short list (folder or archive contents).
    Lines(Vec<String>),
    Message(String),
    Drive { total: u64, free: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewData {
    pub title: String,
    pub subtitle: String,
    pub details: Vec<(String, String)>,
    pub body: Body,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Text,
    Audio,
    Video,
    Pdf,
    Archive,
    Folder,
    Other,
}

const IMAGE: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const AUDIO: &[&str] = &["mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "wma", "aiff", "ape"];
const VIDEO: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "wmv", "flv", "m4v", "mpg", "mpeg", "ts"];
const TEXT: &[&str] = &[
    "txt", "md", "markdown", "log", "csv", "tsv", "json", "toml", "yaml", "yml", "ini", "conf", "cfg", "xml", "html", "htm", "css", "js",
    "ts", "rs", "py", "sh", "bash", "zsh", "lua", "c", "h", "cpp", "hpp", "cc", "go", "java", "kt", "swift", "sql", "service", "desktop",
    "rb", "php", "pl", "bat", "ps1", "cmake", "mk", "diff", "patch", "srt", "vtt", "lock",
];

pub fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn kind_of(p: &Path, is_dir: bool) -> Kind {
    if is_dir {
        return Kind::Folder;
    }
    let e = ext_of(p);
    if IMAGE.contains(&e.as_str()) {
        Kind::Image
    } else if AUDIO.contains(&e.as_str()) {
        Kind::Audio
    } else if VIDEO.contains(&e.as_str()) && e != "ts" {
        Kind::Video
    } else if e == "pdf" {
        Kind::Pdf
    } else if archive::is_archive_name(p) {
        Kind::Archive
    } else if TEXT.contains(&e.as_str()) || p.file_name().is_some_and(|n| matches!(n.to_string_lossy().as_ref(), "Makefile" | "PKGBUILD" | "Dockerfile" | "LICENSE" | "README")) {
        Kind::Text
    } else {
        Kind::Other
    }
}

/// The first part of a text file as an excerpt that fits a pane. `None` for binary data.
pub fn text_excerpt(bytes: &[u8]) -> Option<(String, bool)> {
    if bytes.contains(&0) {
        return None;
    }
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut lines = 0;
    let mut truncated = bytes.len() >= MAX_TEXT_BYTES;
    for line in s.lines() {
        if lines >= TEXT_LINES || out.chars().count() + line.chars().count() > TEXT_CHARS {
            truncated = true;
            break;
        }
        out.push_str(&line.replace('\t', "    "));
        out.push('\n');
        lines += 1;
    }
    Some((out.trim_end().to_string(), truncated))
}

fn common_details(p: &Path, meta: Option<&std::fs::Metadata>, kind_label: &str) -> Vec<(String, String)> {
    let mut d = vec![("Type".to_string(), kind_label.to_string())];
    if let Some(m) = meta {
        if m.is_file() {
            d.push(("Size".into(), listing::human_size(m.len())));
        }
        if let Ok(t) = m.modified() {
            d.push(("Modified".into(), crate::fmt::datetime(t)));
        }
    }
    if let Some(parent) = p.parent() {
        d.push(("Location".into(), parent.to_string_lossy().into_owned()));
    }
    d
}

fn decode_reader<R: std::io::BufRead + std::io::Seek>(r: R, max_px: u32) -> Result<(u32, u32, u32, u32, Vec<u8>, String), String> {
    let reader = image::ImageReader::new(r).with_guessed_format().map_err(|e| e.to_string())?;
    let format = reader.format().map(|f| format!("{f:?}")).unwrap_or_default();
    let img = reader.decode().map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    let small = if w.max(h) > max_px { img.thumbnail(max_px, max_px) } else { img };
    let (sw, sh) = (small.width(), small.height());
    Ok((w, h, sw, sh, small.to_rgba8().into_raw(), format))
}

/// (original width, original height, scaled width, scaled height, RGBA pixels, format name)
fn decode_scaled(src: ImageSource, max_px: u32) -> Result<(u32, u32, u32, u32, Vec<u8>, String), String> {
    match src {
        ImageSource::Path(p) => decode_reader(std::io::BufReader::new(std::fs::File::open(p).map_err(|e| e.to_string())?), max_px),
        ImageSource::Bytes(b) => decode_reader(std::io::Cursor::new(b), max_px),
    }
}

enum ImageSource<'a> {
    Path(&'a Path),
    Bytes(Vec<u8>),
}

/// Run `cmd`, give it `limit` to finish, and kill it after that.
fn run_limited(mut cmd: Command, limit: Duration) -> bool {
    let Ok(mut child) = cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

pub fn tool_exists(name: &str) -> bool {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(&exe).is_file()))
}

fn temp_png(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("minos-preview-{}-{tag}.png", std::process::id()))
}

fn image_body(rgba: Option<(u32, u32, Vec<u8>)>, fallback: &str) -> Body {
    match rgba {
        Some((w, h, px)) => Body::Image { width: w, height: h, rgba: px },
        None => Body::Message(fallback.to_string()),
    }
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn audio(p: &Path, details: &mut Vec<(String, String)>, max_px: u32) -> Body {
    use lofty::prelude::*;
    let Ok(tagged) = lofty::read_from_path(p) else { return Body::Message("Could not read audio tags".into()) };
    let props = tagged.properties();
    if props.duration().as_secs() > 0 {
        details.push(("Duration".into(), fmt_duration(props.duration())));
    }
    if let Some(b) = props.audio_bitrate() {
        details.push(("Bitrate".into(), format!("{b} kbps")));
    }
    if let Some(sr) = props.sample_rate() {
        details.push(("Sample rate".into(), format!("{sr} Hz")));
    }
    let mut cover = None;
    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        for (label, v) in [("Title", tag.title()), ("Artist", tag.artist()), ("Album", tag.album()), ("Genre", tag.genre())] {
            if let Some(v) = v {
                details.push((label.into(), v.to_string()));
            }
        }
        if let Some(y) = tag.get_string(lofty::tag::ItemKey::RecordingDate).or_else(|| tag.get_string(lofty::tag::ItemKey::Year)) {
            details.push(("Year".into(), y.to_string()));
        }
        if let Some(pic) = tag.pictures().first() {
            if let Ok((_, _, w, h, px, _)) = decode_scaled(ImageSource::Bytes(pic.data().to_vec()), max_px) {
                cover = Some((w, h, px));
            }
        }
    }
    image_body(cover, "No cover art")
}

/// Build the preview for `path`. Blocking; call from a worker thread.
pub fn build(path: &Path, max_px: u32) -> PreviewData {
    let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned());

    // Inside an archive the item is not on disk; show what the archive says.
    if let Some((arc, inner)) = archive::split(path) {
        if inner.is_empty() {
            return build_archive(&arc, &title, max_px);
        }
        let entry = archive::list_cached(&arc).ok().and_then(|l| l.into_iter().find(|e| e.path == inner));
        let mut details = vec![("Inside".to_string(), arc.file_name().unwrap_or_default().to_string_lossy().into_owned())];
        let (sub, kind) = match &entry {
            Some(e) if e.is_dir => ("Folder in archive".to_string(), "Folder"),
            Some(e) => {
                details.push(("Size".into(), listing::human_size(e.size)));
                (format!("File in archive \u{2022} {}", listing::human_size(e.size)), "File")
            }
            None => ("In archive".to_string(), "Item"),
        };
        details.insert(0, ("Type".into(), kind.into()));
        return PreviewData { title, subtitle: sub, details, body: Body::Message("Open or extract it to see a preview".into()) };
    }

    let meta = std::fs::metadata(path).ok();
    let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
    let kind = kind_of(path, is_dir);
    let size = meta.as_ref().filter(|m| m.is_file()).map(|m| m.len());

    let label = match kind {
        Kind::Folder => "Folder".to_string(),
        Kind::Other if ext_of(path).is_empty() => "File".to_string(),
        _ => format!("{} file", ext_of(path).to_uppercase()),
    };
    let mut details = common_details(path, meta.as_ref(), &label);
    let subtitle = match size {
        Some(s) => format!("{label} \u{2022} {}", listing::human_size(s)),
        None => label.clone(),
    };

    let body = match kind {
        Kind::Image => {
            if size.unwrap_or(0) > MAX_IMAGE_FILE {
                Body::Message("Too large to preview".into())
            } else {
                match decode_scaled(ImageSource::Path(path), max_px) {
                    Ok((w, h, sw, sh, px, fmt)) => {
                        details.insert(1, ("Dimensions".into(), format!("{w} \u{00D7} {h}")));
                        if !fmt.is_empty() {
                            details.insert(2, ("Format".into(), fmt));
                        }
                        Body::Image { width: sw, height: sh, rgba: px }
                    }
                    Err(e) => Body::Message(format!("Cannot decode image: {e}")),
                }
            }
        }
        Kind::Text => match std::fs::File::open(path) {
            Ok(f) => {
                let mut buf = Vec::new();
                let _ = f.take(MAX_TEXT_BYTES as u64).read_to_end(&mut buf);
                match text_excerpt(&buf) {
                    Some((text, truncated)) => Body::Text { text, truncated },
                    None => Body::Message("Binary file".into()),
                }
            }
            Err(e) => Body::Message(format!("Cannot read: {e}")),
        },
        Kind::Audio => audio(path, &mut details, max_px),
        Kind::Pdf => {
            if tool_exists("pdftoppm") {
                let base = temp_png("pdf");
                let stem = base.with_extension("");
                let mut c = Command::new("pdftoppm");
                c.args(["-png", "-r", "70", "-f", "1", "-l", "1", "-singlefile", "--"]).arg(path).arg(&stem);
                let ok = run_limited(c, Duration::from_secs(10));
                let out = stem.with_extension("png");
                let r = if ok { decode_scaled(ImageSource::Path(&out), max_px).ok().map(|(_, _, w, h, px, _)| (w, h, px)) } else { None };
                let _ = std::fs::remove_file(&out);
                image_body(r, "Could not render the first page")
            } else {
                Body::Message("Install poppler (pdftoppm) to preview PDFs".into())
            }
        }
        Kind::Video => {
            let out = temp_png("video");
            let ok = if tool_exists("ffmpegthumbnailer") {
                let mut c = Command::new("ffmpegthumbnailer");
                c.arg("-i").arg(path).arg("-o").arg(&out).args(["-s", &max_px.to_string(), "-t", "10%"]);
                run_limited(c, Duration::from_secs(12))
            } else if tool_exists("ffmpeg") {
                let mut c = Command::new("ffmpeg");
                c.args(["-y", "-ss", "3", "-i"]).arg(path).args(["-frames:v", "1", "-vf", &format!("scale={max_px}:-1")]).arg(&out);
                run_limited(c, Duration::from_secs(12))
            } else {
                return PreviewData { title, subtitle, details, body: Body::Message("Install ffmpegthumbnailer to preview videos".into()) };
            };
            let r = if ok { decode_scaled(ImageSource::Path(&out), max_px).ok().map(|(_, _, w, h, px, _)| (w, h, px)) } else { None };
            let _ = std::fs::remove_file(&out);
            image_body(r, "Could not read a frame")
        }
        Kind::Archive => return build_archive(path, &title, max_px),
        Kind::Folder => {
            let (mut dirs, mut files) = (0usize, 0usize);
            let mut names = Vec::new();
            if let Ok(rd) = std::fs::read_dir(path) {
                for e in rd.flatten().take(5000) {
                    let is_d = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    if is_d { dirs += 1 } else { files += 1 }
                    if names.len() < 400 {
                        names.push((is_d, e.file_name().to_string_lossy().into_owned()));
                    }
                }
            }
            names.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
            details.push(("Contains".into(), format!("{dirs} folders, {files} files")));
            Body::Lines(names.into_iter().take(14).map(|(d, n)| if d { format!("{n}/") } else { n }).collect())
        }
        Kind::Other => Body::None,
    };
    PreviewData { title, subtitle, details, body }
}

fn build_archive(arc: &Path, title: &str, _max_px: u32) -> PreviewData {
    let meta = std::fs::metadata(arc).ok();
    let label = format!("{} archive", ext_of(arc).to_uppercase());
    let mut details = common_details(arc, meta.as_ref(), &label);
    let subtitle = meta.as_ref().map(|m| format!("{label} \u{2022} {}", listing::human_size(m.len()))).unwrap_or_else(|| label.clone());
    let body = match archive::list_cached(arc) {
        Ok(list) => {
            let files = list.iter().filter(|e| !e.is_dir).count();
            let total: u64 = list.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
            details.push(("Contains".into(), format!("{files} files, {} unpacked", listing::human_size(total))));
            let tops = archive::top_level_names(&list);
            let mut lines: Vec<String> = tops.iter().take(14).cloned().collect();
            if tops.len() > 14 {
                lines.push(format!("... and {} more", tops.len() - 14));
            }
            Body::Lines(lines)
        }
        Err(e) => Body::Message(e),
    };
    PreviewData { title: title.to_string(), subtitle, details, body }
}

/// A drive: its usage, big.
pub fn drive(label: &str, total: u64, free: u64) -> PreviewData {
    let used = total.saturating_sub(free);
    PreviewData {
        title: label.to_string(),
        subtitle: "Drive".into(),
        details: vec![
            ("Total".into(), listing::human_size(total)),
            ("Used".into(), format!("{} ({:.0}%)", listing::human_size(used), listing::used_fraction(total, free) * 100.0)),
            ("Free".into(), listing::human_size(free)),
        ],
        body: Body::Drive { total, free },
    }
}

pub fn message(title: &str, subtitle: &str, msg: &str) -> PreviewData {
    PreviewData { title: title.into(), subtitle: subtitle.into(), details: vec![], body: Body::Message(msg.into()) }
}

/// A one-line summary of several selected items, for the pane when more than one is chosen.
pub fn summarize(paths: &[PathBuf]) -> PreviewData {
    let mut files = 0;
    let mut dirs = 0;
    let mut bytes = 0u64;
    for p in paths {
        match std::fs::metadata(p) {
            Ok(m) if m.is_dir() => dirs += 1,
            Ok(m) => {
                files += 1;
                bytes += m.len();
            }
            Err(_) => {}
        }
    }
    PreviewData {
        title: format!("{} items selected", paths.len()),
        subtitle: format!("{dirs} folders, {files} files"),
        details: vec![("Total size".into(), listing::human_size(bytes)), ("Folders not counted".into(), "sizes of folders are not added".into())],
        body: Body::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pv-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of(Path::new("a.PNG"), false), Kind::Image);
        assert_eq!(kind_of(Path::new("a.flac"), false), Kind::Audio);
        assert_eq!(kind_of(Path::new("a.mkv"), false), Kind::Video);
        assert_eq!(kind_of(Path::new("a.pdf"), false), Kind::Pdf);
        assert_eq!(kind_of(Path::new("a.7z"), false), Kind::Archive);
        assert_eq!(kind_of(Path::new("a.rs"), false), Kind::Text);
        assert_eq!(kind_of(Path::new("PKGBUILD"), false), Kind::Text);
        assert_eq!(kind_of(Path::new("a.xyz"), false), Kind::Other);
        assert_eq!(kind_of(Path::new("a.png"), true), Kind::Folder);
        assert_eq!(kind_of(Path::new("clip.ts"), false), Kind::Text, ".ts is TypeScript here, not MPEG-TS");
    }

    #[test]
    fn text_excerpts_are_short_and_binary_is_refused() {
        let long: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let (t, trunc) = text_excerpt(long.as_bytes()).unwrap();
        assert!(trunc);
        assert!(t.lines().count() <= TEXT_LINES);
        assert!(t.starts_with("line 0"));
        let (s, trunc) = text_excerpt(b"short\nfile\n").unwrap();
        assert_eq!((s.as_str(), trunc), ("short\nfile", false));
        assert!(text_excerpt(b"abc\0def").is_none());
        assert_eq!(text_excerpt(b"a\tb").unwrap().0, "a    b");
        assert!(text_excerpt("h\u{e9}llo \u{1F600}".as_bytes()).is_some());
    }

    #[test]
    fn image_preview_scales_down_and_reports_real_dimensions() {
        let d = tmp("img");
        std::fs::write(d.join("big.png"), png(1200, 600)).unwrap();
        let p = build(&d.join("big.png"), 256);
        match &p.body {
            Body::Image { width, height, rgba } => {
                assert_eq!((*width, *height), (256, 128));
                assert_eq!(rgba.len(), (width * height * 4) as usize);
            }
            other => panic!("{other:?}"),
        }
        assert!(p.details.iter().any(|(k, v)| k == "Dimensions" && v == "1200 \u{00D7} 600"), "{:?}", p.details);
        assert_eq!(p.title, "big.png");
        // a small image is not enlarged
        std::fs::write(d.join("small.png"), png(40, 20)).unwrap();
        assert!(matches!(build(&d.join("small.png"), 256).body, Body::Image { width: 40, height: 20, .. }));
    }

    #[test]
    fn broken_and_missing_files_give_messages_not_panics() {
        let d = tmp("bad");
        std::fs::write(d.join("broken.png"), b"not a png").unwrap();
        assert!(matches!(build(&d.join("broken.png"), 128).body, Body::Message(_)));
        let p = build(&d.join("missing.txt"), 128);
        assert!(matches!(p.body, Body::Message(_)));
        std::fs::write(d.join("bin.txt"), [0u8, 1, 2, 3]).unwrap();
        assert_eq!(build(&d.join("bin.txt"), 128).body, Body::Message("Binary file".into()));
    }

    #[test]
    fn folder_summary_counts_and_lists() {
        let d = tmp("dir");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), "x").unwrap();
        std::fs::write(d.join("b.txt"), "x").unwrap();
        let p = build(&d, 128);
        assert!(p.details.iter().any(|(k, v)| k == "Contains" && v == "1 folders, 2 files"), "{:?}", p.details);
        match p.body {
            Body::Lines(l) => assert_eq!(l, vec!["sub/", "a.txt", "b.txt"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn text_file_preview() {
        let d = tmp("txt");
        std::fs::write(d.join("n.md"), "# Title\nbody\n").unwrap();
        let p = build(&d.join("n.md"), 128);
        assert_eq!(p.body, Body::Text { text: "# Title\nbody".into(), truncated: false });
        assert!(p.subtitle.starts_with("MD file"));
    }

    #[test]
    fn wav_audio_gets_properties() {
        // A real, minimal 8-bit mono 8 kHz WAV, one second of silence.
        let mut w = Vec::new();
        let data = vec![128u8; 8000];
        w.extend(b"RIFF");
        w.extend((36 + data.len() as u32).to_le_bytes());
        w.extend(b"WAVEfmt ");
        w.extend(16u32.to_le_bytes());
        w.extend(1u16.to_le_bytes());
        w.extend(1u16.to_le_bytes());
        w.extend(8000u32.to_le_bytes());
        w.extend(8000u32.to_le_bytes());
        w.extend(1u16.to_le_bytes());
        w.extend(8u16.to_le_bytes());
        w.extend(b"data");
        w.extend((data.len() as u32).to_le_bytes());
        w.extend(&data);
        let d = tmp("wav");
        std::fs::write(d.join("s.wav"), w).unwrap();
        let p = build(&d.join("s.wav"), 128);
        assert!(p.details.iter().any(|(k, v)| k == "Duration" && v == "0:01"), "{:?}", p.details);
        assert!(p.details.iter().any(|(k, v)| k == "Sample rate" && v == "8000 Hz"));
    }

    #[test]
    fn drive_preview_reports_usage() {
        let p = drive("System (/)", 1000, 250);
        assert_eq!(p.body, Body::Drive { total: 1000, free: 250 });
        assert!(p.details.iter().any(|(k, v)| k == "Used" && v.ends_with("(75%)")), "{:?}", p.details);
    }

    #[test]
    fn selection_summary() {
        let d = tmp("sum");
        std::fs::write(d.join("a"), vec![0u8; 2048]).unwrap();
        std::fs::create_dir_all(d.join("f")).unwrap();
        let s = summarize(&[d.join("a"), d.join("f")]);
        assert_eq!(s.title, "2 items selected");
        assert_eq!(s.subtitle, "1 folders, 1 files");
        assert!(s.details.iter().any(|(_, v)| v == "2.0 KB"));
    }

    #[test]
    fn archive_preview_lists_top_level_when_7zip_is_present() {
        if archive::find_7z().is_none() {
            eprintln!("SKIPPED: 7-Zip not installed");
            return;
        }
        let d = tmp("arcpv");
        std::fs::create_dir_all(d.join("src/inner")).unwrap();
        std::fs::write(d.join("src/a.txt"), "alpha").unwrap();
        archive::create(&d.join("t.zip"), &[d.join("src")], archive::Format::Zip).unwrap();
        let p = build(&d.join("t.zip"), 128);
        assert_eq!(p.body, Body::Lines(vec!["src".into()]));
        assert!(p.details.iter().any(|(k, _)| k == "Contains"));
        // an item inside the archive: information only, no extraction
        let inside = build(&d.join("t.zip").join("src").join("a.txt"), 128);
        assert!(inside.subtitle.starts_with("File in archive"), "{}", inside.subtitle);
    }
}
