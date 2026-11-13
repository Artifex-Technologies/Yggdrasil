//! The map: land and the time zone bands, from Natural Earth (public domain), simplified and baked into `data/world.bin` by
//! `examples/bake.rs` (see the README). The file is small on purpose: points in hundredths of a degree, each the difference to
//! the one before as a variable-length number.
//!
//! ```text
//! "YTZ1"
//! land:  count, then each ring: count of points, the points (x, y as zigzag differences)
//! zones: count, then each zone: its standard offset in minutes (zigzag), count of rings, the rings
//! ```

use std::sync::OnceLock;

/// Hundredths of a degree.
pub const UNIT: f32 = 0.01;

const MAGIC: &[u8; 4] = b"YTZ1";

/// A closed outline, in degrees (east, north), with its bounding box.
#[derive(Clone, Debug, PartialEq)]
pub struct Ring {
    pub pts: Vec<[f32; 2]>,
    /// min east, min north, max east, max north
    pub bbox: [f32; 4],
}

/// A time zone band of the map: one standard offset, all the places that keep it (land and sea).
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    /// Seconds east of UTC (standard time).
    pub offset: i32,
    pub rings: Vec<Ring>,
    pub bbox: [f32; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct World {
    pub land: Vec<Ring>,
    pub bands: Vec<Band>,
}

fn bbox(pts: &[[f32; 2]]) -> [f32; 4] {
    pts.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])])
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

impl Ring {
    pub fn new(pts: Vec<[f32; 2]>) -> Ring {
        Ring { bbox: bbox(&pts), pts }
    }

    /// How many times a ray from the point to the east crosses the outline (odd: inside).
    fn crossings(&self, x: f32, y: f32) -> usize {
        if x < self.bbox[0] || x > self.bbox[2] || y < self.bbox[1] || y > self.bbox[3] {
            return 0;
        }
        let mut n = 0;
        let pts = &self.pts;
        let mut j = pts.len().wrapping_sub(1);
        for i in 0..pts.len() {
            let ([xi, yi], [xj, yj]) = (pts[i], pts[j]);
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                n += 1;
            }
            j = i;
        }
        n
    }
}

impl World {
    /// The map that comes with the program.
    pub fn get() -> &'static World {
        static W: OnceLock<World> = OnceLock::new();
        W.get_or_init(|| World::decode(include_bytes!("../data/world.bin")).unwrap_or_default())
    }

    /// The standard offset (seconds) of the band a point is in, if any.
    pub fn offset_at(&self, lon: f32, lat: f32) -> Option<i32> {
        self.bands
            .iter()
            .filter(|b| lon >= b.bbox[0] && lon <= b.bbox[2] && lat >= b.bbox[1] && lat <= b.bbox[3])
            .find(|b| b.rings.iter().map(|r| r.crossings(lon, lat)).sum::<usize>() % 2 == 1)
            .map(|b| b.offset)
    }

    /// `land` and `bands` (standard offset in minutes, rings) in hundredths of a degree, as the file stores them.
    pub fn encode(land: &[Vec<[i32; 2]>], bands: &[(i32, Vec<Vec<[i32; 2]>>)]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        let ring = |out: &mut Vec<u8>, r: &[[i32; 2]]| {
            varint(out, r.len() as u64);
            let mut last = [0, 0];
            for p in r {
                varint(out, zigzag(p[0] - last[0]));
                varint(out, zigzag(p[1] - last[1]));
                last = *p;
            }
        };
        varint(&mut out, land.len() as u64);
        for r in land {
            ring(&mut out, r);
        }
        varint(&mut out, bands.len() as u64);
        for (minutes, rings) in bands {
            varint(&mut out, zigzag(*minutes));
            varint(&mut out, rings.len() as u64);
            for r in rings {
                ring(&mut out, r);
            }
        }
        out
    }

    pub fn decode(data: &[u8]) -> Option<World> {
        let mut r = Reader { data: data.strip_prefix(MAGIC)?, at: 0 };
        let ring = |r: &mut Reader| -> Option<Ring> {
            let n = r.count()?;
            let mut pts = Vec::with_capacity(n);
            let mut last = [0i32, 0i32];
            for _ in 0..n {
                last = [last[0] + unzigzag(r.varint()?), last[1] + unzigzag(r.varint()?)];
                pts.push([last[0] as f32 * UNIT, last[1] as f32 * UNIT]);
            }
            Some(Ring::new(pts))
        };
        let mut w = World::default();
        for _ in 0..r.count()? {
            w.land.push(ring(&mut r)?);
        }
        for _ in 0..r.count()? {
            let offset = unzigzag(r.varint()?) * 60;
            let rings = (0..r.count()?).map(|_| ring(&mut r)).collect::<Option<Vec<_>>>()?;
            let bbox = rings.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, r| union(b, r.bbox));
            w.bands.push(Band { offset, rings, bbox });
        }
        (r.at == r.data.len()).then_some(w)
    }
}

