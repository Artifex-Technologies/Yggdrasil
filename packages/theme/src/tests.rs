//! The Python script's tests (tests/test_ygg_theme.py, the Scheme class and ini_set), carried over, and the colour rules
//! around them. The program's own files are tested in tests/ygg_theme.rs.

use super::colors::*;
use super::files;

/// A colour scheme as {"[section]": {key: value}}, as the Python test read it.
fn parse_ini(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out: Vec<(String, Vec<(String, String)>)> = vec![];
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            out.push((line.to_string(), vec![]));
        } else if let (Some((k, v)), Some(cur)) = (line.split_once('='), out.last_mut()) {
            cur.1.push((k.to_string(), v.to_string()));
        }
    }
    out
}

fn get<'a>(s: &'a [(String, Vec<(String, String)>)], sec: &str, key: &str) -> Option<&'a str> {
    s.iter().find(|(n, _)| n == sec)?.1.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

const KEYS: &[&str] = &[
    "BackgroundAlternate",
    "BackgroundNormal",
    "DecorationFocus",
    "DecorationHover",
    "ForegroundActive",
    "ForegroundInactive",
    "ForegroundLink",
    "ForegroundNegative",
    "ForegroundNeutral",
    "ForegroundNormal",
    "ForegroundPositive",
    "ForegroundVisited",
];

#[test]
fn test_every_section_has_every_key() {
    for dark in [true, false] {
        let s = parse_ini(&scheme(dark, (124, 84, 214)));
        for sec in ["Button", "Complementary", "Header", "Selection", "Tooltip", "View", "Window"] {
            for k in KEYS {
                let v = get(&s, &format!("[Colors:{sec}]"), k).unwrap_or_else(|| panic!("{sec} {k}"));
                assert_eq!(v.split(',').count(), 3);
            }
        }
    }
}

#[test]
fn test_the_name_says_dark_or_light() {
    // the apps decide dark or light from the scheme's name
    assert_eq!(get(&parse_ini(&scheme(true, (1, 2, 3))), "[General]", "ColorScheme"), Some("YggdrasilDark"));
    assert_eq!(get(&parse_ini(&scheme(false, (1, 2, 3))), "[General]", "ColorScheme"), Some("YggdrasilLight"));
}

#[test]
fn test_the_accent_is_the_selection() {
    assert_eq!(get(&parse_ini(&scheme(true, (10, 200, 30))), "[Colors:Selection]", "BackgroundNormal"), Some("10,200,30"));
}

#[test]
fn test_text_on_a_light_accent_is_dark() {
    assert_eq!(get(&parse_ini(&scheme(true, (255, 255, 255))), "[Colors:Selection]", "ForegroundNormal"), Some("24,20,36"));
    assert_eq!(get(&parse_ini(&scheme(true, (124, 84, 214))), "[Colors:Selection]", "ForegroundNormal"), Some("255,255,255"));
}

#[test]
fn test_text_is_readable_on_its_background() {
    for dark in [true, false] {
        let s = parse_ini(&scheme(dark, (124, 84, 214)));
        for sec in ["Window", "View", "Button"] {
            let c = |k: &str| {
                let v: Vec<i64> = get(&s, &format!("[Colors:{sec}]"), k).unwrap().split(',').map(|x| x.parse().unwrap()).collect();
                (v[0], v[1], v[2])
            };
            let (lb, lf) = (lum(c("BackgroundNormal")), lum(c("ForegroundNormal")));
            let ratio = (lb.max(lf) + 0.05) / (lb.min(lf) + 0.05);
            assert!(ratio > 7.0, "{dark} {sec} {ratio}");
        }
    }
}

#[test]
fn test_gtk_colours_carry_the_accent() {
    let g = gtk_colors(true, (10, 200, 30));
    assert!(g.contains("@define-color accent_bg_color rgb(10,200,30);"));
    assert!(g.contains("@define-color theme_selected_bg_color rgb(10,200,30);"));
}

