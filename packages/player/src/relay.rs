//! A small relay on 127.0.0.1 that VLC plays YouTube's streams from.
//!
//! YouTube refuses stream requests that do not come from the kind of client the stream was issued to (403). yt-dlp fetches them
//! correctly, so for a `ytdlp:` source (see [`ytdlp_source`]) the relay has yt-dlp download the stream into a file in the cache,
//! and serves VLC from that file while it fills ("from byte N" waits until byte N is there). A jump within what is already there is
//! instant; a jump further on waits for the download (which runs much faster than the video plays). The file is deleted when the
//! player stops ([`forget`]). No headers are made up: yt-dlp does its own requests.
//!
//! For a plain googlevideo address the relay still asks in pieces named in the address (`&range=START-END`, as YouTube's players
//! do); that works only while YouTube accepts the address without a proof-of-origin token.
//!
//! The relay listens on 127.0.0.1 only, and each stream has an address with 128 random bits in it, so another program cannot guess one.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How much one request upstream asks for.
pub const PIECE: u64 = 10 * 1024 * 1024;

#[derive(Clone)]
enum Source {
    /// Asked for upstream in pieces with curl.
    Url(String),
    /// A file yt-dlp is downloading into; `done` once yt-dlp has stopped (finished or failed).
    File { path: PathBuf, done: Arc<AtomicBool>, pid: u32 },
}

#[derive(Clone)]
struct Stream {
    source: Source,
    length: u64,
    mime: String,
}

struct Relay {
    port: u16,
    streams: Arc<Mutex<HashMap<String, Stream>>>,
}

static RELAY: OnceLock<Option<Relay>> = OnceLock::new();

/// Whether a stream address is one the relay is for: https on a googlevideo.com host, with its length (`clen=`) in it.
pub fn wanted(url: &str) -> bool {
    let host = url.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or("");
    host.ends_with(".googlevideo.com") && param(url, "clen").is_some()
}

/// One query parameter of an address.
pub fn param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    url.split_once('?')?.1.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
}

/// A stream through the relay: the local address to give VLC, or the address itself when the relay is not for it (or cannot start).
/// For a `ytdlp:` source, the `(token, address)` pair: forget the token when done with the stream.
pub fn local(url: &str) -> String {
    local_token(url).map(|(_, a)| a).unwrap_or_else(|| url.to_string())
}

fn clean_mime(m: &str) -> Option<String> {
    let m = m.replace("%2F", "/");
    (!m.is_empty() && m.chars().all(|c| c.is_ascii_alphanumeric() || "/-+.".contains(c))).then_some(m)
}

/// A stream for yt-dlp to fetch: format `format` of the video page `page`, `length` bytes long, of type `mime`.
/// Returns `None` for a page that is not https or a format name that is not a plain one.
pub fn ytdlp_source(format: &str, length: u64, mime: &str, page: &str) -> Option<String> {
    let fine = format.starts_with(|c: char| c.is_ascii_alphanumeric()) && format.len() <= 32 && format.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let mime = clean_mime(mime).unwrap_or_else(|| "application/octet-stream".into());
    (fine && length > 0 && page.starts_with("https://") && !page.contains(char::is_whitespace)).then(|| format!("ytdlp:{format}:{length}:{mime}:{page}"))
}

/// The parts of a `ytdlp:` source: format, length, type, page.
pub fn ytdlp_parts(src: &str) -> Option<(&str, u64, &str, &str)> {
    let rest = src.strip_prefix("ytdlp:")?;
    let mut it = rest.splitn(4, ':');
    let (f, l, m, p) = (it.next()?, it.next()?.parse().ok()?, it.next()?, it.next()?);
    ytdlp_source(f, l, m, p).filter(|s| s == src).map(|_| (f, l, m, p))
}

/// Whether an address is one the relay plays (a `ytdlp:` source or a googlevideo address).
pub fn handles(url: &str) -> bool {
    ytdlp_parts(url).is_some() || wanted(url)
}

