//! The few Python behaviours the helpers' decisions and output depend on, so these helpers write the same bytes and make the
//! same choices as the Python scripts they replace: round() (halves to even), int() on text, str.isdigit(), repr() of text
//! and of numbers, and an OSError's text.

use std::fmt;
use std::io;
use std::path::Path;

/// round(x): the nearest whole number, halves to the even one (round(2.5) == 2, round(3.5) == 4).
pub fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// round(x, 1): one decimal, from the number's exact value, exact halves to even (as Python's float round does).
pub fn round1(x: f64) -> f64 {
    format!("{x:.1}").parse().unwrap_or(x)
}

/// str.isdigit() for the text these helpers see (ASCII digits; at least one).
pub fn isdigit(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Python's str.strip() with no argument.
pub fn strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// int(text): surrounding whitespace, a sign, digits with single underscores between them. A number beyond i64 is clamped
/// (it fails every range these helpers check, as Python's would). Err is the ValueError's text.
pub fn int(text: &str) -> Result<i64, String> {
    let bad = || format!("invalid literal for int() with base 10: {}", repr(text));
    let s = strip(text);
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let b = digits.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() || !b[b.len() - 1].is_ascii_digit() {
        return Err(bad());
    }
    let mut v: i128 = 0;
    let mut prev_us = false;
    for &c in b {
        if c == b'_' {
            if prev_us {
                return Err(bad());
            }
            prev_us = true;
            continue;
        }
        if !c.is_ascii_digit() {
            return Err(bad());
        }
        prev_us = false;
        v = (v * 10 + (c - b'0') as i128).min(i64::MAX as i128 + 1);
    }
    let v = if neg { -v } else { v };
    Ok(v.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
}

/// float(text): surrounding whitespace, a sign, digits with single underscores between them, a point, an exponent, or
/// inf/infinity/nan in any case. Err is the ValueError's text.
pub fn float(text: &str) -> Result<f64, String> {
    let bad = || format!("could not convert string to float: {}", repr(text));
    let s = strip(text);
    let b = s.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c == b'_' && !(i > 0 && b[i - 1].is_ascii_digit() && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            return Err(bad());
        }
    }
    let plain: String = s.chars().filter(|c| *c != '_').collect();
    if plain.is_empty() || plain.contains(char::is_whitespace) {
        return Err(bad());
    }
    plain.parse::<f64>().map_err(|_| bad())
}

/// text.splitlines()[0], or None when there is no line (the separators str.splitlines() knows).
pub fn first_line(text: &str) -> Option<&str> {
    if text.is_empty() {
        return None;
    }
    let end = text.find(['\n', '\r', '\x0b', '\x0c', '\x1c', '\x1d', '\x1e', '\u{85}', '\u{2028}', '\u{2029}']).unwrap_or(text.len());
    Some(&text[..end])
}

fn printable(c: char) -> bool {
    let u = c as u32;
    !(u < 0x20
        || (0x7f..=0xa0).contains(&u)
        || u == 0xad
        || u == 0x1680
        || (0x2000..=0x200f).contains(&u)
        || (0x2028..=0x202f).contains(&u)
        || (0x205f..=0x2064).contains(&u)
        || (0x2066..=0x206f).contains(&u)
        || u == 0x3000
        || u == 0xfeff
        || (0xe000..=0xf8ff).contains(&u)
        || (0xfff9..=0xfffb).contains(&u)
        || u >= 0xf0000)
}

/// repr(text): in single quotes (double ones when the text has a single quote and no double one), with escapes.
pub fn repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(q);
    out
}

/// repr() of a list of texts: ['a', 'b'].
pub fn repr_list(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| repr(s)).collect::<Vec<_>>().join(", "))
}

