//! The few Python behaviours that Yggdrasil's Rust ports of its Python helpers depend on, so a port prints the same bytes and
//! makes the same decisions as the script it replaces:
//!
//!   - json: json.dumps' output (", " and ": ", or indent=N with ","; ensure_ascii; Python's float repr), over serde_json
//!     values that keep a file's key order (as a dict does);
//!   - text: int() (base 10 and 16, with Unicode digits, underscores, whitespace), str.isdigit(), str.isspace(), strip() and
//!     split() with no argument, repr() of text and of a list of text, os.path.normpath().
//!
//! The Unicode tables (unicode_tables.rs) are generated from Python's own str methods by gen_unicode.py.

pub mod json;
mod unicode_tables;

use unicode_tables::{DECIMAL_ZEROS, DIGIT, PRINTABLE, SPACE, WORD};

fn in_ranges(table: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// The value of a decimal digit of any script (what int() and the regular expression \d accept), else None.
pub fn decimal(c: char) -> Option<u32> {
    let cp = c as u32;
    let i = DECIMAL_ZEROS.partition_point(|&z| z <= cp);
    if i == 0 {
        return None;
    }
    let zero = DECIMAL_ZEROS[i - 1];
    (cp - zero < 10).then_some(cp - zero)
}

/// str.isspace() of one character.
pub fn isspace(c: char) -> bool {
    in_ranges(SPACE, c)
}

/// str.isdigit(): at least one character, and every one a digit (decimal digits and the other digits: superscripts, circled ...).
pub fn isdigit(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| in_ranges(DIGIT, c))
}

/// str.isprintable() of one character.
pub fn isprintable(c: char) -> bool {
    in_ranges(PRINTABLE, c)
}

/// \w in a Python pattern on text (str.isalnum() or "_").
pub fn isword(c: char) -> bool {
    in_ranges(WORD, c)
}

/// \s in a Python pattern on text (str.isspace()).
pub const PY_SPACE_CLASS: &str = r"[\s\x1c-\x1f]";

/// str.strip() with no argument.
pub fn strip(s: &str) -> &str {
    s.trim_matches(isspace)
}

/// str.rstrip() with no argument.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(isspace)
}

/// str.split() with no argument: the words between runs of whitespace.
pub fn split(s: &str) -> Vec<&str> {
    s.split(isspace).filter(|w| !w.is_empty()).collect()
}

/// str.splitlines(): lines without their ends, which are \n, \r, \r\n, \v, \f, \x1c-\x1e, \x85, U+2028 and U+2029.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let end = match c {
            '\r' => {
                if let Some((_, '\n')) = it.peek() {
                    it.next();
                    i + 2
                } else {
                    i + 1
                }
            }
            '\n' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}' => i + c.len_utf8(),
            _ => continue,
        };
        out.push(&s[start..i]);
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// int(text) (base 10): surrounding whitespace, a sign, decimal digits of any script with single underscores between them.
/// A number beyond i128 is clamped (it fails every range a helper checks, as Python's big number would). None: a ValueError.
pub fn int(text: &str) -> Option<i128> {
    int_base(text, 10)
}

/// int(text, 16): as int(), with hexadecimal digits and an optional 0x before them.
pub fn int16(text: &str) -> Option<i128> {
    int_base(text, 16)
}

fn int_base(text: &str, base: u32) -> Option<i128> {
    let s = strip(text);
    let (neg, mut rest) = match s.chars().next() {
        Some('-') => (true, &s[1..]),
        Some('+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut after_prefix = false;
    if base == 16 && (rest.starts_with("0x") || rest.starts_with("0X")) {
        rest = &rest[2..];
        after_prefix = true;
    }
    let mut value: i128 = 0;
    let mut digits = 0usize;
    let mut last_underscore = false;
    for (i, c) in rest.chars().enumerate() {
        if c == '_' {
            // between digits, or straight after 0x
            if last_underscore || (i == 0 && !after_prefix) {
                return None;
            }
            last_underscore = true;
            continue;
        }
        let d = match decimal(c) {
            Some(d) => d,
            None if base == 16 && c.is_ascii_hexdigit() => c.to_digit(16).unwrap(),
            None => return None,
        };
        value = value.saturating_mul(base as i128).saturating_add(d as i128);
        digits += 1;
        last_underscore = false;
    }
    if digits == 0 || last_underscore {
        return None;
    }
    Some(if neg { -value } else { value })
}

/// repr(text): in single quotes (double ones when the text has a single quote and no double one), with Python's escapes.
pub fn repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x7f => out.push(c),
            c if isprintable(c) => out.push(c),
            c if (c as u32) <= 0xff => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) <= 0xffff => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}

