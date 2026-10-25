//! The clipboard history (in memory only) and the one-JSON-line-each-way protocol the window speaks to the daemon with.

use serde_json::{json, Value};

/// How many entries the history keeps.
pub const MAX_ITEMS: usize = 50;
/// Longer texts are not kept (a whole log file copied by accident has no place in a picker).
pub const MAX_TEXT: usize = 64 << 10;
/// Pictures: the biggest kept, and how many at most (they are the memory cost; texts are small).
pub const MAX_IMAGE: usize = 8 << 20;
pub const MAX_IMAGES: usize = 5;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Text(String),
    /// A PNG and its size in pixels.
    Image { png: Vec<u8>, w: u32, h: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: u64,
    pub kind: Kind,
    /// When it was copied (seconds since 1970).
    pub at: u64,
}

#[derive(Default)]
pub struct Store {
    pub items: Vec<Item>,
    next: u64,
}

/// A PNG's width and height, from its header.
pub fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
        return None;
    }
    let n = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    Some((n(16), n(20)))
}

impl Store {
    /// Puts a copied text first: no blanks, no repeats (copying it again moves it up), nothing over MAX_TEXT.
    pub fn push_text(&mut self, text: String, at: u64) -> bool {
        let t = text.trim_end_matches(['\n', '\r']).to_string();
        if t.trim().is_empty() || t.len() > MAX_TEXT || t.contains('\0') {
            return false;
        }
        self.push(Kind::Text(t), at);
        true
    }

    /// Puts a copied picture first (PNG only, up to MAX_IMAGE; only the newest MAX_IMAGES are kept).
    pub fn push_image(&mut self, png: Vec<u8>, at: u64) -> bool {
        if png.len() > MAX_IMAGE {
            return false;
        }
        let Some((w, h)) = png_size(&png) else { return false };
        self.push(Kind::Image { png, w, h }, at);
        let mut seen = 0;
        self.items.retain(|i| {
            if matches!(i.kind, Kind::Image { .. }) {
                seen += 1;
                seen <= MAX_IMAGES
            } else {
                true
            }
        });
        true
    }

    fn push(&mut self, kind: Kind, at: u64) {
        let id = match self.items.iter().position(|i| i.kind == kind) {
            Some(p) => self.items.remove(p).id,
            None => {
                self.next += 1;
                self.next
            }
        };
        self.items.insert(0, Item { id, kind, at });
        self.items.truncate(MAX_ITEMS);
    }

    pub fn get(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn delete(&mut self, id: u64) -> bool {
        let n = self.items.len();
        self.items.retain(|i| i.id != id);
        self.items.len() != n
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// The list as the window gets it: texts in full, pictures as their size only.
    pub fn list_json(&self, on: bool) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|i| match &i.kind {
                Kind::Text(t) => json!({"id": i.id, "kind": "text", "text": t, "at": i.at}),
                Kind::Image { w, h, png } => json!({"id": i.id, "kind": "image", "w": w, "h": h, "bytes": png.len(), "at": i.at}),
            })
            .collect();
        json!({"ok": true, "on": on, "items": items})
    }
}

/// What the window asks of the daemon.
#[derive(Debug, PartialEq)]
pub enum Request {
    List,
    Copy(u64),
    Delete(u64),
    Clear,
}

pub fn parse_request(line: &str) -> Option<Request> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let id = || v["id"].as_u64();
    Some(match v["op"].as_str()? {
        "list" => Request::List,
        "copy" => Request::Copy(id()?),
        "delete" => Request::Delete(id()?),
        "clear" => Request::Clear,
        _ => return None,
    })
}

/// One entry as the window sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: u64,
    /// The text, or "" for a picture.
    pub text: String,
    /// A picture's size.
    pub image: Option<(u32, u32)>,
    pub at: u64,
}

