//! yggdrasil-zoom in|out|off: the magnifier. Hyprland's zoom around the pointer (cursor:zoom_factor), in steps of half a time,
//! from 1 (off) to 8. Super+Alt+= zooms in, Super+Alt+- out, Super+Alt+0 turns it off (docs/ACCESSIBILITY.md).
//!
//! As the script: the factor now from `hyprctl getoption cursor:zoom_factor` (its "float:" line), the next one worked out as awk
//! did (a number read from the front of the text, nothing readable being 0; printed with one decimal, rounded as C's printf
//! rounds), then `hyprctl eval` with Hyprland's Lua, its output to /dev/null.

use crate::run;
use std::process::{Command, Stdio};

const ME: &str = "yggdrasil-zoom";

pub fn main() {
    let a = std::env::args().nth(1).unwrap_or_default();
    if !matches!(a.as_str(), "in" | "out" | "off") {
        eprintln!("usage: yggdrasil-zoom in|out|off");
        std::process::exit(2);
    }
    let out = run::output(Command::new("hyprctl").args(["getoption", "cursor:zoom_factor"]));
    let cur = current(&String::from_utf8_lossy(&out));
    let next = next_factor(if cur.is_empty() { "1" } else { &cur }, &a);
    let mut c = Command::new("hyprctl");
    c.arg("eval").arg(format!("hl.config({{ cursor = {{ zoom_factor = {next} }} }})")).stdout(Stdio::null());
    run::exec(ME, c)
}

/// awk '$1 == "float:" { print $2; exit }': the second field of the first line whose first field is "float:" (fields are
/// separated by spaces and tabs).
pub fn current(out: &str) -> String {
    for line in out.split('\n') {
        let mut f = line.split([' ', '\t']).filter(|w| !w.is_empty());
        if f.next() == Some("float:") {
            return f.next().unwrap_or("").to_string();
        }
    }
    String::new()
}

/// The next factor, as the script's awk program worked it out and printed it ("%.1f").
pub fn next_factor(cur: &str, action: &str) -> String {
    let c = awk_number(cur);
    // between 1 and 8 (not a number stays one, as in awk, where both comparisons are false)
    let n = match action {
        "in" => c + 0.5,
        "out" => c - 0.5,
        _ => 1.0,
    }
    .clamp(1.0, 8.0);
    if n.is_nan() {
        return if n.is_sign_negative() { "-nan".into() } else { "+nan".into() };
    }
    format!("{n:.1}")
}

/// A text as gawk reads it as a number: blanks at the front skipped, then the longest decimal number there (sign, digits, a
/// point, an exponent); 0 when there is none. Only the four-letter "+inf", "-inf", "+nan" and "-nan" are gawk's special values
/// (no hexadecimal, no bare "inf").
pub fn awk_number(s: &str) -> f64 {
    let t = s.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    if t.len() == 4 {
        match t.to_ascii_lowercase().as_str() {
            "+inf" => return f64::INFINITY,
            "-inf" => return f64::NEG_INFINITY,
            "+nan" => return f64::NAN,
            "-nan" => return -f64::NAN,
            _ => {}
        }
    }
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    i += int;
    let mut frac = 0;
    if i < b.len() && b[i] == b'.' {
        frac = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if int + frac > 0 {
            i += 1 + frac;
        }
    }
    if int + frac == 0 {
        return 0.0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let digits = b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 {
            i = j + digits;
        }
    }
    t[..i].parse().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factor_hyprland_reports() {
        assert_eq!(current("float: 1.500000\nset: true\n"), "1.500000");
        assert_eq!(current("int: 3\n  float:\t2.0 x\n"), "2.0");
        assert_eq!(current("float:\n"), "");
        assert_eq!(current("no such option\n"), "");
    }

    // each answer is what gawk 5.4 printed for the same input (the script's awk program, run by hand)
    #[test]
    fn steps_and_rounding_as_awk() {
        for (c, a, want) in [
            ("1.000000", "in", "1.5"),
            ("1.25", "in", "1.8"),
            ("1.75", "in", "2.2"),
            ("1.75", "out", "1.2"),
            ("2.35", "in", "2.9"),
            ("1.05", "in", "1.6"),
            ("3.25", "out", "2.8"),
            ("7.75", "in", "8.0"),
            ("7.75", "out", "7.2"),
            ("8.0", "in", "8.0"),
            ("1", "out", "1.0"),
            ("5", "off", "1.0"),
            ("1e1", "in", "8.0"),
            (" 2.5x", "in", "3.0"),
            (".5", "in", "1.0"),
            ("abc", "in", "1.0"),
            ("0x10", "in", "1.0"),
            ("inf", "in", "1.0"),
            ("+inf", "out", "8.0"),
            ("-inf", "in", "1.0"),
            ("+nan", "in", "+nan"),
            ("-nan", "out", "-nan"),
        ] {
            assert_eq!(next_factor(c, a), want, "{c} {a}");
        }
    }
}
