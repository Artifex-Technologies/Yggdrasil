//! JSON as Python's json module reads and writes it: keys keep their order, the text written is byte for byte what
//! json.dumps() (", " and ": ", non-ASCII as \uXXXX) and json.dump(indent=1) write, and the reader takes what json.load()
//! takes (NaN and Infinity too, a repeated key keeps its first place and its last value). No crate: the three helpers only
//! read and write small settings and state files.

use crate::py;

#[derive(Clone, Debug)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    /// A whole number beyond i64, as written (Python's ints have no limit); it fails every range check.
    Big(String),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Obj),
}

/// An object with its keys in order.
#[derive(Clone, Debug, Default)]
pub struct Obj(Vec<(String, Json)>);

static NULL: Json = Json::Null;

impl Obj {
    pub fn new() -> Obj {
        Obj(Vec::new())
    }

    pub fn get(&self, k: &str) -> Option<&Json> {
        self.0.iter().find(|(x, _)| x == k).map(|(_, v)| v)
    }

    /// d.get(k): None (null) when it is not there.
    pub fn at(&self, k: &str) -> &Json {
        self.get(k).unwrap_or(&NULL)
    }

    pub fn contains(&self, k: &str) -> bool {
        self.get(k).is_some()
    }

    /// d[k] = v: in its place when the key is there, at the end when it is new.
    pub fn set(&mut self, k: &str, v: Json) {
        match self.0.iter_mut().find(|(x, _)| x == k) {
            Some(e) => e.1 = v,
            None => self.0.push((k.to_string(), v)),
        }
    }

    pub fn with(mut self, k: &str, v: Json) -> Obj {
        self.set(k, v);
        self
    }

