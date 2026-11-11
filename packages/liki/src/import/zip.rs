//! A zip archive (an .apk) read as Python's zipfile read it: the end record (with a comment, ZIP64, data before the
//! archive), the central directory (cp437 or UTF-8 names, NULs cutting a name short, the Info-ZIP Unicode path field, the
//! ZIP64 sizes), and a member's bytes after the same checks of its local header (name, overlapping members, encryption),
//! inflated and CRC-checked. Every way it can fail is one error: the tool then says the file is not a zip/apk file.
//!
//! Nothing is ever unpacked to disk; a member is read up to its stated size only (Python's zf.read() could inflate a
//! small crafted member to gigabytes before cutting it, see python/liki-icon-import's read_member).

use std::fs::File;
use std::os::unix::fs::FileExt;

#[derive(Debug, Clone)]
pub struct Entry {
    /// the name as zipfile shows it (cut at a NUL, or from the Unicode path field)
    pub filename: String,
    /// the name in the central directory, which the local header must repeat
    orig_filename: String,
    flag_bits: u16,
    compress_type: u16,
    crc: u32,
    compress_size: u64,
    pub file_size: u64,
    header_offset: i128,
    end_offset: i128,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.filename.ends_with('/')
    }
}

pub struct Zip {
    file: File,
    size: u64,
    pub entries: Vec<Entry>,
}

/// Why a zip or one of its members could not be read (BadZipFile, zlib.error, EOFError, NotImplementedError, ...).
#[derive(Debug)]
pub struct Bad(pub String);

fn bad<T>(msg: impl Into<String>) -> Result<T, Bad> {
    Err(Bad(msg.into()))
}

const CP437: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

fn cp437(b: &[u8]) -> String {
    let high: Vec<char> = CP437.chars().collect();
    b.iter()
        .map(|&c| {
            if c < 0x80 {
                c as char
            } else {
                high[(c - 0x80) as usize]
            }
        })
        .collect()
}

/// A name as bytes: UTF-8 when the flag says so (strictly: anything else is an error), else cp437.
fn decode_name(b: &[u8], flags: u16) -> Result<String, Bad> {
    if flags & 0x800 != 0 {
        String::from_utf8(b.to_vec())
            .map_err(|e| Bad(format!("'utf-8' codec can't decode the name: {e}")))
    } else {
        Ok(cp437(b))
    }
}

fn sanitize(name: &str) -> String {
    match name.find('\0') {
        Some(i) => name[..i].to_string(),
        None => name.to_string(),
    }
}

