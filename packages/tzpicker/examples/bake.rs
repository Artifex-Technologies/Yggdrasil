//! Bakes data/world.bin from Natural Earth's shapefiles (public domain, naturalearthdata.com):
//!
//!     cargo run --example bake -- COUNTRIES.shp TIME_ZONES.shp OUT.bin
//!
//! with ne_50m_admin_0_countries.shp and ne_10m_time_zones.shp (its .dbf next to it, for each band's offset). The outlines are
//! simplified (Douglas-Peucker, 0.04 degrees), rounded to hundredths of a degree, and land below 60 degrees south (Antarctica,
//! which the map does not show) and islands too small to see are left out. Only the shapefile parts used are read: polygons and
//! the dBase table's text and number fields.

use std::path::Path;
use yggdrasil_tzpicker::world::World;

/// Douglas-Peucker tolerance, degrees.
const TOLERANCE: f64 = 0.04;
/// Land smaller than this (square degrees) is left out: under a pixel even zoomed in.
const MIN_LAND: f64 = 0.004;

type Polygon = Vec<Vec<[f64; 2]>>;

fn le32(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn f64le(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Every record's polygon (its rings), in the file's order; a record of another kind is an empty polygon.
fn read_shp(path: &Path) -> Vec<Polygon> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out = vec![];
    let mut at = 100;
    while at + 8 <= b.len() {
        let len = u32::from_be_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize * 2;
        let rec = &b[at + 8..(at + 8 + len).min(b.len())];
        at += 8 + len;
        if le32(rec, 0) != Some(5) {
            out.push(vec![]);
            continue;
        }
        let (parts, points) = (le32(rec, 36).unwrap() as usize, le32(rec, 40).unwrap() as usize);
        let starts: Vec<usize> = (0..parts).map(|i| le32(rec, 44 + 4 * i).unwrap() as usize).chain([points]).collect();
        let base = 44 + 4 * parts;
        let pt = |i: usize| [f64le(rec, base + 16 * i).unwrap(), f64le(rec, base + 16 * i + 8).unwrap()];
        out.push(starts.windows(2).map(|w| (w[0]..w[1]).map(pt).collect()).collect());
    }
    out
}

/// One column of the dBase table, as text, for every record.
fn read_dbf(path: &Path, column: &str) -> Vec<String> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let n = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    let (header, rec) = (u16::from_le_bytes([b[8], b[9]]) as usize, u16::from_le_bytes([b[10], b[11]]) as usize);
    let (mut at, mut offset, mut found) = (32, 1, None);
    while b[at] != 0x0d {
        let name = String::from_utf8_lossy(&b[at..at + 11]).trim_end_matches('\0').to_string();
        let len = b[at + 16] as usize;
        if name == column {
            found = Some((offset, len));
        }
        offset += len;
        at += 32;
    }
    let (o, l) = found.unwrap_or_else(|| panic!("{}: no column {column}", path.display()));
    (0..n).map(|i| String::from_utf8_lossy(&b[header + i * rec + o..header + i * rec + o + l]).trim().to_string()).collect()
}

fn simplify(r: &[[f64; 2]]) -> Vec<[f64; 2]> {
    if r.len() < 4 {
        return r.to_vec();
    }
    let mut keep = vec![false; r.len()];
    keep[0] = true;
    // a closed ring: split at its farthest point from the start, so both halves have distinct ends
    let far = (1..r.len()).max_by(|&a, &b| d2(r[0], r[a]).total_cmp(&d2(r[0], r[b]))).unwrap();
    keep[far] = true;
    keep[r.len() - 1] = true;
    let mut stack = vec![(0, far), (far, r.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (mut best, mut at) = (0.0, 0);
        for i in a + 1..b {
            let d = seg_dist(r[i], r[a], r[b]);
            if d > best {
                (best, at) = (d, i);
            }
        }
        if best > TOLERANCE {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    r.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

fn d2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let l = d2(a, b);
    if l == 0.0 {
        return d2(p, a).sqrt();
    }
    let t = (((p[0] - a[0]) * (b[0] - a[0]) + (p[1] - a[1]) * (b[1] - a[1])) / l).clamp(0.0, 1.0);
    d2(p, [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]).sqrt()
}

fn area(r: &[[f64; 2]]) -> f64 {
    let mut s = 0.0;
    for i in 0..r.len() {
        let (a, b) = (r[i], r[(i + 1) % r.len()]);
        s += a[0] * b[1] - b[0] * a[1];
    }
    (s / 2.0).abs()
}

/// Simplified and rounded to hundredths of a degree; the closing point dropped (the map closes every ring); None when too little
/// is left.
fn bake_ring(r: &[[f64; 2]], min_area: f64) -> Option<Vec<[i32; 2]>> {
    if area(r) < min_area {
        return None;
    }
    let mut out: Vec<[i32; 2]> = vec![];
    for p in simplify(r) {
        let q = [(p[0] * 100.0).round() as i32, (p[1].clamp(-90.0, 90.0) * 100.0).round() as i32];
        if out.last() != Some(&q) {
            out.push(q);
        }
    }
    if out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    (out.len() >= 3).then_some(out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [countries, zones, out] = &args[..] else {
        eprintln!("usage: bake COUNTRIES.shp TIME_ZONES.shp OUT.bin");
        std::process::exit(2);
    };
    let land: Vec<Vec<[i32; 2]>> = read_shp(Path::new(countries))
        .iter()
        .flatten()
        .filter(|r| r.iter().any(|p| p[1] > -60.0))
        .filter_map(|r| bake_ring(r, MIN_LAND))
        .collect();
    let zones_path = Path::new(zones);
    let offsets = read_dbf(&zones_path.with_extension("dbf"), "zone");
    let mut bands: Vec<(i32, Vec<Vec<[i32; 2]>>)> = vec![];
    for (poly, off) in read_shp(zones_path).iter().zip(&offsets) {
        let minutes = (off.parse::<f64>().unwrap_or_else(|_| panic!("offset {off:?}")) * 60.0).round() as i32;
        let rings: Vec<Vec<[i32; 2]>> = poly.iter().filter_map(|r| bake_ring(r, 0.0)).collect();
        match bands.iter_mut().find(|(m, _)| *m == minutes) {
            Some((_, v)) => v.extend(rings),
            None => bands.push((minutes, rings)),
        }
    }
    bands.sort_by_key(|(m, _)| *m);
    let data = World::encode(&land, &bands);
    let pts = |v: &[Vec<[i32; 2]>]| v.iter().map(Vec::len).sum::<usize>();
    eprintln!(
        "land: {} rings, {} points; bands: {} offsets, {} rings, {} points; {} bytes",
        land.len(),
        pts(&land),
        bands.len(),
        bands.iter().map(|(_, r)| r.len()).sum::<usize>(),
        bands.iter().map(|(_, r)| pts(r)).sum::<usize>(),
        data.len()
    );
    World::decode(&data).expect("reads back");
    std::fs::write(out, data).unwrap_or_else(|e| panic!("{out}: {e}"));
}
