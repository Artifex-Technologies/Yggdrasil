//! The three tools as programs on scratch folders (HOME and every path point into a temporary folder; nothing of the
//! builder's own is read or written apart from the system's /usr/share/applications, which liki-build and
//! liki-icon-import only read), and the readers of outside input (zip, compiled XML, .icns) fed broken data.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("liki-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("home")).unwrap();
    d
}

fn run(bin: &str, dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut c = Command::new(bin);
    c.args(args).env_clear().env("HOME", dir.join("home")).env("PATH", dir.join("bin")).env("LANG", "C.UTF-8").current_dir(dir);
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

fn brand_icons() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("share/brand-icons").to_string_lossy().into_owned()
}

// ---------------------------------------------------------------- liki-build

#[test]
fn the_package_build_writes_the_natives_only() {
    let d = scratch("natives");
    let out = d.join("Liki");
    let (code, stdout, _) = run(
        env!("CARGO_BIN_EXE_liki-build"),
        &d,
        &["--natives-only", "--out", out.to_str().unwrap()],
        &[("LIKI_BRAND_ICONS", &brand_icons())],
    );
    assert_eq!(code, 0);
    assert!(stdout.starts_with("Liki: ") && stdout.contains("icons (glyph, brand) in"), "{stdout}");
    for f in [
        "apps/scalable/application-default-icon.svg",
        "places/scalable/folder.svg",
        "mimetypes/scalable/application-pdf.svg",
        "devices/scalable/drive-harddisk.svg",
        "index.theme",
    ] {
        assert!(out.join(f).is_file(), "{f}");
    }
    let index = std::fs::read_to_string(out.join("index.theme")).unwrap();
    assert!(
        index.starts_with("[Icon Theme]\nName=Liki\n")
            && index.contains("Directories=apps/scalable,places/scalable,mimetypes/scalable,devices/scalable,status/scalable\n")
    );
    // --out: nothing remembered
    assert!(!d.join("home/.config/labyrinth/brand-pack.json").exists());
    // the accent colours Yggdrasil's own glyphs
    let minos = std::fs::read_to_string(out.join("apps/scalable/minos.svg")).unwrap();
    assert!(minos.contains("fill=\"#7c54d6\""), "{minos}");
}