/// The daemon's answer to "list": whether history is on, and the entries.
pub fn parse_list(line: &str) -> Option<(bool, Vec<Entry>)> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if v["ok"] != Value::Bool(true) {
        return None;
    }
    let items = v["items"]
        .as_array()?
        .iter()
        .filter_map(|e| {
            Some(Entry {
                id: e["id"].as_u64()?,
                text: e["text"].as_str().unwrap_or("").to_string(),
                image: (e["kind"] == "image").then(|| (e["w"].as_u64().unwrap_or(0) as u32, e["h"].as_u64().unwrap_or(0) as u32)),
                at: e["at"].as_u64().unwrap_or(0),
            })
        })
        .collect();
    Some((v["on"].as_bool().unwrap_or(false), items))
}

/// Standard base64 (what `base64` prints) back to bytes; None on anything else.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim().bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, pad: usize) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend(vec![7u8; pad]);
        b
    }

    #[test]
    fn copies_go_first_without_repeats_or_blanks() {
        let mut s = Store::default();
        assert!(s.push_text("one\n".into(), 1));
        assert!(s.push_text("two".into(), 2));
        assert!(!s.push_text("  \n".into(), 3));
        assert!(s.push_text("one".into(), 4));
        let texts: Vec<_> = s.items.iter().map(|i| i.kind.clone()).collect();
        assert_eq!(texts, [Kind::Text("one".into()), Kind::Text("two".into())]);
        assert_eq!(s.items[0].id, 1, "copying again keeps the id");
        assert_eq!(s.items[0].at, 4);
    }

    #[test]
    fn the_history_and_its_entries_are_capped() {
        let mut s = Store::default();
        for i in 0..MAX_ITEMS + 10 {
            s.push_text(format!("t{i}"), i as u64);
        }
        assert_eq!(s.items.len(), MAX_ITEMS);
        assert_eq!(s.items[0].kind, Kind::Text(format!("t{}", MAX_ITEMS + 9)));
        assert!(!s.push_text("x".repeat(MAX_TEXT + 1), 0));
        assert!(!s.push_text("a\0b".into(), 0));
        for i in 0..MAX_IMAGES + 2 {
            assert!(s.push_image(png(i as u32 + 1, 9, 4), 0));
        }
        assert_eq!(s.items.iter().filter(|i| matches!(i.kind, Kind::Image { .. })).count(), MAX_IMAGES);
        assert!(!s.push_image(b"not a png at all, really".to_vec(), 0));
        assert!(!s.push_image(png(1, 1, MAX_IMAGE), 0));
        assert_eq!(png_size(&png(1920, 1080, 0)), Some((1920, 1080)));
    }

    #[test]
    fn delete_clear_and_the_protocol() {
        let mut s = Store::default();
        s.push_text("keep".into(), 1);
        s.push_text("drop".into(), 2);
        s.push_image(png(3, 4, 0), 3);
        assert!(s.delete(2));
        assert!(!s.delete(99));
        let line = s.list_json(true).to_string();
        let (on, es) = parse_list(&line).unwrap();
        assert!(on);
        assert_eq!(es.len(), 2);
        assert_eq!((es[0].image, es[1].text.as_str()), (Some((3, 4)), "keep"));
        s.clear();
        assert!(s.items.is_empty());

        assert_eq!(parse_request(r#"{"op":"list"}"#), Some(Request::List));
        assert_eq!(parse_request(r#"{"op":"copy","id":7}"#), Some(Request::Copy(7)));
        assert_eq!(parse_request(r#"{"op":"delete","id":3}"#), Some(Request::Delete(3)));
        assert_eq!(parse_request(r#"{"op":"clear"}"#), Some(Request::Clear));
        assert_eq!(parse_request(r#"{"op":"copy"}"#), None);
        assert_eq!(parse_request(r#"{"op":"rm -rf"}"#), None);
        assert_eq!(parse_request("garbage"), None);
        assert_eq!(parse_list(r#"{"ok":false}"#), None);
    }

    #[test]
    fn base64_is_read() {
        assert_eq!(b64_decode("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(b64_decode("aGk=\n").unwrap(), b"hi");
        assert!(b64_decode("a*b").is_none());
    }
}
