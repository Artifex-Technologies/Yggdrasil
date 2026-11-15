//! Right-click > Privacy: things that take what a file gives away about you out of a copy of it, never out of the file itself.
//!
//! The one entry so far is **Scrub location sounds**, for a file with sound in it (an audio file, or a video with an audio
//! track): Heimdall's `heimdall-scrub` writes a copy, "name (scrubbed).ext" beside the original, in which the sounds that can
//! place or date a recording are gone (aircraft overhead, whose pitch glide can be matched to flight tracks; mains hum, which
//! follows the power grid's frequency) and the file's hidden information (dates, places, the device) is stripped. The picture of
//! a video is copied untouched. The entry shows only where heimdall-scrub is installed.
//!
//! Whether a file has sound is read from its content, not its name: the first bytes say what kind of file it is; a container
//! that may or may not carry sound (MP4, Matroska, AVI...) is asked with `ffprobe` (a few seconds at most). Nothing is read
//! until the menu is opened on the file.
//!
//! `Entry` is the list the menu draws; a further privacy entry (taking the hidden information out of any file) goes next to
//! Scrub there, with its own condition in `entries` and its own runner.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Heimdall's file tool.
pub const SCRUB_TOOL: &str = "heimdall-scrub";

/// An entry of the Privacy part of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A copy without the sounds that can place or date it.
    ScrubSounds,
}

impl Entry {
    pub fn label(self) -> &'static str {
        match self {
            Entry::ScrubSounds => "Scrub location sounds",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Entry::ScrubSounds => {
                "Save a copy beside it without the sounds that can place or date it (aircraft overhead, mains hum) and without its hidden information"
            }
        }
    }
}

/// What the menu knows about a file: the tools that are installed, and whether it has sound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub scrub_installed: bool,
    pub has_audio: bool,
}

/// The Privacy entries for a file, in menu order.
pub fn entries(f: Facts) -> Vec<Entry> {
    let mut v = vec![];
    if f.scrub_installed && f.has_audio {
        v.push(Entry::ScrubSounds);
    }
    v
}

/// What the first bytes of a file say about sound in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sniff {
    /// A sound file (WAV, FLAC, MP3, Ogg, AIFF, CAF, AMR, WavPack, Monkey's Audio, MIDI is not one).
    Audio,
    /// A container that may carry sound beside a picture (MP4/MOV/M4A, Matroska/WebM, AVI, ASF, MPEG, FLV): ffprobe decides.
    Container,
    /// Anything else (pictures, documents, archives, text).
    Other,
}

/// Reads the kind of a file from its first bytes (at least 16 are wanted; fewer give Other unless they match).
pub fn sniff(head: &[u8]) -> Sniff {
    let at = |i: usize, m: &[u8]| head.len() >= i + m.len() && &head[i..i + m.len()] == m;
    if at(0, b"RIFF") && (at(8, b"WAVE") || at(8, b"RF64")) || at(0, b"RF64") || at(0, b"BW64") {
        return Sniff::Audio;
    }
    if at(0, b"RIFF") && at(8, b"AVI ") {
        return Sniff::Container;
    }
    if at(0, b"fLaC") || at(0, b"ID3") || at(0, b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")) || at(0, b"caff") {
        return Sniff::Audio;
    }
    if at(0, b"#!AMR") || at(0, b"wvpk") || at(0, b"MAC ") || at(0, b"OggS") {
        // Ogg is nearly always sound; an Ogg video (Theora) has sound too as a rule, and ffprobe is not needed to offer it
        return Sniff::Audio;
    }
    // MPEG audio frame sync, 11 set bits: an MP3 without tags (layer bits not 00), or AAC in ADTS (layer 00, 1111 0xx0)
    let mpeg = head.len() >= 2 && head[0] == 0xFF && (head[1] & 0xE0 == 0xE0 && head[1] & 0x06 != 0 || head[1] & 0xF6 == 0xF0);
    if mpeg {
        return Sniff::Audio;
    }
    // ISO base media (MP4, MOV, M4A, 3GP): "ftyp" at 4; old QuickTime files start with moov/mdat/wide/free
    if at(4, b"ftyp") || at(4, b"moov") || at(4, b"mdat") || at(4, b"wide") || at(4, b"free") {
        return Sniff::Container;
    }
    // Matroska / WebM (EBML), ASF (WMA/WMV), FLV, MPEG program stream, MPEG transport stream
    if at(0, b"\x1A\x45\xDF\xA3")
        || at(0, b"\x30\x26\xB2\x75\x8E\x66\xCF\x11")
        || at(0, b"FLV")
        || at(0, b"\x00\x00\x01\xBA")
        || head.len() > 188 && head[0] == 0x47 && head[188] == 0x47
    {
        return Sniff::Container;
    }
    Sniff::Other
}