#[test]
fn a_build_for_the_user_draws_logos_remembers_and_rebuilds() {
    let d = scratch("user");
    let apps = d.join("home/.local/share/applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(apps.join("ff.desktop"), "[Desktop Entry]\nName=Firefox\nIcon=firefox\n").unwrap();
    // an icon name with a "/" is left out (it used to be written outside the pack, or to stop the build)
    std::fs::write(apps.join("evil.desktop"), "[Desktop Entry]\nName=Firefox\nIcon=../../../evil/firefox\n").unwrap();
    std::fs::create_dir_all(d.join("home/.config/yggdrasil")).unwrap();
    std::fs::write(d.join("home/.config/yggdrasil/appearance.conf"), "accent = 200, 100, 50\n").unwrap();
    let env = [("LIKI_BRAND_ICONS", brand_icons())];
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (code, _, _) = run(env!("CARGO_BIN_EXE_liki-build"), &d, &["--shape", "circle", "--style", "dark"], &env);
    assert_eq!(code, 0);
    let pack = d.join("home/.local/share/icons/Liki");
    let ff = std::fs::read_to_string(pack.join("apps/scalable/firefox.svg")).unwrap();
    assert!(ff.contains("<circle cx=\"512\" cy=\"512\" r=\"480\" fill=\"#1b1b22\"/>") && ff.contains("fill=\"#ff7139\""), "{ff}");
    assert!(!d.join("evil").exists() && !d.join("home/.local/share/evil").exists());
    assert!(std::fs::read_to_string(pack.join("apps/scalable/minos.svg")).unwrap().contains("#c86432"));
    let conf = d.join("home/.config/labyrinth/brand-pack.json");
    assert_eq!(std::fs::read_to_string(&conf).unwrap(), r#"{"shape": "circle", "style": "dark", "name": "Liki"}"#);
    // --rebuild repeats it; a damaged file leaves the given values (and the pack) alone
    std::fs::write(&conf, r#"{"shape": "triangle", "style": "purple", "name": "../x"}"#).unwrap();
    let (code, stdout, _) = run(env!("CARGO_BIN_EXE_liki-build"), &d, &["--rebuild"], &env);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("Liki: ") && stdout.contains("(glyph, purple)"), "{stdout}");
    std::fs::write(&conf, "[1]").unwrap();
    let (code, stdout, _) = run(env!("CARGO_BIN_EXE_liki-build"), &d, &["--rebuild"], &env);
    assert_eq!((code, stdout.as_str()), (0, "No earlier build to repeat.\n"));
}

#[test]
fn a_bracket_in_the_name_leaves_other_packs_alone() {
    let d = scratch("glob");
    let other = d.join("home/.local/share/icons/Liki1/apps/scalable");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("keep.svg"), "<svg/>").unwrap();
    let (code, _, _) =
        run(env!("CARGO_BIN_EXE_liki-build"), &d, &["--name", "Liki[12]", "--natives-only"], &[("LIKI_BRAND_ICONS", &brand_icons())]);
    assert_eq!(code, 0);
    assert!(other.join("keep.svg").exists());
}

#[test]
fn the_command_line_is_argparses() {
    let d = scratch("argv");
    let (code, _, err) = run(env!("CARGO_BIN_EXE_liki-build"), &d, &["--shape", "x"], &[]);
    assert_eq!(code, 2);
    assert_eq!(
        err,
        "usage: liki-build [-h] [--shape {glyph,squircle,rounded,circle,hexagon}]\n                  [--style {brand,dark,purple}] [--name NAME] [--out OUT]\n                  [--natives-only] [--rebuild]\nliki-build: error: argument --shape: invalid choice: 'x' (choose from 'glyph', 'squircle', 'rounded', 'circle', 'hexagon')\n"
    );
    let (code, _, err) = run(env!("CARGO_BIN_EXE_liki-build"), &d, &["--n", "x"], &[]);
    assert_eq!(code, 2);
    assert!(err.ends_with("liki-build: error: ambiguous option: --n could match --name, --natives-only\n"), "{err}");
    let (code, out, _) = run(env!("CARGO_BIN_EXE_liki-build"), &d, &["-h"], &[("COLUMNS", "200")]);
    assert_eq!(code, 0);
    assert!(out.starts_with("usage: liki-build [-h] [--shape {glyph,squircle,rounded,circle,hexagon}] [--style {brand,dark,purple}] [--name NAME] [--out OUT] [--natives-only] [--rebuild]\n\nBuild an icon pack of app logos.\n\noptions:\n"), "{out}");
    let (code, _, err) = run(env!("CARGO_BIN_EXE_liki-icon-import"), &d, &[], &[]);
    assert_eq!(code, 2);
    assert!(err.ends_with("liki-icon-import: error: the following arguments are required: pack\n"), "{err}");
}

// ---------------------------------------------------------------- liki-icns

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    v.extend(w.to_be_bytes());
    v.extend(h.to_be_bytes());
    v.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    v
}

fn icns(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut body = vec![];
    for (k, d) in chunks {
        body.extend_from_slice(*k);
        body.extend(((d.len() + 8) as u32).to_be_bytes());
        body.extend(d);
    }
    let mut out = b"icns".to_vec();
    out.extend(((body.len() + 8) as u32).to_be_bytes());
    out.extend(body);
    out
}

