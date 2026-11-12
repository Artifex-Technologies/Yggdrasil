//! How the Python script met the outside, so the port meets it the same way: text read and decoded strictly as UTF-8 in
//! universal-newline mode, files written as open(path, "w") wrote them, folders made as os.makedirs made them, and the words
//! of the exceptions it raised (`[Errno 2] No such file or directory: 'path'`), which the install's ERROR line carries.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;

/// Why the install stops: `Stop` is the script's own message (`raise Stop(...)`), `Crash` an exception it did not expect
/// (in the install, "unexpected: ..."; elsewhere a traceback).
#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    Stop(String),
    Crash(String),
}

pub type R<T> = Result<T, Fail>;

/// The C library's text for an error number (what Python puts in an OSError).
pub fn strerror(code: i32) -> String {
    unsafe { std::ffi::CStr::from_ptr(libc::strerror(code)).to_string_lossy().into_owned() }
}

fn errno_text(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(code) => format!("[Errno {code}] {}", strerror(code)),
        None => e.to_string(),
    }
}

/// str(OSError) for one file: "[Errno 2] No such file or directory: 'path'".
pub fn oserror(e: &io::Error, path: &str) -> Fail {
    Fail::Crash(format!("{}: {}", errno_text(e), pycompat::repr(path)))
}

/// str(OSError) for two files (os.symlink): "[Errno 17] File exists: 'src' -> 'dst'".
pub fn oserror2(e: &io::Error, src: &str, dst: &str) -> Fail {
    Fail::Crash(format!("{}: {} -> {}", errno_text(e), pycompat::repr(src), pycompat::repr(dst)))
}

/// Bytes decoded as Python's strict UTF-8 codec does, with its UnicodeDecodeError's words when they are not UTF-8.
pub fn decode(bytes: &[u8]) -> R<String> {
    match std::str::from_utf8(bytes) {
        Ok(t) => Ok(t.to_string()),
        Err(e) => {
            let start = e.valid_up_to();
            let (end, why) = match e.error_len() {
                None => (bytes.len(), "unexpected end of data"),
                Some(n) => {
                    let b = bytes[start];
                    let reason = if (0x80..=0xc1).contains(&b) || b >= 0xf5 { "invalid start byte" } else { "invalid continuation byte" };
                    (start + n, reason)
                }
            };
            Err(Fail::Crash(if end - start == 1 {
                format!("'utf-8' codec can't decode byte 0x{:02x} in position {start}: {why}", bytes[start])
            } else {
                format!("'utf-8' codec can't decode bytes in position {start}-{}: {why}", end - 1)
            }))
        }
    }
}

/// Text in universal-newline mode: \r\n and a lone \r become \n.
pub fn universal(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_string()
    }
}

/// open(path).read()
pub fn read_text(path: &str) -> R<String> {
    let bytes = fs::read(path).map_err(|e| oserror(&e, path))?;
    Ok(universal(&decode(&bytes)?))
}

/// open(path, "w").write(text)
pub fn write_text(path: &str, text: &str) -> R<()> {
    fs::write(path, text).map_err(|e| oserror(&e, path))
}

/// os.chmod(path, mode)
pub fn chmod(path: &str, mode: u32) -> R<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| oserror(&e, path))
}

/// os.remove(path)
pub fn remove(path: &str) -> R<()> {
    fs::remove_file(path).map_err(|e| oserror(&e, path))
}

/// os.symlink(src, dst)
pub fn symlink(src: &str, dst: &str) -> R<()> {
    std::os::unix::fs::symlink(src, dst).map_err(|e| oserror2(&e, src, dst))
}

/// os.listdir(path): the names, in the order the folder gives them.
pub fn listdir(path: &str) -> R<Vec<String>> {
    let rd = fs::read_dir(path).map_err(|e| oserror(&e, path))?;
    let mut out = vec![];
    for e in rd {
        let e = e.map_err(|e| oserror(&e, path))?;
        out.push(e.file_name().to_string_lossy().into_owned());
    }
    Ok(out)
}

