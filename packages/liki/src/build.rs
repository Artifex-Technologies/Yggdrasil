//! liki-build: the icon pack of app logos for the apps installed here (a selection from Simple Icons, CC0, in
//! /usr/share/liki/brand-icons) and Liki's own glyphs (Yggdrasil's apps, folders, drives, the trash, file types, the
//! default app icon), as SVG text in a shape and a style. The pack goes to ~/.local/share/icons/NAME (or --out).

use crate::argparse::{Arg, Kind, Parser};
use crate::py::{self, f1, g, join};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const PURPLE: &str = "7c54d6";

/// The shapes, in the order argparse lists them, with how large the logo is on each (a share of the canvas).
pub const GLYPH_SIZE: [(&str, f64); 5] = [("glyph", 0.84), ("squircle", 0.56), ("rounded", 0.58), ("circle", 0.52), ("hexagon", 0.48)];
const SHAPES: [&str; 5] = ["glyph", "squircle", "rounded", "circle", "hexagon"];
const STYLES: [&str; 3] = ["brand", "dark", "purple"];

/// Desktop icon names (normalised: lower case, letters and digits only) that are not the logo's own name.
const ALIASES: &[(&str, &str)] = &[
    ("vivaldistable", "vivaldi"),
    ("googlechrome", "googlechrome"),
    ("googlechromestable", "googlechrome"),
    ("braverowser", "brave"),
    ("bravebrowser", "brave"),
    ("vlc", "vlcmediaplayer"),
    ("obs", "obsstudio"),
    ("comobsprojectstudio", "obsstudio"),
    ("orgvideolanvlc", "vlcmediaplayer"),
    ("orgmozillafirefox", "firefox"),
    ("firefoxesr", "firefox"),
    ("signaldesktop", "signal"),
    ("orgtelegramdesktop", "telegram"),
    ("comdiscordapp", "discord"),
    ("discordcanary", "discord"),
    ("spotifylauncher", "spotify"),
    ("spotifyclient", "spotify"),
    ("comspotifyclient", "spotify"),
    ("commitchellhghostty", "ghostty"),
    ("commitchellhghosttydesktop", "ghostty"),
    ("comlibretroretroarch", "retroarch"),
    ("orgfreecadfreecad", "freecad"),
    ("protonvpn", "protonvpn"),
    ("protonvpngtk", "protonvpn"),
    ("orgkdekdenlive", "kdenlive"),
    ("orggimpgimp", "gimp"),
    ("orginkscapeinkscape", "inkscape"),
    ("orgblenderblender", "blender"),
    ("orgkritakrita", "krita"),
    ("torbrowser", "torbrowser"),
    ("torbrowserlauncher", "torbrowser"),
    ("claudedesktop", "claude"),
    ("claudeai", "claude"),
    ("orgqbittorrentqbittorrent", "qbittorrent"),
    ("libreofficestartcenter", "libreoffice"),
    ("libreofficewriter", "libreoffice"),
    ("libreofficecalc", "libreoffice"),
    ("libreofficeimpress", "libreoffice"),
    ("bitwardendesktop", "bitwarden"),
    ("heroicgameslauncher", "heroicgameslauncher"),
    ("githubdesktop", "github"),
    ("sketchup", "sketchup"),
    ("obsidian", "obsidian"),
    ("vscode", "visualstudiocode"),
    ("code", "visualstudiocode"),
    ("comvisualstudiocode", "visualstudiocode"),
    ("commicrosoftvscode", "visualstudiocode"),
    ("visualstudiocodebin", "visualstudiocode"),
    ("visualstudiocode", "visualstudiocode"),
    ("steam", "steam"),
    ("steamicon", "steam"),
];

/// Names the Marketplace and Yggdrasil's own components use, which no installed desktop file would ask for: icon -> logo.
const EXTRA: &[(&str, &str)] = &[("proton-vpn", "protonvpn"), ("proton-pass", "proton"), ("proton-mail", "protonmail")];