/// repr() of bytes: b'...' with \xhh for every byte outside space..~ (and \t \n \r).
pub fn repr_bytes(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') { b'"' } else { b'\'' };
    let mut out = String::from("b");
    out.push(quote as char);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(quote as char);
            }
            0x20..=0x7e => out.push(c as char),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote as char);
    out
}

/// repr() of a list of text: ['a', 'b'].
pub fn repr_list<S: AsRef<str>>(items: &[S]) -> String {
    let parts: Vec<String> = items.iter().map(|s| repr(s.as_ref())).collect();
    format!("[{}]", parts.join(", "))
}

/// os.path.normpath() on a POSIX path: no empty or "." parts, ".." taken back, two leading slashes kept (three or more become one).
pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let mut initial = if path.starts_with('/') { 1 } else { 0 };
    if path.starts_with("//") && !path.starts_with("///") {
        initial = 2;
    }
    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let joined = comps.join("/");
    let out = "/".repeat(initial) + &joined;
    if out.is_empty() {
        ".".into()
    } else {
        out
    }
}

/// round(x) of a float: the nearest whole number, halves to the even one.
pub fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// x % y of two floats, as Python does it: the result has the sign of y.
pub fn fmod(x: f64, y: f64) -> f64 {
    let m = x % y;
    if m != 0.0 && ((y < 0.0) != (m < 0.0)) {
        m + y
    } else if m == 0.0 {
        0.0f64.copysign(y)
    } else {
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_as_python() {
        assert_eq!(int(" 42 "), Some(42));
        assert_eq!(int("+7"), Some(7));
        assert_eq!(int("-1_000"), Some(-1000));
        assert_eq!(int("007"), Some(7));
        assert_eq!(int("\u{0661}\u{0662}"), Some(12), "Arabic-Indic digits");
        assert_eq!(int("\u{ff12}"), Some(2), "a fullwidth digit");
        for bad in ["", " ", "_1", "1_", "1__0", "1.0", "0x10", "\u{b2}", "1 2", "--1"] {
            assert_eq!(int(bad), None, "{bad:?}");
        }
        assert_eq!(int16("0x10de"), Some(0x10de));
        assert_eq!(int16("10DE\n"), Some(0x10de));
        assert_eq!(int16("0x_1f"), Some(0x1f));
        assert_eq!(int16("0x"), None);
        assert_eq!(int16("x10"), None);
    }

    #[test]
    fn digits_and_space() {
        assert!(isdigit("123"));
        assert!(isdigit("\u{b2}"), "a superscript two is a digit");
        assert!(!isdigit(""));
        assert!(!isdigit("1a"));
        assert!(isspace('\u{1c}') && isspace('\u{85}') && isspace('\u{3000}'));
        assert!(!isspace('\u{200b}'));
        assert_eq!(strip("\u{1f} a b\u{3000}"), "a b");
        assert_eq!(split("  a \t b\n"), vec!["a", "b"]);
        assert_eq!(splitlines("a\r\nb\rc\nd\u{b}e\u{2028}f\n"), vec!["a", "b", "c", "d", "e", "f"]);
        assert_eq!(splitlines("a\n\nb"), vec!["a", "", "b"]);
        assert_eq!(splitlines(""), Vec::<&str>::new());
    }

    #[test]
    fn repr_as_python() {
        assert_eq!(repr("abc"), "'abc'");
        assert_eq!(repr("it's"), "\"it's\"");
        assert_eq!(repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(repr("t\tn\nx\x01\x7f\\"), "'t\\tn\\nx\\x01\\x7f\\\\'");
        assert_eq!(repr("caf\u{e9}"), "'caf\u{e9}'");
        assert_eq!(repr("\u{a0}\u{200b}\u{10ffff}"), "'\\xa0\\u200b\\U0010ffff'");
        assert_eq!(repr_list(&["3"]), "['3']");
        assert_eq!(repr_bytes(b"/usr/bin/x"), "b'/usr/bin/x'");
        assert_eq!(repr_bytes("caf\u{e9}'".as_bytes()), "b\"caf\\xc3\\xa9'\"");
        assert_eq!(repr_list::<&str>(&[]), "[]");
    }

    #[test]
    fn normpath_as_python() {
        for (p, want) in [
            ("", "."),
            ("/run/user/1000/agora/", "/run/user/1000/agora"),
            ("//run/x", "//run/x"),
            ("///run//x/./y", "/run/x/y"),
            ("/a/../../b", "/b"),
            ("a/../..", ".."),
            ("../x/..", ".."),
            ("/..", "/"),
        ] {
            assert_eq!(normpath(p), want, "{p}");
        }
    }

    #[test]
    fn floats() {
        assert_eq!(round(2.5), 2);
        assert_eq!(round(3.5), 4);
        assert_eq!(round(-0.5), 0);
        assert_eq!(fmod(-1.0, 6.0), 5.0);
        assert_eq!(fmod(7.5, 6.0), 1.5);
    }
}
