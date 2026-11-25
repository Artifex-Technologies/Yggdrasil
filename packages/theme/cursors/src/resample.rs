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

/// The rounding Pillow adds before it shifts a sum back
const HALF: i32 = 1 << (PRECISION_BITS - 1);

/// One direction's weights (coeffs) kept as running totals: `sums[o * stride + j]` is the sum of output pixel o's first j
/// weights. A stretch of equal input pixels from a to b then costs one multiplication, v * (sums[b] - sums[a]), which is
/// exactly what adding v * weight one by one gives, in integers. No sum comes near i32's limits in any order: the weights'
/// magnitudes add up to less than 1.5 * 2^22, times at most 255.
struct Weights {
    stride: usize,
    /// (the first input pixel, how many)
    bounds: Vec<(usize, usize)>,
    sums: Vec<i32>,
}

impl Weights {
    fn new(in_size: i32, out_size: i32) -> Weights {
        let (ksize, bounds, kk) = coeffs(in_size, 0.0, in_size as f32, out_size);
        let stride = ksize + 1;
        let mut sums = vec![0i32; out_size as usize * stride];
        for (o, k) in kk.chunks_exact(ksize).enumerate() {
            let s = &mut sums[o * stride..(o + 1) * stride];
            for (j, &w) in k.iter().enumerate() {
                s[j + 1] = s[j] + w;
            }
        }
        Weights { stride, bounds: bounds.into_iter().map(|(min, n)| (min as usize, n as usize)).collect(), sums }
    }

    /// The weights for shrinking or stretching `in_size` pixels to `out_size`, made once a run: the shapes are drawn in
    /// five sizes, every frame of a size with the same weights.
    fn get(in_size: i32, out_size: i32) -> std::sync::Arc<Weights> {
        type Cache = Vec<((i32, i32), std::sync::Arc<Weights>)>;
        static CACHE: std::sync::Mutex<Cache> = std::sync::Mutex::new(Vec::new());
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, w)) = cache.iter().find(|(k, _)| *k == (in_size, out_size)) {
            return w.clone();
        }
        let w = std::sync::Arc::new(Weights::new(in_size, out_size));
        cache.push(((in_size, out_size), w.clone()));
        w
    }

    /// output pixel o's first input pixel and the running totals of its weights
    fn at(&self, o: usize) -> (usize, &[i32]) {
        let (min, n) = self.bounds[o];
        (min, &self.sums[o * self.stride..o * self.stride + n + 1])
    }
}

/// RGBA -> RGBa of one pixel (libImaging/Convert.c, rgbA2rgba)
fn premultiply(p: u32) -> [i32; 4] {
    let [r, g, b, a] = p.to_ne_bytes().map(u32::from);
    [muldiv255(r, a) as i32, muldiv255(g, a) as i32, muldiv255(b, a) as i32, a as i32]
}

/// ImagingResampleHorizontal_8bpc of one row of RGBA pixels, premultiplied on the way in as Pillow premultiplies the whole
/// image first. The row is taken as runs of equal pixels (the shapes are drawn without smoothing at 8x, so a row is a
/// few long runs), and each run's share of an output pixel is one multiplication a channel; runs of transparent black add
/// nothing and are left out, and so are the output pixels that reach none of the other runs (they are 0, as `out` is
/// given). `runs` is room to reuse. Returns the output pixels that may not be 0, lo..hi.
fn horizontal_row(row: &[u8], wt: &Weights, runs: &mut Vec<(usize, usize, [i32; 4])>, out: &mut [u8]) -> (usize, usize) {
    runs.clear();
    let (mut start, mut prev) = (0, 0u32);
    for (x, &p) in row.as_chunks::<4>().0.iter().enumerate() {
        let v = u32::from_ne_bytes(p);
        if v != prev {
            if prev != 0 {
                runs.push((start, x, premultiply(prev)));
            }
            (start, prev) = (x, v);
        }
    }
    if prev != 0 {
        runs.push((start, row.len() / 4, premultiply(prev)));
    }
    let (mut lo, mut hi) = (usize::MAX, 0);
    let mut r = 0;
    for (xx, o) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (xmin, sums) = wt.at(xx);
        let end = xmin + sums.len() - 1;
        while r < runs.len() && runs[r].1 <= xmin {
            r += 1;
        }
        if r == runs.len() {
            break;
        }
        if runs[r].0 >= end {
            continue;
        }
        let mut ss = [HALF; 4];
        for &(a, b, v) in runs[r..].iter().take_while(|run| run.0 < end) {
            let k = sums[b.min(end) - xmin] - sums[a.max(xmin) - xmin];
            for (s, c) in ss.iter_mut().zip(v) {
                *s += c * k;
            }
        }
        for (o, s) in o.iter_mut().zip(ss) {
            *o = clip8(s);
        }
        lo = lo.min(xx);
        hi = xx + 1;
    }
    (lo.min(hi), hi)
}

