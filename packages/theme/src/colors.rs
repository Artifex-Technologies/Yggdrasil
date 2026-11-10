//! The colours: Yggdrasil's palette, the colour scheme for programs built on KDE's libraries, the GTK colour definitions, and
//! the system GTK 3 and GTK 4 themes (GTK 4's own theme with its colours replaced). Every number is worked out as the Python
//! script worked it out (the same float operations in the same order, round() with halves to even).

pub type Rgb = (i64, i64, i64);

pub const DEFAULT_ACCENT: Rgb = (124, 84, 214);

/// One of the two palettes of labyrinth-ui (theme.rs): window, view, field (buttons), strip (headers), borders, text.
pub struct Palette {
    pub bg: Rgb,
    pub alt: Rgb,
    pub view: Rgb,
    pub field: Rgb,
    pub field_alt: Rgb,
    pub strip: Rgb,
    pub strip_alt: Rgb,
    pub border: Rgb,
    pub text: Rgb,
    pub dim: Rgb,
    pub red: Rgb,
    pub amber: Rgb,
    pub green: Rgb,
}

pub const DARK: Palette = Palette {
    bg: (16, 17, 22),
    alt: (19, 20, 26),
    view: (14, 15, 20),
    field: (24, 25, 32),
    field_alt: (32, 34, 44),
    strip: (11, 12, 16),
    strip_alt: (14, 15, 20),
    border: (38, 40, 52),
    text: (232, 233, 238),
    dim: (140, 143, 157),
    red: (232, 92, 106),
    amber: (225, 170, 70),
    green: (84, 190, 120),
};

pub const LIGHT: Palette = Palette {
    bg: (250, 250, 252),
    alt: (244, 244, 248),
    view: (255, 255, 255),
    field: (243, 243, 247),
    field_alt: (232, 232, 240),
    strip: (232, 232, 238),
    strip_alt: (240, 240, 245),
    border: (214, 214, 224),
    text: (28, 29, 36),
    dim: (110, 113, 128),
    red: (190, 50, 60),
    amber: (196, 130, 30),
    green: (46, 140, 80),
};

fn pal(dark: bool) -> &'static Palette {
    if dark {
        &DARK
    } else {
        &LIGHT
    }
}

const WHITE: Rgb = (255, 255, 255);
const BLACK: Rgb = (0, 0, 0);

fn r(x: f64) -> i64 {
    pycompat::round(x)
}

pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let m = |x: i64, y: i64| r(x as f64 + (y - x) as f64 * t);
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

pub fn lum(c: Rgb) -> f64 {
    let f = |v: i64| {
        let v = v as f64 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.0) + 0.7152 * f(c.1) + 0.0722 * f(c.2)
}

/// Text on the accent: white, or near-black when the accent itself is light (the same rule as the apps').
pub fn on_accent(accent: Rgb) -> Rgb {
    if lum(accent) > 0.45 {
        (24, 20, 36)
    } else {
        WHITE
    }
}

pub fn rgb(c: Rgb) -> String {
    format!("{},{},{}", c.0, c.1, c.2)
}

pub fn css(c: Rgb) -> String {
    format!("rgb({},{},{})", c.0, c.1, c.2)
}

/// "R,G,B" with each 0 to 255 (each read as int() reads a number), else None.
pub fn parse_accent(s: &str) -> Option<Rgb> {
    let p: Vec<i128> = s.split(',').map(pycompat::int).collect::<Option<Vec<_>>>()?;
    if p.len() == 3 && p.iter().all(|v| (0..=255).contains(v)) {
        Some((p[0] as i64, p[1] as i64, p[2] as i64))
    } else {
        None
    }
}

