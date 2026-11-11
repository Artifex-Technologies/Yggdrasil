//! JSON as the Python script read it: values with Python's kinds (whole numbers of any size, floats with NaN and the
//! infinities, text, lists, and objects that keep the first place of a key given twice and its last value), from a reader that
//! accepts exactly what `json.loads` accepted, and written back as `json.dumps`, `str()` and `repr()` wrote them.
//!
//! The plan is read with a limit on nesting (`PLAN_NESTING`) and refused when it holds a lone surrogate (a `\ud800` escape):
//! the script refuses both since its port (they got as far as a command line before). The script's other JSON (lsblk's and
//! sfdisk's output, the OS's lists) is read with a far deeper limit.

use std::collections::HashMap;

/// How many levels of lists and objects a plan may have inside it (it needs three).
pub const PLAN_NESTING: usize = 20;
/// The same for everything else read as JSON (Python had no limit short of its stack).
pub const OTHER_NESTING: usize = 512;
/// int() refuses text with more digits than this (sys.int_info.default_max_str_digits); json.loads does too.
const MAX_DIGITS: usize = 4300;
/// Whole numbers are worked with clamped to this (far beyond any disk; the sum of two still fits an i128). Their text is kept.
pub const CLAMP: i128 = 1_000_000_000_000_000_000_000_000_000_000;

/// A whole number: its text as str() writes it, and its value (clamped to ±CLAMP).
#[derive(Clone, Debug, PartialEq)]
pub struct Int {
    pub text: String,
    pub value: i128,
}

impl Int {
    pub fn new(v: i128) -> Int {
        Int {
            text: v.to_string(),
            value: v.clamp(-CLAMP, CLAMP),
        }
    }
}

/// A JSON object as a Python dict: keys in the order they first came, each with the last value given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dict {
    keys: Vec<String>,
    values: Vec<Py>,
    index: HashMap<String, usize>,
}

impl Dict {
    pub fn insert(&mut self, k: String, v: Py) {
        match self.index.get(&k) {
            Some(&i) => self.values[i] = v,
            None => {
                self.index.insert(k.clone(), self.keys.len());
                self.keys.push(k);
                self.values.push(v);
            }
        }
    }

    pub fn get(&self, k: &str) -> Option<&Py> {
        self.index.get(k).map(|&i| &self.values[i])
    }

    pub fn contains(&self, k: &str) -> bool {
        self.index.contains_key(k)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Py)> {
        self.keys.iter().zip(self.values.iter())
    }