/// The decision: a sound file has sound; a container has sound when ffprobe found an audio stream (None: ffprobe is missing,
/// failed or took too long, and then the entry is not offered).
pub fn decide(s: Sniff, probe: Option<bool>) -> bool {
    match s {
        Sniff::Audio => true,
        Sniff::Container => probe == Some(true),
        Sniff::Other => false,
    }
}

/// `ffprobe -select_streams a` printed at least one stream index.
pub fn probe_says_audio(stdout: &str) -> bool {
    stdout.lines().any(|l| !l.trim().is_empty() && l.trim().trim_end_matches(',').chars().all(|c| c.is_ascii_digit()))
}

/// Whether `p` has sound, from its content (see the module's text). Runs ffprobe for a container, `limit` at most.
pub fn has_audio(p: &Path, limit: Duration) -> bool {
    let Ok(meta) = std::fs::metadata(p) else { return false };
    if !meta.is_file() {
        return false;
    }
    let mut head = [0u8; 200];
    let n = std::fs::File::open(p).and_then(|mut f| read_full(&mut f, &mut head)).unwrap_or(0);
    let s = sniff(&head[..n]);
    let probe = if s == Sniff::Container { ffprobe_audio(p, limit) } else { None };
    decide(s, probe)
}

fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// ffprobe's input for a path: `file:` and the absolute path, so no name is read as an option or as another of ffmpeg's
/// protocols ("-x.mp4", "concat:a|b", "http:..." are only file names here).
pub fn ffmpeg_input(p: &Path) -> OsString {
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(p) };
    let mut s = OsString::from("file:");
    s.push(abs.as_os_str());
    s
}

/// ffprobe on the file (argv only, never a shell line).
fn ffprobe_audio(p: &Path, limit: Duration) -> Option<bool> {
    let mut child = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index", "-of", "csv=p=0", "-i"])
        .arg(ffmpeg_input(p))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return st.success().then(|| probe_says_audio(&out));
            }
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Whether a program is on PATH (no process started to find out).
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|d| {
            std::fs::metadata(d.join(program))
                .is_ok_and(|m| m.is_file() && std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0)
        })
    })
}

/// The scrubbed copy's path: "name (scrubbed).ext" beside the original, or "name (scrubbed 2).ext", 3... when that is taken.
/// `taken` says whether a path exists (the file system, or a test's list). The name's bytes are kept as they are.
pub fn scrubbed_path(original: &Path, taken: &dyn Fn(&Path) -> bool) -> PathBuf {
    let dir = original.parent().unwrap_or(Path::new("."));
    let name = original.file_name().unwrap_or(OsStr::new("recording")).as_bytes();
    // the extension: after the last dot, unless the dot starts the name (".hidden") or ends it
    let (stem, ext) = match name.iter().rposition(|&b| b == b'.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], &name[i..]),
        _ => (name, &b""[..]),
    };
    let make = |n: u32| {
        let mut v = stem.to_vec();
        v.extend_from_slice(if n == 1 { b" (scrubbed)".to_vec() } else { format!(" (scrubbed {n})").into_bytes() }.as_slice());
        v.extend_from_slice(ext);
        dir.join(OsString::from_vec(v))
    };
    (1..).map(make).find(|p| !taken(p)).expect("unbounded range")
}

/// One line heimdall-scrub prints with `--progress`.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// How far, 0 to 1.
    Progress(f32),
    /// The copy is written.
    Wrote(PathBuf),
}

/// Reads a line of `heimdall-scrub --progress` (anything else is None and is ignored).
pub fn parse_line(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\n', '\r']);
    if let Some(v) = line.strip_prefix("progress ") {
        let f: f32 = v.trim().parse().ok()?;
        return f.is_finite().then_some(Line::Progress(f.clamp(0.0, 1.0)));
    }
    line.strip_prefix("wrote ").filter(|p| !p.is_empty()).map(|p| Line::Wrote(PathBuf::from(p)))
}