/// The relay's token and local address for a stream it handles.
pub fn local_token(url: &str) -> Option<(String, String)> {
    let r = RELAY.get_or_init(start).as_ref()?;
    let (source, length, mime) = if let Some((format, length, mime, page)) = ytdlp_parts(url) {
        (fetch(format, page)?, length, mime.to_string())
    } else if wanted(url) {
        let length = param(url, "clen").and_then(|c| c.parse::<u64>().ok()).unwrap_or(0);
        (Source::Url(url.to_string()), length, param(url, "mime").and_then(clean_mime).unwrap_or_else(|| "application/octet-stream".into()))
    } else {
        return None;
    };
    let token = token();
    if let Ok(mut s) = r.streams.lock() {
        // keep the table small: a session plays a few videos (forgetting stops their downloads)
        if s.len() > 64 {
            for (_, st) in s.drain() {
                drop_source(&st.source);
            }
        }
        s.insert(token.clone(), Stream { source, length, mime });
    }
    Some((token.clone(), format!("http://127.0.0.1:{}/s/{token}", r.port)))
}

/// Done with a stream: its download stops and its file is deleted.
pub fn forget(token: &str) {
    let gone = RELAY.get().and_then(|r| r.as_ref()).and_then(|r| r.streams.lock().ok()?.remove(token));
    if let Some(st) = gone {
        drop_source(&st.source);
    }
}

fn drop_source(s: &Source) {
    if let Source::File { path, done, pid } = s {
        if !done.load(Ordering::SeqCst) {
            // our own child (spawned below), still running: SIGTERM makes yt-dlp stop
            let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
        }
        let _ = std::fs::remove_file(path);
    }
}

/// Where downloads for playing go: a folder per player process, so a second player does not delete the first one's files.
fn folder() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))).unwrap_or_else(std::env::temp_dir);
    cache.join("yggdrasil-player").join("streams")
}

/// Folders left by players that are gone (a crash, a power cut) are removed.
fn tidy(root: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let alive = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()).map(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists());
        if alive == Some(false) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn fetch(format: &str, page: &str) -> Option<Source> {
    let root = folder();
    tidy(&root);
    let dir = root.join(std::process::id().to_string());
    std::fs::create_dir_all(&dir).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join(format!("{}.stream", token()));
    // yt-dlp stops when the player does, even when the player is killed (setpriv's parent-death signal, from util-linux)
    let setpriv = std::path::Path::new("/usr/bin/setpriv").exists();
    let mut cmd = if setpriv { std::process::Command::new("/usr/bin/setpriv") } else { std::process::Command::new("yt-dlp") };
    if setpriv {
        cmd.args(["--pdeathsig", "TERM", "--", "yt-dlp"]);
    }
    let mut child = cmd
        .args(["--ignore-config", "--no-warnings", "--quiet", "--no-progress", "--no-part", "--no-continue", "--no-playlist", "--no-cache-dir", "-f", format, "-o"])
        .arg(&path)
        .arg("--")
        .arg(page)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let pid = child.id();
    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    std::thread::spawn(move || {
        let _ = child.wait();
        d.store(true, Ordering::SeqCst);
    });
    Some(Source::File { path, done, pid })
}

fn token() -> String {
    let mut b = [0u8; 16];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        // never without randomness: the time and the process id, mixed (only reached without /dev/urandom)
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) ^ ((std::process::id() as u128) << 64);
        b = t.to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn start() -> Option<Relay> {
    let l = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = l.local_addr().ok()?.port();
    let streams: Arc<Mutex<HashMap<String, Stream>>> = Arc::default();
    let table = streams.clone();
    std::thread::spawn(move || {
        for conn in l.incoming().flatten() {
            let table = table.clone();
            std::thread::spawn(move || {
                let _ = serve(conn, &table);
            });
        }
    });
    Some(Relay { port, streams })
}

/// The bytes asked for by a `Range` header (`bytes=N-` or `bytes=N-M`), within a stream of `length` bytes.
pub fn requested(range: Option<&str>, length: u64) -> Option<(u64, u64)> {
    if length == 0 {
        return None;
    }
    let Some(r) = range else { return Some((0, length - 1)) };
    let spec = r.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let start: u64 = a.trim().parse().ok()?;
    let end: u64 = if b.trim().is_empty() { length - 1 } else { b.trim().parse::<u64>().ok()?.min(length - 1) };
    (start <= end).then_some((start, end))
}

