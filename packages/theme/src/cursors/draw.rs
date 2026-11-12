//! Pillow's drawing, for the cursors: ImageDraw's line (with joint="curve"), polygon, ellipse, pieslice, arc and
//! rounded_rectangle, as Pillow 13.0's ImageDraw.py and libImaging/Draw.c draw them on an image without blending (a
//! drawing sets pixels to the ink). Translated from Pillow's C and Python (MIT-CMU licence, see NOTICE-Pillow in this
//! folder) so every pixel is the one Pillow draws: C's float arithmetic is kept as float (f32) and its double as double,
//! in the same order, and its integer conversions as they were.

/// An image of 1 (mode "L") or 4 (mode "RGBA", bytes R G B A) bytes a pixel.
#[derive(Clone)]
pub struct Img {
    pub w: i32,
    pub h: i32,
    pub ps: usize,
    pub px: Vec<u8>,
}

pub type Ink = [u8; 4];

impl Img {
    pub fn new(w: i32, h: i32, ps: usize, fill: Ink) -> Img {
        let mut px = vec![0u8; w as usize * h as usize * ps];
        for p in px.chunks_mut(ps) {
            p.copy_from_slice(&fill[..ps]);
        }
        Img { w, h, ps, px }
    }

    fn point(&mut self, x: i32, y: i32, ink: &Ink) {
        if x >= 0 && x < self.w && y >= 0 && y < self.h {
            let i = (y as usize * self.w as usize + x as usize) * self.ps;
            self.px[i..i + self.ps].copy_from_slice(&ink[..self.ps]);
        }
    }

    /// hline8 / hline32: x0..=x1 on row y0, clipped
    fn hline(&mut self, mut x0: i32, y0: i32, mut x1: i32, ink: &Ink) {
        if y0 < 0 || y0 >= self.h {
            return;
        }
        if x0 < 0 {
            x0 = 0;
        } else if x0 >= self.w {
            return;
        }
        if x1 < 0 {
            return;
        } else if x1 >= self.w {
            x1 = self.w - 1;
        }
        let row = y0 as usize * self.w as usize;
        let mut x = x0;
        while x <= x1 {
            let i = (row + x as usize) * self.ps;
            self.px[i..i + self.ps].copy_from_slice(&ink[..self.ps]);
            x += 1;
        }
    }
}

/// ROUND_UP of a float: floor(f + 0.5F) away from zero (the addition in float)
fn round_up_f(f: f32) -> i32 {
    if f >= 0.0 {
        ((f + 0.5f32) as f64).floor() as i32
    } else {
        -((f.abs() + 0.5f32) as f64).floor() as i32
    }
}

/// ROUND_DOWN of a float
fn round_down_f(f: f32) -> i32 {
    if f >= 0.0 {
        ((f - 0.5f32) as f64).ceil() as i32
    } else {
        -((f.abs() - 0.5f32) as f64).ceil() as i32
    }
}

/// ROUND_UP of a double
fn round_up_d(f: f64) -> i32 {
    if f >= 0.0 {
        (f + 0.5).floor() as i32
    } else {
        -(f.abs() + 0.5).floor() as i32
    }
}

/// ROUND_DOWN of a double
fn round_down_d(f: f64) -> i32 {
    if f >= 0.0 {
        (f - 0.5).ceil() as i32
    } else {
        -(f.abs() - 0.5).ceil() as i32
    }
}