/// repr(float): the shortest digits that read back as the same number, as Python writes them (1.0, 0.0001, 1e-05, 1e+16).
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let e = format!("{:e}", x.abs());
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let sign = if x.is_sign_negative() { "-" } else { "" };
    let decpt = exp + 1;
    if decpt <= -4 || decpt > 16 {
        let m = if digits.len() > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits.clone() };
        return format!("{sign}{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    let n = digits.len() as i32;
    let body = if decpt <= 0 {
        format!("0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if decpt >= n {
        format!("{}{}.0", digits, "0".repeat((decpt - n) as usize))
    } else {
        format!("{}.{}", &digits[..decpt as usize], &digits[decpt as usize..])
    };
    format!("{sign}{body}")
}

/// An OSError as Python prints it: "[Errno 13] Permission denied: '/dev/cpu/0/msr'", or a plain message.
#[derive(Clone, Debug, PartialEq)]
pub enum OsErr {
    Errno { code: i32, path: Option<String>, path2: Option<String> },
    Msg(String),
}

impl OsErr {
    pub fn at(e: &io::Error, path: &Path) -> OsErr {
        match e.raw_os_error() {
            Some(code) => OsErr::Errno { code, path: Some(path.display().to_string()), path2: None },
            None => OsErr::Msg(e.to_string()),
        }
    }

    pub fn between(e: &io::Error, a: &Path, b: &Path) -> OsErr {
        match e.raw_os_error() {
            Some(code) => OsErr::Errno { code, path: Some(a.display().to_string()), path2: Some(b.display().to_string()) },
            None => OsErr::Msg(e.to_string()),
        }
    }

    pub fn code(&self) -> Option<i32> {
        match self {
            OsErr::Errno { code, .. } => Some(*code),
            OsErr::Msg(_) => None,
        }
    }
}

/// The C library's text for an error number ("Permission denied").
pub fn strerror(code: i32) -> String {
    let s = io::Error::from_raw_os_error(code).to_string();
    match s.rfind(" (os error ") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

impl fmt::Display for OsErr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            OsErr::Msg(m) => f.write_str(m),
            OsErr::Errno { code, path, path2 } => {
                write!(f, "[Errno {code}] {}", strerror(*code))?;
                if let Some(p) = path {
                    write!(f, ": {}", repr(p))?;
                }
                if let Some(p) = path2 {
                    write!(f, " -> {}", repr(p))?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_pythons() {
        assert_eq!((round(2.5), round(3.5), round(-0.5), round(127.5), round(76.5)), (2, 4, 0, 128, 76));
        assert_eq!((round1(0.25), round1(0.75), round1(2.675), round1(12.25)), (0.2, 0.8, 2.7, 12.2));
    }

    #[test]
    fn int_and_isdigit_are_pythons() {
        assert_eq!(int(" 42\n"), Ok(42));
        assert_eq!(int("+7"), Ok(7));
        assert_eq!(int("-300"), Ok(-300));
        assert_eq!(int("1_000"), Ok(1000));
        assert_eq!(int("99999999999999999999999"), Ok(i64::MAX));
        assert_eq!(int("abc"), Err("invalid literal for int() with base 10: 'abc'".into()));
        assert!(int("1__0").is_err() && int("_1").is_err() && int("").is_err() && int("--5").is_err());
        assert!(isdigit("007") && !isdigit("") && !isdigit("-1") && !isdigit("1.0"));
    }

    #[test]
    fn float_and_splitlines_are_pythons() {
        assert_eq!(float(" 250.00 "), Ok(250.0));
        assert_eq!((float("1_000.5"), float("-3"), float(".5"), float("1e3")), (Ok(1000.5), Ok(-3.0), Ok(0.5), Ok(1000.0)));
        assert!(float("inf").unwrap().is_infinite() && float("NaN").unwrap().is_nan());
        assert_eq!(float("[N/A]"), Err("could not convert string to float: '[N/A]'".into()));
        assert!(float("1__0").is_err() && float("_1").is_err() && float("").is_err() && float("1 0").is_err());
        assert_eq!(first_line("a, b\nc\n"), Some("a, b"));
        assert_eq!((first_line("x\r\ny"), first_line("\n"), first_line("")), (Some("x"), Some(""), None));
    }

    #[test]
    fn repr_is_pythons() {
        assert_eq!(repr("40-30"), "'40-30'");
        assert_eq!(repr("it's"), "\"it's\"");
        assert_eq!(repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(repr("tab\there\x01"), "'tab\\there\\x01'");
        assert_eq!(repr_list(&["pl1=120".into(), "pl2=160".into()]), "['pl1=120', 'pl2=160']");
        let cases = [
            (1.0, "1.0"),
            (0.0001, "0.0001"),
            (1.5e-5, "1.5e-05"),
            (1e16, "1e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (-0.0, "-0.0"),
            (12.3, "12.3"),
            (1762687421.123456, "1762687421.123456"),
            (1e100, "1e+100"),
            (0.1, "0.1"),
        ];
        for (x, s) in cases {
            assert_eq!(float_repr(x), s);
        }
    }

    #[test]
    fn oserror_text_is_pythons() {
        let e = io::Error::from_raw_os_error(13);
        assert_eq!(OsErr::at(&e, Path::new("/dev/cpu/0/msr")).to_string(), "[Errno 13] Permission denied: '/dev/cpu/0/msr'");
        assert_eq!(OsErr::Msg("NVML: init failed (error 9)".into()).to_string(), "NVML: init failed (error 9)");
    }
}
