//! Yggdrasil's Color Picker, the parts without a screen: the formats a colour is copied in, the history of picked colours, the
//! format chosen in Settings, and where the loupe goes. The picker itself is `main.rs`; Settings uses this library for its
//! Color Picker section.
//!
//!   ~/.config/yggdrasil/tools.conf       `color_format = hex|rgb|hsl|hsv|cmyk|float` (other tools' lines are kept)
//!   ~/.local/state/yggdrasil/colors.txt  the last picks, newest first, one `#RRGGBB` per line

use std::path::{Path, PathBuf};

pub type Rgb = (u8, u8, u8);

/// How a picked colour is copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Hex,
    Rgb,
    Hsl,
    Hsv,
    Cmyk,
    Float,
}

impl Format {
    pub const ALL: [Format; 6] = [Format::Hex, Format::Rgb, Format::Hsl, Format::Hsv, Format::Cmyk, Format::Float];

    pub fn id(self) -> &'static str {
        match self {
            Format::Hex => "hex",
            Format::Rgb => "rgb",
            Format::Hsl => "hsl",
            Format::Hsv => "hsv",
            Format::Cmyk => "cmyk",
            Format::Float => "float",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Hex => "HEX",
            Format::Rgb => "RGB",
            Format::Hsl => "HSL",
            Format::Hsv => "HSV",
            Format::Cmyk => "CMYK",
            Format::Float => "Float",
        }
    }

    pub fn parse(s: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.id() == s.trim().to_ascii_lowercase())
    }

    /// The colour written in this format.
    pub fn text(self, c: Rgb) -> String {
        let (r, g, b) = c;
        match self {
            Format::Hex => hex(c),
            Format::Rgb => format!("rgb({r}, {g}, {b})"),
            Format::Hsl => {
                let (h, s, l) = hsl(c);
                format!("hsl({h}, {s}%, {l}%)")
            }
            Format::Hsv => {
                let (h, s, v) = hsv(c);
                format!("hsv({h}, {s}%, {v}%)")
            }
            Format::Cmyk => {
                let (c_, m, y, k) = cmyk(c);
                format!("cmyk({c_}%, {m}%, {y}%, {k}%)")
            }
            Format::Float => format!("({:.2}f, {:.2}f, {:.2}f, 1f)", r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0),
        }
    }
}

pub fn hex(c: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2)
}

/// `#RRGGBB` (or `RRGGBB`, either case).
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some((p(0)?, p(2)?, p(4)?))
}

/// Hue in degrees and the largest and smallest channel (0-1), shared by HSL and HSV.
fn hue(c: Rgb) -> (f64, f64, f64) {
    let (r, g, b) = (c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, max, min)
}

fn round(x: f64) -> u32 {
    x.round() as u32
}

/// Hue (0-359), saturation and lightness (percent), rounded as PowerToys shows them.
pub fn hsl(c: Rgb) -> (u32, u32, u32) {
    let (h, max, min) = hue(c);
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    (round(h) % 360, round(s * 100.0), round(l * 100.0))
}

pub fn hsv(c: Rgb) -> (u32, u32, u32) {
    let (h, max, min) = hue(c);
    let s = if max == 0.0 { 0.0 } else { (max - min) / max };
    (round(h) % 360, round(s * 100.0), round(max * 100.0))
}

pub fn cmyk(c: Rgb) -> (u32, u32, u32, u32) {
    let (r, g, b) = (c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0);
    let k = 1.0 - r.max(g).max(b);
    if k >= 1.0 {
        return (0, 0, 0, 100);
    }
    let f = |x: f64| round((1.0 - x - k) / (1.0 - k) * 100.0);
    (f(r), f(g), f(b), round(k * 100.0))
}

// ---------------------------------------------------------------- the chosen format

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
}

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state"))
}

pub fn tools_conf() -> PathBuf {
    config_dir().join("yggdrasil/tools.conf")
}

/// `color_format` from a tools.conf text, if it names a format.
pub fn format_in(text: &str) -> Option<Format> {
    text.lines().find_map(|l| {
        let (k, v) = l.split('#').next()?.split_once('=')?;
        (k.trim() == "color_format").then(|| Format::parse(v)).flatten()
    })
}

/// The format chosen in Settings: Yggdrasil's tools.conf, else the earlier Labyrinth one, else HEX.
pub fn chosen_format() -> Format {
    [tools_conf(), config_dir().join("labyrinth/tools.conf")]
        .iter()
        .find_map(|p| format_in(&std::fs::read_to_string(p).ok()?))
        .unwrap_or(Format::Hex)
}