/// Draw.c's line(): Bresenham, the end point not drawn
pub fn line(im: &mut Img, mut x0: i32, mut y0: i32, x1: i32, y1: i32, ink: &Ink) {
    let mut dy = y1 - y0;
    let ys;
    if dy < 0 {
        dy = -dy;
        ys = -1;
    } else if dy == 0 {
        if x1 > x0 {
            im.hline(x0, y0, x1 - 1, ink);
        } else if x0 > x1 {
            im.hline(x1 + 1, y0, x0, ink);
        }
        return;
    } else {
        ys = 1;
    }
    let mut dx = x1 - x0;
    let xs;
    if dx < 0 {
        dx = -dx;
        xs = -1;
    } else {
        xs = 1;
    }
    if dx == 0 {
        for _ in 0..dy {
            im.point(x0, y0, ink);
            y0 += ys;
        }
    } else if dx > dy {
        let n = dx;
        dy += dy;
        let mut e = dy - dx;
        dx += dx;
        for _ in 0..n {
            im.point(x0, y0, ink);
            if e >= 0 {
                y0 += ys;
                e -= dx;
            }
            e += dy;
            x0 += xs;
        }
    } else {
        let n = dy;
        dx += dx;
        let mut e = dx - dy;
        dy += dy;
        for _ in 0..n {
            im.point(x0, y0, ink);
            if e >= 0 {
                x0 += xs;
                e -= dy;
            }
            e += dx;
            y0 += ys;
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Edge {
    x0: i32,
    y0: i32,
    xmin: i32,
    ymin: i32,
    xmax: i32,
    ymax: i32,
    dx: f32,
}

fn add_edge(x0: i32, y0: i32, x1: i32, y1: i32) -> Edge {
    let (xmin, xmax) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let (ymin, ymax) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    let dx = if y0 == y1 { 0.0 } else { (x1 - x0) as f32 / (y1 - y0) as f32 };
    Edge { x0, y0, xmin, ymin, xmax, ymax, dx }
}

/// polygon_generic without blending: the scan line fill
fn polygon_generic(im: &mut Img, e: &[Edge], ink: &Ink) {
    if e.is_empty() {
        return;
    }
    let mut ymin = im.h - 1;
    let mut ymax = 0;
    let mut table: Vec<Edge> = vec![];
    for ed in e {
        ymin = ymin.min(ed.ymin);
        ymax = ymax.max(ed.ymax);
        if ed.ymin == ed.ymax {
            im.hline(ed.xmin, ed.ymin, ed.xmax, ink);
            continue;
        }
        table.push(*ed);
    }
    if ymin < 0 {
        ymin = 0;
    }
    if ymax > im.h {
        ymax = im.h;
    }
    let at = |ed: &Edge, y: i32| -> f32 { (y - ed.y0) as f32 * ed.dx + ed.x0 as f32 };
    let mut xx: Vec<f32> = Vec::with_capacity(table.len() * 2);
    while ymin <= ymax {
        xx.clear();
        for i in 0..table.len() {
            let current = table[i];
            if ymin >= current.ymin && ymin <= current.ymax {
                xx.push(at(&current, ymin));
                if ymin == current.ymax && ymin < ymax {
                    // needed to draw consistent polygons
                    let last = *xx.last().unwrap();
                    xx.push(last);
                } else if (ymin == current.ymin || ymin == current.ymax) && current.dx != 0.0 {
                    // connect discontiguous corners
                    for other in &table[..i] {
                        if (ymin != other.ymin && ymin != other.ymax) || other.dx == 0.0 {
                            continue;
                        }
                        let last = *xx.last().unwrap();
                        if last.round() == at(other, ymin).round() {
                            let offset = if ymin == current.ymax { -1 } else { 1 };
                            let adjacent = at(&current, ymin + offset);
                            if ymin + offset >= other.ymin && ymin + offset <= other.ymax {
                                let adjacent_other = at(other, ymin + offset);
                                let n = xx.len();
                                if xx[n - 1] > adjacent + 1.0 && xx[n - 1] > adjacent_other + 1.0 {
                                    xx[n - 1] = ((adjacent as f64).max(adjacent_other as f64) as f32).round() + 1.0;
                                } else if xx[n - 1] < adjacent - 1.0 && xx[n - 1] < adjacent_other - 1.0 {
                                    xx[n - 1] = ((adjacent as f64).min(adjacent_other as f64) as f32).round() - 1.0;
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }
        xx.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut i = 1;
        while i < xx.len() {
            im.hline(round_up_f(xx[i - 1]), ymin, round_down_f(xx[i]), ink);
            i += 2;
        }
        ymin += 1;
    }
}

/// ImagingDrawWideLine: a four-cornered polygon around the segment
pub fn wide_line(im: &mut Img, x0: i32, y0: i32, x1: i32, y1: i32, ink: &Ink, width: i32) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    if dx == 0 && dy == 0 {
        im.point(x0, y0, ink);
        return;
    }
    let big = (dx as f64).hypot(dy as f64);
    let small = (width - 1) as f64 / 2.0;
    let ratio_max = round_up_d(small) as f64 / big;
    let ratio_min = round_down_d(small) as f64 / big;
    let dxmin = round_down_d(ratio_min * dy as f64);
    let dxmax = round_down_d(ratio_max * dy as f64);
    let dymin = round_down_d(ratio_min * dx as f64);
    let dymax = round_down_d(ratio_max * dx as f64);
    let v = [(x0 - dxmin, y0 + dymax), (x1 - dxmin, y1 + dymax), (x1 + dxmax, y1 - dymin), (x0 + dxmax, y0 - dymin)];
    let e = [
        add_edge(v[0].0, v[0].1, v[1].0, v[1].1),
        add_edge(v[1].0, v[1].1, v[2].0, v[2].1),
        add_edge(v[2].0, v[2].1, v[3].0, v[3].1),
        add_edge(v[3].0, v[3].1, v[0].0, v[0].1),
    ];
    polygon_generic(im, &e, ink);
}

/// ImagingDrawPolygon with fill
pub fn polygon_fill(im: &mut Img, xy: &[(i32, i32)], ink: &Ink) {
    let count = xy.len();
    if count == 0 {
        return;
    }
    let mut e: Vec<Edge> = vec![];
    let mut i = 0;
    while i < count - 1 {
        let (x0, y0) = xy[i];
        let (x1, y1) = xy[i + 1];
        if y0 == y1 && i != 0 && y0 == xy[i - 1].1 {
            // a horizontal line straight after another horizontal line
            let last = e.len() - 1;
            if x1 > x0 && x0 > xy[i - 1].0 {
                e[last].xmax = x1;
                i += 1;
                continue;
            } else if x1 < x0 && x0 < xy[i - 1].0 {
                e[last].xmin = x1;
                i += 1;
                continue;
            }
        }
        e.push(add_edge(x0, y0, x1, y1));
        i += 1;
    }
    if xy[i] != xy[0] {
        e.push(add_edge(xy[i].0, xy[i].1, xy[0].0, xy[0].1));
    }
    polygon_generic(im, &e, ink);
}

/// ImagingDrawRectangle with fill
pub fn rectangle_fill(im: &mut Img, x0: i32, mut y0: i32, x1: i32, mut y1: i32, ink: &Ink) {
    if y0 > y1 {
        std::mem::swap(&mut y0, &mut y1);
    }
    if y0 < 0 {
        y0 = 0;
    } else if y0 >= im.h {
        return;
    }
    if y1 < 0 {
        return;
    } else if y1 > im.h {
        y1 = im.h;
    }
    for y in y0..=y1 {
        im.hline(x0, y, x1, ink);
    }
}

// ---------------------------------------------------------------- ellipses (quarter_*, ellipse_*, the clip tree)

struct Quarter {
    a: i32,
    b: i32,
    cx: i32,
    cy: i32,
    ex: i32,
    ey: i32,
    a2: i64,
    b2: i64,
    a2b2: i64,
    finished: bool,
}

impl Quarter {
    fn new(a: i32, b: i32) -> Quarter {
        if a < 0 || b < 0 {
            return Quarter { a, b, cx: 0, cy: 0, ex: 0, ey: 0, a2: 0, b2: 0, a2b2: 0, finished: true };
        }
        let a2 = a as i64 * a as i64;
        let b2 = b as i64 * b as i64;
        Quarter { a, b, cx: a, cy: b % 2, ex: a % 2, ey: b, a2, b2, a2b2: a2 * b2, finished: false }
    }

    fn delta(&self, x: i64, y: i64) -> i64 {
        (self.a2 * y * y + self.b2 * x * x - self.a2b2).abs()
    }

    fn next(&mut self) -> Option<(i32, i32)> {
        if self.finished {
            return None;
        }
        let ret = (self.cx, self.cy);
        if self.cx == self.ex && self.cy == self.ey {
            self.finished = true;
        } else {
            let mut nx = self.cx;
            let mut ny = self.cy + 2;
            let mut ndelta = self.delta(nx as i64, ny as i64);
            if nx > 1 {
                let newdelta = self.delta((self.cx - 2) as i64, (self.cy + 2) as i64);
                if ndelta > newdelta {
                    nx = self.cx - 2;
                    ny = self.cy + 2;
                    ndelta = newdelta;
                }
                let newdelta = self.delta((self.cx - 2) as i64, self.cy as i64);
                if ndelta > newdelta {
                    nx = self.cx - 2;
                    ny = self.cy;
                }
            }
            self.cx = nx;
            self.cy = ny;
        }
        let _ = (self.a, self.b);
        Some(ret)
    }
}

struct Ellipse {
    st_o: Quarter,
    st_i: Quarter,
    py: i32,
    pl: i32,
    pr: i32,
    buf: Vec<(i32, i32, i32)>,
    finished: bool,
    leftmost: i32,
}

impl Ellipse {
    fn new(a: i32, b: i32, w: i32) -> Ellipse {
        let mut st_o = Quarter::new(a, b);
        let leftmost = a % 2;
        let first = if w < 1 { None } else { st_o.next() };
        match first {
            None => Ellipse { st_o, st_i: Quarter::new(-1, -1), py: 0, pl: 0, pr: 0, buf: vec![], finished: true, leftmost },
            Some((pr, py)) => Ellipse {
                st_o,
                st_i: Quarter::new(a - 2 * (w - 1), b - 2 * (w - 1)),
                py,
                pl: leftmost,
                pr,
                buf: vec![],
                finished: false,
                leftmost,
            },
        }
    }

    /// (x0, y, x1) on the grid of steps of 2
    fn next(&mut self) -> Option<(i32, i32, i32)> {
        if self.buf.is_empty() {
            if self.finished {
                return None;
            }
            let y = self.py;
            let mut l = self.pl;
            let r = self.pr;
            let mut c;
            loop {
                c = self.st_o.next();
                match c {
                    Some((_, cy)) if cy <= y => continue,
                    _ => break,
                }
            }
            match c {
                None => self.finished = true,
                Some((cx, cy)) => {
                    self.pr = cx;
                    self.py = cy;
                }
            }
            let mut ci;
            loop {
                ci = self.st_i.next();
                match ci {
                    Some((cx, cy)) if cy <= y => l = cx,
                    _ => break,
                }
            }
            self.pl = match ci {
                None => self.leftmost,
                Some((cx, _)) => cx,
            };
            if (l > 0 || l < r) && y > 0 {
                self.buf.push((if l == 0 { 2 } else { l }, y, r));
            }
            if y > 0 {
                self.buf.push((-r, y, -l));
            }
            if l > 0 || l < r {
                self.buf.push((if l == 0 { 2 } else { l }, -y, r));
            }
            self.buf.push((-r, -y, -l));
        }
        self.buf.pop()
    }
}

#[derive(Clone)]
enum Clip {
    And(Box<Clip>, Box<Clip>),
    Or(Box<Clip>, Box<Clip>),
    /// a x + b y + c >= 0
    Half(f64, f64, f64),
}

impl Clip {
    fn transpose(&mut self) {
        match self {
            Clip::Half(a, b, _) => std::mem::swap(a, b),
            Clip::And(l, r) | Clip::Or(l, r) => {
                l.transpose();
                r.transpose();
            }
        }
    }

    /// clip_tree_do_clip: the ends of the clipped segments, (x, 1) opening and (x, -1) closing
    fn clip(&self, x0: i32, y: i32, x1: i32) -> Vec<(i32, i8)> {
        match self {
            Clip::Half(a, b, c) => {
                let (mut x0, mut x1) = (x0, x1);
                let eps = 1e-9;
                if a.abs() < eps {
                    if b * y as f64 + c < -eps {
                        x0 = 1;
                        x1 = 0;
                    }
                } else {
                    let ix = -(b * y as f64 + c) / a;
                    if a * x0 as f64 + b * y as f64 + c < eps {
                        x0 = (x0 as f64).max(ix).round() as i32;
                    }
                    if a * x1 as f64 + b * y as f64 + c < eps {
                        x1 = (x1 as f64).min(ix).round() as i32;
                    }
                }
                if x0 <= x1 {
                    vec![(x0, 1), (x1, -1)]
                } else {
                    vec![]
                }
            }
            Clip::And(l, r) | Clip::Or(l, r) => {
                let or = matches!(self, Clip::Or(..));
                let l1 = l.clip(x0, y, x1);
                let l2 = r.clip(x0, y, x1);
                let (mut i, mut j) = (0, 0);
                let (mut k1, mut k2) = (0i32, 0i32);
                let mut out: Vec<(i32, i8)> = vec![];
                while i < l1.len() || j < l2.len() {
                    let t;
                    if j >= l2.len() || (i < l1.len() && (l1[i].0 < l2[j].0 || (l1[i].0 == l2[j].0 && l1[i].1 > l2[j].1))) {
                        t = l1[i];
                        k1 += t.1 as i32;
                        i += 1;
                    } else {
                        t = l2[j];
                        k2 += t.1 as i32;
                        j += 1;
                    }
                    let tail = out.last().map(|e| e.1);
                    let keep = if or {
                        (t.1 == 1 && (tail.is_none() || tail == Some(-1))) || (t.1 == -1 && k1 == 0 && k2 == 0)
                    } else {
                        (t.1 == 1 && (tail.is_none() || tail == Some(-1)) && k1 > 0 && k2 > 0)
                            || (t.1 == -1 && tail == Some(1) && (k1 == 0 || k2 == 0))
                    };
                    if keep {
                        out.push(t);
                    }
                }
                out
            }
        }
    }
}

/// normalize_angles: 0 <= al < 360, al <= ar <= al + 360 (in float, through double fmod as in C)
fn normalize_angles(al: &mut f32, ar: &mut f32) {
    if *ar - *al >= 360.0 {
        *al = 0.0;
        *ar = 360.0;
    } else {
        let x = if *al < 0.0 { 360.0 - ((-*al) as f64 % 360.0) } else { *al as f64 };
        *al = (x % 360.0) as f32;
        let y = if *ar < *al { 360.0 - ((*al - *ar) as f64 % 360.0) } else { (*ar - *al) as f64 };
        *ar = (*al as f64 + y % 360.0) as f32;
    }
}

fn rad(deg: f32, div: f64) -> f64 {
    deg as f64 * std::f64::consts::PI / div
}

/// arc_init: an arc with caps square to the curve
fn arc_clip(a: i32, b: i32, al: f32, ar: f32) -> Option<Clip> {
    if a < b {
        let mut c = arc_clip(b, a, 90.0 - ar, 90.0 - al);
        if let Some(c) = c.as_mut() {
            c.transpose();
        }
        return c;
    }
    let (mut al, mut ar) = (al, ar);
    normalize_angles(&mut al, &mut ar);
    if ar == al + 360.0 {
        return None;
    }
    let (af, bf) = (a as f64, b as f64);
    let lc = Clip::Half((-a) as f64 * rad(al, 180.0).sin(), bf * rad(al, 180.0).cos(), (a * a - b * b) as f64 * rad(al, 90.0).sin() / 2.0);
    let rc = Clip::Half(af * rad(ar, 180.0).sin(), (-b) as f64 * rad(ar, 180.0).cos(), (b * b - a * a) as f64 * rad(ar, 90.0).sin() / 2.0);
    let small = ar - al < 180.0;
    let combine = |l: Clip, r: Clip| {
        if small {
            Clip::And(Box::new(l), Box::new(r))
        } else {
            Clip::Or(Box::new(l), Box::new(r))
        }
    };
    if al as f64 % 180.0 == 0.0 || ar as f64 % 180.0 == 0.0 {
        Some(combine(lc, rc))
    } else if ((al / 180.0) as i32 + (ar / 180.0) as i32) % 2 == 1 {
        let lb = if (al / 180.0) as i32 % 2 == 0 { 1.0 } else { -1.0 };
        let rb = if (ar / 180.0) as i32 % 2 == 0 { 1.0 } else { -1.0 };
        Some(Clip::Or(
            Box::new(Clip::And(Box::new(Clip::Half(0.0, lb, 0.0)), Box::new(lc))),
            Box::new(Clip::And(Box::new(Clip::Half(0.0, rb, 0.0)), Box::new(rc))),
        ))
    } else {
        let half = Clip::Half(0.0, if ar < 180.0 || ar > 540.0 { 1.0 } else { -1.0 }, 0.0);
        Some(combine(combine(lc, rc), half))
    }
}

/// pie_init: the two sides of a pie, and a spike guard below 90 degrees
fn pie_clip(a: i32, b: i32, al: f32, ar: f32) -> Clip {
    let xl = a as f64 * rad(al, 180.0).cos();
    let xr = a as f64 * rad(ar, 180.0).cos();
    let yl = b as f64 * rad(al, 180.0).sin();
    let yr = b as f64 * rad(ar, 180.0).sin();
    let lc = Clip::Half(-yl, xl, 0.0);
    let rc = Clip::Half(yr, -xr, 0.0);
    let root = if ar - al < 180.0 { Clip::And(Box::new(lc), Box::new(rc)) } else { Clip::Or(Box::new(lc), Box::new(rc)) };
    if ar - al < 90.0 {
        Clip::And(Box::new(root), Box::new(Clip::Half((xl + xr) / 2.0, (yl + yr) / 2.0, 0.0)))
    } else {
        root
    }
}

/// ellipseNew: an ellipse in the box, filled or an outline `width` wide
#[allow(clippy::too_many_arguments)] // the C function's arguments
fn ellipse_new(im: &mut Img, x0: i32, y0: i32, x1: i32, y1: i32, ink: &Ink, fill: bool, mut width: i32) {
    let a = x1 - x0;
    let b = y1 - y0;
    if a < 0 || b < 0 {
        return;
    }
    if fill {
        width = a + b;
    }
    let mut st = Ellipse::new(a, b, width);
    while let Some((xa, y, xb)) = st.next() {
        im.hline(x0 + (xa + a) / 2, y0 + (y + b) / 2, x0 + (xb + a) / 2, ink);
    }
}

/// clipEllipseNew: an ellipse's rows through a clip tree
#[allow(clippy::too_many_arguments)] // the C function's arguments
fn clip_ellipse_new(im: &mut Img, x0: i32, y0: i32, x1: i32, y1: i32, ink: &Ink, width: i32, clip: Option<Clip>) {
    let a = x1 - x0;
    let b = y1 - y0;
    if a < 0 || b < 0 {
        return;
    }
    let mut st = Ellipse::new(a, b, width);
    while let Some((xa, y, xb)) = st.next() {
        let events = match &clip {
            Some(c) => c.clip(xa, y, xb),
            None => vec![(xa, 1), (xb, -1)],
        };
        for pair in events.chunks(2) {
            im.hline(x0 + (pair[0].0 + a) / 2, y0 + (y + b) / 2, x0 + (pair[1].0 + a) / 2, ink);
        }
    }
}

/// ImagingDrawEllipse
#[allow(clippy::too_many_arguments)] // the C function's arguments
pub fn ellipse(im: &mut Img, x0: i32, y0: i32, x1: i32, y1: i32, ink: &Ink, fill: bool, width: i32) {
    ellipse_new(im, x0, y0, x1, y1, ink, fill, width);
}

/// ImagingDrawArc
#[allow(clippy::too_many_arguments)] // the C function's arguments
pub fn arc(im: &mut Img, x0: i32, y0: i32, x1: i32, y1: i32, mut start: f32, mut end: f32, ink: &Ink, width: i32) {
    normalize_angles(&mut start, &mut end);
    if start + 360.0 == end {
        ellipse_new(im, x0, y0, x1, y1, ink, false, width);
        return;
    }
    if start == end {
        return;
    }
    let (a, b) = (x1 - x0, y1 - y0);
    if a < 0 || b < 0 {
        return;
    }
    clip_ellipse_new(im, x0, y0, x1, y1, ink, width, arc_clip(a, b, start, end));
}

/// ImagingDrawPieslice with fill
#[allow(clippy::too_many_arguments)] // the C function's arguments
pub fn pieslice_fill(im: &mut Img, x0: i32, y0: i32, x1: i32, y1: i32, mut start: f32, mut end: f32, ink: &Ink) {
    normalize_angles(&mut start, &mut end);
    if start + 360.0 == end {
        ellipse_new(im, x0, y0, x1, y1, ink, true, 0);
        return;
    }
    if start == end {
        return;
    }
    let (a, b) = (x1 - x0, y1 - y0);
    if a < 0 || b < 0 {
        return;
    }
    clip_ellipse_new(im, x0, y0, x1, y1, ink, x1 + y1 - x0 - y0, Some(pie_clip(a, b, start, end)));
}

// ---------------------------------------------------------------- ImageDraw (Python): the calls the cursors make

/// The coordinates C gets from Python floats: (int) truncation
fn t(v: f64) -> i32 {
    v as i32
}

/// A failure where Pillow raised (a box with x1 < x0, ...): the script stopped with a traceback.
#[derive(Debug)]
pub struct DrawError(pub &'static str);

fn check_box(b: [f64; 4]) -> Result<(), DrawError> {
    if b[2] < b[0] {
        return Err(DrawError("x1 must be greater than or equal to x0"));
    }
    if b[3] < b[1] {
        return Err(DrawError("y1 must be greater than or equal to y0"));
    }
    Ok(())
}

/// ImageDraw.ellipse(box, fill=ink)
pub fn d_ellipse_fill(im: &mut Img, b: [f64; 4], ink: &Ink) -> Result<(), DrawError> {
    check_box(b)?;
    ellipse(im, t(b[0]), t(b[1]), t(b[2]), t(b[3]), ink, true, 0);
    Ok(())
}

/// ImageDraw.ellipse(box, outline=ink, width=width)
pub fn d_ellipse_outline(im: &mut Img, b: [f64; 4], ink: &Ink, width: i32) -> Result<(), DrawError> {
    check_box(b)?;
    if width != 0 {
        ellipse(im, t(b[0]), t(b[1]), t(b[2]), t(b[3]), ink, false, width);
    }
    Ok(())
}

/// ImageDraw.pieslice(box, start, end, fill=ink)
pub fn d_pieslice_fill(im: &mut Img, b: [f64; 4], start: f64, end: f64, ink: &Ink) -> Result<(), DrawError> {
    check_box(b)?;
    pieslice_fill(im, t(b[0]), t(b[1]), t(b[2]), t(b[3]), start as f32, end as f32, ink);
    Ok(())
}

/// ImageDraw.arc(box, start, end, fill=ink, width=width)
pub fn d_arc(im: &mut Img, b: [f64; 4], start: f64, end: f64, ink: &Ink, width: i32) -> Result<(), DrawError> {
    if width == 0 {
        return Ok(());
    }
    check_box(b)?;
    arc(im, t(b[0]), t(b[1]), t(b[2]), t(b[3]), start as f32, end as f32, ink, width);
    Ok(())
}

/// ImageDraw.polygon(points, fill=ink)
pub fn d_polygon_fill(im: &mut Img, pts: &[(f64, f64)], ink: &Ink) {
    let xy: Vec<(i32, i32)> = pts.iter().map(|&(x, y)| (t(x), t(y))).collect();
    polygon_fill(im, &xy, ink);
}

/// ImageDraw.rectangle(box, fill=ink)
pub fn d_rectangle_fill(im: &mut Img, b: [f64; 4], ink: &Ink) -> Result<(), DrawError> {
    check_box(b)?;
    rectangle_fill(im, t(b[0]), t(b[1]), t(b[2]), t(b[3]), ink);
    Ok(())
}

/// draw_lines: the segments (one pixel wide, or wide lines)
fn draw_lines(im: &mut Img, pts: &[(f64, f64)], ink: &Ink, width: i32) {
    if width == 1 {
        for s in pts.windows(2) {
            line(im, t(s[0].0), t(s[0].1), t(s[1].0), t(s[1].1), ink);
        }
        if let Some(p) = pts.last().filter(|_| pts.len() > 1) {
            im.point(t(p.0), t(p.1), ink);
        }
    } else {
        for s in pts.windows(2) {
            wide_line(im, t(s[0].0), t(s[0].1), t(s[1].0), t(s[1].1), ink, width);
        }
    }
}

/// Python's x % 360 of a float (the sign of the divisor)
fn pymod(x: f64, m: f64) -> f64 {
    let r = x % m;
    if r == 0.0 {
        0.0f64.copysign(m)
    } else if (r < 0.0) != (m < 0.0) {
        r + m
    } else {
        r
    }
}

/// ImageDraw.line(points, fill=ink, width=width, joint=None or "curve")
pub fn d_line(im: &mut Img, pts: &[(f64, f64)], ink: &Ink, width: i32, curve: bool) -> Result<(), DrawError> {
    if width == 0 {
        return Ok(());
    }
    draw_lines(im, pts, ink, width);
    if !(curve && width > 4) {
        return Ok(());
    }
    let w = width as f64;
    for i in 1..pts.len().saturating_sub(1) {
        let point = pts[i];
        let angle = |start: (f64, f64), end: (f64, f64)| pymod((end.0 - start.0).atan2(start.1 - end.1).to_degrees(), 360.0);
        let angles = [angle(pts[i - 1], point), angle(point, pts[i + 1])];
        if angles[0] == angles[1] {
            // a straight line, no joint
            continue;
        }
        let coord_at_angle = |c: (f64, f64), angle: f64| -> (f64, f64) {
            let a = angle - 90.0;
            let distance = w / 2.0 - 1.0;
            let step = |p: f64, pd: f64| p + if pd > 0.0 { pd.floor() } else { pd.ceil() };
            (step(c.0, distance * a.to_radians().cos()), step(c.1, distance * a.to_radians().sin()))
        };
        let flipped = (angles[1] > angles[0] && angles[1] - 180.0 > angles[0]) || (angles[1] < angles[0] && angles[1] + 180.0 > angles[0]);
        let coords = [point.0 - w / 2.0 + 1.0, point.1 - w / 2.0 + 1.0, point.0 + w / 2.0 - 1.0, point.1 + w / 2.0 - 1.0];
        let (start, end) = if flipped { (angles[1] + 90.0, angles[0] + 90.0) } else { (angles[0] - 90.0, angles[1] - 90.0) };
        d_pieslice_fill(im, coords, start - 90.0, end - 90.0, ink)?;
        if width > 8 {
            // cover the gaps between the line and the joint
            let gap = if flipped {
                [coord_at_angle(point, angles[0] + 90.0), point, coord_at_angle(point, angles[1] + 90.0)]
            } else {
                [coord_at_angle(point, angles[0] - 90.0), point, coord_at_angle(point, angles[1] - 90.0)]
            };
            d_line(im, &gap, ink, 3, false)?;
        }
    }
    Ok(())
}

/// Python's round() of a float to a whole number (halves to even)
fn pyround(x: f64) -> f64 {
    x.round_ties_even()
}

/// ImageDraw.rounded_rectangle(box, radius, fill=ink) with all four corners
pub fn d_rounded_rectangle_fill(im: &mut Img, b: [f64; 4], radius: f64, ink: &Ink) -> Result<(), DrawError> {
    let [fx0, fy0, fx1, fy1] = b;
    if fx1 < fx0 {
        return Err(DrawError("x1 must be greater than or equal to x0"));
    }
    if fy1 < fy0 {
        return Err(DrawError("y1 must be greater than or equal to y0"));
    }
    let mut d = (fx1 - fx0).min(fy1 - fy0).min(radius * 2.0);
    // Python's min() keeps the first of equal values; for floats that is the same number
    let x0 = pyround(fx0);
    let y0 = pyround(fy0);
    let x1 = pyround(fx1);
    let y1 = pyround(fy1);
    let full_x = d >= x1 - x0 - 1.0;
    if full_x {
        d = x1 - x0;
    }
    let full_y = d >= y1 - y0 - 1.0;
    if full_y {
        d = y1 - y0;
    }
    if full_x && full_y {
        return d_ellipse_fill(im, b, ink);
    }
    if d == 0.0 {
        return d_rectangle_fill(im, b, ink);
    }
    let r = (d / 2.0).floor();
    let parts: Vec<([f64; 4], f64, f64)> = if full_x {
        vec![([x0, y0, x0 + d, y0 + d], 180.0, 360.0), ([x0, y1 - d, x0 + d, y1], 0.0, 180.0)]
    } else if full_y {
        vec![([x0, y0, x0 + d, y0 + d], 90.0, 270.0), ([x1 - d, y0, x1, y0 + d], 270.0, 90.0)]
    } else {
        vec![
            ([x0, y0, x0 + d, y0 + d], 180.0, 270.0),
            ([x1 - d, y0, x1, y0 + d], 270.0, 360.0),
            ([x1 - d, y1 - d, x1, y1], 0.0, 90.0),
            ([x0, y1 - d, x0 + d, y1], 90.0, 180.0),
        ]
    };
    for (bx, s, e) in parts {
        d_pieslice_fill(im, bx, s, e, ink)?;
    }
    if full_x {
        d_rectangle_fill(im, [x0, y0 + r + 1.0, x1, y1 - r - 1.0], ink)?;
    } else if x1 - r - 1.0 >= x0 + r + 1.0 {
        d_rectangle_fill(im, [x0 + r + 1.0, y0, x1 - r - 1.0, y1], ink)?;
    }
    if !full_x && !full_y {
        d_rectangle_fill(im, [x0, y0 + r + 1.0, x0 + r, y1 - (r + 1.0)], ink)?;
        d_rectangle_fill(im, [x1 - r, y0 + r + 1.0, x1, y1 - (r + 1.0)], ink)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_normalise_as_in_c() {
        let (mut a, mut b) = (-90.0f32, 10.0f32);
        normalize_angles(&mut a, &mut b);
        assert_eq!((a, b), (270.0, 370.0));
        let (mut a, mut b) = (0.0f32, 720.0f32);
        normalize_angles(&mut a, &mut b);
        assert_eq!((a, b), (0.0, 360.0));
    }

    #[test]
    fn a_filled_ellipse_is_symmetric() {
        let mut im = Img::new(20, 20, 1, [0; 4]);
        ellipse(&mut im, 2, 2, 17, 17, &[255, 0, 0, 0], true, 0);
        for y in 0..20 {
            for x in 0..20 {
                assert_eq!(im.px[y * 20 + x], im.px[y * 20 + (19 - x)]);
            }
        }
        assert_eq!(im.px[10 * 20 + 2], 255);
        assert_eq!(im.px[10 * 20 + 1], 0);
    }
}