/// The colour scheme for programs built on KDE's libraries.
pub fn scheme(dark: bool, accent: Rgb) -> String {
    let p = pal(dark);
    let name = if dark { "YggdrasilDark" } else { "YggdrasilLight" };
    // readable accent text on the background: lifted on dark, darkened on light
    let acc_text = if dark { mix(accent, WHITE, 0.25) } else { mix(accent, BLACK, 0.15) };
    let link = if dark { mix(accent, WHITE, 0.35) } else { mix(accent, BLACK, 0.25) };
    let visited = mix(accent, (215, 130, 200), 0.5);
    let sel_alt = mix(accent, p.bg, 0.25);
    let block = |bg: Rgb, alt: Rgb, fg: Option<Rgb>, deco: Option<Rgb>, active: Option<Rgb>| -> Vec<(&'static str, Rgb)> {
        let fg = fg.unwrap_or(p.text);
        vec![
            ("BackgroundAlternate", alt),
            ("BackgroundNormal", bg),
            ("DecorationFocus", deco.unwrap_or(accent)),
            ("DecorationHover", deco.unwrap_or(accent)),
            ("ForegroundActive", active.unwrap_or(acc_text)),
            ("ForegroundInactive", p.dim),
            ("ForegroundLink", link),
            ("ForegroundNegative", p.red),
            ("ForegroundNeutral", p.amber),
            ("ForegroundNormal", fg),
            ("ForegroundPositive", p.green),
            ("ForegroundVisited", visited),
        ]
    };
    let sections = [
        ("Button", block(p.field, p.field_alt, None, None, None)),
        ("Complementary", block(p.strip, p.strip_alt, None, None, None)),
        ("Header", block(p.strip, p.strip_alt, None, None, None)),
        ("Selection", block(accent, sel_alt, Some(on_accent(accent)), Some(mix(accent, WHITE, 0.3)), Some(on_accent(accent)))),
        ("Tooltip", block(p.field, p.field_alt, None, None, None)),
        ("View", block(p.view, p.alt, None, None, None)),
        ("Window", block(p.bg, p.alt, None, None, None)),
    ];
    let mut out: Vec<String> = vec!["# Yggdrasil colour scheme (written by ygg-theme from the palette Yggdrasil's own programs use)".into(), String::new()];
    out.extend(
        ["[ColorEffects:Disabled]", "Color=56,56,56", "ColorAmount=0", "ColorEffect=0", "ContrastAmount=0.65", "ContrastEffect=1", "IntensityAmount=0.1", "IntensityEffect=2", ""]
            .map(String::from),
    );
    out.extend(
        [
            "[ColorEffects:Inactive]",
            "ChangeSelectionColor=true",
            "Color=112,111,110",
            "ColorAmount=0.025",
            "ColorEffect=2",
            "ContrastAmount=0.1",
            "ContrastEffect=2",
            "Enable=false",
            "IntensityAmount=0",
            "IntensityEffect=0",
            "",
        ]
        .map(String::from),
    );
    for (sec, items) in sections {
        out.push(format!("[Colors:{sec}]"));
        out.extend(items.iter().map(|(k, v)| format!("{k}={}", rgb(*v))));
        out.push(String::new());
    }
    out.extend([
        "[General]".to_string(),
        format!("ColorScheme={name}"),
        format!("Name={}", if dark { "Yggdrasil dark" } else { "Yggdrasil light" }),
        "shadeSortColumn=true".into(),
        String::new(),
    ]);
    out.extend(["[KDE]", "contrast=4", ""].map(String::from));
    out.extend([
        "[WM]".to_string(),
        format!("activeBackground={}", rgb(p.strip)),
        format!("activeForeground={}", rgb(p.text)),
        format!("inactiveBackground={}", rgb(p.bg)),
        format!("inactiveForeground={}", rgb(p.dim)),
        format!("activeBlend={}", rgb(p.text)),
        format!("inactiveBlend={}", rgb(p.dim)),
        String::new(),
    ]);
    out.join("\n")
}

