//! A small relay on 127.0.0.1 for video servers that hand out a stream in pieces named in its address (YouTube's googlevideo
//! servers: `...&range=START-END`, the way YouTube's own players ask; a plain HTTP `Range: bytes=N-` for the whole rest of the file is
//! refused with 403). VLC asks the relay as it would any server ("from byte N"); the relay fetches the stream in pieces of 10 MB with
//! curl and passes them on. A jump in the video is a new "from byte N". The relay listens on 127.0.0.1 only, and each stream has an
//! address with 128 random bits in it, so another program cannot guess one.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};

/// How much one request upstream asks for.
pub const PIECE: u64 = 10 * 1024 * 1024;

#[derive(Clone)]
struct Stream {
    url: String,
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
pub fn local(url: &str) -> String {
    if !wanted(url) {
        return url.to_string();
    }
    let Some(r) = RELAY.get_or_init(start).as_ref() else { return url.to_string() };
    let length = param(url, "clen").and_then(|c| c.parse::<u64>().ok()).unwrap_or(0);
    let mime = param(url, "mime").map(|m| m.replace("%2F", "/")).filter(|m| m.chars().all(|c| c.is_ascii_alphanumeric() || "/-+.".contains(c))).unwrap_or_else(|| "application/octet-stream".into());
    let token = token();
    if let Ok(mut s) = r.streams.lock() {
        // keep the table small: a session plays a few videos
        if s.len() > 64 {
            s.clear();
        }
        s.insert(token.clone(), Stream { url: url.to_string(), length, mime });
    }
    format!("http://127.0.0.1:{}/s/{token}", r.port)
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
    let mut at = start;
    while at <= end {
        let to = (at + PIECE - 1).min(end);
        let mut child = std::process::Command::new("curl")
            .args(["-sfL", "--proto", "=https", "--proto-redir", "=https", "--max-time", "120"])
            .arg(format!("{}&range={at}-{to}", s.url))
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
}