/// tools.conf's text with `color_format` set, every other line kept as it was.
pub fn with_format(text: &str, f: Format) -> String {
    let mut out = String::new();
    let mut done = false;
    for l in text.lines() {
        let key = l.split('#').next().and_then(|x| x.split_once('=')).map(|(k, _)| k.trim());
        if key == Some("color_format") {
            if !done {
                out.push_str(&format!("color_format = {}\n", f.id()));
                done = true;
            }
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !done {
        out.push_str(&format!("color_format = {}\n", f.id()));
    }
    out
}

pub fn save_format(f: Format) -> std::io::Result<()> {
    let p = tools_conf();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text = with_format(&std::fs::read_to_string(&p).unwrap_or_default(), f);
    let tmp = p.with_extension("conf.colorpicker.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, p)
}

// ---------------------------------------------------------------- history

/// How many picks are kept.
pub const HISTORY_MAX: usize = 20;

pub fn history_path() -> PathBuf {
    state_dir().join("yggdrasil/colors.txt")
}

/// The picks in a history file's text: valid colours only, newest first, each once, at most HISTORY_MAX.
pub fn parse_history(text: &str) -> Vec<Rgb> {
    let mut out: Vec<Rgb> = vec![];
    for c in text.lines().filter_map(parse_hex) {
        if !out.contains(&c) && out.len() < HISTORY_MAX {
            out.push(c);
        }
    }
    out
}

/// The history with `c` added at the top (moved there if it was already in it).
pub fn add_to(history: &[Rgb], c: Rgb) -> Vec<Rgb> {
    let mut out = vec![c];
    out.extend(history.iter().copied().filter(|x| *x != c));
    out.truncate(HISTORY_MAX);
    out
}

fn read_history_at(p: &Path) -> Option<Vec<Rgb>> {
    std::fs::read_to_string(p).ok().map(|t| parse_history(&t))
}

/// The history, carried over from Labyrinth's earlier picker the first time.
pub fn load_history() -> Vec<Rgb> {
    read_history_at(&history_path()).or_else(|| read_history_at(&state_dir().join("labyrinth/colors.txt"))).unwrap_or_default()
}

pub fn save_history(h: &[Rgb]) -> std::io::Result<()> {
    let p = history_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text: String = h.iter().map(|c| hex(*c) + "\n").collect();
    let tmp = p.with_extension("txt.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, p)
}

pub fn remember(c: Rgb) -> std::io::Result<()> {
    save_history(&add_to(&load_history(), c))
}

// ---------------------------------------------------------------- the loupe

/// The loupe: `cells` x `cells` screen pixels around the pointer, each drawn `cell` pixels wide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loupe {
    pub cells: u32,
    pub cell: u32,
}

/// The zoom steps the wheel moves between (pixels shown across, always odd so one sits in the middle).
pub const ZOOMS: [u32; 6] = [5, 7, 9, 11, 15, 21];

impl Loupe {
    /// The loupe for zoom step `z` (an index into ZOOMS), about `size` pixels across.
    pub fn new(z: usize, size: u32) -> Loupe {
        let cells = ZOOMS[z.min(ZOOMS.len() - 1)];
        Loupe { cells, cell: (size / cells).max(2) }
    }

    pub fn side(&self) -> u32 {
        self.cells * self.cell
    }

    /// The screen pixel each cell shows, for a pointer on pixel (px, py); cells off the screen are None.
    pub fn source(&self, px: i32, py: i32, w: u32, h: u32) -> Vec<Option<(u32, u32)>> {
        let half = (self.cells / 2) as i32;
        let mut v = Vec::with_capacity((self.cells * self.cells) as usize);
        for dy in -half..=half {
            for dx in -half..=half {
                let (x, y) = (px + dx, py + dy);
                v.push((x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h).then_some((x as u32, y as u32)));
            }
        }
        v
    }
}

/// Where a box of `bw` x `bh` goes next to the pointer at (px, py) on a `w` x `h` screen: below and to the right, flipped to the
/// other side where it would leave the screen, and kept on it.
pub fn place(px: i32, py: i32, bw: i32, bh: i32, w: i32, h: i32, gap: i32) -> (i32, i32) {
    let x = if px + gap + bw <= w { px + gap } else { px - gap - bw };
    let y = if py + gap + bh <= h { py + gap } else { py - gap - bh };
    (x.clamp(0, (w - bw).max(0)), y.clamp(0, (h - bh).max(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_gives_the_exact_text() {
        let orange = (255, 128, 0);
        assert_eq!(Format::Hex.text(orange), "#FF8000");
        assert_eq!(Format::Rgb.text(orange), "rgb(255, 128, 0)");
        assert_eq!(Format::Hsl.text(orange), "hsl(30, 100%, 50%)");
        assert_eq!(Format::Hsv.text(orange), "hsv(30, 100%, 100%)");
        assert_eq!(Format::Cmyk.text(orange), "cmyk(0%, 50%, 100%, 0%)");
        assert_eq!(Format::Float.text(orange), "(1.00f, 0.50f, 0.00f, 1f)");
        // Labyrinth's purple
        let p = (0x7c, 0x54, 0xd6);
        assert_eq!(Format::Hex.text(p), "#7C54D6");
        assert_eq!(Format::Hsl.text(p), "hsl(258, 61%, 58%)");
        assert_eq!(Format::Hsv.text(p), "hsv(258, 61%, 84%)");
        assert_eq!(Format::Cmyk.text(p), "cmyk(42%, 61%, 0%, 16%)");
        // greys have no hue or saturation; black is all key
        assert_eq!(Format::Hsl.text((128, 128, 128)), "hsl(0, 0%, 50%)");
        assert_eq!(Format::Cmyk.text((0, 0, 0)), "cmyk(0%, 0%, 0%, 100%)");
        assert_eq!(Format::Hsv.text((0, 0, 255)), "hsv(240, 100%, 100%)");
        assert_eq!(Format::Hsl.text((255, 0, 128)), "hsl(330, 100%, 50%)");
    }

    #[test]
    fn hex_reads_back_and_formats_parse() {
        assert_eq!(parse_hex("#7c54d6"), Some((0x7c, 0x54, 0xd6)));
        assert_eq!(parse_hex("7C54D6"), Some((0x7c, 0x54, 0xd6)));
        assert_eq!(parse_hex("#7c54d"), None);
        assert_eq!(parse_hex("#zzzzzz"), None);
        assert_eq!(Format::parse(" HSL "), Some(Format::Hsl));
        assert_eq!(Format::parse("lab"), None);
        for f in Format::ALL {
            assert_eq!(Format::parse(f.id()), Some(f));
        }
    }

    #[test]
    fn the_format_lives_in_tools_conf_beside_other_tools() {
        let conf = "# Yggdrasil's tools\nfind_mouse = yes\ncolor_format = rgb # picked in Settings\n";
        assert_eq!(format_in(conf), Some(Format::Rgb));
        assert_eq!(format_in("find_mouse = yes\n"), None);
        assert_eq!(format_in("color_format = nonsense\n"), None);
        assert_eq!(with_format(conf, Format::Hsl), "# Yggdrasil's tools\nfind_mouse = yes\ncolor_format = hsl\n");
        assert_eq!(with_format("find_mouse = no\n", Format::Hex), "find_mouse = no\ncolor_format = hex\n");
    }

    #[test]
    fn history_is_newest_first_once_each_and_capped() {
        let h = parse_history("#000001\nnot a colour\n#000002\n#000001\n");
        assert_eq!(h, vec![(0, 0, 1), (0, 0, 2)]);
        let h = add_to(&h, (0, 0, 2));
        assert_eq!(h, vec![(0, 0, 2), (0, 0, 1)], "a repeat moves to the top");
        let mut long = vec![];
        for i in 0..30u8 {
            long = add_to(&long, (i, 0, 0));
        }
        assert_eq!(long.len(), HISTORY_MAX);
        assert_eq!(long[0], (29, 0, 0));
        let text: String = (0..30u8).map(|i| hex((i, 0, 0)) + "\n").collect();
        assert_eq!(parse_history(&text).len(), HISTORY_MAX);
    }

    #[test]
    fn the_loupe_shows_the_pixels_around_the_pointer() {
        let l = Loupe::new(3, 132);
        assert_eq!((l.cells, l.cell, l.side()), (11, 12, 132));
        let src = l.source(10, 10, 100, 100);
        assert_eq!(src.len(), 121);
        assert_eq!(src[60], Some((10, 10)), "the middle cell is the pixel under the pointer");
        assert_eq!(src[0], Some((5, 5)));
        assert_eq!(src[120], Some((15, 15)));
        let edge = l.source(0, 0, 100, 100);
        assert_eq!(edge[0], None, "off the screen");
        assert_eq!(edge[60], Some((0, 0)));
        assert_eq!(Loupe::new(99, 132).cells, 21, "zoom steps stop at the last");
        assert_eq!(Loupe::new(0, 132).cells, 5);
    }

    #[test]
    fn the_loupe_stays_on_the_screen() {
        assert_eq!(place(100, 100, 150, 180, 1920, 1080, 24), (124, 124));
        assert_eq!(place(1900, 100, 150, 180, 1920, 1080, 24), (1726, 124), "flips left at the right edge");
        assert_eq!(place(100, 1070, 150, 180, 1920, 1080, 24), (124, 866), "flips up at the bottom");
        assert_eq!(place(10, 10, 150, 180, 100, 100, 24), (0, 0), "a screen smaller than the loupe");
    }
}