fn zigzag(v: i32) -> u64 {
    ((v << 1) ^ (v >> 31)) as u32 as u64
}

fn unzigzag(v: u64) -> i32 {
    let v = v as u32;
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self.data.get(self.at)?;
            self.at += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Some(v);
            }
        }
        None
    }

    /// A count, no bigger than the bytes left could hold (a damaged file cannot ask for a huge allocation).
    fn count(&mut self) -> Option<usize> {
        let n = self.varint()? as usize;
        (n <= self.data.len() - self.at).then_some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two bands side by side, -1 h west of 0 and +1 h east of it, with a hole in the east one; one square of land.
    fn sample() -> World {
        let sq = |x0: i32, y0: i32, x1: i32, y1: i32| vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        let data = World::encode(
            &[sq(-500, -500, 500, 500)],
            &[(-60, vec![sq(-1500, -9000, 0, 9000)]), (60, vec![sq(0, -9000, 1500, 9000), sq(500, 500, 1000, 1000)])],
        );
        World::decode(&data).unwrap()
    }

    #[test]
    fn the_file_reads_back_as_written() {
        let w = sample();
        assert_eq!(w.land.len(), 1);
        assert_eq!(w.land[0].pts[2], [5.0, 5.0]);
        assert_eq!(w.bands.iter().map(|b| b.offset).collect::<Vec<_>>(), [-3600, 3600]);
        assert_eq!(w.bands[1].bbox, [0.0, -90.0, 15.0, 90.0]);
        for v in [0, 1, -1, 12345, -12345, i32::MAX, i32::MIN] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        let good = World::encode(&[vec![[1, 2], [3, 4], [5, 6]]], &[]);
        assert!(World::decode(&good).is_some());
        assert_eq!(World::decode(&good[..good.len() - 1]), None, "cut short");
        assert_eq!(World::decode(b"YTZ1\xff\xff\xff\xff\x0f"), None, "a count the file cannot hold");
        assert_eq!(World::decode(b"PNG"), None);
    }

    #[test]
    fn a_point_is_in_the_band_around_it() {
        let w = sample();
        assert_eq!(w.offset_at(-7.0, 40.0), Some(-3600));
        assert_eq!(w.offset_at(3.0, -40.0), Some(3600));
        assert_eq!(w.offset_at(7.0, 7.0), None, "in the hole");
        assert_eq!(w.offset_at(20.0, 0.0), None, "outside every band");
    }

    #[test]
    fn the_baked_map_is_whole_and_small() {
        let data = include_bytes!("../data/world.bin");
        assert!(data.len() < 400_000, "{} bytes", data.len());
        let w = World::get();
        assert!(w.land.len() > 100 && w.bands.len() > 30);
        let at = |lon, lat| w.offset_at(lon, lat).map(|s| s as f32 / 3600.0);
        assert_eq!(at(-104.98, 39.74), Some(-7.0), "Denver");
        assert_eq!(at(-87.65, 41.85), Some(-6.0), "Chicago");
        assert_eq!(at(13.4, 52.5), Some(1.0), "Berlin");
        assert_eq!(at(88.37, 22.57), Some(5.5), "Kolkata");
        assert_eq!(at(-150.0, 0.0), Some(-10.0), "the Pacific, south of Hawaii");
    }
}