    /// d.pop(k, None)
    pub fn remove(&mut self, k: &str) -> Option<Json> {
        let i = self.0.iter().position(|(x, _)| x == k)?;
        Some(self.0.remove(i).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Json)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Obj> for Json {
    fn from(o: Obj) -> Json {
        Json::Obj(o)
    }
}

impl From<i64> for Json {
    fn from(v: i64) -> Json {
        Json::Int(v)
    }
}

impl From<bool> for Json {
    fn from(v: bool) -> Json {
        Json::Bool(v)
    }
}

impl From<&str> for Json {
    fn from(v: &str) -> Json {
        Json::Str(v.to_string())
    }
}

impl From<String> for Json {
    fn from(v: String) -> Json {
        Json::Str(v)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Json {
        v.map(Into::into).unwrap_or(Json::Null)
    }
}

impl Json {
    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// Python's truth: None, False, 0, 0.0, "", [] and {} are false.
    pub fn truthy(&self) -> bool {
        match self {
            Json::Null => false,
            Json::Bool(b) => *b,
            Json::Int(i) => *i != 0,
            Json::Big(_) => true,
            Json::Float(f) => *f != 0.0,
            Json::Str(s) => !s.is_empty(),
            Json::Arr(a) => !a.is_empty(),
            Json::Obj(o) => !o.is_empty(),
        }
    }

    pub fn obj(&self) -> Option<&Obj> {
        match self {
            Json::Obj(o) => Some(o),
            _ => None,
        }
    }

    pub fn arr(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// isinstance(v, int) and not isinstance(v, bool): the number (clamped when beyond i64).
    pub fn int(&self) -> Option<i64> {
        match self {
            Json::Int(i) => Some(*i),
            Json::Big(s) => Some(if s.starts_with('-') { i64::MIN } else { i64::MAX }),
            _ => None,
        }
    }

    /// isinstance(v, int), where True and False count as 1 and 0.
    pub fn int_or_bool(&self) -> Option<i64> {
        match self {
            Json::Bool(b) => Some(*b as i64),
            v => v.int(),
        }
    }

    /// Python's type name, for the messages Python's own errors carry.
    pub fn type_name(&self) -> &'static str {
        match self {
            Json::Null => "NoneType",
            Json::Bool(_) => "bool",
            Json::Int(_) | Json::Big(_) => "int",
            Json::Float(_) => "float",
            Json::Str(_) => "str",
            Json::Arr(_) => "list",
            Json::Obj(_) => "dict",
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Json::Bool(b) => Some(*b as i64 as f64),
            Json::Int(i) => Some(*i as f64),
            Json::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Python's ==: 1 == 1.0 == True; dicts equal whatever their order.
    pub fn py_eq(&self, other: &Json) -> bool {
        match (self, other) {
            (Json::Null, Json::Null) => true,
            (Json::Big(a), Json::Big(b)) => a == b,
            (Json::Big(_), _) | (_, Json::Big(_)) => false,
            (Json::Int(a), Json::Int(b)) => a == b,
            (Json::Str(a), Json::Str(b)) => a == b,
            (Json::Arr(a), Json::Arr(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.py_eq(y)),
            (Json::Obj(a), Json::Obj(b)) => a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| v.py_eq(w))),
            (a, b) => match (a.number(), b.number()) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            },
        }
    }

    /// repr() of the value as Python holds it ({'ratios': [51, 51]}, None, True).
    pub fn repr(&self) -> String {
        match self {
            Json::Null => "None".into(),
            Json::Bool(b) => (if *b { "True" } else { "False" }).into(),
            Json::Int(i) => i.to_string(),
            Json::Big(s) => s.clone(),
            Json::Float(f) => py::float_repr(*f),
            Json::Str(s) => py::repr(s),
            Json::Arr(a) => format!("[{}]", a.iter().map(Json::repr).collect::<Vec<_>>().join(", ")),
            Json::Obj(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", py::repr(k), v.repr())).collect::<Vec<_>>().join(", ")),
        }
    }

    /// str() of the value: a text as it is, anything else as repr().
    pub fn py_str(&self) -> String {
        match self {
            Json::Str(s) => s.clone(),
            v => v.repr(),
        }
    }
}

// ---------------------------------------------------------------- reading

/// json.loads(text); Err is the ValueError's text.
pub fn parse(text: &str) -> Result<Json, String> {
    if text.starts_with('\u{feff}') {
        return Err("Unexpected UTF-8 BOM (decode using utf-8-sig)".into());
    }
    let mut p = Parser { s: text.as_bytes(), i: 0, depth: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return p.err("Extra data");
    }
    Ok(v)
}

/// json.load() of a file's bytes (they must be UTF-8, as open(encoding="utf-8") wants them).
pub fn parse_bytes(b: &[u8]) -> Result<Json, String> {
    parse(std::str::from_utf8(b).map_err(|e| e.to_string())?)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, word: &str) -> bool {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            true
        } else {
            false
        }
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        self.err_at(what, self.i)
    }

    /// The JSONDecodeError's text: "Expecting value: line 1 column 1 (char 0)" (positions in characters, as Python counts).
    fn err_at<T>(&self, what: &str, at: usize) -> Result<T, String> {
        let before = String::from_utf8_lossy(&self.s[..at.min(self.s.len())]);
        let pos = before.chars().count();
        let line = before.matches('\n').count() + 1;
        let col = match before.rfind('\n') {
            Some(nl) => before[nl + 1..].chars().count() + 1,
            None => pos + 1,
        };
        Err(format!("{what}: line {line} column {col} (char {pos})"))
    }

    fn value(&mut self) -> Result<Json, String> {
        match self.peek() {
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'n') if self.eat("null") => Ok(Json::Null),
            Some(b't') if self.eat("true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Json::Bool(false)),
            Some(b'-' | b'0'..=b'9') if self.number_starts() => self.number(),
            Some(b'N') if self.eat("NaN") => Ok(Json::Float(f64::NAN)),
            Some(b'I') if self.eat("Infinity") => Ok(Json::Float(f64::INFINITY)),
            Some(b'-') if self.eat("-Infinity") => Ok(Json::Float(f64::NEG_INFINITY)),
            _ => self.err("Expecting value"),
        }
    }

    fn number_starts(&self) -> bool {
        let j = if self.s[self.i] == b'-' { self.i + 1 } else { self.i };
        self.s.get(j).is_some_and(|c| c.is_ascii_digit())
    }

    /// -?(0|[1-9]\d*)(\.\d+)?([eE][-+]?\d+)?: a float with a fraction or an exponent, a whole number without.
    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        if self.s[self.i] == b'0' {
            self.i += 1;
        } else {
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        let mut float = false;
        if self.peek() == Some(b'.') && self.s.get(self.i + 1).is_some_and(|c| c.is_ascii_digit()) {
            float = true;
            self.i += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let mut j = self.i + 1;
            if matches!(self.s.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if self.s.get(j).is_some_and(|c| c.is_ascii_digit()) {
                float = true;
                self.i = j;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.i += 1;
                }
            }
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("0");
        if float {
            return Ok(Json::Float(text.parse().unwrap_or(f64::NAN)));
        }
        Ok(match text.parse::<i64>() {
            Ok(v) => Json::Int(v),
            Err(_) => Json::Big(text.to_string()),
        })
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        let h = self.s.get(at..at + 4)?;
        if !h.iter().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()
    }

    fn string(&mut self) -> Result<String, String> {
        let start = self.i;
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let Some(c) = self.peek() else { return self.err_at("Unterminated string starting at", start) };
            match c {
                b'"' => {
                    self.i += 1;
                    return String::from_utf8(out).map_err(|e| e.to_string());
                }
                0..=0x1f => return self.err("Invalid control character at"),
                b'\\' => {
                    let Some(e) = self.s.get(self.i + 1).copied() else { return self.err_at("Unterminated string starting at", start) };
                    let simple = match e {
                        b'"' => Some(b'"'),
                        b'\\' => Some(b'\\'),
                        b'/' => Some(b'/'),
                        b'b' => Some(8),
                        b'f' => Some(12),
                        b'n' => Some(b'\n'),
                        b'r' => Some(b'\r'),
                        b't' => Some(b'\t'),
                        _ => None,
                    };
                    if let Some(b) = simple {
                        out.push(b);
                        self.i += 2;
                        continue;
                    }
                    if e != b'u' {
                        return self.err("Invalid \\escape");
                    }
                    let Some(mut u) = self.hex4(self.i + 2) else { return self.err_at("Invalid \\uXXXX escape", self.i + 1) };
                    self.i += 6;
                    if (0xd800..0xdc00).contains(&u) && self.s.get(self.i) == Some(&b'\\') && self.s.get(self.i + 1) == Some(&b'u') {
                        if let Some(lo) = self.hex4(self.i + 2).filter(|lo| (0xdc00..0xe000).contains(lo)) {
                            u = 0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00);
                            self.i += 6;
                        }
                    }
                    let ch = char::from_u32(u).unwrap_or('\u{fffd}');
                    let mut b = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                }
                _ => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn deeper(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > 900 {
            return self.err("Too deeply nested");
        }
        Ok(())
    }

    fn object(&mut self) -> Result<Json, String> {
        self.deeper()?;
        self.i += 1;
        let mut o = Obj::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Json::Obj(o));
        }
        loop {
            if self.peek() != Some(b'"') {
                return self.err("Expecting property name enclosed in double quotes");
            }
            let k = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return self.err("Expecting ':' delimiter");
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            o.set(&k, v);
            self.ws();
            match self.peek() {
                Some(b'}') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Json::Obj(o));
                }
                Some(b',') => {
                    let comma = self.i;
                    self.i += 1;
                    self.ws();
                    if self.peek() == Some(b'}') {
                        return self.err_at("Illegal trailing comma before end of object", comma);
                    }
                }
                _ => return self.err("Expecting ',' delimiter"),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.deeper()?;
        self.i += 1;
        let mut a = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Json::Arr(a));
        }
        loop {
            a.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b']') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Json::Arr(a));
                }
                Some(b',') => {
                    let comma = self.i;
                    self.i += 1;
                    self.ws();
                    if self.peek() == Some(b']') {
                        return self.err_at("Illegal trailing comma before end of array", comma);
                    }
                }
                _ => return self.err("Expecting ',' delimiter"),
            }
        }
    }
}