#[test]
fn icns_gives_the_biggest_png_and_leaves_nothing_on_failure() {
    let d = scratch("icns");
    std::fs::write(d.join("a.icns"), icns(&[(b"ic07", png(128, 128)), (b"ic09", png(512, 512)), (b"ic08", png(256, 256))])).unwrap();
    let (code, _, _) = run(env!("CARGO_BIN_EXE_liki-icns"), &d, &["a.icns", "a.png"], &[]);
    assert_eq!(code, 0);
    assert_eq!(std::fs::read(d.join("a.png")).unwrap(), png(512, 512));
    std::fs::write(d.join("b.icns"), b"nope").unwrap();
    let (code, _, err) = run(env!("CARGO_BIN_EXE_liki-icns"), &d, &["b.icns", "b.png"], &[]);
    assert_eq!((code, err.lines().count()), (1, 1));
    assert!(!d.join("b.png").exists(), "an empty picture was left behind");
    let (code, _, err) = run(env!("CARGO_BIN_EXE_liki-icns"), &d, &["one"], &[]);
    assert_eq!(code, 1);
    assert!(err.starts_with("Extract the biggest PNG from a macOS .icns file"));
}

#[test]
fn broken_icns_files_never_panic() {
    let good = icns(&[(b"ic07", png(128, 128)), (b"ARGB", vec![7; 40]), (b"ic09", png(512, 512))]);
    let mut seed = 0x9e37_79b9u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as usize
    };
    for _ in 0..20000 {
        let mut d = good.clone();
        for _ in 0..1 + next() % 4 {
            let i = next() % d.len();
            match next() % 3 {
                0 => d[i] = next() as u8,
                1 => d.truncate(i.max(1)),
                _ => d.insert(i, next() as u8),
            }
        }
        let _ = liki::icns::biggest_png(&d);
    }
}

// ---------------------------------------------------------------- liki-icon-import