/// Liki's own glyphs: file -> the (folder in the pack, icon name) it is written as.
const NATIVE: &[(&str, &[(&str, &str)])] = &[
    ("sarina", &[("apps", "sarina"), ("apps", "labyrinth-sarina")]),
    ("daedalus", &[("apps", "daedalus-cad")]),
    ("icarus", &[("apps", "icarus")]),
    ("heimdall", &[("apps", "heimdall")]),
    ("limbo", &[("apps", "limbo")]),
    ("marketplace", &[("apps", "agora"), ("apps", "applications-all")]),
    ("minos", &[("apps", "minos")]),
    (
        "settings",
        &[("apps", "yggdrasil-settings"), ("apps", "preferences-system"), ("apps", "systemsettings"), ("apps", "org.kde.systemsettings")],
    ),
    (
        "calculator",
        &[("apps", "yggdrasil-calculator"), ("apps", "accessories-calculator"), ("apps", "org.kde.kcalc"), ("apps", "calculator")],
    ),
    ("timer", &[("apps", "yggdrasil-timer")]),
    ("health", &[("apps", "yggdrasil-health")]),
    ("browser-warm", &[("apps", "yggdrasil-browser-warm")]),
    ("clipboard", &[("apps", "yggdrasil-clipboard")]),
    ("weather", &[("apps", "yggdrasil-weather")]),
    ("2048", &[("apps", "yggdrasil-2048")]),
    ("trash", &[("places", "user-trash"), ("places", "trash-empty"), ("places", "trashcan_empty")]),
    ("trash-full", &[("places", "user-trash-full"), ("places", "trash-full"), ("places", "trashcan_full")]),
    (
        "folder",
        &[
            ("places", "folder"),
            ("places", "folder-open"),
            ("places", "folder-documents"),
            ("places", "folder-download"),
            ("places", "folder-music"),
            ("places", "folder-pictures"),
            ("places", "folder-videos"),
            ("places", "folder-public"),
            ("places", "folder-templates"),
            ("places", "folder-desktop"),
            ("places", "user-desktop"),
            ("places", "folder-remote"),
            ("mimetypes", "inode-directory"),
        ],
    ),
    ("home", &[("places", "user-home"), ("places", "folder-home")]),
    (
        "drive",
        &[
            ("devices", "drive-harddisk"),
            ("devices", "drive-harddisk-solidstate"),
            ("devices", "drive-removable-media"),
            ("devices", "drive-multidisk"),
        ],
    ),
    ("sudoku", &[("apps", "yggdrasil-sudoku")]),
    (
        "default-app",
        &[("apps", "application-default-icon"), ("apps", "application-x-executable"), ("apps", "applications-other"), ("apps", "unknown")],
    ),
];

const G_LINES: &str = r##"<path d="M5 7h14M5 12h14M5 17h9" stroke="#fff" stroke-width="2.2" stroke-linecap="round" fill="none"/>"##;