/// ImagingResampleVertical_8bpc over stretches of equal rows: stretch i begins at row starts[i] and ends where the next
/// begins (the last at `end`); its row, `cw` pixels, is rows[i], and its pixels outside lo..hi (nonzero[i]) are 0.
/// Each output row adds up whole rows, so the inner loop runs along a row, and a stretch's rows at once with the sum of
/// their weights.
fn vertical(rows: &[u8], nonzero: &[(usize, usize)], starts: &[usize], end: usize, cw: usize, wt: &Weights, out: &mut [u8]) {
    let mut acc = vec![0i32; cw * 4];
    let mut s = 0;
    for (yy, orow) in out.chunks_exact_mut(cw * 4).enumerate() {
        let (ymin, sums) = wt.at(yy);
        let wend = ymin + sums.len() - 1;
        while s + 1 < starts.len() && starts[s + 1] <= ymin {
            s += 1;
        }
        acc.fill(HALF);
        for j in s..starts.len() {
            if starts[j] >= wend {
                break;
            }
            let stop = starts.get(j + 1).copied().unwrap_or(end);
            let k = sums[stop.min(wend) - ymin] - sums[starts[j].max(ymin) - ymin];
            let (lo, hi) = nonzero[j];
            if k == 0 || lo >= hi {
                continue;
            }
            let row = &rows[j * cw * 4..(j + 1) * cw * 4];
            for (a, &v) in acc[lo * 4..hi * 4].iter_mut().zip(&row[lo * 4..hi * 4]) {
                *a += v as i32 * k;
            }
        }
        for (o, &a) in orow.iter_mut().zip(&acc) {
            *o = clip8(a);
        }
    }
}

/// ImagingResample of a 4-band 8-bit image to xsize x ysize with the whole image as the box, premultiplied on the way in.
/// Only the order ygg-cursors needs: a resize in both directions, horizontal pass first (Pillow goes vertical first only
/// when the height shrinks by more than twice the width does; the shapes are square and the preview sheet grows).
fn resample4(im: &Img, xsize: i32, ysize: i32) -> Vec<u8> {
    let (w, h) = (im.w, im.h);
    let need_h = xsize != w || (w as f32) != xsize as f32;
    let need_v = ysize != h || (h as f32) != ysize as f32;
    let horizontal_first = !((h - ysize) > 0 && (h - ysize) > (w - xsize) * 2);
    assert!(need_h && need_v && horizontal_first, "resample4: {w}x{h} to {xsize}x{ysize} is not a resize ygg-cursors makes");
    let (w, xsize, ysize) = (w as usize, xsize as usize, ysize as usize);
    let horiz = Weights::get(im.w, xsize as i32);
    let vert = Weights::get(h, ysize as i32);
    // the rows the vertical pass uses
    let first = vert.bounds[0].0;
    let last = vert.bounds[ysize - 1].0 + vert.bounds[ysize - 1].1;
    // stretches of equal rows (above and below a shape, and along its straight sides): each is resampled once
    let src = |y: usize| &im.px[y * w * 4..(y + 1) * w * 4];
    let starts: Vec<usize> = (first..last).filter(|&y| y == first || src(y) != src(y - 1)).collect();
    let mut rows = vec![0u8; starts.len() * xsize * 4];
    let mut nonzero = Vec::with_capacity(starts.len());
    let mut runs = vec![];
    for (&y, row) in starts.iter().zip(rows.chunks_exact_mut(xsize * 4)) {
        nonzero.push(horizontal_row(src(y), &horiz, &mut runs, row));
    }
    let mut out = vec![0u8; xsize * ysize * 4];
    vertical(&rows, &nonzero, &starts, last, xsize, &vert, &mut out);
    out
}

