//! One request on a Unix socket and its answer, bounded: a time limit for the whole exchange and a size limit for the answer, so
//! a compositor that hangs or talks without end cannot hold up or fill up the program asking.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

/// The largest answer read (Hyprland's client list with a hundred windows is about 100 KB).
pub const MAX_REPLY: usize = 4 << 20;

/// Sends `msg` and reads until the other side closes. None when the socket cannot be reached, the time runs out first or the
/// answer grows past `MAX_REPLY`.
pub fn request(path: &Path, msg: &[u8], timeout: Duration) -> Option<String> {
    request_limited(path, msg, timeout, MAX_REPLY)
}

pub fn request_limited(path: &Path, msg: &[u8], timeout: Duration, max: usize) -> Option<String> {
    let deadline = Instant::now() + timeout;
    let mut s = UnixStream::connect(path).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    s.write_all(msg).ok()?;
    let mut answer: Vec<u8> = vec![];
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let left = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero())?;
        s.set_read_timeout(Some(left)).ok()?;
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if answer.len() + n > max {
                    return None;
                }
                answer.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
    Some(String::from_utf8_lossy(&answer).into_owned())
}

/// A socket this user made: a socket file (not a link to one elsewhere) owned by this user. Anything else at that path is not
/// spoken to.
pub fn ours(path: &Path) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let Ok(m) = std::fs::symlink_metadata(path) else { return false };
    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    m.file_type().is_socket() && Some(m.uid()) == me
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ygg-wm-sock-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_answer_is_read_to_the_end() {
        let d = dir("end");
        let p = d.join("s");
        let l = UnixListener::bind(&p).unwrap();
        let t = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut b = [0u8; 64];
            let n = c.read(&mut b).unwrap();
            assert_eq!(&b[..n], b"j/clients");
            c.write_all(&vec![b'x'; 100_000]).unwrap();
        });
        assert_eq!(request(&p, b"j/clients", Duration::from_secs(5)).map(|a| a.len()), Some(100_000));
        t.join().unwrap();
        assert!(ours(&p));
        assert!(!ours(&d), "a folder is not a socket");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn too_long_or_too_slow_is_nothing() {
        let d = dir("bound");
        let p = d.join("s");
        let l = UnixListener::bind(&p).unwrap();
        let t = std::thread::spawn(move || {
            // too long
            let (mut c, _) = l.accept().unwrap();
            let _ = c.write_all(&vec![b'x'; 5000]);
            // too slow: answers and never closes
            let (mut c2, _) = l.accept().unwrap();
            let _ = c2.write_all(b"ok");
            std::thread::sleep(Duration::from_millis(800));
        });
        assert_eq!(request_limited(&p, b"q", Duration::from_secs(5), 4096), None);
        let start = Instant::now();
        assert_eq!(request(&p, b"q", Duration::from_millis(300)), None);
        assert!(start.elapsed() < Duration::from_millis(700), "the time limit is for the whole exchange");
        t.join().unwrap();
        assert_eq!(request(&d.join("none"), b"q", Duration::from_millis(100)), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