/// GTK colour definitions: libadwaita's names (GTK 4) and Adwaita's theme_* names (GTK 3).
pub fn gtk_colors(dark: bool, accent: Rgb) -> String {
    let p = pal(dark);
    let fg_on = on_accent(accent);
    let acc = if dark { mix(accent, WHITE, 0.25) } else { mix(accent, BLACK, 0.15) };
    let pick = |light: Rgb, dark_value: Rgb| if dark { dark_value } else { light };
    let d: Vec<(&str, Rgb)> = vec![
        ("accent_bg_color", accent),
        ("accent_fg_color", fg_on),
        ("accent_color", acc),
        ("destructive_bg_color", pick(p.red, (180, 58, 69))),
        ("destructive_fg_color", WHITE),
        ("destructive_color", p.red),
        ("success_bg_color", pick(p.green, (58, 158, 94))),
        ("success_fg_color", WHITE),
        ("success_color", p.green),
        ("warning_bg_color", pick(p.amber, (201, 146, 58))),
        ("warning_fg_color", (24, 20, 36)),
        ("warning_color", p.amber),
        ("error_bg_color", pick(p.red, (180, 58, 69))),
        ("error_fg_color", WHITE),
        ("error_color", p.red),
        ("window_bg_color", p.bg),
        ("window_fg_color", p.text),
        ("view_bg_color", p.view),
        ("view_fg_color", p.text),
        ("headerbar_bg_color", p.strip),
        ("headerbar_fg_color", p.text),
        ("headerbar_border_color", p.border),
        ("headerbar_backdrop_color", p.bg),
        ("headerbar_shade_color", p.strip),
        ("headerbar_darker_shade_color", p.strip),
        ("card_bg_color", p.field),
        ("card_fg_color", p.text),
        ("card_shade_color", p.border),
        ("dialog_bg_color", p.bg),
        ("dialog_fg_color", p.text),
        ("popover_bg_color", p.field),
        ("popover_fg_color", p.text),
        ("popover_shade_color", p.border),
        ("sidebar_bg_color", p.strip_alt),
        ("sidebar_fg_color", p.text),
        ("sidebar_backdrop_color", p.bg),
        ("sidebar_shade_color", p.border),
        ("secondary_sidebar_bg_color", p.strip_alt),
        ("secondary_sidebar_fg_color", p.text),
        ("thumbnail_bg_color", p.field),
        ("thumbnail_fg_color", p.text),
        ("shade_color", BLACK),
        ("scrollbar_outline_color", p.border),
        // GTK 3's Adwaita
        ("theme_bg_color", p.bg),
        ("theme_fg_color", p.text),
        ("theme_base_color", p.view),
        ("theme_text_color", p.text),
        ("theme_selected_bg_color", accent),
        ("theme_selected_fg_color", fg_on),
        ("theme_unfocused_bg_color", p.bg),
        ("theme_unfocused_fg_color", p.dim),
        ("theme_unfocused_base_color", p.view),
        ("theme_unfocused_text_color", p.dim),
        ("theme_unfocused_selected_bg_color", mix(accent, p.bg, 0.35)),
        ("theme_unfocused_selected_fg_color", p.text),
        ("borders", p.border),
        ("unfocused_borders", p.border),
        ("insensitive_bg_color", p.alt),
        ("insensitive_fg_color", p.dim),
        ("insensitive_base_color", p.view),
        ("insensitive_borders", p.border),
        ("unfocused_insensitive_borders", p.border),
        ("content_view_bg", p.view),
        ("link_color", if dark { mix(accent, WHITE, 0.35) } else { mix(accent, BLACK, 0.25) }),
        ("wm_title", p.text),
        ("wm_unfocused_title", p.dim),
        ("wm_bg", p.strip),
        ("wm_bg_a", p.strip),
        ("wm_bg_b", p.strip),
    ];
    let mut lines = vec!["/* Yggdrasil colours (written by ygg-theme from the palette Yggdrasil's own programs use, with the accent) */".to_string()];
    lines.extend(d.iter().map(|(k, v)| format!("@define-color {k} {};", css(*v))));
    lines.join("\n") + "\n"
}

pub fn gtk3_theme_css(dark: bool) -> String {
    format!(
        "/* Yggdrasil for GTK 3: GTK's own Adwaita with Yggdrasil's colours (the accent is written to ~/.config/gtk-3.0/gtk.css by ygg-theme). */\n@import url(\"resource:///org/gtk/libgtk/theme/Adwaita/gtk-contained{}.css\");\n{}",
        if dark { "-dark" } else { "" },
        gtk_colors(dark, DEFAULT_ACCENT)
    )
}

pub const GTK4_RULES: &str = include_str!("gtk4_rules.css");
/// after the colours are replaced a few small parts still need a push to read well on Yggdrasil's dark greys
pub const GTK4_FIXES: &str = include_str!("gtk4_fixes.css");