// ---------------------------------------------------------------- writing

fn put_str(out: &mut String, s: &str) {
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
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
        }
    }
    out.push('"');
}

fn put_float(out: &mut String, f: f64) {
    if f.is_nan() {
        out.push_str("NaN");
    } else if f.is_infinite() {
        out.push_str(if f > 0.0 { "Infinity" } else { "-Infinity" });
    } else {
        out.push_str(&py::float_repr(f));
    }
}

fn put(out: &mut String, v: &Json, indent: Option<usize>, level: usize) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Int(i) => out.push_str(&i.to_string()),
        Json::Big(s) => out.push_str(s),
        Json::Float(f) => put_float(out, *f),
        Json::Str(s) => put_str(out, s),
        Json::Arr(a) if a.is_empty() => out.push_str("[]"),
        Json::Obj(o) if o.is_empty() => out.push_str("{}"),
        Json::Arr(a) => {
            out.push('[');
            for (n, x) in a.iter().enumerate() {
                sep(out, n, indent, level + 1);
                put(out, x, indent, level + 1);
            }
            close(out, indent, level);
            out.push(']');
        }
        Json::Obj(o) => {
            out.push('{');
            for (n, (k, x)) in o.iter().enumerate() {
                sep(out, n, indent, level + 1);
                put_str(out, k);
                out.push_str(": ");
                put(out, x, indent, level + 1);
            }
            close(out, indent, level);
            out.push('}');
        }
    }
}

fn sep(out: &mut String, n: usize, indent: Option<usize>, level: usize) {
    match indent {
        None if n > 0 => out.push_str(", "),
        None => {}
        Some(w) => {
            if n > 0 {
                out.push(',');
            }
            out.push('\n');
            out.push_str(&" ".repeat(w * level));
        }
    }
}

fn close(out: &mut String, indent: Option<usize>, level: usize) {
    if let Some(w) = indent {
        out.push('\n');
        out.push_str(&" ".repeat(w * level));
    }
}

/// json.dumps(v)
pub fn dumps(v: &Json) -> String {
    let mut s = String::new();
    put(&mut s, v, None, 0);
    s
}