struct Tmp(std::path::PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tmp() -> Tmp {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("ygg-theme-test-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Tmp(d)
}

#[test]
fn test_ini_set_keeps_the_rest() {
    let d = tmp();
    let p = d.0.join("kdeglobals").to_string_lossy().into_owned();
    std::fs::write(&p, "[General]\nfoo=1\nColorScheme=Old\n\n[Other]\nx=2\n").unwrap();
    files::ini_set(&p, "General", "ColorScheme", "New").unwrap();
    files::ini_set(&p, "General", "bar", "3").unwrap();
    files::ini_set(&p, "Icons", "Theme", "Liki").unwrap();
    let s = parse_ini(&std::fs::read_to_string(&p).unwrap());
    let sec = |n: &str| s.iter().find(|(x, _)| x == n).unwrap().1.clone();
    let kv = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
    assert_eq!(sec("[General]"), kv(&[("foo", "1"), ("ColorScheme", "New"), ("bar", "3")]));
    assert_eq!(sec("[Other]"), kv(&[("x", "2")]));
    assert_eq!(sec("[Icons]"), kv(&[("Theme", "Liki")]));
}

// ---- more

#[test]
fn ini_files_are_written_as_the_script_wrote_them() {
    let d = tmp();
    let p = d.0.join("a.ini").to_string_lossy().into_owned();
    // a file that starts with a group gets an empty first line, once (the script's join of an empty first block)
    std::fs::write(&p, "[General]\nfoo=1\n").unwrap();
    files::ini_set(&p, "General", "foo", "2").unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "\n[General]\nfoo=2\n");
    files::ini_set(&p, "General", "foo", "3").unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "\n[General]\nfoo=3\n");
    // a group twice: both get the key; comments are not keys; a new group after an empty line
    std::fs::write(&p, "top=1\n[S]\n# k=old\nk=1\n[S]\nother=1\n").unwrap();
    files::ini_set(&p, "S", "k", "2").unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "top=1\n[S]\n# k=old\nk=2\n\n[S]\nother=1\nk=2\n");
    files::ini_set(&p, "New", "x", "y").unwrap();
    assert!(std::fs::read_to_string(&p).unwrap().ends_with("other=1\nk=2\n\n[New]\nx=y\n"));
}

#[test]
fn accents_are_read_as_int_read_them() {
    assert_eq!(parse_accent("1,2,3"), Some((1, 2, 3)));
    assert_eq!(parse_accent(" 12 , 34,56"), Some((12, 34, 56)));
    assert_eq!(parse_accent("+5,6,7"), Some((5, 6, 7)));
    assert_eq!(parse_accent("\u{0661},2,3"), Some((1, 2, 3)));
    for bad in ["", "1,2", "1,2,3,4", "256,0,0", "-1,2,3", "a,b,c", "1.0,2,3", "1;2;3"] {
        assert_eq!(parse_accent(bad), None, "{bad}");
    }
}

#[test]
fn gtks_colours_become_yggdrasils() {
    // a grey follows Yggdrasil's dark ramp; light keeps it
    assert_eq!(retint_color((53, 53, 53), true, DEFAULT_ACCENT), (16, 17, 22));
    assert_eq!(retint_color((53, 53, 53), false, DEFAULT_ACCENT), (53, 53, 53));
    // GTK's own accent blue becomes the accent itself; a red stays
    assert_eq!(retint_color((21, 83, 158), true, (10, 200, 30)), (10, 200, 30));
    assert_eq!(retint_color((224, 27, 36), true, (10, 200, 30)), (224, 27, 36));
    let css = "a { color: #15539e; background: rgba(21, 83, 158, 0.5); border: #fff; x: #ffff; y: rgb(1,2,3); }";
    assert_eq!(retint_css(css, true, (10, 200, 30)), "a { color: #0ac81e; background: rgba(10, 200, 30, 0.5); border: #ffffff; x: #ffff; y: rgb(1, 1, 1); }");
}

#[test]
fn the_gtk4_theme_without_libgtk_falls_back_to_gtks_own_underneath() {
    let rules = scope_gtk4(GTK4_RULES);
    assert!(rules.contains("window.background, window .background { background-color: @window_bg_color;"));
    assert!(rules.lines().all(|l| l.is_empty() || l.starts_with("/*") || l.starts_with("window")));
}