pub fn crc32(data: &[u8], mut crc: u32) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    crc = !crc;
    for &b in data {
        crc = t[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}
fn u64le(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}

impl Zip {
    /// Read at `pos` up to `n` bytes (fewer at the end of the file, none past it).
    fn read(&self, pos: i128, n: u64) -> Result<Vec<u8>, Bad> {
        if pos < 0 {
            return bad("negative seek position");
        }
        if pos >= self.size as i128 || n == 0 {
            return Ok(vec![]);
        }
        let n = n.min(self.size - pos as u64) as usize;
        let mut buf = vec![0u8; n];
        let mut got = 0;
        while got < n {
            match self.file.read_at(&mut buf[got..], pos as u64 + got as u64) {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return bad(e.to_string()),
            }
        }
        buf.truncate(got);
        Ok(buf)
    }

    pub fn open(file: File) -> Result<Zip, Bad> {
        let size = file.metadata().map_err(|e| Bad(e.to_string()))?.len();
        let mut z = Zip {
            file,
            size,
            entries: vec![],
        };
        z.contents()?;
        Ok(z)
    }

    /// _EndRecData: (size of the central directory, its offset, where the end record is), or None (not a zip file)
    fn end_record(&self) -> Result<Option<(u64, u64, i128)>, Bad> {
        if self.size < 22 {
            return Ok(None);
        }
        let last = self.read(self.size as i128 - 22, 22)?;
        if last.len() == 22 && &last[..4] == b"PK\x05\x06" && last[20..] == [0, 0] {
            return self
                .end_record64(
                    self.size as i128 - 22,
                    (u32le(&last, 12) as u64, u32le(&last, 16) as u64),
                )
                .map(Some);
        }
        let max_comment_start = self.size.saturating_sub(65535 + 22);
        let data = self.read(max_comment_start as i128, 65535 + 22)?;
        let Some(start) = data.windows(4).rposition(|w| w == b"PK\x05\x06") else {
            return Ok(None);
        };
        let rec = &data[start..(start + 22).min(data.len())];
        if rec.len() != 22 {
            return Ok(None);
        }
        self.end_record64(
            max_comment_start as i128 + start as i128,
            (u32le(rec, 12) as u64, u32le(rec, 16) as u64),
        )
        .map(Some)
    }

    /// _EndRecData64: the ZIP64 end record, when its locator is there
    fn end_record64(
        &self,
        offset: i128,
        (size_cd, offset_cd): (u64, u64),
    ) -> Result<(u64, u64, i128), Bad> {
        let mut offset = offset - 20;
        if offset < 0 {
            return Ok((size_cd, offset_cd, offset + 20));
        }
        let loc = self.read(offset, 20)?;
        if loc.len() != 20 {
            return bad("File is not a zip file");
        }
        if &loc[..4] != b"PK\x06\x07" {
            return Ok((size_cd, offset_cd, offset + 20));
        }
        let (diskno, reloff, disks) = (u32le(&loc, 4), u64le(&loc, 8), u32le(&loc, 16));
        if diskno != 0 || disks > 1 {
            return bad("zipfiles that span multiple disks are not supported");
        }
        offset -= 56;
        if reloff as i128 > offset {
            return bad("Corrupt zip64 end of central directory locator");
        }
        let mut extrasz = offset - reloff as i128;
        let mut data = self.read(reloff as i128, 56)?;
        if data.len() != 56 {
            return bad("File is not a zip file");
        }
        if !data.starts_with(b"PK\x06\x06") && reloff as i128 != offset {
            extrasz = 0;
            data = self.read(offset, 56)?;
            if data.len() != 56 {
                return bad("File is not a zip file");
            }
        }
        if !data.starts_with(b"PK\x06\x06") {
            return bad("Zip64 end of central directory record not found");
        }
        let (sz, dirsize, diroffset) = (u64le(&data, 4), u64le(&data, 40), u64le(&data, 48));
        if diroffset as i128 + dirsize as i128 != reloff as i128 || sz as i128 + 12 != 56 + extrasz
        {
            return bad("Corrupt zip64 end of central directory record");
        }
        Ok((dirsize, diroffset, offset - extrasz))
    }

    /// _RealGetContents: the central directory
    fn contents(&mut self) -> Result<(), Bad> {
        let Some((size_cd, offset_cd, location)) = self.end_record()? else {
            return bad("File is not a zip file");
        };
        let concat = location - size_cd as i128 - offset_cd as i128;
        let start_dir = offset_cd as i128 + concat;
        if start_dir < 0 {
            return bad("Bad offset for central directory");
        }
        let data = self.read(start_dir, size_cd)?;
        let mut p = 0usize;
        let mut total: u128 = 0;
        let take = |p: &mut usize, n: usize| -> &[u8] {
            let s = (*p).min(data.len());
            let e = (*p + n).min(data.len());
            *p += n;
            &data[s..e]
        };
        while total < size_cd as u128 {
            let cd = take(&mut p, 46).to_vec();
            if cd.len() != 46 {
                return bad("Truncated central directory");
            }
            if &cd[..4] != b"PK\x01\x02" {
                return bad("Bad magic number for central directory");
            }
            let (fn_len, extra_len, comment_len) = (
                u16le(&cd, 28) as usize,
                u16le(&cd, 30) as usize,
                u16le(&cd, 32) as usize,
            );
            let name_bytes = take(&mut p, fn_len).to_vec();
            let flags = u16le(&cd, 8);
            let orig = decode_name(&name_bytes, flags)?;
            let mut extra = take(&mut p, extra_len).to_vec();
            take(&mut p, comment_len);
            let extract_version = cd[6];
            if extract_version > 63 {
                return bad(format!(
                    "zip file version {:.1}",
                    extract_version as f64 / 10.0
                ));
            }
            let mut e = Entry {
                filename: sanitize(&orig),
                orig_filename: orig,
                flag_bits: flags,
                compress_type: u16le(&cd, 10),
                crc: u32le(&cd, 16),
                compress_size: u32le(&cd, 20) as u64,
                file_size: u32le(&cd, 24) as u64,
                header_offset: u32le(&cd, 42) as i128,
                end_offset: 0,
            };
            // _decodeExtra
            let name_crc = crc32(&name_bytes, 0);
            while extra.len() >= 4 {
                let (tp, ln) = (u16le(&extra, 0), u16le(&extra, 2) as usize);
                if ln + 4 > extra.len() {
                    return bad(format!("Corrupt extra field {tp:04x} (size={ln})"));
                }
                let mut d = &extra[4..ln + 4];
                if tp == 0x0001 {
                    let q = |d: &mut &[u8], what: &str| -> Result<u64, Bad> {
                        if d.len() < 8 {
                            return bad(format!("Corrupt zip64 extra field. {what} not found."));
                        }
                        let v = u64le(d, 0);
                        *d = &d[8..];
                        Ok(v)
                    };
                    if e.file_size == 0xFFFF_FFFF {
                        e.file_size = q(&mut d, "File size")?;
                    }
                    if e.compress_size == 0xFFFF_FFFF {
                        e.compress_size = q(&mut d, "Compress size")?;
                    }
                    if e.header_offset == 0xFFFF_FFFF {
                        e.header_offset = q(&mut d, "Header offset")? as i128;
                    }
                } else if tp == 0x7075 {
                    if d.len() < 5 {
                        return bad("Corrupt unicode path extra field (0x7075)");
                    }
                    if d[0] == 1 && u32le(d, 1) == name_crc {
                        let name = String::from_utf8(d[5..].to_vec()).map_err(|_| {
                            Bad(
                                "Corrupt unicode path extra field (0x7075): invalid utf-8 bytes"
                                    .into(),
                            )
                        })?;
                        if !name.is_empty() {
                            e.filename = sanitize(&name);
                        }
                    }
                }
                extra = extra[ln + 4..].to_vec();
            }
            e.header_offset += concat;
            self.entries.push(e);
            total += 46 + fn_len as u128 + extra_len as u128 + comment_len as u128;
        }
        // where each member must end: the next member's start, or the central directory
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by_key(|&i| self.entries[i].header_offset);
        let mut end = start_dir;
        for &i in order.iter().rev() {
            self.entries[i].end_offset = end;
            end = self.entries[i].header_offset;
        }
        Ok(())
    }

    /// The entry zipfile's NameToInfo has for a name: the last one of that name.
    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().rev().find(|e| e.filename == name)
    }

    /// A member's bytes (at most its stated size), after the checks ZipFile.open makes; an error where zipfile raised one.
    pub fn read_member(&self, e: &Entry) -> Result<Vec<u8>, Bad> {
        let header = self.read(e.header_offset, 30)?;
        if header.len() != 30 {
            return bad("Truncated file header");
        }
        if &header[..4] != b"PK\x03\x04" {
            return bad("Bad magic number for file header");
        }
        let (fn_len, extra_len) = (u16le(&header, 26) as u64, u16le(&header, 28) as i128);
        let fname = self.read(e.header_offset + 30, fn_len)?;
        let data_start = e.header_offset + 30 + fname.len() as i128 + extra_len;
        if e.flag_bits & 0x20 != 0 {
            return bad("compressed patched data (flag bit 5)");
        }
        if e.flag_bits & 0x40 != 0 {
            return bad("strong encryption (flag bit 6)");
        }
        let local_name = decode_name(&fname, u16le(&header, 6))?;
        if local_name != e.orig_filename {
            return bad("File name in directory and header differ.");
        }
        // a member reaching into the next one (a zip bomb's trick); two members at the same place only warned
        if data_start + e.compress_size as i128 > e.end_offset && e.end_offset != e.header_offset {
            return bad("Overlapped entries (possible zip bomb)");
        }
        if e.flag_bits & 0x1 != 0 {
            return bad("File is encrypted, password required for extraction");
        }
        if !matches!(e.compress_type, 0 | 8) {
            // bzip2, LZMA and Zstandard (12, 14, 93), which zipfile could read, are never used in an .apk
            return bad(format!("compression type {}", e.compress_type));
        }
        self.member_bytes(e, data_start)
    }

    /// ZipExtFile.read(max(file_size, 1)) step by step, as python/liki-icon-import reads a member: compressed bytes are
    /// read in pieces of max(n, 4096) (n: what is still wanted), inflated to at most max(n, 4096) bytes at a time, the
    /// member ends when its compressed bytes are used up, its stream ends or its stated size is reached; the file ending
    /// before that is an error (EOFError), and so is a CRC that does not match.
    fn member_bytes(&self, e: &Entry, start: i128) -> Result<Vec<u8>, Bad> {
        use miniz_oxide::inflate::stream::{inflate, InflateState};
        use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};
        let deflated = e.compress_type == 8;
        let mut state = InflateState::new_boxed(DataFormat::Raw);
        let mut compress_left = e.compress_size;
        let mut left = e.file_size;
        let mut wanted = e.file_size.max(1);
        let mut pos = start;
        let mut tail: Vec<u8> = vec![];
        let mut out: Vec<u8> = vec![];
        let mut eof = false;
        // _read2: the next compressed bytes
        let read2 = |n: u64, compress_left: &mut u64, pos: &mut i128| -> Result<Vec<u8>, Bad> {
            if *compress_left == 0 {
                return Ok(vec![]);
            }
            let n = n.max(4096).min(*compress_left);
            let d = self.read(*pos, n)?;
            *pos += d.len() as i128;
            *compress_left -= d.len() as u64;
            if d.is_empty() {
                return bad("EOFError");
            }
            Ok(d)
        };
        while wanted > 0 && !eof {
            let n = wanted;
            let mut data;
            if deflated {
                data = std::mem::take(&mut tail);
                if n > data.len() as u64 {
                    let more = read2(n - data.len() as u64, &mut compress_left, &mut pos)?;
                    data.extend(more);
                }
                // decompress(data, max(n, 4096))
                let max_len = n.max(4096).min(usize::MAX as u64 / 2) as usize;
                let mut produced: Vec<u8> = vec![];
                let mut buf = vec![0u8; max_len.min(1 << 20)];
                let mut consumed = 0usize;
                let mut stream_end = false;
                loop {
                    let room = (max_len - produced.len()).min(buf.len());
                    if room == 0 {
                        break;
                    }
                    let r = inflate(
                        &mut state,
                        &data[consumed..],
                        &mut buf[..room],
                        MZFlush::None,
                    );
                    consumed += r.bytes_consumed;
                    produced.extend_from_slice(&buf[..r.bytes_written]);
                    match r.status {
                        Ok(MZStatus::StreamEnd) => {
                            stream_end = true;
                            break;
                        }
                        Ok(_) if r.bytes_consumed > 0 || r.bytes_written > 0 => {}
                        Ok(_) | Err(MZError::Buf) => break,
                        Err(_) => return bad("Error -3 while decompressing data"),
                    }
                }
                // after the end of the stream the rest is unused, not a tail to inflate later
                tail = if stream_end {
                    vec![]
                } else {
                    data[consumed..].to_vec()
                };
                eof = stream_end || (compress_left == 0 && tail.is_empty());
                data = produced;
            } else {
                data = read2(n, &mut compress_left, &mut pos)?;
                eof = compress_left == 0;
            }
            data.truncate(left.min(data.len() as u64) as usize);
            left -= data.len() as u64;
            if left == 0 {
                eof = true;
            }
            wanted = wanted.saturating_sub(data.len() as u64);
            out.extend(data);
        }
        if crc32(&out, 0) != e.crc {
            return bad("Bad CRC-32");
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_zlibs() {
        assert_eq!(crc32(b"", 0), 0);
        assert_eq!(crc32(b"123456789", 0), 0xcbf4_3926);
        assert_eq!(crc32(b"6789", crc32(b"12345", 0)), 0xcbf4_3926);
    }

    #[test]
    fn names_are_cp437_unless_flagged() {
        assert_eq!(cp437(b"a\x80\xff"), "aÇ\u{a0}");
        assert!(decode_name(b"\xff", 0x800).is_err());
        assert_eq!(sanitize("a\0b"), "a");
    }
}
