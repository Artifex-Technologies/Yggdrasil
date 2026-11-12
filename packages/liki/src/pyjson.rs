//! json.loads as Python accepts it (NaN, Infinity, -Infinity, any size of number, lone surrogates in \u escapes), for the
//! file liki-build remembers its last build in. Only what the tool looks at is kept: objects, and text that is valid Unicode.

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Object(Vec<(String, Json)>),
    Str(String),
    /// any other value, or text with a lone surrogate (which no choice and no pack name can be)
    Other,
}

impl Json {
    /// dict.get(key) of an object (the last of repeated keys, as a dict keeps it)
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(items) => items.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Deeper than this is refused (Python's own recursion limit stops near a thousand).
const MAX_DEPTH: usize = 500;

/// json.loads(text); None where Python raises ValueError (JSONDecodeError).
pub fn loads(text: &str) -> Option<Json> {
    let mut p = P { s: text.as_bytes(), i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    (p.i == p.s.len()).then_some(v)
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        match *self.s.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut items = vec![];
                self.ws();
                if self.eat("}") {
                    return Some(Json::Object(items));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return None;
                    }
                    let key = self.string()?;
                    self.ws();
                    if !self.eat(":") {
                        return None;
                    }
                    self.ws();
                    let v = self.value(depth + 1)?;
                    // a key that is not valid Unicode can never be one the tool asks for
                    items.push((key.unwrap_or_else(|| "\u{fffd}\u{fffd}".into()), v));
                    self.ws();
                    if self.eat("}") {
                        return Some(Json::Object(items));
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
            }
            b'[' => {
                self.i += 1;
                self.ws();
                if self.eat("]") {
                    return Some(Json::Other);
                }
                loop {
                    self.ws();
                    self.value(depth + 1)?;
                    self.ws();
                    if self.eat("]") {
                        return Some(Json::Other);
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
            }
            b'"' => Some(match self.string()? {
                Some(s) => Json::Str(s),
                None => Json::Other,
            }),
            b'n' if self.eat("null") => Some(Json::Other),
            b't' if self.eat("true") => Some(Json::Other),
            b'f' if self.eat("false") => Some(Json::Other),
            b'N' if self.eat("NaN") => Some(Json::Other),
            b'I' if self.eat("Infinity") => Some(Json::Other),
            b'-' if self.s[self.i..].starts_with(b"-Infinity") => {
                self.i += 9;
                Some(Json::Other)
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn number(&mut self) -> Option<Json> {
        let digit = |p: &P, i: usize| p.s.get(i).is_some_and(|c| c.is_ascii_digit());
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        match self.s.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                while digit(self, self.i) {
                    self.i += 1;
                }
            }
            _ => return None,
        }
        if self.s.get(self.i) == Some(&b'.') && digit(self, self.i + 1) {
            self.i += 1;
            while digit(self, self.i) {
                self.i += 1;
            }
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            let mut j = self.i + 1;
            if matches!(self.s.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if digit(self, j) {
                self.i = j;
                while digit(self, self.i) {
                    self.i += 1;
                }
            }
        }
        Some(Json::Other)
    }

    /// A string at the opening quote; Some(None) when it is valid JSON but holds a lone surrogate.
    fn string(&mut self) -> Option<Option<String>> {
        self.i += 1;
        let mut units: Vec<u16> = vec![];
        loop {
            let c = *self.s.get(self.i)?;
            match c {
                b'"' => {
                    self.i += 1;
                    break;
                }
                0..=0x1f => return None,
                b'\\' => {
                    let e = *self.s.get(self.i + 1)?;
                    self.i += 2;
                    let u = match e {
                        b'"' => b'"' as u16,
                        b'\\' => b'\\' as u16,
                        b'/' => b'/' as u16,
                        b'b' => 8,
                        b'f' => 12,
                        b'n' => 10,
                        b'r' => 13,
                        b't' => 9,
                        b'u' => {
                            let hex = self.s.get(self.i..self.i + 4)?;
                            if !hex.iter().all(|c| c.is_ascii_hexdigit()) {
                                return None;
                            }
                            self.i += 4;
                            u16::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?
                        }
                        _ => return None,
                    };
                    units.push(u);
                }
                _ => {
                    // one UTF-8 character as it is (the text was valid UTF-8)
                    let len = match c {
                        0..=0x7f => 1,
                        0xc0..=0xdf => 2,
                        0xe0..=0xef => 3,
                        _ => 4,
                    };
                    let ch = std::str::from_utf8(&self.s[self.i..self.i + len]).ok()?;
                    units.extend(ch.encode_utf16());
                    self.i += len;
                }
            }
        }
        Some(String::from_utf16(&units).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_json_is_read() {
        let v = loads(r#" {"shape": "circle", "style": "dark", "name": "Liki", "x": NaN, "y": [1, -Infinity, 1e999]} "#).unwrap();
        assert_eq!(v.get("shape"), Some(&Json::Str("circle".into())));
        assert_eq!(v.get("x"), Some(&Json::Other));
        assert_eq!(loads(r#"{"name": "a", "name": "b"}"#).unwrap().get("name"), Some(&Json::Str("b".into())));
        assert_eq!(loads(r#"{"name": "\ud800"}"#).unwrap().get("name"), Some(&Json::Other));
        assert_eq!(loads(r#"{"name": "\ud83d\ude00"}"#).unwrap().get("name"), Some(&Json::Str("\u{1f600}".into())));
        assert_eq!(loads(r#"["a"]"#), Some(Json::Other));
    }

    #[test]
    fn what_python_refuses_is_refused() {
        for bad in
            ["", "{", "{\"a\" 1}", "{\"a\": 1,}", "[1,]", "01", "1.", "-", "\u{feff}{}", "{\"a\": \"\t\"}", "{} x", "{'a': 1}", "\"\\x\""]
        {
            assert_eq!(loads(bad), None, "{bad:?}");
        }
    }
}