    pub fn values(&self) -> impl Iterator<Item = &Py> {
        self.values.iter()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// A value json.loads made.
#[derive(Clone, Debug, PartialEq)]
pub enum Py {
    None,
    Bool(bool),
    Int(Int),
    Float(f64),
    Str(String),
    List(Vec<Py>),
    Dict(Dict),
}

pub static NONE: Py = Py::None;

impl Py {
    /// bool(v)
    pub fn truthy(&self) -> bool {
        match self {
            Py::None => false,
            Py::Bool(b) => *b,
            Py::Int(i) => i.value != 0,
            Py::Float(f) => *f != 0.0,
            Py::Str(s) => !s.is_empty(),
            Py::List(l) => !l.is_empty(),
            Py::Dict(d) => !d.is_empty(),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Py::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Py::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Py]> {
        match self {
            Py::List(l) => Some(l),
            _ => None,
        }
    }

    /// A whole number (isinstance(v, int) and not isinstance(v, bool)): its clamped value.
    pub fn whole(&self) -> Option<i128> {
        match self {
            Py::Int(i) => Some(i.value),
            _ => None,
        }
    }

    /// v.get(key) of a dict (None for anything else, where the script made sure it had a dict first).
    pub fn get(&self, key: &str) -> Option<&Py> {
        self.as_dict().and_then(|d| d.get(key))
    }

    /// v.get(key) or None, as a value.
    pub fn at(&self, key: &str) -> &Py {
        self.get(key).unwrap_or(&NONE)
    }

    /// The name type(v).__name__ gives.
    pub fn type_name(&self) -> &'static str {
        match self {
            Py::None => "NoneType",
            Py::Bool(_) => "bool",
            Py::Int(_) => "int",
            Py::Float(_) => "float",
            Py::Str(_) => "str",
            Py::List(_) => "list",
            Py::Dict(_) => "dict",
        }
    }

    /// str(v)
    pub fn py_str(&self) -> String {
        match self {
            Py::Str(s) => s.clone(),
            _ => self.py_repr(),
        }
    }

    /// repr(v)
    pub fn py_repr(&self) -> String {
        match self {
            Py::None => "None".into(),
            Py::Bool(b) => (if *b { "True" } else { "False" }).into(),
            Py::Int(i) => i.text.clone(),
            Py::Float(f) => float_repr(*f),
            Py::Str(s) => pycompat::repr(s),
            Py::List(l) => format!(
                "[{}]",
                l.iter().map(Py::py_repr).collect::<Vec<_>>().join(", ")
            ),
            Py::Dict(d) => format!(
                "{{{}}}",
                d.iter()
                    .map(|(k, v)| format!("{}: {}", pycompat::repr(k), v.py_repr()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// json.dumps(v)
    pub fn dumps(&self) -> String {
        let mut out = String::new();
        self.dump(&mut out);
        out
    }

    fn dump(&self, out: &mut String) {
        match self {
            Py::None => out.push_str("null"),
            Py::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Py::Int(i) => out.push_str(&i.text),
            Py::Float(f) => out.push_str(&pycompat::json::float_repr(*f)),
            Py::Str(s) => pycompat::json::string(out, s),
            Py::List(l) => {
                out.push('[');
                for (i, x) in l.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    x.dump(out);
                }
                out.push(']');
            }
            Py::Dict(d) => {
                out.push('{');
                for (i, (k, x)) in d.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    pycompat::json::string(out, k);
                    out.push_str(": ");
                    x.dump(out);
                }
                out.push('}');
            }
        }
    }
}

/// repr(float): inf, -inf and nan by name, the rest as Python writes them.
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        (if f > 0.0 { "inf" } else { "-inf" }).into()
    } else {
        pycompat::json::float_repr(f)
    }
}

pub fn s(text: &str) -> Py {
    Py::Str(text.to_string())
}

pub fn int(v: i128) -> Py {
    Py::Int(Int::new(v))
}

/// An object from (key, value) pairs.
pub fn dict(pairs: Vec<(&str, Py)>) -> Py {
    let mut d = Dict::default();
    for (k, v) in pairs {
        d.insert(k.to_string(), v);
    }
    Py::Dict(d)
}

/// Text that json.loads refused (or that holds a lone surrogate, or nests deeper than allowed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NotJson;

/// json.loads(text), with at most `nesting` levels inside the top value.
pub fn loads(text: &str, nesting: usize) -> Result<Py, NotJson> {
    let mut p = Reader {
        s: text.as_bytes(),
        src: text,
        i: 0,
        max: nesting,
    };
    p.ws();
    let v = p.value(0).map_err(|()| NotJson)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(NotJson);
    }
    Ok(v)
}

struct Reader<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
    max: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn word(&mut self, w: &str) -> bool {
        if self.s[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Py, ()> {
        if depth > self.max {
            return Err(());
        }
        match self.peek() {
            Some(b'"') => {
                self.i += 1;
                Ok(Py::Str(self.string()?))
            }
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'n') if self.word("null") => Ok(Py::None),
            Some(b't') if self.word("true") => Ok(Py::Bool(true)),
            Some(b'f') if self.word("false") => Ok(Py::Bool(false)),
            Some(b'N') if self.word("NaN") => Ok(Py::Float(f64::NAN)),
            Some(b'I') if self.word("Infinity") => Ok(Py::Float(f64::INFINITY)),
            Some(b'-') if self.word("-Infinity") => Ok(Py::Float(f64::NEG_INFINITY)),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(()),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Py, ()> {
        self.i += 1;
        self.ws();
        let mut d = Dict::default();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Py::Dict(d));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(());
            }
            self.i += 1;
            let k = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(());
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            d.insert(k, v);
            self.ws();
            match self.peek() {
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Py::Dict(d));
                }
                Some(b',') => {
                    self.i += 1;
                    self.ws();
                }
                _ => return Err(()),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Py, ()> {
        self.i += 1;
        self.ws();
        let mut l = vec![];
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Py::List(l));
        }
        loop {
            l.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b']') => {
                    self.i += 1;
                    return Ok(Py::List(l));
                }
                Some(b',') => {
                    self.i += 1;
                    self.ws();
                }
                _ => return Err(()),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, ()> {
        let h = self.s.get(self.i..self.i + 4).ok_or(())?;
        let mut v = 0;
        for &c in h {
            v = v * 16 + (c as char).to_digit(16).ok_or(())?;
        }
        self.i += 4;
        Ok(v)
    }

    /// The rest of a string after its opening quote. Control characters are refused (json's strict mode), and so is a
    /// surrogate escape that is not half of a pair.
    fn string(&mut self) -> Result<String, ()> {
        let mut out = String::new();
        loop {
            let start = self.i;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.i += 1;
            }
            out.push_str(&self.src[start..self.i]);
            match self.peek() {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let e = self.peek().ok_or(())?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let c = self.hex4()?;
                            let c = if (0xd800..0xdc00).contains(&c) {
                                if !self.word("\\u") {
                                    return Err(());
                                }
                                let low = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&low) {
                                    return Err(());
                                }
                                0x10000 + ((c - 0xd800) << 10) + (low - 0xdc00)
                            } else {
                                c
                            };
                            out.push(char::from_u32(c).ok_or(())?);
                        }
                        _ => return Err(()),
                    }
                }
                _ => return Err(()),
            }
        }
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
    }

    /// A number as json's scanner matches it: -?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?, a float when it has a fraction
    /// or an exponent.
    fn number(&mut self) -> Result<Py, ()> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek() {
            Some(b'1'..=b'9') => {
                self.i += 1;
                self.digits();
            }
            Some(b'0') => self.i += 1,
            _ => return Err(()),
        }
        let mut float = false;
        if self.peek() == Some(b'.') && matches!(self.s.get(self.i + 1), Some(b'0'..=b'9')) {
            float = true;
            self.i += 2;
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let e = self.i;
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            let d = self.i;
            self.digits();
            if self.i > d {
                float = true;
            } else {
                self.i = e;
            }
        }
        let text = &self.src[start..self.i];
        if float {
            return text.parse::<f64>().map(Py::Float).map_err(|_| ());
        }
        let digits = text.trim_start_matches('-');
        if digits.len() > MAX_DIGITS {
            return Err(());
        }
        let mut v: i128 = 0;
        for c in digits.bytes() {
            v = v.saturating_mul(10).saturating_add((c - b'0') as i128);
        }
        let neg = text.starts_with('-');
        let v = if neg { -v } else { v }.clamp(-CLAMP, CLAMP);
        let text = if v == 0 {
            "0".to_string()
        } else {
            text.to_string()
        };
        Ok(Py::Int(Int { text, value: v }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(t: &str) -> Py {
        loads(t, PLAN_NESTING).unwrap_or_else(|_| panic!("{t:?} should read"))
    }

    #[test]
    fn what_json_loads_accepts() {
        assert_eq!(
            ok(" [1e400, -0, NaN, -Infinity, 1E2, -0.0] ").py_repr(),
            "[inf, 0, nan, -inf, 100.0, -0.0]"
        );
        assert_eq!(ok(r#"{"a":1,"b":2,"a":3}"#).py_repr(), "{'a': 3, 'b': 2}");
        assert_eq!(ok(r#""\ud83d\ude00\u00e9\/""#), s("\u{1f600}é/"));
        assert_eq!(ok(&"1".repeat(4300)).py_str().len(), 4300);
        assert_eq!(
            ok("123456789012345678901234567890123456789").py_str(),
            "123456789012345678901234567890123456789"
        );
        assert_eq!(
            ok("123456789012345678901234567890123456789").whole(),
            Some(CLAMP)
        );
        assert_eq!(ok("\"\u{7f}\u{2028}\""), s("\u{7f}\u{2028}"));
    }

    #[test]
    fn what_it_refuses() {
        for t in [
            "",
            " ",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1.",
            ".5",
            "-",
            "1e",
            "+1",
            "nan",
            "\"\\ud800\"",
            "\"\\udc00\"",
            "\"\\ud800\\u0041\"",
            "\"a\nb\"",
            "\"\\x\"",
            "\u{feff}{}",
            "{} x",
            "'a'",
            "[1 2]",
            "{\"a\" 1}",
            "{1: 2}",
            "tru",
            "\"\\u12\"",
        ] {
            assert!(loads(t, PLAN_NESTING).is_err(), "{t:?}");
        }
        assert!(loads(&"1".repeat(4301), PLAN_NESTING).is_err());
        assert!(loads(
            &format!("{}{}", "[".repeat(21), "]".repeat(21)),
            PLAN_NESTING
        )
        .is_ok());
        assert!(loads(
            &format!("{}{}", "[".repeat(22), "]".repeat(22)),
            PLAN_NESTING
        )
        .is_err());
        assert!(
            loads(
                &format!("{}1{}", "[".repeat(21), "]".repeat(21)),
                PLAN_NESTING
            )
            .is_err(),
            "a value 21 levels in"
        );
        assert!(loads(&"[".repeat(1_000_000), PLAN_NESTING).is_err());
    }

    #[test]
    fn written_as_python_writes_it() {
        let v = ok(r#"{"a": [1, "x\u00e9", null, true, 1.5, Infinity, NaN, {}], "b": -0}"#);
        assert_eq!(
            v.py_str(),
            "{'a': [1, 'xé', None, True, 1.5, inf, nan, {}], 'b': 0}"
        );
        assert_eq!(
            v.dumps(),
            r#"{"a": [1, "x\u00e9", null, true, 1.5, Infinity, NaN, {}], "b": 0}"#
        );
        assert_eq!(ok("1e16").py_str(), "1e+16");
        assert_eq!(ok("[\"it's\"]").py_str(), "[\"it's\"]");
        assert!(
            !ok("0.0").truthy()
                && ok("NaN").truthy()
                && !ok("[]").truthy()
                && ok("\"0\"").truthy()
                && !ok("-0").truthy()
        );
    }
}