/// The words of a failure for the notice: heimdall-scrub's own message without its "heimdall-scrub: " prefix, else the exit.
pub fn failure(stderr: &str, code: Option<i32>) -> String {
    let msg =
        stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(|l| l.strip_prefix("heimdall-scrub: ").unwrap_or(l).to_string());
    match (msg, code) {
        (Some(m), _) => m,
        (None, Some(c)) => format!("heimdall-scrub stopped with exit code {c}"),
        (None, None) => "heimdall-scrub was stopped".into(),
    }
}

/// A running scrub: the child, so it can be stopped.
pub struct Running {
    child: std::process::Child,
}

impl Running {
    /// Starts `heimdall-scrub --progress IN OUT` (argv, never a shell line).
    pub fn start(input: &Path, output: &Path) -> Result<Running, String> {
        Command::new(SCRUB_TOOL)
            .arg("--progress")
            .arg(input)
            .arg(output)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map(|child| Running { child })
            .map_err(|e| format!("could not start {SCRUB_TOOL}: {e}"))
    }

    /// The process id, for Stop.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Reads its progress until it ends; `on` gets each fraction. Ok: the copy's path.
    pub fn finish(mut self, on: &mut dyn FnMut(f32)) -> Result<PathBuf, String> {
        let mut wrote = None;
        let err_reader = self.child.stderr.take().map(|e| {
            std::thread::spawn(move || {
                let mut s = String::new();
                let _ = BufReader::new(e).take(64 * 1024).read_to_string(&mut s);
                s
            })
        });
        if let Some(out) = self.child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                match parse_line(&line) {
                    Some(Line::Progress(f)) => on(f),
                    Some(Line::Wrote(p)) => wrote = Some(p),
                    None => {}
                }
            }
        }
        let status = self.child.wait().map_err(|e| e.to_string())?;
        let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
        match (status.success(), wrote) {
            (true, Some(p)) => Ok(p),
            (true, None) => Err("heimdall-scrub did not say it wrote the copy".into()),
            (false, _) => Err(failure(&stderr, status.code())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_is_named_beside_the_original_and_never_over_a_file() {
        let none = |_: &Path| false;
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(scrubbed_path(&p("/m/Holiday.mov"), &none), p("/m/Holiday (scrubbed).mov"));
        assert_eq!(scrubbed_path(&p("/m/voice memo.m4a"), &none), p("/m/voice memo (scrubbed).m4a"));
        assert_eq!(scrubbed_path(&p("/m/take.final.wav"), &none), p("/m/take.final (scrubbed).wav"), "only the last extension");
        assert_eq!(scrubbed_path(&p("/m/.hidden"), &none), p("/m/.hidden (scrubbed)"));
        assert_eq!(scrubbed_path(&p("/m/noext"), &none), p("/m/noext (scrubbed)"));
        let taken = |q: &Path| [p("/m/a (scrubbed).wav"), p("/m/a (scrubbed 2).wav")].contains(&q.to_path_buf());
        assert_eq!(scrubbed_path(&p("/m/a.wav"), &taken), p("/m/a (scrubbed 3).wav"));
        // a name that is not UTF-8 keeps its bytes
        let raw = PathBuf::from(OsString::from_vec(b"/m/\xffx.ogg".to_vec()));
        assert_eq!(scrubbed_path(&raw, &none).as_os_str().as_bytes(), b"/m/\xffx (scrubbed).ogg");
    }

    #[test]
    fn progress_lines_are_read() {
        assert_eq!(parse_line("progress 0.25\n"), Some(Line::Progress(0.25)));
        assert_eq!(parse_line("progress 1"), Some(Line::Progress(1.0)));
        assert_eq!(parse_line("progress 7"), Some(Line::Progress(1.0)), "kept within 0..1");
        assert_eq!(parse_line("progress NaN"), None);
        assert_eq!(parse_line("progress"), None);
        assert_eq!(parse_line("wrote /m/a (scrubbed).wav"), Some(Line::Wrote(PathBuf::from("/m/a (scrubbed).wav"))));
        assert_eq!(parse_line("wrote "), None);
        assert_eq!(parse_line("grid hum removed"), None, "other words are ignored");
    }

    #[test]
    fn failures_read_plainly() {
        assert_eq!(failure("heimdall-scrub: the output exists\n", Some(1)), "the output exists");
        assert_eq!(failure("warning\nheimdall-scrub: ffmpeg failed\n\n", Some(1)), "ffmpeg failed");
        assert_eq!(failure("", Some(2)), "heimdall-scrub stopped with exit code 2");
        assert_eq!(failure("", None), "heimdall-scrub was stopped");
    }

    #[test]
    fn sound_is_read_from_the_content() {
        let mut wav = b"RIFF\x24\x00\x00\x00WAVEfmt ".to_vec();
        wav.resize(32, 0);
        assert_eq!(sniff(&wav), Sniff::Audio);
        assert_eq!(sniff(b"fLaC\x00\x00\x00\x22................"), Sniff::Audio);
        assert_eq!(sniff(b"ID3\x04\x00\x00\x00\x00\x00\x00........"), Sniff::Audio);
        assert_eq!(sniff(b"\xFF\xFB\x90\x64................"), Sniff::Audio, "an MP3 frame without tags");
        assert_eq!(sniff(b"\xFF\xF1\x50\x80................"), Sniff::Audio, "AAC in ADTS");
        assert_eq!(sniff(b"OggS\x00\x02................"), Sniff::Audio);
        assert_eq!(sniff(b"FORM\x00\x00\x00\x00AIFF........"), Sniff::Audio);
        assert_eq!(sniff(b"\x00\x00\x00\x20ftypisom........"), Sniff::Container, "MP4/MOV: ffprobe decides");
        assert_eq!(sniff(b"\x00\x00\x00\x14ftypqt  ........"), Sniff::Container);
        assert_eq!(sniff(b"\x1A\x45\xDF\xA3\x01\x00..........."), Sniff::Container, "Matroska/WebM");
        assert_eq!(sniff(b"RIFF\x00\x00\x00\x00AVI LIST...."), Sniff::Container);
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n........"), Sniff::Other);
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0........"), Sniff::Other, "a JPEG starts with FF too, but is no audio frame");
        assert_eq!(sniff(b"PK\x03\x04............"), Sniff::Other);
        assert_eq!(sniff(b"%PDF-1.7"), Sniff::Other);
        assert_eq!(sniff(b""), Sniff::Other);
        // a .wav name on a text file does not count: only the content does
        assert_eq!(sniff(b"just some notes, renamed"), Sniff::Other);
    }

    #[test]
    fn a_video_is_offered_only_when_ffprobe_finds_sound() {
        assert!(decide(Sniff::Audio, None));
        assert!(decide(Sniff::Container, Some(true)));
        assert!(!decide(Sniff::Container, Some(false)), "a silent video");
        assert!(!decide(Sniff::Container, None), "ffprobe missing or too slow: not offered");
        assert!(!decide(Sniff::Other, Some(true)));
        assert!(probe_says_audio("1\n"));
        assert!(probe_says_audio("1\n2\n"));
        assert!(!probe_says_audio(""));
        assert!(!probe_says_audio("\n"));
        assert_eq!(ffmpeg_input(Path::new("/m/-x concat:a|b.mp4")), OsString::from("file:/m/-x concat:a|b.mp4"));
    }

    #[test]
    fn the_entry_needs_the_tool_and_sound() {
        assert_eq!(entries(Facts { scrub_installed: true, has_audio: true }), [Entry::ScrubSounds]);
        assert!(entries(Facts { scrub_installed: false, has_audio: true }).is_empty(), "heimdall-scrub not installed");
        assert!(entries(Facts { scrub_installed: true, has_audio: false }).is_empty());
        assert_eq!(Entry::ScrubSounds.label(), "Scrub location sounds");
    }

    /// The runner against a stand-in heimdall-scrub (a shell script on PATH): progress, the path, a failure's message.
    #[test]
    fn the_runner_reads_the_tool() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("minos-scrub-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let tool = d.join("heimdall-scrub");
        std::fs::write(
            &tool,
            "#!/bin/sh\n[ \"$1\" = --progress ] || exit 2\ncase \"$2\" in *bad*) echo 'heimdall-scrub: no sound in it' >&2; exit 1;; esac\n\
             echo 'progress 0.5'; echo 'progress 1'; : > \"$3\"; echo \"wrote $3\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        // run the stand-in by its full path: the test does not change PATH for the other tests
        let run = |input: &str, output: &Path| {
            let child = Command::new(&tool)
                .arg("--progress")
                .arg(input)
                .arg(output)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut seen = vec![];
            let r = Running { child }.finish(&mut |f| seen.push(f));
            (r, seen)
        };
        let out = d.join("a (scrubbed).wav");
        let (r, seen) = run("a.wav", &out);
        assert_eq!(r, Ok(out.clone()));
        assert_eq!(seen, [0.5, 1.0]);
        assert!(out.exists());
        let (r, _) = run("bad.wav", &d.join("b (scrubbed).wav"));
        assert_eq!(r, Err("no sound in it".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }
}
