//! Image.resize(size, Image.LANCZOS) of an RGBA image as Pillow 13.0 does it: premultiplied (RGBA -> RGBa), resampled in
//! two passes with Lanczos-3 weights in 22-bit fixed point (libImaging/Resample.c), then back (RGBa -> RGBA,
//! libImaging/Convert.c); Image.alpha_composite (libImaging/AlphaComposite.c); and a PNG file for the preview sheet.
//! Translated from Pillow (MIT-CMU licence, see NOTICE-Pillow).

use super::draw::Img;

const PRECISION_BITS: i32 = 32 - 8 - 2;

/// MULDIV255(a, b): a * b / 255, rounded as Pillow rounds it
fn muldiv255(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    ((t >> 8) + t) >> 8
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let x = x * std::f64::consts::PI;
    x.sin() / x
}

fn lanczos(x: f64) -> f64 {
    if (-3.0..3.0).contains(&x) {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

/// precompute_coeffs + normalize_coeffs_8bpc: (ksize, bounds (xmin, count), weights in fixed point)
fn coeffs(in_size: i32, in0: f32, in1: f32, out_size: i32) -> (usize, Vec<(i32, i32)>, Vec<i32>) {
    let scale = (in1 - in0) as f64 / out_size as f64;
    let filterscale = if scale < 1.0 { 1.0 } else { scale };
    let support = 3.0 * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let inv = 1.0 / filterscale;
    let mut bounds = Vec::with_capacity(out_size as usize);
    let mut kk = vec![0f64; out_size as usize * ksize];
    for xx in 0..out_size {
        let center = in0 as f64 + (xx as f64 + 0.5) * scale;
        let mut ww = 0.0;
        let mut xmin = (center - support + 0.5) as i32;
        if xmin < 0 {
            xmin = 0;
        }
        let mut xmax = (center + support + 0.5) as i32;
        if xmax > in_size {
            xmax = in_size;
        }
        xmax -= xmin;
        let k = &mut kk[xx as usize * ksize..(xx as usize + 1) * ksize];
        for x in 0..xmax {
            let w = lanczos(((x + xmin) as f64 - center + 0.5) * inv);
            k[x as usize] = w;
            ww += w;
        }
        if ww != 0.0 {
            for v in k.iter_mut().take(xmax as usize) {
                *v /= ww;
            }
        }
        bounds.push((xmin, xmax));
    }
    let one = (1i64 << PRECISION_BITS) as f64;
    let fixed = kk.iter().map(|&v| if v < 0.0 { (-0.5 + v * one) as i32 } else { (0.5 + v * one) as i32 }).collect();
    (ksize, bounds, fixed)
}

fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// ImagingResample of a 4-band 8-bit image (w x h, bytes) to xsize x ysize with the whole image as the box
fn resample4(src: &[u8], w: i32, h: i32, xsize: i32, ysize: i32) -> Vec<u8> {
    let need_h = xsize != w || (w as f32) != xsize as f32;
    let need_v = ysize != h || (h as f32) != ysize as f32;
    let horizontal_first = !((h - ysize) > 0 && (h - ysize) > (w - xsize) * 2);
    let vert = if (need_h && horizontal_first) || need_v { Some(coeffs(h, 0.0, h as f32, ysize)) } else { None };
    let mut img = src.to_vec();
    let (mut cw, mut ch) = (w, h);
    let horizontal = |img: &[u8],
                      cw: i32,
                      rows: std::ops::Range<i32>,
                      xsize: i32,
                      (ksize, bounds, kk): &(usize, Vec<(i32, i32)>, Vec<i32>)|
     -> Vec<u8> {
        let mut out = vec![0u8; xsize as usize * rows.len() * 4];
        for (oy, y) in rows.enumerate() {
            let line = &img[y as usize * cw as usize * 4..(y as usize + 1) * cw as usize * 4];
            for xx in 0..xsize as usize {
                let (xmin, xmax) = bounds[xx];
                let k = &kk[xx * ksize..];
                let mut ss = [1i32 << (PRECISION_BITS - 1); 4];
                for (x, &w) in k[..xmax as usize].iter().enumerate() {
                    let p = (x + xmin as usize) * 4;
                    for (c, s) in ss.iter_mut().enumerate() {
                        *s += line[p + c] as i32 * w;
                    }
                }
                for c in 0..4 {
                    out[(oy * xsize as usize + xx) * 4 + c] = clip8(ss[c]);
                }
            }
        }
        out
    };
    let vertical = |img: &[u8], cw: i32, ysize: i32, (ksize, bounds, kk): &(usize, Vec<(i32, i32)>, Vec<i32>)| -> Vec<u8> {
        let mut out = vec![0u8; cw as usize * ysize as usize * 4];
        for yy in 0..ysize as usize {
            let (ymin, ymax) = bounds[yy];
            let k = &kk[yy * ksize..];
            for xx in 0..cw as usize {
                let mut ss = [1i32 << (PRECISION_BITS - 1); 4];
                for (y, &w) in k[..ymax as usize].iter().enumerate() {
                    let p = ((y + ymin as usize) * cw as usize + xx) * 4;
                    for (c, s) in ss.iter_mut().enumerate() {
                        *s += img[p + c] as i32 * w;
                    }
                }
                for c in 0..4 {
                    out[(yy * cw as usize + xx) * 4 + c] = clip8(ss[c]);
                }
            }
        }
        out
    };
    if horizontal_first {
        let mut vert = vert;
        if need_h {
            let horiz = coeffs(w, 0.0, w as f32, xsize);
            let (first, last) = match &mut vert {
                Some((_, b, _)) => {
                    let first = b[0].0;
                    let last = b[ysize as usize - 1].0 + b[ysize as usize - 1].1;
                    if first != 0 {
                        for e in b.iter_mut() {
                            e.0 -= first;
                        }
                    }
                    (first, last)
                }
                None => (0, h),
            };
            img = horizontal(&img, cw, first..last, xsize, &horiz);
            cw = xsize;
            ch = last - first;
        }
        if need_v {
            img = vertical(&img, cw, ysize, vert.as_ref().unwrap());
            ch = ysize;
        }
    } else {
        if need_v {
            img = vertical(&img, cw, ysize, vert.as_ref().unwrap());
            ch = ysize;
        }
        if need_h {
            let horiz = coeffs(w, 0.0, w as f32, xsize);
            img = horizontal(&img, cw, 0..ch, xsize, &horiz);
        }
    }
    let _ = ch;
    img
}

/// Image.resize((xsize, ysize), Image.LANCZOS) of an RGBA image
pub fn resize_lanczos(im: &Img, xsize: i32, ysize: i32) -> Img {
    if im.w == xsize && im.h == ysize {
        return im.clone();
    }
    // RGBA -> RGBa
    let mut pre = im.px.clone();
    for p in pre.chunks_mut(4) {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = muldiv255(*c as u32, a) as u8;
        }
    }
    let mut out = resample4(&pre, im.w, im.h, xsize, ysize);
    // RGBa -> RGBA
    for p in out.chunks_mut(4) {
        let a = p[3] as u32;
        if a != 255 && a != 0 {
            for c in &mut p[..3] {
                *c = ((255 * *c as u32) / a).min(255) as u8;
            }
        }
    }
    Img { w: xsize, h: ysize, ps: 4, px: out }
}

/// Image.alpha_composite(im, dest): `src` over `dst` at (dx, dy), both RGBA, the part inside `dst`
pub fn alpha_composite(dst: &mut Img, src: &Img, dx: i32, dy: i32) {
    for y in 0..src.h {
        for x in 0..src.w {
            let (tx, ty) = (dx + x, dy + y);
            if tx < 0 || ty < 0 || tx >= dst.w || ty >= dst.h {
                continue;
            }
            let si = ((y * src.w + x) * 4) as usize;
            let di = ((ty * dst.w + tx) * 4) as usize;
            let s = &src.px[si..si + 4];
            if s[3] == 0 {
                continue;
            }
            let d = [dst.px[di] as u32, dst.px[di + 1] as u32, dst.px[di + 2] as u32, dst.px[di + 3] as u32];
            let sa = s[3] as u32;
            let blend = d[3] * (255 - sa);
            let outa255 = sa * 255 + blend;
            let coef1 = sa * 255 * 255 * (1 << 7) / outa255;
            let coef2 = 255 * (1 << 7) - coef1;
            let shift = |v: u32| (((v >> 8) + v) >> 8) >> 7;
            for c in 0..3 {
                let tmp = s[c] as u32 * coef1 + d[c] * coef2;
                dst.px[di + c] = shift(tmp + (0x80 << 7)) as u8;
            }
            let a = outa255 + 0x80;
            dst.px[di + 3] = (((a >> 8) + a) >> 8) as u8;
        }
    }
}

/// An RGBA image as a PNG file (8 bits a channel, no interlacing).
pub fn png(im: &Img) -> Vec<u8> {
    let mut raw = Vec::with_capacity((im.w as usize * 4 + 1) * im.h as usize);
    for row in im.px.chunks(im.w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend((data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend(crc32(&c).to_be_bytes());
    };
    let mut ihdr = vec![];
    ihdr.extend((im.w as u32).to_be_bytes());
    ihdr.extend((im.h as u32).to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xedb8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    !crc
}