/// os.path.exists (following links; False on any error)
pub fn exists(path: &str) -> bool {
    !path.contains('\0') && fs::metadata(path).is_ok()
}

/// os.path.lexists
pub fn lexists(path: &str) -> bool {
    !path.contains('\0') && fs::symlink_metadata(path).is_ok()
}

/// os.path.isfile
pub fn isfile(path: &str) -> bool {
    !path.contains('\0') && fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// os.path.isdir
pub fn isdir(path: &str) -> bool {
    !path.contains('\0') && fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// os.path.islink
pub fn islink(path: &str) -> bool {
    !path.contains('\0') && fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// os.path.split(p): the head (without trailing slashes, unless it is all slashes) and the last part.
pub fn split(p: &str) -> (String, String) {
    let i = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let (head, tail) = (&p[..i], &p[i..]);
    let trimmed = head.trim_end_matches('/');
    (if trimmed.is_empty() { head.to_string() } else { trimmed.to_string() }, tail.to_string())
}

/// os.path.dirname(p)
pub fn dirname(p: &str) -> String {
    split(p).0
}

/// os.path.join(a, b)
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// os.makedirs(name, exist_ok=True), step by step as Python takes them (so a failure names the same folder).
pub fn makedirs(name: &str) -> R<()> {
    let (mut head, mut tail) = split(name);
    if tail.is_empty() {
        (head, tail) = split(&head);
    }
    if !head.is_empty() && !tail.is_empty() && !exists(&head) {
        match makedirs(&head) {
            Ok(()) => {}
            Err(Fail::Crash(m)) if m.starts_with(&format!("[Errno {}]", libc::EEXIST)) => {}
            Err(e) => return Err(e),
        }
        if tail == "." {
            return Ok(());
        }
    }
    match fs::create_dir(name) {
        Ok(()) => Ok(()),
        Err(e) => {
            if isdir(name) {
                Ok(())
            } else {
                Err(oserror(&e, name))
            }
        }
    }
}

/// The last `n` characters of a text (text[-n:], n > 0).
pub fn tail_chars(text: &str, n: usize) -> &str {
    match text.char_indices().rev().nth(n.saturating_sub(1)) {
        Some((i, _)) => &text[i..],
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_errors_in_pythons_words() {
        let m = |b: &[u8]| match decode(b) {
            Err(Fail::Crash(m)) => m,
            other => panic!("{other:?}"),
        };
        assert_eq!(m(b"abc\xff"), "'utf-8' codec can't decode byte 0xff in position 3: invalid start byte");
        assert_eq!(m(b"\xe2\x28\xa1"), "'utf-8' codec can't decode byte 0xe2 in position 0: invalid continuation byte");
        assert_eq!(m(b"a\xe2\x82\x28"), "'utf-8' codec can't decode bytes in position 1-2: invalid continuation byte");
        assert_eq!(m(b"a\xe2\x82"), "'utf-8' codec can't decode bytes in position 1-2: unexpected end of data");
        assert_eq!(m(b"a\xe2"), "'utf-8' codec can't decode byte 0xe2 in position 1: unexpected end of data");
        assert_eq!(m(b"\xed\xa0\x80"), "'utf-8' codec can't decode byte 0xed in position 0: invalid continuation byte");
        assert_eq!(m(b"\xc0\x80"), "'utf-8' codec can't decode byte 0xc0 in position 0: invalid start byte");
    }

    #[test]
    fn paths_as_os_path_splits_them() {
        assert_eq!(split("/a/b"), ("/a".into(), "b".into()));
        assert_eq!(split("/a/b/"), ("/a/b".into(), "".into()));
        assert_eq!(split("/a"), ("/".into(), "a".into()));
        assert_eq!(split("//a"), ("//".into(), "a".into()));
        assert_eq!(split("a"), ("".into(), "a".into()));
        assert_eq!(join("/x", "/y"), "/y");
        assert_eq!(join("/x/", "y"), "/x/y");
        assert_eq!(tail_chars("abcdé", 2), "dé");
        assert_eq!(tail_chars("ab", 5), "ab");
    }
}