/// json.dump(v, f, indent=1): what the helpers write into their files.
pub fn dump_indent1(v: &Json) -> String {
    let mut s = String::new();
    put(&mut s, v, Some(1), 0);
    s
}

/// The value with every object's keys in order (what json.dump(sort_keys=True) writes).
pub fn sorted(v: &Json) -> Json {
    match v {
        Json::Obj(o) => {
            let mut items: Vec<(&str, &Json)> = o.iter().collect();
            items.sort_by(|a, b| a.0.cmp(b.0));
            Json::Obj(items.into_iter().fold(Obj::new(), |acc, (k, v)| acc.with(k, sorted(v))))
        }
        Json::Arr(a) => Json::Arr(a.iter().map(sorted).collect()),
        v => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_python_reads() {
        let v = parse(" {\"b\": 1, \"a\": [1.5, -0, 1e2, true, null, \"\\u00e9\\ud83d\\ude00\"], \"b\": 2}\n").unwrap();
        let o = v.obj().unwrap();
        assert_eq!(o.keys().collect::<Vec<_>>(), ["b", "a"], "a repeated key keeps its first place");
        assert_eq!(o.at("b").int(), Some(2), "and its last value");
        assert_eq!(dumps(&v), "{\"b\": 2, \"a\": [1.5, 0, 100.0, true, null, \"\\u00e9\\ud83d\\ude00\"]}");
        assert!(matches!(parse("NaN").unwrap(), Json::Float(f) if f.is_nan()));
        assert!(matches!(parse("123456789012345678901234567890").unwrap(), Json::Big(_)));
        for bad in ["", "[1,]", "{\"a\":1,}", "01", "1.", "[1] 2", "\"a\nb\"", "{'a': 1}", "\u{feff}{}", "tru"] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(parse(&"[".repeat(5000)).is_err(), "too deep is an error, not a crash");
        // Python 3.14's own texts
        let cases = [
            ("", "Expecting value: line 1 column 1 (char 0)"),
            ("[1,]", "Illegal trailing comma before end of array: line 1 column 3 (char 2)"),
            ("{\"a\":1,}", "Illegal trailing comma before end of object: line 1 column 7 (char 6)"),
            ("01", "Extra data: line 1 column 2 (char 1)"),
            ("{\"a\" 1}", "Expecting ':' delimiter: line 1 column 6 (char 5)"),
            ("[1 2]", "Expecting ',' delimiter: line 1 column 4 (char 3)"),
            ("\"abc", "Unterminated string starting at: line 1 column 1 (char 0)"),
            ("\"a\u{1}\"", "Invalid control character at: line 1 column 3 (char 2)"),
            ("\"\\q\"", "Invalid \\escape: line 1 column 2 (char 1)"),
            ("{1:2}", "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"),
            ("tru", "Expecting value: line 1 column 1 (char 0)"),
            ("[1]\n 2", "Extra data: line 2 column 2 (char 5)"),
            ("\"\\u12\"", "Invalid \\uXXXX escape: line 1 column 3 (char 2)"),
        ];
        for (text, msg) in cases {
            assert_eq!(parse(text).unwrap_err(), msg, "{text:?}");
        }
    }

    #[test]
    fn writes_what_python_writes() {
        let v = parse("{\"ports\": {\"1\": {\"name\": \"Pump \u{2013} GPU\", \"cpu\": [[50, 25]], \"e\": [], \"o\": {}}}}").unwrap();
        assert_eq!(dump_indent1(&v), "{\n \"ports\": {\n  \"1\": {\n   \"name\": \"Pump \\u2013 GPU\",\n   \"cpu\": [\n    [\n     50,\n     25\n    ]\n   ],\n   \"e\": [],\n   \"o\": {}\n  }\n }\n}");
        assert_eq!(dumps(&Json::Float(1e-5)), "1e-05");
        assert_eq!(dumps(&Json::Str("\u{7f}\"\\/".into())), "\"\\u007f\\\"\\\\/\"");
    }

    #[test]
    fn python_equality_and_repr() {
        assert!(parse("[52, 1.0, true]").unwrap().py_eq(&parse("[52.0, 1, 1]").unwrap()));
        assert!(parse("{\"a\": 1, \"b\": 2}").unwrap().py_eq(&parse("{\"b\": 2, \"a\": 1}").unwrap()));
        assert!(!parse("[1]").unwrap().py_eq(&parse("[\"1\"]").unwrap()));
        assert_eq!(parse("{\"ratios\": [51, 50], \"x\": null, \"y\": false, \"z\": \"it's\"}").unwrap().repr(),
                   "{'ratios': [51, 50], 'x': None, 'y': False, 'z': \"it's\"}");
    }
}
