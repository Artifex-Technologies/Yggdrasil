//! Android's compiled XML (res/xml/appfilter.xml in a built .apk): only what appfilter.xml needs, the elements in order with
//! their attributes, read exactly as the script read them. The data comes from the pack: every offset and count is checked
//! against the data; a file that does not hold together is an error (the tool then tries the next place an appfilter.xml
//! may be, as the script did).

/// (tag, attributes in first-seen order, a repeated name keeping its first place and its last value)
pub type Element = (String, Vec<(String, String)>);

/// A document that does not parse (the script's ValueError, struct.error, IndexError or ET.ParseError).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unparsable;

fn u16_at(d: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        d.get(p..p.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        d.get(p..p.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn i32_at(d: &[u8], p: usize) -> Option<i32> {
    u32_at(d, p).map(|v| v as i32)
}

/// bytes.decode("utf-16-le", "replace"), as Python replaces: a lone low surrogate, or a high one not followed by a low
/// one, is one U+FFFD each; a high surrogate in the last whole unit takes any odd byte after it into its one U+FFFD; an odd
/// last byte otherwise is one more.
pub fn utf16le_lossy(b: &[u8]) -> String {
    let mut out = String::new();
    let units: Vec<u16> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let odd = b.len() % 2 == 1;
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if !(0xd800..0xe000).contains(&u) {
            out.push(char::from_u32(u as u32).unwrap());
            i += 1;
        } else if u >= 0xdc00 {
            out.push('\u{fffd}');
            i += 1;
        } else if i + 1 >= units.len() {
            // unexpected end of data: the high surrogate and anything after it
            out.push('\u{fffd}');
            return out;
        } else if (0xdc00..0xe000).contains(&units[i + 1]) {
            out.push(
                char::from_u32(
                    0x10000 + (((u as u32) - 0xd800) << 10) + (units[i + 1] as u32 - 0xdc00),
                )
                .unwrap(),
            );
            i += 2;
        } else {
            out.push('\u{fffd}');
            i += 1;
        }
    }
    if odd {
        out.push('\u{fffd}');
    }
    out
}

/// The elements of a compiled Android XML file; Err where the script's parse_axml raised (ValueError, struct.error, or
/// IndexError, which the script did not catch before this fix).
pub fn parse(data: &[u8]) -> Result<Vec<Element>, Unparsable> {
    let e = || Unparsable;
    if data.len() < 8 || u16_at(data, 0) != Some(0x0003) {
        return Err(e());
    }
    let mut pos = u16_at(data, 2).ok_or_else(e)? as usize;
    let end = u32_at(data, 4).ok_or_else(e)? as usize;
    let mut strings: Vec<String> = vec![];
    let mut out: Vec<Element> = vec![];
    while pos + 8 <= end.min(data.len()) {
        let ctype = u16_at(data, pos).ok_or_else(e)?;
        let hsize = u16_at(data, pos + 2).ok_or_else(e)? as usize;
        let csize = u32_at(data, pos + 4).ok_or_else(e)? as usize;
        if csize < 8 {
            break;
        }
        if ctype == 0x0001 {
            // string pool
            let count = u32_at(data, pos + 8).ok_or_else(e)? as usize;
            u32_at(data, pos + 12).ok_or_else(e)?;
            let flags = u32_at(data, pos + 16).ok_or_else(e)?;
            let str_start = u32_at(data, pos + 20).ok_or_else(e)? as usize;
            u32_at(data, pos + 24).ok_or_else(e)?;
            let utf8 = flags & 0x100 != 0;
            let offs_at = pos + hsize;
            // struct.unpack_from("<%dI" % count) needs all of them
            if count
                .checked_mul(4)
                .and_then(|n| n.checked_add(offs_at))
                .is_none_or(|end| end > data.len())
            {
                return Err(e());
            }
            let base = pos + str_start;
            for k in 0..count {
                let o = u32_at(data, offs_at + 4 * k).unwrap() as usize;
                let mut p = base + o;
                let s = if utf8 {
                    let n = *data.get(p).ok_or_else(e)?;
                    p += if n & 0x80 != 0 { 2 } else { 1 };
                    let mut b = *data.get(p).ok_or_else(e)? as usize;
                    p += 1;
                    if b & 0x80 != 0 {
                        b = ((b & 0x7f) << 8) | *data.get(p).ok_or_else(e)? as usize;
                        p += 1;
                    }
                    let slice = &data[p.min(data.len())..(p + b).min(data.len())];
                    String::from_utf8_lossy(slice).into_owned()
                } else {
                    let mut n = u16_at(data, p).ok_or_else(e)? as usize;
                    p += 2;
                    if n & 0x8000 != 0 {
                        n = ((n & 0x7fff) << 16) | u16_at(data, p).ok_or_else(e)? as usize;
                        p += 2;
                    }
                    utf16le_lossy(&data[p.min(data.len())..(p + 2 * n).min(data.len())])
                };
                strings.push(s);
            }
        } else if ctype == 0x0102 {
            // start of an element
            let at = pos + 8;
            if at + 22 > data.len() {
                return Err(e());
            }
            let name = i32_at(data, at + 12).unwrap();
            let astart = u16_at(data, at + 16).unwrap() as usize;
            let asize = u16_at(data, at + 18).unwrap() as usize;
            let acount = u16_at(data, at + 20).unwrap() as usize;
            let mut attrs: Vec<(String, String)> = vec![];
            let mut ap = pos + 16 + astart;
            for _ in 0..acount {
                if ap + 20 > data.len() {
                    return Err(e());
                }
                let aname = i32_at(data, ap + 4).unwrap();
                let araw = i32_at(data, ap + 8).unwrap();
                let atype = data[ap + 15];
                let adata = u32_at(data, ap + 16).unwrap() as usize;
                ap += if asize >= 20 { asize } else { 20 };
                if aname < 0 || aname as usize >= strings.len() {
                    continue;
                }
                let key = strings[aname as usize].clone();
                let value = if araw >= 0 && (araw as usize) < strings.len() {
                    strings[araw as usize].clone()
                } else if atype == 0x03 && adata < strings.len() {
                    strings[adata].clone()
                } else {
                    continue;
                };
                match attrs.iter_mut().find(|(k, _)| *k == key) {
                    Some(slot) => slot.1 = value,
                    None => attrs.push((key, value)),
                }
            }
            if name >= 0 && (name as usize) < strings.len() {
                out.push((strings[name as usize].clone(), attrs));
            }
        }
        pos += csize;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_is_replaced_as_python_replaces() {
        let cases: [(&[u8], &str); 12] = [
            (b"\x41\x00\x42", "A\u{fffd}"),
            (b"\x00\xd8", "\u{fffd}"),
            (b"\x00\xd8\x41", "\u{fffd}"),
            (b"\x00\xd8\x41\x00", "\u{fffd}A"),
            (b"\x00\xdc\x41\x00", "\u{fffd}A"),
            (b"\x00\xd8\x00\xd8\x00\xdc", "\u{fffd}\u{10000}"),
            (b"\x00\xd8\x00\xdc\x41", "\u{10000}\u{fffd}"),
            (b"\x00\xdc", "\u{fffd}"),
            (b"\x00\xdc\x41", "\u{fffd}\u{fffd}"),
            (b"\x41", "\u{fffd}"),
            (b"\x00\xd8\x00\xd8", "\u{fffd}\u{fffd}"),
            (b"\x00\xd8\x00\xd8\x41", "\u{fffd}\u{fffd}"),
        ];
        for (b, want) in cases {
            assert_eq!(utf16le_lossy(b), want, "{b:?}");
        }
    }

    #[test]
    fn junk_never_panics() {
        // every prefix and many corruptions of a small document
        let mut doc = vec![
            3u8, 0, 8, 0, 200, 0, 0, 0, 1, 0, 28, 0, 60, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0,
            0, 36, 0, 0, 0, 0, 0, 0, 0,
        ];
        doc.extend([0, 0, 0, 0, 4, 0, 0, 0, 1, 1, 0x61, 0, 1, 1, 0x62, 0]);
        doc.extend([
            2, 1, 16, 0, 56, 0, 0, 0, 1, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0,
            0, 20, 0, 20, 0, 1, 0, 0, 0, 0, 0,
        ]);
        doc.extend([
            255, 255, 255, 255, 1, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 3, 0, 0, 0, 0,
        ]);
        let mut seed = 0x1234_5678u32;
        for len in 0..doc.len() {
            let _ = parse(&doc[..len]);
        }
        for _ in 0..20000 {
            let mut d = doc.clone();
            for _ in 0..3 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let i = seed as usize % d.len();
                d[i] = (seed >> 8) as u8;
            }
            let _ = parse(&d);
        }
    }
}
