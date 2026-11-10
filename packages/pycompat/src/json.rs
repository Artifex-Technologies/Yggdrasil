//! json.dumps' output for serde_json values: `dumps(v)` is json.dumps(v) (", " between items, ": " after keys), `dumps_indent(v,
//! n)` is json.dumps(v, indent=n) ("," at line ends). Text is written as ensure_ascii does (\uXXXX for everything outside
//! space..~, surrogate pairs above U+FFFF), floats as Python's repr writes them (1e-05, 1e+16, 2.0), and NaN and infinities as
//! NaN, Infinity and -Infinity (json's allow_nan).
//!
//! Reading: `loads` is serde_json with the keys in the file's order (Python's json.load keeps them so too).

pub use serde_json::{json, Map, Value};

/// json.loads(text); Err is the parser's message.
pub fn loads(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

/// json.dumps(v)
pub fn dumps(v: &Value) -> String {
    let mut out = String::new();
    write(&mut out, v, None, 0);
    out
}

/// json.dumps(v, indent=n)
pub fn dumps_indent(v: &Value, n: usize) -> String {
    let mut out = String::new();
    write(&mut out, v, Some(n), 0);
    out
}

fn newline(out: &mut String, indent: Option<usize>, level: usize) {
    if let Some(n) = indent {
        out.push('\n');
        out.push_str(&" ".repeat(n * level));
    }
}

fn write(out: &mut String, v: &Value, indent: Option<usize>, level: usize) {
    let sep = if indent.is_some() { "," } else { ", " };
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&float_repr(n.as_f64().unwrap_or(f64::NAN)));
            }
        }
        Value::String(s) => string(out, s),
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(sep);
                }
                newline(out, indent, level + 1);
                write(out, x, indent, level + 1);
            }
            newline(out, indent, level);
            out.push(']');
        }
        Value::Object(m) => {
            if m.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(sep);
                }
                newline(out, indent, level + 1);
                string(out, k);
                out.push_str(": ");
                write(out, x, indent, level + 1);
            }
            newline(out, indent, level);
            out.push('}');
        }
    }
}

/// A JSON string as ensure_ascii writes it.
pub fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
        }
    }
    out.push('"');
}

/// repr() of a float (as json.dumps writes it): the shortest digits that read back the same, positional from 1e-4 to below 1e16
/// (with ".0" when whole), else d.ddde+XX.
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    // Rust's {:e} gives the shortest round-trip digits: "-1.2345e-7"
    let e = format!("{:e}", x);
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let (neg, mant) = match mant.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mant),
    };
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if !(-4..16).contains(&exp) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&format!("e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs()));
    } else if exp < 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat((-exp - 1) as usize));
        out.push_str(&digits);
    } else {
        let point = exp as usize + 1;
        if digits.len() <= point {
            out.push_str(&digits);
            out.push_str(&"0".repeat(point - digits.len()));
            out.push_str(".0");
        } else {
            out.push_str(&digits[..point]);
            out.push('.');
            out.push_str(&digits[point..]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn as_python_writes_it() {
        let v = json!({"b": 1, "a": [true, null, "x\u{e9}\u{1f600}\"\n"], "e": {}, "l": []});
        assert_eq!(dumps(&v), r#"{"b": 1, "a": [true, null, "x\u00e9\ud83d\ude00\"\n"], "e": {}, "l": []}"#);
        assert_eq!(dumps_indent(&json!([{"a": 1, "b": [1, 2]}]), 1), "[\n {\n  \"a\": 1,\n  \"b\": [\n   1,\n   2\n  ]\n }\n]");
        assert_eq!(dumps(&json!("\u{7f}\u{1f}")), r#""\u007f\u001f""#);
    }

    #[test]
    fn keys_keep_the_file_order() {
        let v = loads(r#"{"z": 1, "a": 2, "z": 3}"#).unwrap();
        assert_eq!(dumps(&v), r#"{"z": 3, "a": 2}"#);
    }

    #[test]
    fn floats_as_repr() {
        for (x, want) in [
            (1.0, "1.0"),
            (0.1, "0.1"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1e16, "1e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (-2.5, "-2.5"),
            (1.5e300, "1.5e+300"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (123.456, "123.456"),
        ] {
            assert_eq!(float_repr(x), want, "{x}");
        }
    }
}