/// A zip archive written by hand: (name, data, deflated?)
fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut out = vec![];
    let mut central = vec![];
    for (name, data, deflate) in entries {
        let comp = if *deflate { miniz_oxide::deflate::compress_to_vec(data, 6) } else { data.to_vec() };
        let crc = liki::import::zip::crc32(data, 0);
        let offset = out.len() as u32;
        let method: u16 = if *deflate { 8 } else { 0 };
        let mut head = vec![];
        head.extend(20u16.to_le_bytes());
        head.extend(0u16.to_le_bytes());
        head.extend(method.to_le_bytes());
        head.extend([0, 0, 0x21, 0]);
        head.extend(crc.to_le_bytes());
        head.extend((comp.len() as u32).to_le_bytes());
        head.extend((data.len() as u32).to_le_bytes());
        head.extend((name.len() as u16).to_le_bytes());
        head.extend(0u16.to_le_bytes());
        out.extend(b"PK\x03\x04");
        out.extend(&head);
        out.extend(name.as_bytes());
        out.extend(&comp);
        central.extend(b"PK\x01\x02");
        central.extend(20u16.to_le_bytes());
        central.extend(&head);
        central.extend([0u8; 6]);
        central.extend(0u32.to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let cd_at = out.len() as u32;
    out.extend(&central);
    out.extend(b"PK\x05\x06\0\0\0\0");
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(cd_at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

const APPFILTER: &str = r#"<resources><item component="ComponentInfo{org.videolan.vlc/x.Y}" drawable="vlc"/><item component="ComponentInfo{com.x/com.x.A}" drawable="other"/></resources>"#;

#[test]
fn a_pack_becomes_a_theme_for_the_installed_programs() {
    let d = scratch("import");
    let apps = d.join("home/.local/share/applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(apps.join("vlc.desktop"), "[Desktop Entry]\nName=VLC\nIcon=vlc\n").unwrap();
    let big = png(192, 192);
    let pack = zip(&[
        ("assets/appfilter.xml", APPFILTER.as_bytes(), true),
        ("res/drawable-nodpi/vlc.png", &big, true),
        ("res/drawable-mdpi/vlc.png", &png(48, 48), false),
    ]);
    std::fs::write(d.join("Test Pack v1.2.3 (Patched).apk"), pack).unwrap();
    let out = d.join("themes");
    let (code, stdout, err) =
        run(env!("CARGO_BIN_EXE_liki-icon-import"), &d, &["Test Pack v1.2.3 (Patched).apk", "--out", out.to_str().unwrap()], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(stdout.starts_with("The pack lists 2 app(s), 1 of them with a usable PNG picture (1 PNG pictures in all).\n"), "{stdout}");
    assert_eq!(std::fs::read(out.join("Test Pack/192x192/apps/vlc.png")).unwrap(), big);
    let index = std::fs::read_to_string(out.join("Test Pack/index.theme")).unwrap();
    assert!(index.contains("Directories=192x192/apps\n") && index.contains("Inherits=hicolor\n"));
}

#[test]
fn junk_is_refused() {
    let d = scratch("junk");
    std::fs::write(d.join("x.apk"), zip(&[("readme.txt", b"no icons", false)])).unwrap();
    let (code, _, err) = run(env!("CARGO_BIN_EXE_liki-icon-import"), &d, &["x.apk", "--out", "o"], &[]);
    assert_eq!((code, err.as_str()), (1, "No PNG pictures found in that file (is it an Android icon pack?).\n"));
    std::fs::write(d.join("y.apk"), b"not a zip").unwrap();
    let (code, _, err) = run(env!("CARGO_BIN_EXE_liki-icon-import"), &d, &["y.apk", "--out", "o"], &[]);
    assert_eq!((code, err.as_str()), (1, "error: that is not a zip/apk file\n"));
    let (code, _, err) = run(env!("CARGO_BIN_EXE_liki-icon-import"), &d, &["missing.apk"], &[]);
    assert_eq!((code, err.as_str()), (2, "error: no such file: missing.apk\n"));
}

#[test]
fn broken_archives_never_panic_and_never_read_past_a_members_size() {
    let d = scratch("zipfuzz");
    let good = zip(&[
        ("assets/appfilter.xml", APPFILTER.as_bytes(), true),
        ("res/drawable/vlc.png", &png(64, 64), true),
        ("res/drawable/other.png", &png(64, 64), false),
    ]);
    let path = d.join("p.apk");
    let mut seed = 0x1234_5678u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as usize
    };
    for _ in 0..3000 {
        let mut data = good.clone();
        for _ in 0..1 + next() % 4 {
            let i = next() % data.len();
            match next() % 4 {
                0 | 1 => data[i] = next() as u8,
                2 => data.truncate(i.max(1)),
                _ => data.insert(i, next() as u8),
            }
        }
        std::fs::write(&path, &data).unwrap();
        if let Ok(z) = liki::import::zip::Zip::open(std::fs::File::open(&path).unwrap()) {
            for e in &z.entries {
                if let Ok(b) = z.read_member(e) {
                    assert!(b.len() as u64 <= e.file_size.max(1));
                }
            }
            let _ = liki::import::parse_appfilter(&z);
            let _ = liki::import::collect_pngs(&z);
        }
    }
}

#[test]
fn plain_xml_parses_or_fails_as_expat_decides() {
    let ok = liki::import::xml::parse(
        br#"<?xml version="1.0"?><!DOCTYPE r [<!ENTITY f "firefox">]><r xmlns:a="u"><item a:drawable="x" drawable="&f;"/></r>"#,
    )
    .unwrap();
    assert_eq!(
        ok[1],
        ("item".to_string(), vec![("{u}drawable".to_string(), "x".to_string()), ("drawable".to_string(), "firefox".to_string())])
    );
    assert!(liki::import::xml::parse(b"<r><item drawable='x'></r>").is_err());
    assert!(liki::import::xml::parse(br#"<!DOCTYPE r SYSTEM "x.dtd"><r>&undefined;</r>"#).is_err());
    assert!(liki::import::xml::parse(b"").is_err());
    let mut seed = 77u32;
    let doc = br#"<resources><item component="ComponentInfo{a/b}" drawable="x"/><!-- c --><?p q?><![CDATA[z]]>&amp;&#65;</resources>"#;
    for _ in 0..5000 {
        let mut d = doc.to_vec();
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let i = seed as usize % d.len();
        d[i] = (seed >> 8) as u8;
        let _ = liki::import::xml::parse(&d);
    }
}