fn serve(mut conn: TcpStream, table: &Mutex<HashMap<String, Stream>>) -> std::io::Result<()> {
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut range = None;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("range") {
                range = Some(v.trim().to_string());
            }
        }
    }
    let stream = path.strip_prefix("/s/").and_then(|t| table.lock().ok()?.get(t).cloned());
    let Some(s) = stream.filter(|_| method == "GET" || method == "HEAD") else {
        conn.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        return Ok(());
    };
    let Some((start, end)) = requested(range.as_deref(), s.length) else {
        conn.write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nConnection: close\r\n\r\n", s.length).as_bytes())?;
        return Ok(());
    };
    let status = if range.is_some() { "206 Partial Content" } else { "200 OK" };
    conn.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n", s.mime, end - start + 1, s.length).as_bytes())?;
    if method == "HEAD" {
        return Ok(());
    }
    match &s.source {
        Source::Url(url) => from_url(&mut conn, url, start, end),
        Source::File { path, done, .. } => from_file(&mut conn, path, done, start, end),
    }
}

/// Serves bytes `start..=end` of a file that is still being written: waits for bytes not there yet, gives up when the download
/// has stopped or nothing new came for a minute.
fn from_file(conn: &mut TcpStream, path: &std::path::Path, done: &AtomicBool, start: u64, end: u64) -> std::io::Result<()> {
    use std::io::Seek;
    let mut waited = Instant::now();
    let mut f = loop {
        match std::fs::File::open(path) {
            Ok(f) => break f,
            Err(_) if !done.load(Ordering::SeqCst) && waited.elapsed() < Duration::from_secs(60) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e),
        }
    };
    let mut at = start;
    let mut buf = vec![0u8; 256 * 1024];
    while at <= end {
        let have = f.metadata()?.len();
        if have <= at {
            if done.load(Ordering::SeqCst) || waited.elapsed() > Duration::from_secs(60) {
                return Ok(()); // the download stopped short (or stalled)
            }
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        f.seek(std::io::SeekFrom::Start(at))?;
        let want = (have.min(end + 1) - at).min(buf.len() as u64) as usize;
        let n = f.read(&mut buf[..want])?;
        if n == 0 {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        if conn.write_all(&buf[..n]).is_err() {
            return Ok(()); // VLC went elsewhere
        }
        at += n as u64;
        waited = Instant::now();
    }
    Ok(())
}

fn from_url(conn: &mut TcpStream, url: &str, start: u64, end: u64) -> std::io::Result<()> {
    let mut at = start;
    while at <= end {
        let to = (at + PIECE - 1).min(end);
        let mut child = std::process::Command::new("curl")
            .args(["-sfL", "--proto", "=https", "--proto-redir", "=https", "--max-time", "120"])
            .arg(format!("{url}&range={at}-{to}"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let mut out = child.stdout.take().ok_or_else(|| std::io::Error::other("no output"))?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut got = 0u64;
        loop {
            let n = out.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if conn.write_all(&buf[..n]).is_err() {
                // VLC went elsewhere (a jump, or the end): stop fetching
                let _ = child.kill();
                let _ = child.wait();
                return Ok(());
            }
            got += n as u64;
        }
        let _ = child.wait();
        if got == 0 {
            return Ok(()); // the server stopped answering this stream
        }
        at += got;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_addresses() {
        assert!(wanted("https://rr1---sn-x.googlevideo.com/videoplayback?expire=1&clen=118698939&mime=video%2Fmp4"));
        assert!(!wanted("https://rr1---sn-x.googlevideo.com/videoplayback?expire=1"), "no length");
        assert!(!wanted("http://rr1---sn-x.googlevideo.com/videoplayback?clen=5"), "https only");
        assert!(!wanted("https://evil.example/x.googlevideo.com/videoplayback?clen=5"));
        assert!(!wanted("https://googlevideo.com.evil.example/videoplayback?clen=5"));
        assert_eq!(param("https://h/v?a=1&clen=42&b=2", "clen"), Some("42"));
    }

    #[test]
    fn ranges() {
        assert_eq!(requested(None, 100), Some((0, 99)));
        assert_eq!(requested(Some("bytes=0-"), 100), Some((0, 99)));
        assert_eq!(requested(Some("bytes=40-"), 100), Some((40, 99)));
        assert_eq!(requested(Some("bytes=40-49"), 100), Some((40, 49)));
        assert_eq!(requested(Some("bytes=40-5000"), 100), Some((40, 99)));
        assert_eq!(requested(Some("bytes=200-"), 100), None);
        assert_eq!(requested(Some("items=0-"), 100), None);
    }

    #[test]
    fn the_relay_serves_only_its_own_streams() {
        // a stream that is not for the relay is given back unchanged; an unknown token is a 404
        assert_eq!(local("https://example.org/x.mp4"), "https://example.org/x.mp4");
        let u = local("https://rr1---sn-test.googlevideo.com/videoplayback?clen=1000&mime=video%2Fmp4");
        assert!(u.starts_with("http://127.0.0.1:") && u.len() > 40);
        let port: u16 = u["http://127.0.0.1:".len()..].split('/').next().unwrap().parse().unwrap();
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(b"GET /s/0123456789abcdef0123456789abcdef HTTP/1.1\r\n\r\n").unwrap();
        let mut answer = String::new();
        let _ = c.read_to_string(&mut answer);
        assert!(answer.starts_with("HTTP/1.1 404"));
        // a known one answers with the length and the range (HEAD, so nothing is fetched)
        let path = &u[u.find("/s/").unwrap()..];
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(format!("HEAD {path} HTTP/1.1\r\nRange: bytes=100-\r\n\r\n").as_bytes()).unwrap();
        let mut answer = String::new();
        let _ = c.read_to_string(&mut answer);
        assert!(answer.starts_with("HTTP/1.1 206") && answer.contains("Content-Range: bytes 100-999/1000") && answer.contains("Content-Type: video/mp4"));
    }

    #[test]
    fn ytdlp_sources() {
        let src = ytdlp_source("137", 1000, "video%2Fmp4", "https://www.youtube.com/watch?v=abcdefghijk").unwrap();
        assert_eq!(src, "ytdlp:137:1000:video/mp4:https://www.youtube.com/watch?v=abcdefghijk");
        assert_eq!(ytdlp_parts(&src), Some(("137", 1000, "video/mp4", "https://www.youtube.com/watch?v=abcdefghijk")));
        assert!(handles(&src));
        assert_eq!(ytdlp_source("137", 1000, "video/mp4", "http://plain/x"), None, "https pages only");
        assert_eq!(ytdlp_source("--exec", 1000, "video/mp4", "https://x/y"), None, "no options as a format");
        assert_eq!(ytdlp_source("137", 0, "video/mp4", "https://x/y"), None, "needs a length");
        assert_eq!(ytdlp_parts("ytdlp:137:10:video/mp4:https://x/a b"), None);
    }

    #[test]
    fn a_growing_file_is_served_as_it_fills() {
        let dir = std::env::temp_dir().join(format!("relay-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s");
        let done = Arc::new(AtomicBool::new(false));
        let (p, d) = (path.clone(), done.clone());
        let writer = std::thread::spawn(move || {
            let mut f = std::fs::File::create(&p).unwrap();
            for i in 0..10u8 {
                f.write_all(&[i; 100]).unwrap();
                f.flush().unwrap();
                std::thread::sleep(Duration::from_millis(20));
            }
            d.store(true, Ordering::SeqCst);
        });
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (pa, dn) = (path.clone(), done.clone());
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            from_file(&mut c, &pa, &dn, 550, 999).unwrap();
        });
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut got = vec![];
        c.read_to_end(&mut got).unwrap();
        writer.join().unwrap();
        assert_eq!(got.len(), 450);
        assert!(got[..50].iter().all(|b| *b == 5) && got[50..150].iter().all(|b| *b == 6) && got[350..].iter().all(|b| *b == 9));
        let _ = std::fs::remove_dir_all(dir);
    }
}
