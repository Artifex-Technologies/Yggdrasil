//! The Python behaviours the scripts' results depend on.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// Text as open(path, encoding="utf-8").read() gives it: universal newlines ("\r\n" and "\r" become "\n"). `replace`:
/// errors="replace" (bad bytes become U+FFFD); without it, bytes that are not UTF-8 are an error (UnicodeDecodeError).
pub fn read_text(path: &Path, replace: bool) -> std::io::Result<Result<String, String>> {
    let bytes = std::fs::read(path)?;
    Ok(decode(bytes, replace).map(|s| universal_newlines(&s)))
}

/// bytes.decode("utf-8") (strict: Err is the message) or with errors="replace".
pub fn decode(bytes: Vec<u8>, replace: bool) -> Result<String, String> {
    match String::from_utf8(bytes) {
        Ok(s) => Ok(s),
        Err(e) if replace => Ok(String::from_utf8_lossy(e.as_bytes()).into_owned()),
        Err(e) => Err(format!("'utf-8' codec can't decode: {}", e.utf8_error())),
    }
}

/// Python's text mode reading with newline=None: "\r\n" and a lone "\r" are read as "\n".
pub fn universal_newlines(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// The files glob.glob(os.path.join(glob.escape(dir), "*" + suffix)) finds: the folder's names in its own order (as
/// os.scandir gives them), without hidden ones (a leading dot), ending in `suffix`; nothing when the folder cannot be read.
pub fn listing(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out = vec![];
    for entry in rd {
        let Ok(entry) = entry else {
            // os.scandir stops at a read error
            break;
        };
        let name = entry.file_name();
        let b = name.as_bytes();
        if b.first() == Some(&b'.') || !b.ends_with(suffix.as_bytes()) {
            continue;
        }
        out.push(join(dir, &name));
    }
    out
}

/// os.path.join(a, b) for one more part.
pub fn join(a: &Path, b: impl AsRef<OsStr>) -> PathBuf {
    let b = b.as_ref();
    if b.as_bytes().first() == Some(&b'/') {
        return PathBuf::from(b);
    }
    let mut v = a.as_os_str().as_bytes().to_vec();
    if !v.is_empty() && v.last() != Some(&b'/') {
        v.push(b'/');
    }
    v.extend_from_slice(b.as_bytes());
    PathBuf::from(OsString::from_vec(v))
}

/// os.path.dirname(p)
pub fn dirname(p: &OsStr) -> OsString {
    let b = p.as_bytes();
    let i = b
        .iter()
        .rposition(|&c| c == b'/')
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut head = &b[..i];
    if !head.is_empty() && head.iter().any(|&c| c != b'/') {
        while head.last() == Some(&b'/') {
            head = &head[..head.len() - 1];
        }
    }
    OsString::from_vec(head.to_vec())
}

/// os.path.basename(p)
pub fn basename(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

/// os.path.expanduser("~" + rest) for a rest starting with "/": $HOME (or the account's home folder when HOME is not set)
/// without its trailing slashes, then the rest.
pub fn expanduser(rest: &str) -> PathBuf {
    let home: Vec<u8> = match std::env::var_os("HOME") {
        Some(h) => h.into_vec(),
        None => passwd_home().unwrap_or_default(),
    };
    let mut h = &home[..];
    while h.last() == Some(&b'/') {
        h = &h[..h.len() - 1];
    }
    let mut v = h.to_vec();
    v.extend_from_slice(rest.as_bytes());
    if v.is_empty() {
        v.push(b'/');
    }
    PathBuf::from(OsString::from_vec(v))
}

fn passwd_home() -> Option<Vec<u8>> {
    let mut buf = vec![0 as libc::c_char; 16384];
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    let r = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut pw,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        )
    };
    if r != 0 || out.is_null() || pw.pw_dir.is_null() {
        return None;
    }
    Some(
        unsafe { std::ffi::CStr::from_ptr(pw.pw_dir) }
            .to_bytes()
            .to_vec(),
    )
}

/// os.environ.get(name, default): a variable that is set, even to nothing, is used.
pub fn env_or(name: &str, default: impl FnOnce() -> PathBuf) -> PathBuf {
    match std::env::var_os(name) {
        Some(v) => PathBuf::from(v),
        None => default(),
    }
}

/// os.environ.get(name) when it is set and not empty.
pub fn env_nonempty(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|v| !v.is_empty())
}

/// f"{x:.1f}"
pub fn f1(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    format!("{x:.1}")
}

