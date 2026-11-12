//! liki-icns: the biggest PNG inside a macOS .icns file. An .icns is "icns", a length, then (type, length, data) chunks; the
//! modern ones (ic07..ic14, icp4..icp6) hold plain PNG data, the old ones (ARGB, JPEG 2000) are skipped.

pub const DOC: &str = "Extract the biggest PNG from a macOS .icns file: liki-icns in.icns out.png
An .icns is a list of (type, length, data) chunks; the modern ones (ic07..ic14, icp4..icp6) hold
plain PNG data. Old ones (ARGB/JPEG 2000) are skipped.";

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The PNG with the most pixels (the first of equal ones), or why there is none.
pub fn biggest_png(data: &[u8]) -> Result<&[u8], String> {
    if data.get(..4) != Some(b"icns") {
        return Err("not an .icns file".into());
    }
    let mut pos: usize = 8;
    let mut best: Option<(u64, &[u8])> = None;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if size < 8 {
            break;
        }
        let end = (pos + size).min(data.len());
        let chunk = &data[(pos + 8).min(end)..end];
        if chunk.starts_with(PNG_SIGNATURE) {
            // the width and height of the PNG's header; a chunk too short for them is broken (the script's struct.error)
            let Some(wh) = chunk.get(16..24) else {
                return Err("unpack requires a buffer of 8 bytes".into());
            };
            let w = u32::from_be_bytes(wh[..4].try_into().unwrap()) as u64;
            let h = u32::from_be_bytes(wh[4..].try_into().unwrap()) as u64;
            if best.is_none_or(|(area, _)| w * h > area) {
                best = Some((w * h, chunk));
            }
        }
        pos += size;
    }
    best.map(|(_, c)| c).ok_or_else(|| "no PNG data inside".into())
}

/// liki-icns IN OUT; the exit code. The .icns is read and checked before the output file is opened, so a file that is not
/// one leaves no empty picture behind (the script opened the output first).
pub fn main(args: &[String]) -> i32 {
    if args.len() != 2 {
        eprintln!("{DOC}");
        return 1;
    }
    let data = match std::fs::read(&args[0]) {
        Ok(d) => d,
        Err(e) => crate::crash("liki-icns", format!("{}: {e}", args[0])),
    };
    let png = match biggest_png(&data) {
        Ok(p) => p,
        Err(e) => crate::crash("liki-icns", e),
    };
    if let Err(e) = std::fs::write(&args[1], png) {
        crate::crash("liki-icns", format!("{}: {e}", args[1]));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, tail: &[u8]) -> Vec<u8> {
        let mut v = PNG_SIGNATURE.to_vec();
        v.extend_from_slice(&[0, 0, 0, 13]);
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(tail);
        v
    }

    fn icns(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut body = vec![];
        for (kind, data) in chunks {
            body.extend_from_slice(*kind);
            body.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
            body.extend_from_slice(data);
        }
        let mut out = b"icns".to_vec();
        out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
        out.extend(body);
        out
    }

    #[test]
    fn the_biggest_png_wins_and_the_first_of_equals() {
        let a = png(128, 128, b"a");
        let b = png(512, 512, b"b");
        let c = png(512, 512, b"c");
        let file = icns(&[(b"ic07", a), (b"ARGB", vec![1, 2, 3]), (b"ic09", b.clone()), (b"ic10", c)]);
        assert_eq!(biggest_png(&file).unwrap(), &b[..]);
    }

    #[test]
    fn junk_is_refused() {
        assert_eq!(biggest_png(b"nope").unwrap_err(), "not an .icns file");
        assert_eq!(biggest_png(&icns(&[(b"ARGB", vec![0; 40])])).unwrap_err(), "no PNG data inside");
        assert!(biggest_png(&icns(&[(b"ic07", PNG_SIGNATURE.to_vec())])).is_err());
        // a length below 8 ends the list; one past the end is cut at the end
        let mut f = icns(&[(b"ic07", png(16, 16, b""))]);
        f.extend_from_slice(b"ic08\x00\x00\x00\x02");
        assert!(biggest_png(&f).is_ok());
        let mut g = b"icns\0\0\0\0ic08\xff\xff\xff\xff".to_vec();
        g.extend(png(32, 32, b"x"));
        assert_eq!(biggest_png(&g).unwrap().len(), png(32, 32, b"x").len());
    }
}
