//! The "?" of the help cursor, drawn as Pillow 13.0 draws text (ImageDraw.text with anchor "mm" over FreeTypeFont.getmask2
//! and _imagingft.c's font_render): the font fc-match names for sans-serif:bold through FreeType, laid out by libraqm when
//! it is there (as Pillow does when it has raqm) or glyph by glyph, rendered by FreeType and put on the cursor as Pillow's
//! fill with a mask does. Without that font (no fc-match, nothing it names that FreeType opens) it is Pillow's own
//! default font, the Aileron subset Pillow carries (CC0), at size 10, laid out glyph by glyph, as load_default() gives it.
//!
//! libfreetype and libraqm are opened at run time, only while the help cursor is drawn, so ygg-cursors needs neither to
//! start. Translated from Pillow's C and Python (MIT-CMU licence, see NOTICE-Pillow).

use super::draw::{Img, Ink};
use std::ffi::{c_char, c_int, c_long, c_uint, c_ushort, c_void, CStr};

const AILERON: &[u8] = include_bytes!("aileron-subset.ttf");

#[repr(C)]
struct Generic {
    data: *mut c_void,
    finalizer: *mut c_void,
}

#[repr(C)]
struct BBox {
    x_min: c_long,
    y_min: c_long,
    x_max: c_long,
    y_max: c_long,
}

#[repr(C)]
struct FaceRec {
    num_faces: c_long,
    face_index: c_long,
    face_flags: c_long,
    style_flags: c_long,
    num_glyphs: c_long,
    family_name: *mut c_char,
    style_name: *mut c_char,
    num_fixed_sizes: c_int,
    available_sizes: *mut c_void,
    num_charmaps: c_int,
    charmaps: *mut c_void,
    generic: Generic,
    bbox: BBox,
    units_per_em: c_ushort,
    ascender: i16,
    descender: i16,
    height: i16,
    max_advance_width: i16,
    max_advance_height: i16,
    underline_position: i16,
    underline_thickness: i16,
    glyph: *mut GlyphSlotRec,
    size: *mut SizeRec,
}

#[repr(C)]
struct SizeMetrics {
    x_ppem: c_ushort,
    y_ppem: c_ushort,
    x_scale: c_long,
    y_scale: c_long,
    ascender: c_long,
    descender: c_long,
    height: c_long,
    max_advance: c_long,
}

#[repr(C)]
struct SizeRec {
    face: *mut FaceRec,
    generic: Generic,
    metrics: SizeMetrics,
}

#[repr(C)]
struct Bitmap {
    rows: c_uint,
    width: c_uint,
    pitch: c_int,
    buffer: *mut u8,
    num_grays: c_ushort,
    pixel_mode: u8,
    palette_mode: u8,
    palette: *mut c_void,
}

#[repr(C)]
struct GlyphSlotRec {
    library: *mut c_void,
    face: *mut FaceRec,
    next: *mut c_void,
    glyph_index: c_uint,
    generic: Generic,
    metrics: [c_long; 8],
    linear_hori_advance: c_long,
    linear_vert_advance: c_long,
    advance: [c_long; 2],
    format: c_uint,
    bitmap: Bitmap,
    bitmap_left: c_int,
    bitmap_top: c_int,
}