/// GTK 4's own theme (the "Default" one inside libgtk) has its colours written as fixed values, not named ones. So
/// Yggdrasil's GTK 4 theme is made from it by replacing those values: greys become Yggdrasil's own greys (same lightness
/// order), blues become the accent (same lightness), anything else (reds, greens, yellows: errors, warnings, success)
/// stays. This is done when the package is built, from the GTK version that is installed, and the result is plain CSS.
pub const GREY_RAMP_DARK: &[(i64, Rgb)] = &[
    (0, (0, 0, 0)),
    (30, (11, 12, 16)),
    (45, (14, 15, 20)),
    (53, (16, 17, 22)),
    (60, (28, 29, 38)),
    (75, (40, 42, 54)),
    (95, (56, 58, 72)),
    (140, (100, 102, 120)),
    (190, (150, 153, 168)),
    (230, (222, 223, 230)),
    (255, (255, 255, 255)),
];

pub fn ramp(g: i64, stops: &[(i64, Rgb)]) -> Rgb {
    for w in stops.windows(2) {
        let ((g0, c0), (g1, c1)) = (w[0], w[1]);
        if g <= g1 {
            let t = if g1 == g0 { 0.0 } else { (g - g0) as f64 / (g1 - g0) as f64 };
            let m = |a: i64, b: i64| r(a as f64 + (b - a) as f64 * t);
            return (m(c0.0, c1.0), m(c0.1, c1.1), m(c0.2, c1.2));
        }
    }
    stops[stops.len() - 1].1
}

pub fn hue(c: Rgb) -> f64 {
    let (r_, g, b) = (c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0);
    let mx = r_.max(g).max(b);
    let mn = r_.min(g).min(b);
    if mx == mn {
        return 0.0;
    }
    let d = mx - mn;
    let h = if mx == r_ {
        pycompat::fmod((g - b) / d, 6.0)
    } else if mx == g {
        (b - r_) / d + 2.0
    } else {
        (r_ - g) / d + 4.0
    };
    h * 60.0
}

/// One colour of GTK's own theme as it becomes in Yggdrasil's.
pub fn retint_color(c: Rgb, dark: bool, accent: Rgb) -> Rgb {
    let (r_, g, b) = c;
    let sat = r_.max(g).max(b) - r_.min(g).min(b);
    if sat < 20 {
        // a grey
        return if dark { ramp(r((r_ + g + b) as f64 / 3.0), GREY_RAMP_DARK) } else { c };
    }
    let h = hue(c);
    if b > r_ + 25 && b >= g && (150.0..=270.0).contains(&h) {
        // GTK's blue: the accent, as light or dark as that blue was. GTK's main accent surface is #15539e (white text on it):
        // that one becomes the accent itself, darker blues darker, lighter blues lighter
        let t = lum(c) / lum((21, 83, 158));
        return if t < 1.0 { mix(accent, BLACK, 0.85f64.min(1.0 - t)) } else { mix(accent, WHITE, 0.7f64.min((t - 1.0) * 0.25)) };
    }
    c
}

/// GTK's own CSS with its colours replaced: #rrggbb and #rgb, and rgb()/rgba() with whole numbers.
pub fn retint_css(text: &str, dark: bool, accent: Rgb) -> String {
    let hexes = regex::Regex::new(r"#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b").unwrap();
    let text = hexes.replace_all(text, |m: &regex::Captures| {
        let t = &m[0];
        let mut v = t[1..].to_string();
        if v.len() == 3 {
            v = v.chars().flat_map(|ch| [ch, ch]).collect();
        }
        let c = |i: usize| i64::from_str_radix(&v[i..i + 2], 16).unwrap();
        let n = retint_color((c(0), c(2), c(4)), dark, accent);
        format!("#{:02x}{:02x}{:02x}", n.0, n.1, n.2)
    });
    let s = pycompat::PY_SPACE_CLASS;
    let funcs = regex::Regex::new(&format!(r"(rgba?)\({s}*(\d+{s}*,{s}*\d+{s}*,{s}*\d+{s}*(?:,{s}*[\d.]+{s}*)?)\)")).unwrap();
    funcs
        .replace_all(&text, |m: &regex::Captures| {
            let nums: Vec<String> = m[2].split(',').map(|x| pycompat::strip(x).to_string()).collect();
            // int(float(x)) of whole numbers (the pattern takes digits only for the first three)
            let n3: Vec<i64> = nums[..3].iter().map(|x| x.parse::<f64>().map(|f| f.trunc() as i64).unwrap_or_else(|_| pycompat::int(x).unwrap_or(0) as i64)).collect();
            let n = retint_color((n3[0], n3[1], n3[2]), dark, accent);
            let extra = if nums.len() > 3 { format!(", {}", nums[3]) } else { String::new() };
            format!("{}({}, {}, {}{extra})", &m[1], n.0, n.1, n.2)
        })
        .into_owned()
}