/// File types' glyphs, drawn in a 24-unit box (no text: the icons are drawn without fonts).
const GLYPHS: &[(&str, &str)] = &[
    ("lines", G_LINES),
    (
        "dots",
        r##"<g fill="#fff"><circle cx="7" cy="8" r="2"/><circle cx="17" cy="8" r="2"/><circle cx="7" cy="16" r="2"/><circle cx="17" cy="16" r="2"/></g>"##,
    ),
    (
        "markdown",
        r##"<path d="M3 18V6l4.5 5L12 6v12" stroke="#fff" stroke-width="2.4" stroke-linejoin="round" stroke-linecap="round" fill="none"/><path d="M18 6v10M14.5 13l3.5 4 3.5-4" stroke="#fff" stroke-width="2.4" stroke-linejoin="round" stroke-linecap="round" fill="none"/>"##,
    ),
    (
        "pdf",
        r##"<path d="M4 19c3-2 6-7 7-13 1 5 4 10 9 11-5 0-11 1-16 2z" stroke="#fff" stroke-width="2" stroke-linejoin="round" fill="none"/>"##,
    ),
    ("image", r##"<circle cx="8" cy="8" r="2.4" fill="#fff"/><path d="M3 19l6-7 4 4 3-3 5 6z" fill="#fff"/>"##),
    (
        "audio",
        r##"<path d="M9 17V5l11-2v12" stroke="#fff" stroke-width="2.2" stroke-linejoin="round" fill="none"/><circle cx="6.5" cy="17.5" r="3" fill="#fff"/><circle cx="17.5" cy="15.5" r="3" fill="#fff"/>"##,
    ),
    ("video", r##"<path d="M8 5l11 7-11 7z" fill="#fff"/>"##),
    (
        "archive",
        r##"<path d="M12 2v3M10 5h4M12 5v3M10 8h4M12 8v3M10 11h4" stroke="#fff" stroke-width="2" fill="none"/><rect x="9" y="13" width="6" height="7" rx="1.5" stroke="#fff" stroke-width="2" fill="none"/>"##,
    ),
    (
        "package",
        r##"<path d="M12 3l8 4v10l-8 4-8-4V7z M4 7l8 4 8-4 M12 11v10" stroke="#fff" stroke-width="2" stroke-linejoin="round" fill="none"/>"##,
    ),
    ("document", r##"<path d="M4 6h16M4 10h16M4 14h16M4 18h10" stroke="#fff" stroke-width="2" stroke-linecap="round" fill="none"/>"##),
    (
        "grid",
        r##"<rect x="3" y="4" width="18" height="16" rx="1.5" stroke="#fff" stroke-width="2" fill="none"/><path d="M3 9.5h18M3 15h18M9 4v16M15 4v16" stroke="#fff" stroke-width="1.8" fill="none"/>"##,
    ),
    (
        "slides",
        r##"<rect x="3" y="4" width="18" height="12" rx="1.5" stroke="#fff" stroke-width="2" fill="none"/><path d="M8 12V9M12 12V7M16 12v-2M12 16v4M8 20h8" stroke="#fff" stroke-width="2" stroke-linecap="round" fill="none"/>"##,
    ),
    (
        "code",
        r##"<path d="M8 6l-6 6 6 6M16 6l6 6-6 6M14 4l-4 16" stroke="#fff" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"##,
    ),
    (
        "shell",
        r##"<path d="M4 7l5 5-5 5M12 18h8" stroke="#fff" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"##,
    ),
    (
        "braces",
        r##"<path d="M8 4c-3 0-3 2-3 4s0 3-2 4c2 1 2 2 2 4s0 4 3 4M16 4c3 0 3 2 3 4s0 3 2 4c-2 1-2 2-2 4s0 4-3 4" stroke="#fff" stroke-width="2.2" stroke-linecap="round" fill="none"/>"##,
    ),
    (
        "config",
        r##"<path d="M5 6h14M5 12h14M5 18h14" stroke="#fff" stroke-width="2" stroke-linecap="round" fill="none"/><circle cx="9" cy="6" r="2.4" fill="#fff"/><circle cx="15" cy="12" r="2.4" fill="#fff"/><circle cx="8" cy="18" r="2.4" fill="#fff"/>"##,
    ),
    (
        "log",
        r##"<path d="M9 6h11M9 12h11M9 18h11" stroke="#fff" stroke-width="2" stroke-linecap="round" fill="none"/><circle cx="4.5" cy="6" r="1.6" fill="#fff"/><circle cx="4.5" cy="12" r="1.6" fill="#fff"/><circle cx="4.5" cy="18" r="1.6" fill="#fff"/>"##,
    ),
    (
        "font",
        r##"<path d="M4 20L12 4l8 16M7.5 14h9" stroke="#fff" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"##,
    ),
    (
        "disc",
        r##"<circle cx="12" cy="12" r="9" stroke="#fff" stroke-width="2.2" fill="none"/><circle cx="12" cy="12" r="2.5" fill="#fff"/>"##,
    ),
    (
        "gear",
        r##"<circle cx="12" cy="12" r="3.2" stroke="#fff" stroke-width="2.2" fill="none"/><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.3 5.3l2.1 2.1M16.6 16.6l2.1 2.1M5.3 18.7l2.1-2.1M16.6 7.4l2.1-2.1" stroke="#fff" stroke-width="2.2" stroke-linecap="round" fill="none"/>"##,
    ),
    (
        "database",
        r##"<ellipse cx="12" cy="6" rx="7" ry="2.8" stroke="#fff" stroke-width="2" fill="none"/><path d="M5 6v12c0 1.5 3 2.8 7 2.8s7-1.3 7-2.8V6M5 12c0 1.5 3 2.8 7 2.8s7-1.3 7-2.8" stroke="#fff" stroke-width="2" fill="none"/>"##,
    ),
    (
        "book",
        r##"<path d="M4 5c3-1 6-1 8 1 2-2 5-2 8-1v14c-3-1-6-1-8 1-2-2-5-2-8-1zM12 6v14" stroke="#fff" stroke-width="2" stroke-linejoin="round" fill="none"/>"##,
    ),
    (
        "key",
        r##"<circle cx="8" cy="12" r="4" stroke="#fff" stroke-width="2.2" fill="none"/><path d="M12 12h9M18 12v3M21 12v2" stroke="#fff" stroke-width="2.2" stroke-linecap="round" fill="none"/>"##,
    ),
    (
        "web",
        r##"<circle cx="12" cy="12" r="9" stroke="#fff" stroke-width="2" fill="none"/><path d="M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18" stroke="#fff" stroke-width="2" fill="none"/>"##,
    ),
];

/// freedesktop icon name -> (page colour, glyph): each file type in a colour of its own, never the accent.
const MIME: &[(&str, &str, &str)] = &[
    ("text-x-generic", "8a93a6", "lines"),
    ("text-plain", "8a93a6", "lines"),
    ("application-x-generic", "6b7280", "dots"),
    ("text-markdown", "37474f", "markdown"),
    ("text-x-markdown", "37474f", "markdown"),
    ("application-pdf", "d93025", "pdf"),
    ("image-x-generic", "0d9488", "image"),
    ("image-svg+xml", "0f766e", "image"),
    ("audio-x-generic", "ea8c00", "audio"),
    ("video-x-generic", "db2777", "video"),
    ("application-zip", "b45309", "archive"),
    ("application-x-archive", "b45309", "archive"),
    ("application-x-compressed-tar", "b45309", "archive"),
    ("package-x-generic", "8d5a2b", "package"),
    ("x-office-document", "2563eb", "document"),
    ("x-office-spreadsheet", "15803d", "grid"),
    ("text-csv", "16a34a", "grid"),
    ("x-office-presentation", "e0531f", "slides"),
    ("text-x-script", "334155", "code"),
    ("application-x-shellscript", "1f2937", "shell"),
    ("text-x-python", "3776ab", "code"),
    ("text-x-rust", "b7410e", "code"),
    ("text-x-go", "00a0c6", "code"),
    ("text-x-csrc", "555fa8", "code"),
    ("text-x-c++src", "00599c", "code"),
    ("text-x-chdr", "6b6fb0", "code"),
    ("text-x-java", "d46a00", "code"),
    ("application-javascript", "c79a00", "code"),
    ("text-x-typescript", "3178c6", "code"),
    ("text-x-lua", "2c2d72", "code"),
    ("text-html", "e34f26", "web"),
    ("text-css", "2965f1", "braces"),
    ("application-json", "a16207", "braces"),
    ("application-xml", "7c6f1e", "code"),
    ("text-x-config", "64748b", "config"),
    ("text-x-log", "475569", "log"),
    ("text-x-patch", "9a3412", "code"),
    ("font-x-generic", "0f766e", "font"),
    ("application-x-cd-image", "4b5563", "disc"),
    ("application-x-executable", "1f2937", "gear"),
    ("application-x-sharedlib", "374151", "gear"),
    ("application-sql", "0369a1", "database"),
    ("application-x-sqlite3", "0369a1", "database"),
    ("application-epub+zip", "9a3412", "book"),
    ("application-pgp-keys", "854d0e", "key"),
];

const CONTEXTS: [(&str, &str); 5] =
    [("apps", "Applications"), ("places", "Places"), ("mimetypes", "MimeTypes"), ("devices", "Devices"), ("status", "Status")];
const SUFFIXES: [&str; 10] = ["stable", "launcher", "client", "desktop", "browser", "bin", "beta", "dev", "gtk", "flatpak"];

/// A hex colour channel as int(text, 16) reads it; None is the script's ValueError.
fn hex2(s: &str, i: usize) -> Option<i128> {
    let part: String = s.chars().skip(i).take(2).collect();
    pycompat::int16(&part)
}

/// A file type's icon: a page in its colour with a white glyph.
pub fn mime_icon(colour: &str, glyph: &str) -> String {
    let dark =
        |c: &str, k: f64| -> String { [0, 2, 4].iter().map(|&i| format!("{:02x}", (hex2(c, i).unwrap() as f64 * k) as i64)).collect() };
    let glyph_svg = GLYPHS.iter().find(|(n, _)| *n == glyph).map(|(_, s)| *s).unwrap();
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"256\" height=\"256\" viewBox=\"0 0 1024 1024\">\
<path d=\"M260 72H650L840 262V900a52 52 0 0 1-52 52H260a52 52 0 0 1-52-52V124a52 52 0 0 1 52-52Z\" fill=\"#{colour}\"/>\
<path d=\"M650 72V214a48 48 0 0 0 48 48H840Z\" fill=\"#{}\"/>\
<svg x=\"318\" y=\"430\" width=\"412\" height=\"412\" viewBox=\"0 0 24 24\">{glyph_svg}</svg></svg>\n",
        dark(colour, 0.72)
    )
}

/// The macOS / iOS tile: a superellipse (n = 5) through 240 points.
pub fn squircle() -> String {
    let (n, a) = (5.0f64, 496.0f64);
    let mut pts = Vec::with_capacity(240);
    for i in 0..240 {
        let t = 2.0 * std::f64::consts::PI * i as f64 / 240.0;
        let (c, s) = (t.cos(), t.sin());
        let x = 512.0 + a * c.abs().powf(2.0 / n).copysign(c);
        let y = 512.0 + a * s.abs().powf(2.0 / n).copysign(s);
        pts.push(format!("{},{}", f1(x), f1(y)));
    }
    format!("<path d=\"M{} Z\" fill=\"{{fill}}\"/>", pts.join(" L"))
}

fn shape_tile(shape: &str, fill: &str) -> String {
    let template = match shape {
        "squircle" => squircle(),
        "rounded" => "<rect x=\"40\" y=\"40\" width=\"944\" height=\"944\" rx=\"190\" fill=\"{fill}\"/>".into(),
        "circle" => "<circle cx=\"512\" cy=\"512\" r=\"480\" fill=\"{fill}\"/>".into(),
        _ => "<polygon points=\"512,40 921,276 921,748 512,984 103,748 103,276\" fill=\"{fill}\" stroke=\"{fill}\" stroke-width=\"64\" stroke-linejoin=\"round\"/>".into(),
    };
    template.replace("{fill}", fill)
}

/// 0.2126 r + 0.7152 g + 0.0722 b of a colour's channels (0..1); None where int(.., 16) failed.
pub fn luminance(hexc: &str) -> Option<f64> {
    let r = hex2(hexc, 0)? as f64 / 255.0;
    let g = hex2(hexc, 2)? as f64 / 255.0;
    let b = hex2(hexc, 4)? as f64 / 255.0;
    Some(0.2126 * r + 0.7152 * g + 0.0722 * b)
}

/// The drawing inside an svg file and its viewBox, as the script cut them out; Err where it stopped with a traceback.
pub fn inner(svg: &str) -> Result<(Vec<f64>, String), String> {
    // re.search(r'viewBox="([^"]+)"')
    let mut vb_text = None;
    let mut from = 0;
    while let Some(k) = svg[from..].find("viewBox=\"") {
        let start = from + k + 9;
        match svg[start..].find('"') {
            Some(end) if end > 0 => {
                vb_text = Some(&svg[start..start + end]);
                break;
            }
            _ => from = from + k + 1,
        }
    }
    let vb_text = vb_text.ok_or("'NoneType' object has no attribute 'group'")?;
    let mut vb = vec![];
    for v in pycompat::split(vb_text) {
        vb.push(py::float(v).ok_or_else(|| format!("could not convert string to float: {}", pycompat::repr(v)))?);
    }
    // re.sub(r"^.*?<svg[^>]*>", "", count=1, flags=re.S)
    let mut body = svg;
    if let Some(i) = svg.find("<svg") {
        if let Some(j) = svg[i + 4..].find('>') {
            body = &svg[i + 4 + j + 1..];
        }
    }
    // re.sub(r"</svg>\s*$", "")
    let trimmed = body.trim_end_matches(pycompat::isspace);
    let body = trimmed.strip_suffix("</svg>").unwrap_or(body);
    // re.sub(r"<title>.*?</title>", "", flags=re.S)
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    loop {
        match rest.find("<title>") {
            Some(i) => match rest[i..].find("</title>") {
                Some(j) => {
                    out.push_str(&rest[..i]);
                    rest = &rest[i + j + 8..];
                }
                None => {
                    out.push_str(rest);
                    break;
                }
            },
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    Ok((vb, out))
}

/// One icon: the logo (single colour) on the tile the shape and style ask for.
pub fn make(shape: &str, style: &str, svg: &str, brand: &str, purple: &str) -> Result<String, String> {
    let (vb, body) = inner(svg)?;
    if vb.len() < 4 {
        return Err("list index out of range".into());
    }
    let size = GLYPH_SIZE.iter().find(|(s, _)| *s == shape).unwrap().1 * 1024.0;
    let m = py::max(vb[2], vb[3]);
    if m == 0.0 {
        return Err("float division by zero".into());
    }
    let scale = size / m;
    let (w, h) = (vb[2] * scale, vb[3] * scale);
    let (x, y) = ((1024.0 - w) / 2.0, (1024.0 - h) / 2.0);
    let dark_brand = luminance(brand).ok_or_else(|| format!("invalid literal for int() with base 16: {}", pycompat::repr(brand)))? < 0.14;
    let (tile, mut glyph) = match style {
        "purple" => ("1b1b22".to_string(), purple.to_string()),
        "dark" => ("1b1b22".to_string(), if dark_brand { "ffffff".into() } else { brand.to_string() }),
        _ => (if dark_brand { "2b2b36".into() } else { brand.to_string() }, "ffffff".to_string()),
    };
    let tilesvg = if shape == "glyph" {
        glyph = if style == "purple" {
            purple.to_string()
        } else if dark_brand {
            "ffffff".into()
        } else {
            brand.to_string()
        };
        String::new()
    } else {
        shape_tile(shape, &format!("#{tile}"))
    };
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"256\" height=\"256\" viewBox=\"0 0 1024 1024\">{tilesvg}<svg x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\" fill=\"#{glyph}\">{body}</svg></svg>\n",
        f1(x),
        f1(y),
        f1(w),
        f1(h),
        g(vb[0]),
        g(vb[1]),
        g(vb[2]),
        g(vb[3]),
    ))
}

/// $XDG_CONFIG_HOME (even set to nothing), else ~/.config
fn config_home() -> PathBuf {
    py::env_or("XDG_CONFIG_HOME", || py::expanduser("/.config"))
}

/// The desktop's accent colour (Settings > Appearance: `accent = r,g,b` in ~/.config/yggdrasil/appearance.conf) as rrggbb;
/// Labyrinth purple when it is not set, or when the first accent line that is not three whole numbers is met.
pub fn accent_hex() -> String {
    let path = join(&join(&config_home(), "yggdrasil"), "appearance.conf");
    let Ok(Ok(text)) = py::read_text(&path, true) else {
        return PURPLE.into();
    };
    for line in text.split_inclusive('\n') {
        let before_hash = line.split('#').next().unwrap();
        let (key, value) = before_hash.split_once('=').unwrap_or((before_hash, ""));
        if pycompat::strip(key) == "accent" {
            let parts: Vec<&str> = value.split(',').collect();
            if parts.len() != 3 {
                return PURPLE.into();
            }
            let mut c = [0i128; 3];
            for (k, p) in parts.iter().enumerate() {
                match pycompat::int(p) {
                    Some(v) => c[k] = v,
                    None => return PURPLE.into(),
                }
            }
            if c.iter().all(|v| (0..256).contains(v)) {
                return format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
            }
        }
    }
    PURPLE.into()
}

pub fn conf_path() -> PathBuf {
    join(&join(&config_home(), "labyrinth"), "brand-pack.json")
}

/// (icon name, app name) of each desktop file here: the first Icon= and Name= lines, icons that are not paths. An icon
/// name with "/" or a NUL in it is left out (the script wrote such a logo outside the pack, or stopped).
pub fn installed_icon_names() -> Vec<(String, String)> {
    let mut out = vec![];
    let dirs = [
        PathBuf::from("/usr/share/applications"),
        py::expanduser("/.local/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
    ];
    for d in dirs {
        for f in py::listing(&d, ".desktop") {
            let Ok(Ok(text)) = py::read_text(&f, true) else {
                continue;
            };
            let (mut icon, mut name) = (String::new(), String::new());
            for line in text.split_inclusive('\n') {
                if let Some(v) = line.strip_prefix("Icon=").filter(|_| icon.is_empty()) {
                    icon = pycompat::strip(v).to_string();
                } else if let Some(v) = line.strip_prefix("Name=").filter(|_| name.is_empty()) {
                    name = pycompat::strip(v).to_string();
                }
            }
            if !icon.is_empty() && !icon.contains('/') && !icon.contains('\0') {
                out.push((icon, name));
            }
        }
    }
    out
}

/// The logo for an app: its icon name or app name (whole, in parts, without a suffix such as "stable"), as is or through
/// the aliases.
pub fn find_logo(icon: &str, name: &str, slugs: &std::collections::HashSet<String>) -> Option<String> {
    let mut cands: Vec<String> = vec![];
    for c in [icon, name] {
        let n = py::norm(c);
        cands.push(n.clone());
        for p in c.to_lowercase().split(['.', '-', '_', ' ']) {
            cands.push(py::norm(p));
        }
        for s in SUFFIXES {
            if n.ends_with(s) {
                cands.push(n[..n.len() - s.len()].to_string());
            }
        }
    }
    for c in cands {
        if let Some((_, logo)) = ALIASES.iter().rev().find(|(k, _)| *k == c) {
            if slugs.contains(*logo) {
                return Some(logo.to_string());
            }
        }
        if slugs.contains(&c) {
            return Some(c);
        }
    }
    None
}

fn is_native_name(icon: &str) -> bool {
    NATIVE.iter().any(|(_, v)| v.iter().any(|(_, n)| *n == icon))
}

pub fn parser() -> Parser {
    Parser::new(
        Parser::prog_from_argv0(),
        "Build an icon pack of app logos.",
        vec![
            Arg { flags: &["--shape"], dest: "shape", kind: Kind::Value { choices: Some(&SHAPES), default: Some("glyph") }, help: None },
            Arg { flags: &["--style"], dest: "style", kind: Kind::Value { choices: Some(&STYLES), default: Some("brand") }, help: None },
            Arg { flags: &["--name"], dest: "name", kind: Kind::Value { choices: None, default: Some("Liki") }, help: None },
            Arg { flags: &["--out"], dest: "out", kind: Kind::Value { choices: None, default: None }, help: Some("folder to write (default ~/.local/share/icons/NAME)") },
            Arg {
                flags: &["--natives-only"],
                dest: "natives_only",
                kind: Kind::Switch,
                help: Some("only the pack's own icons (folders, drives, file types, Yggdrasil's apps, the default app icon), no app logos: what the package ships"),
            },
            Arg { flags: &["--rebuild"], dest: "rebuild", kind: Kind::Switch, help: Some("build again with the shape, style and name of the last build (used when the accent changes)") },
        ],
    )
}

fn write(path: &Path, text: &str) {
    if let Err(e) = std::fs::write(path, text) {
        crate::crash("liki-build", format!("{}: {e}", path.display()));
    }
}

/// A pack name the last build may have remembered: not empty, printable, one folder name.
fn usable_name(v: &str) -> bool {
    !v.is_empty() && v.chars().all(pycompat::isprintable) && !v.contains('/') && v != "." && v != ".."
}

/// liki-build; the exit code.
pub fn main(argv: &[String]) -> i32 {
    let p = parser();
    let a = match p.parse(argv) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let mut shape = a.str("shape").unwrap().to_string();
    let mut style = a.str("style").unwrap().to_string();
    let mut name = a.str("name").unwrap().to_string();
    // `--out ""` counts as no --out, as in the script
    let out_arg = a.str("out").filter(|s| !s.is_empty()).map(PathBuf::from);
    let natives_only = a.flag("natives_only");
    if a.flag("rebuild") {
        let last = match py::read_text(&conf_path(), false) {
            Ok(Ok(text)) => crate::pyjson::loads(&text),
            _ => None,
        };
        let Some(last @ crate::pyjson::Json::Object(_)) = last else {
            println!("No earlier build to repeat.");
            return 0;
        };
        // what the last build remembered; a value that is not a shape, a style or a usable pack name is left as given
        let remembered = |key: &str, ok: &dyn Fn(&str) -> bool, given: String| -> String {
            match last.get(key) {
                Some(crate::pyjson::Json::Str(v)) if ok(v) => v.clone(),
                _ => given,
            }
        };
        shape = remembered("shape", &|v| SHAPES.contains(&v), shape);
        style = remembered("style", &|v| STYLES.contains(&v), style);
        name = remembered("name", &usable_name, name);
    }
    let accent = accent_hex();

    let src: PathBuf = py::env_nonempty("LIKI_BRAND_ICONS")
        .or_else(|| py::env_nonempty("LABYRINTH_BRAND_ICONS"))
        .map(PathBuf::from)
        .unwrap_or_else(|| "/usr/share/liki/brand-icons".into());
    let colors_path = join(&src, "colors.json");
    let colors: Vec<(String, String)> = match py::read_text(&colors_path, false) {
        Ok(Ok(text)) => match pycompat::json::loads(&text) {
            Ok(pycompat::json::Value::Object(m)) => {
                m.into_iter().map(|(k, v)| (k, v.as_str().map(str::to_string).unwrap_or_default())).collect()
            }
            _ => crate::crash("liki-build", format!("{}: not a JSON object of colours", colors_path.display())),
        },
        Ok(Err(e)) => crate::crash("liki-build", e),
        Err(e) => crate::crash("liki-build", format!("{}: {e}", colors_path.display())),
    };
    let slugs: std::collections::HashSet<String> = colors.iter().map(|(k, _)| k.clone()).collect();
    let colour_of = |slug: &str| colors.iter().rev().find(|(k, _)| k == slug).map(|(_, v)| v.to_lowercase()).unwrap();

    let out =
        out_arg.clone().unwrap_or_else(|| join(&join(&py::env_or("XDG_DATA_HOME", || py::expanduser("/.local/share")), "icons"), &name));
    let apps = join(&join(&out, "apps"), "scalable");
    for (ctx, _) in CONTEXTS {
        let d = join(&join(&out, ctx), "scalable");
        if let Err(e) = std::fs::create_dir_all(&d) {
            crate::crash("liki-build", format!("{}: {e}", d.display()));
        }
        for f in py::listing(&d, ".svg") {
            if let Err(e) = std::fs::remove_file(&f) {
                crate::crash("liki-build", format!("{}: {e}", f.display()));
            }
        }
    }

    let read_svg = |path: PathBuf| -> String {
        match py::read_text(&path, false) {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => crate::crash("liki-build", e),
            Err(e) => crate::crash("liki-build", format!("{}: {e}", path.display())),
        }
    };
    let made = |shape: &str, style: &str, svg: &str, brand: &str| -> String {
        make(shape, style, svg, brand, &accent).unwrap_or_else(|e| crate::crash("liki-build", e))
    };

    let mut written = 0usize;
    // Yggdrasil's own apps and places
    for (key, names) in NATIVE {
        let svg = read_svg(join(&join(&src, "native"), format!("{key}.svg")));
        let icon = made(&shape, &style, &svg, &accent);
        for (ctx, n) in names.iter() {
            write(&join(&join(&join(&out, ctx), "scalable"), format!("{n}.svg")), &icon);
            written += 1;
        }
    }
    // file types, each in its own colour (not the accent)
    for (n, colour, glyph) in MIME {
        write(&join(&join(&join(&out, "mimetypes"), "scalable"), format!("{n}.svg")), &mime_icon(colour, glyph));
        written += 1;
    }
    // the system's symbolic icons (Lucide, ISC, in currentColor so the program tints them)
    let symbolic = join(&PathBuf::from(py::dirname(src.as_os_str())), "symbolic");
    for f in py::listing(&symbolic, ".svg") {
        let dest = join(&join(&join(&out, "status"), "scalable"), f.file_name().unwrap_or_default());
        if let Err(e) = std::fs::copy(&f, &dest) {
            crate::crash("liki-build", format!("{}: {e}", f.display()));
        }
        written += 1;
    }
    // the brands' apps (the file is opened before the icon is made, as the script's open(..).write(make(..)) did)
    let logo = |path: PathBuf, slug: &str| {
        let svg = read_svg(join(&join(&src, "icons"), format!("{slug}.svg")));
        let mut f = std::fs::File::create(&path).unwrap_or_else(|e| crate::crash("liki-build", format!("{}: {e}", path.display())));
        let icon = made(&shape, &style, &svg, &colour_of(slug));
        if let Err(e) = std::io::Write::write_all(&mut f, icon.as_bytes()) {
            crate::crash("liki-build", format!("{}: {e}", path.display()));
        }
    };
    let mut seen: std::collections::HashSet<String> = Default::default();
    if !natives_only {
        for (icon_name, app_name) in installed_icon_names() {
            let Some(slug) = find_logo(&icon_name, &app_name, &slugs).filter(|s| !s.is_empty()) else {
                continue;
            };
            if seen.contains(&icon_name) || is_native_name(&icon_name) {
                continue;
            }
            seen.insert(icon_name.clone());
            logo(join(&apps, OsString::from(format!("{icon_name}.svg"))), &slug);
            written += 1;
        }
        for (icon_name, slug) in EXTRA {
            if seen.contains(*icon_name) || !slugs.contains(*slug) {
                continue;
            }
            seen.insert(icon_name.to_string());
            logo(join(&apps, format!("{icon_name}.svg")), slug);
            written += 1;
        }
    }
    let dirs: Vec<String> = CONTEXTS.iter().map(|(c, _)| format!("{c}/scalable")).collect();
    let sections: String = CONTEXTS
        .iter()
        .map(|(c, x)| format!("\n[{c}/scalable]\nSize=128\nMinSize=8\nMaxSize=1024\nType=Scalable\nContext={x}\n"))
        .collect();
    let text = format!(
        "[Icon Theme]\nName={name}\nComment=Yggdrasil's icon pack: app logos from Simple Icons ({shape}, {style}) and its own glyphs; an app without an icon gets the default app icon\nInherits=hicolor\nDirectories={}\n{sections}",
        dirs.join(",")
    );
    write(&join(&out, "index.theme"), &text);
    // GTK's icon cache, when the tool is there (it is optional: the pack works without it)
    let _ = std::process::Command::new("gtk-update-icon-cache").arg("-qf").arg(&out).stderr(std::process::Stdio::null()).status();
    if out_arg.is_none() {
        // remembered, so the pack can be made again in the new accent colour when it changes (--rebuild)
        let conf = conf_path();
        let parent = PathBuf::from(py::dirname(conf.as_os_str()));
        if std::fs::create_dir_all(&parent).is_ok() {
            let mut m = pycompat::json::Map::new();
            m.insert("shape".into(), shape.clone().into());
            m.insert("style".into(), style.clone().into());
            m.insert("name".into(), name.clone().into());
            let _ = std::fs::write(&conf, pycompat::json::dumps(&pycompat::json::Value::Object(m)));
        }
    }
    println!("{name}: {written} icons ({shape}, {style}) in {}", out.display());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_is_found_through_names_parts_suffixes_and_aliases() {
        let slugs: std::collections::HashSet<String> =
            ["firefox", "vlcmediaplayer", "visualstudiocode", "steam", "spotify"].iter().map(|s| s.to_string()).collect();
        assert_eq!(find_logo("org.mozilla.firefox", "Firefox", &slugs).as_deref(), Some("firefox"));
        assert_eq!(find_logo("vlc", "VLC media player", &slugs).as_deref(), Some("vlcmediaplayer"));
        assert_eq!(find_logo("code", "Visual Studio Code", &slugs).as_deref(), Some("visualstudiocode"));
        assert_eq!(find_logo("spotify-launcher", "", &slugs).as_deref(), Some("spotify"));
        assert_eq!(find_logo("steam_icon_123", "Some Game", &slugs).as_deref(), Some("steam"));
        assert_eq!(find_logo("nothing", "here", &slugs), None);
    }

    #[test]
    fn an_icon_is_cut_out_and_placed() {
        let svg = "<?xml?>\n<svg role=\"img\" viewBox=\"0 0 24 24\" xmlns=\"x\"><title>A</title><path d=\"M0 0h24\"/></svg>\n";
        let (vb, body) = inner(svg).unwrap();
        assert_eq!(vb, vec![0.0, 0.0, 24.0, 24.0]);
        assert_eq!(body, "<path d=\"M0 0h24\"/>");
        let icon = make("circle", "brand", svg, "ff7139", PURPLE).unwrap();
        assert!(icon.contains("<circle cx=\"512\" cy=\"512\" r=\"480\" fill=\"#ff7139\"/>"));
        assert!(icon.contains("<svg x=\"245.8\" y=\"245.8\" width=\"532.5\" height=\"532.5\" viewBox=\"0 0 24 24\" fill=\"#ffffff\">"));
        assert!(make("glyph", "brand", "<svg viewBox=\"0 0 0 0\"></svg>", "ffffff", PURPLE).is_err());
        assert!(make("glyph", "brand", "<svg></svg>", "ffffff", PURPLE).is_err());
    }

    #[test]
    fn a_dark_brand_gets_a_lighter_tile() {
        assert!(luminance("000000").unwrap() < 0.14);
        let icon = make("rounded", "brand", "<svg viewBox=\"0 0 24 24\"><path/></svg>", "000000", PURPLE).unwrap();
        assert!(icon.contains("fill=\"#2b2b36\""));
        assert!(luminance("zz0000").is_none());
    }

    #[test]
    fn mime_pages_are_darkened_at_the_fold() {
        assert!(mime_icon("ffffff", "lines").contains("fill=\"#b7b7b7\""));
    }
}