#[repr(C)]
struct SizeRequest {
    kind: c_uint,
    width: c_long,
    height: c_long,
    hori_resolution: c_uint,
    vert_resolution: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RaqmGlyph {
    index: c_uint,
    x_advance: c_int,
    y_advance: c_int,
    x_offset: c_int,
    y_offset: c_int,
    cluster: u32,
    ftface: *mut c_void,
}

const FT_LOAD_RENDER: i32 = 1 << 2;
const FT_GLYPH_BBOX_PIXELS: c_uint = 3;
const FT_PIXEL_MODE_GRAY: u8 = 2;

type Face = *mut FaceRec;

/// The FreeType library handle is used by one thread only (ygg-cursors draws in one), and never freed.
struct Library(*mut c_void);
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

struct FreeType {
    library: Library,
    new_face: unsafe extern "C" fn(*mut c_void, *const c_char, c_long, *mut Face) -> c_int,
    new_memory_face: unsafe extern "C" fn(*mut c_void, *const u8, c_long, c_long, *mut Face) -> c_int,
    request_size: unsafe extern "C" fn(Face, *mut SizeRequest) -> c_int,
    get_char_index: unsafe extern "C" fn(Face, u64) -> c_uint,
    load_glyph: unsafe extern "C" fn(Face, c_uint, i32) -> c_int,
    get_glyph: unsafe extern "C" fn(*mut GlyphSlotRec, *mut *mut c_void) -> c_int,
    glyph_get_cbox: unsafe extern "C" fn(*mut c_void, c_uint, *mut BBox),
    done_glyph: unsafe extern "C" fn(*mut c_void),
    done_face: unsafe extern "C" fn(Face) -> c_int,
}

struct Raqm {
    create: unsafe extern "C" fn() -> *mut c_void,
    set_text: unsafe extern "C" fn(*mut c_void, *const u32, usize) -> bool,
    set_par_direction: unsafe extern "C" fn(*mut c_void, c_int) -> bool,
    set_freetype_face: unsafe extern "C" fn(*mut c_void, Face) -> bool,
    layout: unsafe extern "C" fn(*mut c_void) -> bool,
    get_glyphs: unsafe extern "C" fn(*mut c_void, *mut usize) -> *mut RaqmGlyph,
    destroy: unsafe extern "C" fn(*mut c_void),
}

unsafe fn sym<T>(h: *mut c_void, name: &CStr) -> Option<T> {
    let p = libc::dlsym(h, name.as_ptr());
    (!p.is_null()).then(|| std::mem::transmute_copy::<*mut c_void, T>(&p))
}

fn freetype() -> Option<&'static FreeType> {
    static FT: std::sync::OnceLock<Option<FreeType>> = std::sync::OnceLock::new();
    FT.get_or_init(|| unsafe {
        let h = libc::dlopen(c"libfreetype.so.6".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if h.is_null() {
            return None;
        }
        let init: unsafe extern "C" fn(*mut *mut c_void) -> c_int = sym(h, c"FT_Init_FreeType")?;
        let mut library = std::ptr::null_mut();
        if init(&mut library) != 0 {
            return None;
        }
        Some(FreeType {
            library: Library(library),
            new_face: sym(h, c"FT_New_Face")?,
            new_memory_face: sym(h, c"FT_New_Memory_Face")?,
            request_size: sym(h, c"FT_Request_Size")?,
            get_char_index: sym(h, c"FT_Get_Char_Index")?,
            load_glyph: sym(h, c"FT_Load_Glyph")?,
            get_glyph: sym(h, c"FT_Get_Glyph")?,
            glyph_get_cbox: sym(h, c"FT_Glyph_Get_CBox")?,
            done_glyph: sym(h, c"FT_Done_Glyph")?,
            done_face: sym(h, c"FT_Done_Face")?,
        })
    })
    .as_ref()
}

fn raqm() -> Option<&'static Raqm> {
    static RAQM: std::sync::OnceLock<Option<Raqm>> = std::sync::OnceLock::new();
    RAQM.get_or_init(|| unsafe {
        let h = libc::dlopen(c"libraqm.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if h.is_null() {
            return None;
        }
        Some(Raqm {
            create: sym(h, c"raqm_create")?,
            set_text: sym(h, c"raqm_set_text")?,
            set_par_direction: sym(h, c"raqm_set_par_direction")?,
            set_freetype_face: sym(h, c"raqm_set_freetype_face")?,
            layout: sym(h, c"raqm_layout")?,
            get_glyphs: sym(h, c"raqm_get_glyphs")?,
            destroy: sym(h, c"raqm_destroy")?,
        })
    })
    .as_ref()
}