/// GTK 4's own theme file from the installed libgtk, as the script read it through python-gobject: libgtk-4 is opened at
/// run time, initialised (gtk_init_check; without a screen that is fine), and the theme read from its resources.
pub fn gtk_own_css(dark: bool) -> Option<String> {
    use std::ffi::{c_char, c_void, CString};
    type InitCheck = unsafe extern "C" fn() -> i32;
    type Lookup = unsafe extern "C" fn(*const c_char, u32, *mut *mut c_void) -> *mut c_void;
    type GetData = unsafe extern "C" fn(*mut c_void, *mut usize) -> *const u8;
    type Unref = unsafe extern "C" fn(*mut c_void);
    unsafe {
        let lib = libc::dlopen(c"libgtk-4.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
        if lib.is_null() {
            return None;
        }
        let sym = |name: &std::ffi::CStr| {
            let p = libc::dlsym(lib, name.as_ptr());
            (!p.is_null()).then_some(p)
        };
        let init: InitCheck = std::mem::transmute(sym(c"gtk_init_check")?);
        let lookup: Lookup = std::mem::transmute(sym(c"g_resources_lookup_data")?);
        let get_data: GetData = std::mem::transmute(sym(c"g_bytes_get_data")?);
        let unref: Unref = std::mem::transmute(sym(c"g_bytes_unref")?);
        init();
        let path = CString::new(format!("/org/gtk/libgtk/theme/Default/Default-{}.css", if dark { "dark" } else { "light" })).ok()?;
        let mut err: *mut c_void = std::ptr::null_mut();
        let bytes = lookup(path.as_ptr(), 0, &mut err);
        if bytes.is_null() {
            return None;
        }
        let mut len = 0usize;
        let data = get_data(bytes, &mut len);
        let text = if data.is_null() { None } else { String::from_utf8(std::slice::from_raw_parts(data, len).to_vec()).ok() };
        unref(bytes);
        text
    }
}

/// Each selector gets `window ` in front, so these rules are more specific than GTK's own theme (which they are written
/// after) and win.
pub fn scope_gtk4(rules: &str) -> String {
    let mut out = vec![];
    for line in pycompat::strip(rules).split('\n') {
        if line.starts_with("/*") || pycompat::strip(line).is_empty() {
            out.push(line.to_string());
            continue;
        }
        let (sel, body) = line.split_once('{').unwrap_or((line, ""));
        let parts: Vec<String> = sel
            .split(',')
            .map(|p| {
                let p = pycompat::strip(p);
                match p {
                    "window" => "window.background".to_string(),
                    ".background" => "window .background".to_string(),
                    _ => format!("window {p}"),
                }
            })
            .collect();
        out.push(format!("{} {{{body}", parts.join(", ")));
    }
    out.join("\n") + "\n"
}

/// The system GTK 4 theme: GTK's own with Yggdrasil's colours, or (without libgtk-4) GTK's own underneath and Yggdrasil's
/// surfaces on top.
pub fn gtk4_theme_css(dark: bool) -> String {
    let head = "/* Yggdrasil for GTK 4: GTK's own theme with its colours replaced by Yggdrasil's (the accent is written to ~/.config/gtk-4.0/gtk.css by ygg-theme). */\n";
    let body = match gtk_own_css(dark) {
        Some(own) => retint_css(&own, dark, DEFAULT_ACCENT) + GTK4_FIXES,
        None => format!(
            "@import url(\"resource:///org/gtk/libgtk/theme/Default/Default-{}.css\");\n{}",
            if dark { "dark" } else { "light" },
            scope_gtk4(GTK4_RULES)
        ),
    };
    format!("{head}{}{body}", gtk_colors(dark, DEFAULT_ACCENT))
}