/// Image.resize((xsize, ysize), Image.LANCZOS) of an RGBA image (see resample4 for the sizes it takes)
pub fn resize_lanczos(im: &Img, xsize: i32, ysize: i32) -> Img {
    let mut out = resample4(im, xsize, ysize);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Pillow's two passes as Resample.c writes them, tap by tap: premultiplied, the horizontal pass over the rows the
    /// vertical one uses, then the vertical pass
    fn tap_by_tap(im: &Img, xsize: i32, ysize: i32) -> Vec<u8> {
        let w = im.w as usize;
        let pre: Vec<u8> = im.px.as_chunks::<4>().0.iter().flat_map(|&p| premultiply(u32::from_ne_bytes(p)).map(|v| v as u8)).collect();
        let (hk, hb, hw) = coeffs(im.w, 0.0, im.w as f32, xsize);
        let (vk, vb, vw) = coeffs(im.h, 0.0, im.h as f32, ysize);
        let first = vb[0].0 as usize;
        let last = (vb[ysize as usize - 1].0 + vb[ysize as usize - 1].1) as usize;
        let xsize = xsize as usize;
        let mut mid = vec![0u8; xsize * (last - first) * 4];
        for y in first..last {
            for xx in 0..xsize {
                let mut ss = [HALF; 4];
                for x in 0..hb[xx].1 as usize {
                    for c in 0..4 {
                        ss[c] += pre[(y * w + hb[xx].0 as usize + x) * 4 + c] as i32 * hw[xx * hk + x];
                    }
                }
                for c in 0..4 {
                    mid[((y - first) * xsize + xx) * 4 + c] = clip8(ss[c]);
                }
            }
        }
        let mut out = vec![0u8; xsize * ysize as usize * 4];
        for yy in 0..ysize as usize {
            for xx in 0..xsize {
                let mut ss = [HALF; 4];
                for y in 0..vb[yy].1 as usize {
                    for c in 0..4 {
                        ss[c] += mid[((vb[yy].0 as usize - first + y) * xsize + xx) * 4 + c] as i32 * vw[yy * vk + y];
                    }
                }
                for c in 0..4 {
                    out[(yy * xsize + xx) * 4 + c] = clip8(ss[c]);
                }
            }
        }
        out
    }

    /// xorshift: made-up pictures that are the same every run
    fn rng(seed: u64) -> impl FnMut() -> u64 {
        let mut s = seed;
        move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        }
    }

    /// noise (runs of one pixel, any alpha), and blocks of a few colours with repeated rows and transparent black, as the
    /// shapes are drawn
    fn pictures(w: i32, h: i32) -> Vec<Img> {
        let mut r = rng(0x9e37_79b9_7f4a_7c15 ^ (w as u64) << 20 ^ h as u64);
        let mut noise = Img::new(w, h, 4, [0; 4]);
        noise.px.iter_mut().for_each(|v| *v = r() as u8);
        let inks: [[u8; 4]; 5] = [[0; 4], [24, 16, 40, 255], [255, 255, 255, 255], [255, 255, 255, 120], [124, 84, 214, 3]];
        let mut blocks = Img::new(w, h, 4, [0; 4]);
        for _ in 0..12 {
            let (x0, y0) = ((r() % w as u64) as i32, (r() % h as u64) as i32);
            let (x1, y1) = ((x0 + (r() % w as u64) as i32).min(w), (y0 + (r() % h as u64) as i32).min(h));
            let ink = inks[(r() % 5) as usize];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * w + x) * 4) as usize;
                    blocks.px[i..i + 4].copy_from_slice(&ink);
                }
            }
        }
        vec![noise, blocks]
    }

    #[test]
    fn runs_and_stretches_give_what_pillows_taps_give() {
        // the shapes' sizes, the preview sheet's, and odd ones both ways
        for (w, h, xsize, ysize) in [(192, 192, 24, 24), (768, 768, 96, 96), (512, 192, 768, 288), (101, 37, 33, 20), (40, 30, 97, 61)] {
            for im in pictures(w, h) {
                assert!(resample4(&im, xsize, ysize) == tap_by_tap(&im, xsize, ysize), "{w}x{h} to {xsize}x{ysize}");
            }
        }
    }
}