/// PIXEL(): 26.6 to whole pixels, rounded
fn pixel(x: i64) -> i64 {
    ((x + 32) & -64) >> 6
}

/// A font as ImageFont.truetype gives it: the face at `size` pixels, and whether raqm lays it out.
struct Font {
    face: Face,
    raqm: bool,
}

impl Drop for Font {
    fn drop(&mut self) {
        if let Some(ft) = freetype() {
            unsafe { (ft.done_face)(self.face) };
        }
    }
}

fn open_font(path: Option<&[u8]>, size: f32, raqm_wanted: bool) -> Option<Font> {
    let ft = freetype()?;
    let mut face: Face = std::ptr::null_mut();
    let err = unsafe {
        match path {
            Some(p) => {
                let c = std::ffi::CString::new(p).ok()?;
                (ft.new_face)(ft.library.0, c.as_ptr(), 0, &mut face)
            }
            None => (ft.new_memory_face)(ft.library.0, AILERON.as_ptr(), AILERON.len() as c_long, 0, &mut face),
        }
    };
    if err != 0 {
        return None;
    }
    let width = (size * 64.0) as c_long;
    let mut req = SizeRequest { kind: 0, width, height: width, hori_resolution: 0, vert_resolution: 0 };
    let font = Font { face, raqm: raqm_wanted && raqm().is_some() };
    if unsafe { (ft.request_size)(face, &mut req) } != 0 {
        return None;
    }
    Some(font)
}

/// ImageFont.truetype's second try when FreeType cannot open the path: a file of that name in the font folders
/// ($XDG_DATA_HOME or ~/.local/share, then $XDG_DATA_DIRS or /usr/local/share:/usr/share, each with /fonts), walked as
/// os.walk walks them; with no extension in the name, a .ttf of that name first, else the first of any extension.
fn search_font_dirs(path: &str) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    let name = path.rsplit('/').next().unwrap_or(path);
    let splitext = |n: &str| -> (String, String) {
        match n.rfind('.') {
            Some(i) if n[..i].chars().any(|c| c != '.') => (n[..i].to_string(), n[i..].to_string()),
            _ => (n.to_string(), String::new()),
        }
    };
    let ext = splitext(name).1;
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<PathBuf> =
        vec![PathBuf::from(env("XDG_DATA_HOME").unwrap_or_else(|| format!("{}/.local/share", home.trim_end_matches('/'))))];
    dirs.extend(env("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()).split(':').map(PathBuf::from));
    let mut first_other: Option<PathBuf> = None;
    fn walk(d: &Path, visit: &mut dyn FnMut(&Path, &str) -> bool) -> bool {
        let Ok(rd) = std::fs::read_dir(d) else {
            return false;
        };
        let mut subdirs = vec![];
        let mut files = vec![];
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() {
                if !e.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                    subdirs.push(e.path());
                }
            } else {
                files.push(n);
            }
        }
        for f in files {
            if visit(d, &f) {
                return true;
            }
        }
        subdirs.into_iter().any(|s| walk(&s, visit))
    }
    let mut found: Option<PathBuf> = None;
    for d in dirs {
        let fonts = d.join("fonts");
        walk(&fonts, &mut |root, f| {
            if !ext.is_empty() && f == name {
                found = Some(root.join(f));
                return true;
            }
            if ext.is_empty() && splitext(f).0 == name {
                let p = root.join(f);
                if splitext(&p.to_string_lossy()).1 == ".ttf" {
                    found = Some(p);
                    return true;
                }
                if first_other.is_none() {
                    first_other = Some(p);
                }
            }
            false
        });
        if found.is_some() {
            break;
        }
    }
    found.or(first_other)
}