/// f"{x:g}": six significant digits, trailing zeros dropped, an exponent below 1e-4 and from 1e6.
pub fn g(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    // the exponent after rounding to six digits decides the form
    let e_form = format!("{x:.5e}");
    let (mant, exp) = e_form.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    if (-4..6).contains(&exp) {
        let digits = (5 - exp) as usize;
        strip_zeros(&format!("{x:.digits$}"))
    } else {
        let m = strip_zeros(mant);
        format!("{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

fn strip_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// float(text): surrounding whitespace, decimal digits of any script, single underscores between digits, inf/infinity/nan
/// in any case, a sign. None: a ValueError.
pub fn float(text: &str) -> Option<f64> {
    let s = pycompat::strip(text);
    let mut ascii = String::with_capacity(s.len());
    for c in s.chars() {
        if let Some(d) = pycompat::decimal(c) {
            ascii.push(char::from_digit(d, 10).unwrap());
        } else if c.is_ascii() && !c.is_ascii_whitespace() {
            ascii.push(c);
        } else {
            return None;
        }
    }
    let (sign, body) = match ascii.as_bytes().first() {
        Some(b'-') => (-1.0, &ascii[1..]),
        Some(b'+') => (1.0, &ascii[1..]),
        _ => (1.0, &ascii[..]),
    };
    let lower = body.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" {
        return Some(sign * f64::INFINITY);
    }
    if lower == "nan" {
        return Some(if sign < 0.0 { -f64::NAN } else { f64::NAN });
    }
    // digits [. digits] [e [sign] digits], underscores only between two digits
    let b = body.as_bytes();
    let mut plain = String::with_capacity(b.len());
    let mut i = 0;
    let digits = |i: &mut usize, plain: &mut String| -> usize {
        let start = *i;
        let mut n = 0;
        while *i < b.len() {
            if b[*i].is_ascii_digit() {
                plain.push(b[*i] as char);
                n += 1;
                *i += 1;
            } else if b[*i] == b'_'
                && *i > start
                && b[*i - 1].is_ascii_digit()
                && b.get(*i + 1).is_some_and(|c| c.is_ascii_digit())
            {
                *i += 1;
            } else {
                break;
            }
        }
        n
    };
    let int_digits = digits(&mut i, &mut plain);
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        plain.push('.');
        i += 1;
        frac_digits = digits(&mut i, &mut plain);
    }
    if int_digits + frac_digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        plain.push('e');
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            plain.push(b[i] as char);
            i += 1;
        }
        if digits(&mut i, &mut plain) == 0 {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    plain.parse::<f64>().ok().map(|v| sign * v)
}

/// max(a, b) of two floats as Python picks: the first unless the second is greater (so a NaN first stays).
pub fn max(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

/// str.lower() with only the letters and digits a-z 0-9 kept (the scripts' norm()).
pub fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_as_python_does() {
        assert_eq!(f1(0.25), "0.2");
        assert_eq!(f1(0.35), "0.3");
        assert_eq!(f1(-0.0), "-0.0");
        assert_eq!(f1(1008.05), "1008.0");
        assert_eq!(g(24.0), "24");
        assert_eq!(g(0.0), "0");
        assert_eq!(g(-0.0), "-0");
        assert_eq!(g(1e-5), "1e-05");
        assert_eq!(g(123456.0), "123456");
        assert_eq!(g(1234567.0), "1.23457e+06");
        assert_eq!(g(0.0001), "0.0001");
        assert_eq!(g(2.5), "2.5");
        assert_eq!(g(f64::NAN), "nan");
    }

    #[test]
    fn float_reads_as_python_does() {
        assert_eq!(float(" 24 "), Some(24.0));
        assert_eq!(float("1_0"), Some(10.0));
        assert_eq!(float("1__0"), None);
        assert_eq!(float("_1"), None);
        assert_eq!(float(".5"), Some(0.5));
        assert_eq!(float("5."), Some(5.0));
        assert_eq!(float("."), None);
        assert_eq!(float("1e"), None);
        assert_eq!(float("-Infinity"), Some(f64::NEG_INFINITY));
        assert!(float("nan").unwrap().is_nan());
        assert_eq!(float("١٢"), Some(12.0));
        assert_eq!(float("0x10"), None);
        assert_eq!(float("1,2"), None);
    }

    #[test]
    fn paths_join_as_os_path_does() {
        assert_eq!(join(Path::new("a"), "b"), PathBuf::from("a/b"));
        assert_eq!(join(Path::new("a/"), "b"), PathBuf::from("a/b"));
        assert_eq!(join(Path::new(""), "b"), PathBuf::from("b"));
        assert_eq!(join(Path::new("a"), "/b"), PathBuf::from("/b"));
        assert_eq!(join(Path::new("a"), "").as_os_str(), "a/");
        assert_eq!(
            dirname(OsStr::new("/usr/share/liki/brand-icons")),
            "/usr/share/liki"
        );
        assert_eq!(dirname(OsStr::new("x/brand-icons/")), "x/brand-icons");
        assert_eq!(dirname(OsStr::new("///a")), "///");
        assert_eq!(dirname(OsStr::new("a")), "");
        assert_eq!(universal_newlines("a\r\nb\rc\n"), "a\nb\nc\n");
    }
}