/// font(px) in ygg-cursors: what fc-match names for sans-serif:bold, else Pillow's default font.
fn cursor_font(px: f64) -> Option<Font> {
    let named = std::process::Command::new("fc-match")
        .args(["sans-serif:bold", "-f", "%{file}"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| pycompat::strip(&s).to_string());
    if let Some(path) = named {
        let size = px as i32 as f32;
        match open_font(Some(path.as_bytes()), size, true) {
            Some(f) => return Some(f),
            None => {
                if let Some(f) = search_font_dirs(&path).and_then(|p| open_font(Some(p.as_os_str().as_encoded_bytes()), size, true)) {
                    return Some(f);
                }
            }
        }
    }
    open_font(None, 10.0, false)
}

struct Glyph {
    index: u32,
    x_advance: i64,
    y_advance: i64,
    x_offset: i64,
    y_offset: i64,
}

#[allow(clippy::unnecessary_cast)]
fn layout(font: &Font, text: &str) -> Vec<Glyph> {
    let ft = freetype().unwrap();
    if font.raqm {
        let r = raqm().unwrap();
        let chars: Vec<u32> = text.chars().map(|c| c as u32).collect();
        unsafe {
            let rq = (r.create)();
            if rq.is_null() {
                return vec![];
            }
            let mut out = vec![];
            if (r.set_text)(rq, chars.as_ptr(), chars.len())
                && (r.set_par_direction)(rq, 0)
                && (r.set_freetype_face)(rq, font.face)
                && (r.layout)(rq)
            {
                let mut n = 0usize;
                let g = (r.get_glyphs)(rq, &mut n);
                if !g.is_null() {
                    for gl in std::slice::from_raw_parts(g, n) {
                        out.push(Glyph {
                            index: gl.index,
                            x_advance: gl.x_advance as i64,
                            y_advance: gl.y_advance as i64,
                            x_offset: gl.x_offset as i64,
                            y_offset: gl.y_offset as i64,
                        });
                    }
                }
            }
            (r.destroy)(rq);
            out
        }
    } else {
        // text_layout_fallback: one glyph a character, its hinted advance (one character here: no kerning pair)
        text.chars()
            .map(|c| unsafe {
                let index = (ft.get_char_index)(font.face, c as u64);
                (ft.load_glyph)(font.face, index, 0);
                Glyph { index, x_advance: (*(*font.face).glyph).metrics[4] as i64, y_advance: 0, x_offset: 0, y_offset: 0 }
            })
            .collect()
    }
}

/// draw.text((x, y), text, font=font(px), fill=ink, anchor="mm") on the canvas (FreeType's longs are i64 here; the casts
/// keep the code right where they are not)
#[allow(clippy::unnecessary_cast)]
pub fn draw_text_mm(im: &mut Img, xy: (f64, f64), text: &str, px: f64, ink: &Ink) {
    let Some(font) = cursor_font(px) else {
        // no FreeType at all: Pillow could not have drawn it either (it needs FreeType for ImageDraw.text)
        return;
    };
    let ft = freetype().unwrap();
    let glyphs = layout(&font, text);
    let face = font.face;
    // bounding_box_and_anchors, horizontal, anchor "mm"
    let (mut position, mut x_min, mut x_max, mut y_min, mut y_max) = (0i64, 0i64, 0i64, 0i64, 0i64);
    for g in &glyphs {
        let gx = pixel(position + g.x_offset);
        let gy = pixel(g.y_offset);
        position += g.x_advance;
        x_max = x_max.max(pixel(position));
        let mut bbox = BBox { x_min: 0, y_min: 0, x_max: 0, y_max: 0 };
        unsafe {
            if (ft.load_glyph)(face, g.index, 0) != 0 {
                return;
            }
            let mut glyph = std::ptr::null_mut();
            if (ft.get_glyph)((*face).glyph, &mut glyph) != 0 {
                return;
            }
            (ft.glyph_get_cbox)(glyph, FT_GLYPH_BBOX_PIXELS, &mut bbox);
            (ft.done_glyph)(glyph);
        }
        x_max = x_max.max(bbox.x_max as i64 + gx);
        x_min = x_min.min(bbox.x_min as i64 + gx);
        y_max = y_max.max(bbox.y_max as i64 + gy);
        y_min = y_min.min(bbox.y_min as i64 + gy);
    }
    let (x_anchor, y_anchor) = if glyphs.is_empty() {
        (0, 0)
    } else {
        let m = unsafe { &(*(*face).size).metrics };
        (pixel(position / 2), pixel((m.ascender as i64 + m.descender as i64) / 2))
    };
    // ImageDraw.text: the whole part of the position, the fraction passed on as the start
    let (x, y) = (xy.0 as i64, xy.1 as i64);
    let x_start = (xy.0 - xy.0.trunc()) as f32;
    let y_start = (xy.1 - xy.1.trunc()) as f32;
    let mut width = x_max - x_min;
    let mut height = y_max - y_min;
    width = (width as f64 + ((0.0f32 * 2.0 + x_start) as f64).ceil()) as i64;
    height = (height as f64 + ((0.0f32 * 2.0 + y_start) as f64).ceil()) as i64;
    let x_offset = ((-x_anchor + x_min) as f64).round() as i64;
    let y_offset = ((y_anchor - y_max) as f64).round() as i64;
    if glyphs.is_empty() || width == 0 || height == 0 {
        return;
    }
    let mut mask = Img::new(width as i32, height as i32, 1, [0; 4]);
    // the pen: x_min and y_max of the rendered glyphs
    let (mut pen_x, mut pen_y, mut min_x, mut max_y) = (0i64, 0i64, 0i64, 0i64);
    for g in &glyphs {
        let gx = pixel(pen_x + g.x_offset);
        let gy = pixel(pen_y + g.y_offset);
        unsafe {
            if (ft.load_glyph)(face, g.index, FT_LOAD_RENDER) != 0 {
                return;
            }
            let slot = &*(*face).glyph;
            max_y = max_y.max(slot.bitmap_top as i64 + gy);
            min_x = min_x.min(slot.bitmap_left as i64 + gx);
        }
        pen_x += g.x_advance;
        pen_y += g.y_advance;
    }
    // round((-x_min + stroke_width + x_start) * 64) in float, then double
    pen_x = round_f32((-min_x) as f32 + 0.0f32 + x_start);
    pen_y = round_f32((-max_y) as f32 + (-0.0f32) - y_start);
    for g in &glyphs {
        let gx = pixel(pen_x + g.x_offset);
        let gy = pixel(pen_y + g.y_offset);
        unsafe {
            if (ft.load_glyph)(face, g.index, FT_LOAD_RENDER) != 0 {
                return;
            }
            let slot = &*(*face).glyph;
            let bm = &slot.bitmap;
            let xx = gx + slot.bitmap_left as i64;
            let mut yy = -(gy + slot.bitmap_top as i64);
            if !bm.buffer.is_null() && bm.pixel_mode == FT_PIXEL_MODE_GRAY {
                let mut x0 = 0i64;
                let mut x1 = bm.width as i64;
                if xx < 0 {
                    x0 = -xx;
                }
                if xx + x1 > mask.w as i64 {
                    x1 = mask.w as i64 - xx;
                }
                let mut source = bm.buffer as *const u8;
                for _ in 0..bm.rows {
                    if yy >= 0 && yy < mask.h as i64 {
                        for k in x0..x1 {
                            let src = *source.offset(k as isize) as u32;
                            if src > 0 {
                                let i = (yy * mask.w as i64 + xx + k) as usize;
                                let t = mask.px[i] as u32;
                                mask.px[i] = if t > 0 { (src + muldiv255(t, 255 - src)).min(255) as u8 } else { src as u8 };
                            }
                        }
                    }
                    source = source.offset(bm.pitch as isize);
                    yy += 1;
                }
            }
        }
        pen_x += g.x_advance;
        pen_y += g.y_advance;
    }
    super::fill_mask(im, ink, &mask, (x + x_offset) as i32, (y + y_offset) as i32);
}

fn round_f32(v: f32) -> i64 {
    ((v * 64.0f32) as f64).round() as i64
}

fn muldiv255(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    ((t >> 8) + t) >> 8
}
