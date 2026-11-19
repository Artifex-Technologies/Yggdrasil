//! The font library on made-up files: egui's own Ubuntu Light stands in for a font, and a shell script for Heimdall's file
//! guard (its `scan` and `extract` answers as documented; a file holding "EVIL" is refused, one holding "WARN" warned about).

use super::*;
use std::os::unix::fs::PermissionsExt;

fn font_bytes() -> Vec<u8> {
    eframe::egui::FontDefinitions::default().font_data["Ubuntu-Light"].font.to_vec()
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ygg-fontlib-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const GUARD_SCRIPT: &str = r#"#!/bin/bash
verdict() { if grep -q EVIL "$1"; then echo refused; elif grep -q WARN "$1"; then echo warn; else echo ok; fi; }
case "$1" in
--help) printf 'heimdall-fileguard check [--json] FILE...\nheimdall-fileguard scan --json FILE...\nheimdall-fileguard extract --json --only KINDS ARCHIVE OUTDIR\n' ;;
scan)
  shift; shift; rc=0; sep=""; printf '{"files":['
  for f in "$@"; do v=$(verdict "$f"); [ "$v" = refused ] && rc=1
    printf '%s{"path":"%s","verdict":"%s","reasons":["%s"],"clamav":"clean"}' "$sep" "$f" "$v" "$v reason"; sep=","; done
  printf ']}\n'; exit $rc ;;
extract)
  archive=$5; out=$6
  [ -e "$out" ] && { echo "outdir exists" >&2; exit 2; }
  if grep -q BOMB "$archive"; then printf '{"files":[],"skipped":[],"refused":"too large when unpacked"}\n'; exit 1; fi
  mkdir "$out"; cp "$FONT" "$out/inner.ttf"
  printf '{"files":[{"name":"dir/Inner Font.ttf","path":"%s/inner.ttf","size":1,"verdict":"ok","reasons":[],"clamav":"clean"}],"skipped":[{"name":"readme.exe","reason":"not a font"}],"refused":null}\n' "$out" ;;
*) exit 2 ;;
esac
"#;

/// A stand-in guard in `dir`; its extract copies `font` into the archive's folder.
fn guard(dir: &Path, font: &Path) -> String {
    let p = dir.join("guard");
    std::fs::write(&p, GUARD_SCRIPT.replace("$FONT", &font.display().to_string())).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.display().to_string()
}

#[test]
fn the_installed_list_is_grouped_by_family() {
    let yours = Path::new("/home/a/.local/share/fonts");
    let text = "Noto Sans\tBold\t/usr/share/fonts/noto/NotoSans-Bold.ttf\t0\n\
                Noto Sans\tRegular\t/usr/share/fonts/noto/NotoSans-Regular.ttf\t0\n\
                Lexend\tThin\t/home/a/.local/share/fonts/Lexend.ttf\t65536\n\
                Lexend\tRegular\t/home/a/.local/share/fonts/Lexend.ttf\t262144\n\
                Lexend\tRegular\t/home/a/.local/share/fonts/Lexend.ttf\t0\n\
                broken line\n\
                \tRegular\t/x.ttf\t0\n\
                abc\tRegular\t/home/a/.local/share/fonts/abc.otf\t0\n";
    let l = parse_list(text, yours);
    assert_eq!(l.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["abc", "Lexend", "Noto Sans"], "by name, any case");
    let lexend = &l[1];
    assert_eq!((lexend.faces, lexend.files.len(), lexend.yours), (2, 1, true));
    assert_eq!(lexend.sample, Some((PathBuf::from("/home/a/.local/share/fonts/Lexend.ttf"), 0)), "the regular face");
    let noto = &l[2];
    assert!(!noto.yours, "the system's");
    assert_eq!(noto.sample.as_ref().unwrap().0, PathBuf::from("/usr/share/fonts/noto/NotoSans-Regular.ttf"));
    assert_eq!(search(&l, "SANS").len(), 1);
    assert_eq!(search(&l, " ").len(), 3);
}

#[test]
fn a_guard_without_scan_and_extract_takes_nothing() {
    let d = tmp("old-guard");
    let f = d.join("a.ttf");
    std::fs::write(&f, font_bytes()).unwrap();
    let old = d.join("old");
    std::fs::write(&old, "#!/bin/bash\nprintf 'heimdall-fileguard check [--json] FILE...\\nheimdall-fileguard watch\\n'\n").unwrap();
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();
    let old = old.display().to_string();
    assert_eq!(guard_ready(&old), Err(GuardMissing::TooOld));
    let e = bring_in(&old, std::slice::from_ref(&f), &d).unwrap_err();
    assert!(e.contains("newer Heimdall"), "{e}");
    assert_eq!(guard_ready("/nonexistent/heimdall-fileguard"), Err(GuardMissing::Absent));
    assert!(bring_in("/nonexistent/heimdall-fileguard", &[f], &d).is_err());
    assert_eq!(guard_ready(&guard(&d, &d.join("a.ttf"))), Ok(()));
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn the_guards_answers_are_read_strictly() {
    let a = [PathBuf::from("/s/0.ttf"), PathBuf::from("/s/1.otf")];
    let t = r#"{"files":[{"path":"/s/1.otf","verdict":"warn","reasons":["odd"],"clamav":"unavailable"},
                {"path":"/s/0.ttf","verdict":"ok","reasons":[],"clamav":"infected:Font.Exploit"}]}"#;
    let v = parse_scan(t, &a).unwrap();
    assert_eq!(v[0].level, Level::Refused, "an infection is a refusal whatever the verdict says");
    assert!(v[0].words().contains("Font.Exploit"));
    assert_eq!(v[1].level, Level::Warn);
    assert_eq!(v[1].words(), "odd; the virus scanner (ClamAV) is not available");
    assert!(parse_scan(r#"{"files":[]}"#, &a).is_err(), "a file it said nothing about");
    assert!(parse_scan("nonsense", &a).is_err());
    assert!(parse_scan(r#"{"files":[{"path":"/s/0.ttf","verdict":"fine"},{"path":"/s/1.otf","verdict":"ok"}]}"#, &a).is_err());
    let out = Path::new("/stage/zip-0");
    let e = r#"{"files":[{"name":"a.ttf","path":"/stage/zip-0/../../home/x/.bashrc","verdict":"ok","reasons":[],"clamav":"clean"}],"skipped":[],"refused":null}"#;
    assert!(parse_extract(e, out).is_err(), "nothing written outside its folder is taken");
    let e = r#"{"files":[{"name":"a.ttf","path":"/elsewhere/a.ttf","verdict":"ok","reasons":[],"clamav":"clean"}],"skipped":[],"refused":null}"#;
    assert!(parse_extract(e, out).is_err());
    let u = parse_extract(r#"{"files":[],"skipped":[{"name":"x","reason":"y"}],"refused":"a zip bomb"}"#, out).unwrap();
    assert_eq!((u.refused.as_deref(), u.skipped.len()), (Some("a zip bomb"), 1));
}

#[test]
fn only_real_fonts_pass() {
    let good = font_bytes();
    assert_eq!(check_font(&good).unwrap(), "Ubuntu");
    assert!(check_font(b"hello, not a font at all").is_err());
    assert!(check_font(&good[..good.len() / 3]).is_err() || check_font(&good[..200]).is_err(), "a cut-off file");
    assert!(check_font(&[]).is_err());
    let d = tmp("validate");
    let named = |n: &str, b: &[u8]| {
        let p = d.join(n);
        std::fs::write(&p, b).unwrap();
        p
    };
    assert!(validate(&named("a.ttf", &good)).is_ok());
    assert!(validate(&named("a.exe", &good)).is_err(), "the kind is part of it");
    assert!(validate(&named("b.otf", b"MZ\x90\x00 an executable")).is_err());
    let link = d.join("l.ttf");
    std::os::unix::fs::symlink(d.join("a.ttf"), &link).unwrap();
    assert!(validate(&link).is_err(), "a link is not a plain file");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_woff2_header_is_checked() {
    let mut h = vec![0u8; 64];
    h[0..4].copy_from_slice(b"wOF2");
    h[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    h[8..12].copy_from_slice(&64u32.to_be_bytes());
    h[12..14].copy_from_slice(&10u16.to_be_bytes());
    assert!(check_woff2_header(&h).is_ok());
    let mut bad = h.clone();
    bad[8..12].copy_from_slice(&99u32.to_be_bytes());
    assert!(check_woff2_header(&bad).is_err(), "its own length");
    let mut bad = h.clone();
    bad[4..8].copy_from_slice(b"EVIL");
    assert!(check_woff2_header(&bad).is_err(), "an unknown flavour");
    assert!(check_woff2_header(b"wOF2").is_err());
    assert!(check_woff2_header(&[b'w'; 64]).is_err());
}

#[test]
fn files_and_archives_are_checked_before_anything_is_installed() {
    let d = tmp("bring");
    let src = d.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let font = src.join("My Font.ttf");
    std::fs::write(&font, font_bytes()).unwrap();
    let mut warn = font_bytes();
    warn.extend_from_slice(b"WARN");
    std::fs::write(src.join("warned.otf"), &warn).unwrap();
    let mut evil = font_bytes();
    evil.extend_from_slice(b"EVIL");
    std::fs::write(src.join("evil.ttf"), &evil).unwrap();
    std::fs::write(src.join("fake.ttf"), b"not a font").unwrap();
    std::fs::write(src.join("notes.txt"), b"x").unwrap();
    std::fs::write(src.join("pack.zip"), b"PK archive").unwrap();
    std::fs::write(src.join("bomb.zip"), b"PK BOMB").unwrap();
    let g = guard(&d, &font);
    let stage = d.join("stage");
    std::fs::create_dir_all(&stage).unwrap();
    let names = ["My Font.ttf", "warned.otf", "evil.ttf", "fake.ttf", "notes.txt", "pack.zip", "bomb.zip"];
    let list = bring_in(&g, &names.map(|n| src.join(n)), &stage).unwrap();
    let by = |n: &str| list.iter().find(|i| i.name == n).unwrap_or_else(|| panic!("{n}: {list:#?}"));
    assert!(by("My Font.ttf").clean());
    assert_eq!(by("My Font.ttf").family.as_deref(), Some("Ubuntu"));
    assert!(by("My Font.ttf").staged.as_ref().unwrap().starts_with(&stage), "the private copy is what was checked");
    assert!(by("warned.otf").warned() && !by("warned.otf").clean());
    assert!(by("evil.ttf").problem.is_some() && by("evil.ttf").staged.is_none(), "refused: nothing to install");
    assert!(by("fake.ttf").problem.as_deref().is_some_and(|p| p.contains("not a font")), "{:?}", by("fake.ttf"));
    assert!(by("notes.txt").problem.is_some());
    assert!(by("pack.zip: dir/Inner Font.ttf").clean());
    assert!(by("pack.zip: readme.exe").problem.as_deref() == Some("not a font"));
    assert_eq!(by("bomb.zip").problem.as_deref(), Some("too large when unpacked"));
    // installing: a safe name, the same file twice is one file, another file of that name gets a number
    let fonts = d.join("fonts");
    let to = install(by("My Font.ttf"), &fonts).unwrap();
    assert_eq!(to, fonts.join("My_Font.ttf"));
    assert_eq!(std::fs::metadata(&to).unwrap().permissions().mode() & 0o777, 0o644);
    assert!(install(by("evil.ttf"), &fonts).is_err());
    let again = bring_in(&g, std::slice::from_ref(&font), &stage).unwrap();
    assert_eq!(install(&again[0], &fonts).unwrap(), to, "already installed");
    let src2 = d.join("src2");
    std::fs::create_dir_all(&src2).unwrap();
    let mut changed = font_bytes();
    changed.extend_from_slice(b"another build");
    std::fs::write(src2.join("My Font.ttf"), &changed).unwrap();
    let other = bring_in(&g, &[src2.join("My Font.ttf")], &stage).unwrap();
    assert_eq!(install(&other[0], &fonts).unwrap(), fonts.join("My_Font-1.ttf"), "another file of the same name");
    assert_eq!(std::fs::read(fonts.join("My_Font.ttf")).unwrap(), font_bytes(), "the first one is untouched");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn safe_names() {
    assert_eq!(safe_name("My Font (Bold).ttf", "ttf"), "My_Font__Bold_.ttf");
    assert_eq!(safe_name("../../.bashrc", "otf"), "bashrc.otf");
    assert_eq!(safe_name(".ttf", "ttf"), "ttf.ttf");
    assert_eq!(safe_name("", "ttf"), "font.ttf");
    assert_eq!(safe_name("Ωmega.otf", "otf"), "_mega.otf");
}

#[test]
fn only_your_own_fonts_are_removed() {
    let d = tmp("remove");
    let fonts = d.join("fonts");
    std::fs::create_dir_all(&fonts).unwrap();
    let mine = fonts.join("a.ttf");
    std::fs::write(&mine, b"x").unwrap();
    let fam = |files: Vec<PathBuf>, yours: bool| Family { name: "A".into(), faces: 1, files, yours, sample: None };
    assert!(remove(&fam(vec![PathBuf::from("/usr/share/fonts/x.ttf")], false), &fonts).is_err());
    // a link in your folder to a file elsewhere is not removed through
    let outside = d.join("outside.ttf");
    std::fs::write(&outside, b"keep").unwrap();
    let link = fonts.join("link.ttf");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert!(remove(&fam(vec![link.clone()], true), &fonts).is_err());
    assert!(outside.exists());
    remove(&fam(vec![mine.clone()], true), &fonts).unwrap();
    assert!(!mine.exists());
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_sample_is_drawn() {
    let (w, h, mask) = render_sample(&font_bytes(), 0, "Aa Il1 O0", 24.0).unwrap();
    assert!(w > 60 && h > 20, "{w}x{h}");
    assert_eq!(mask.len(), w * h);
    assert!(mask.iter().any(|&a| a > 200), "ink");
    assert!(render_sample(b"not a font", 0, "Aa", 24.0).is_none());
    assert!(render_sample(&font_bytes(), 0, "   ", 24.0).is_none(), "spaces only: nothing to show");
}

#[test]
fn fc_lists_families_are_read() {
    let got = parse_families("Noto Sans\nAtkinson Hyperlegible Mono,Atkinson Hyperlegible Mono ExtraLight\nA\\,B\n\n");
    let want: std::collections::HashSet<String> =
        ["Noto Sans", "Atkinson Hyperlegible Mono", "Atkinson Hyperlegible Mono ExtraLight", "A,B"].iter().map(|s| s.to_string()).collect();
    assert_eq!(got, want);
}

/// Stand-ins for fc-list and fc-cache: fc-list lists Noto Sans (and Lexend once fc-cache has run, as a stale cache made
/// again would), fc-cache logs how it was run.
fn cache_standins(dir: &Path, after: &str) -> (Vec<String>, Vec<String>) {
    let (list, cache, done) = (dir.join("fc-list"), dir.join("fc-cache"), dir.join("refreshed"));
    std::fs::write(
        &list,
        format!(
            "#!/bin/sh\n[ \"$*\" = ': family' ] || exit 2\necho 'Noto Sans'\necho 'Noto Sans Mono'\n[ -e '{}' ] && printf '%s\\n' '{after}'\nexit 0\n",
            done.display()
        ),
    )
    .unwrap();
    std::fs::write(&cache, format!("#!/bin/sh\necho \"$@\" >> '{}'\n", done.display())).unwrap();
    let sh = |p: &Path| vec!["/bin/sh".to_string(), p.display().to_string()];
    (sh(&list), sh(&cache))
}

#[test]
fn a_stale_cache_is_made_again_once() {
    let lexend = Path::new(yggdrasil_conf::fonts::sans("lexend").unwrap().file);
    let noto = Path::new(yggdrasil_conf::fonts::SANS[0].file);
    let noto_mono = Path::new(yggdrasil_conf::fonts::MONO[0].file);
    // Lexend's file is on disk but not listed: fc-cache -f, and then it is
    let d = tmp("heal");
    let (list, cache) = cache_standins(&d, "Lexend");
    let here = |p: &Path| p == lexend || p == noto || p == noto_mono;
    assert_eq!(heal(&list, &cache, here), Healed::Refreshed(vec![]));
    assert_eq!(std::fs::read_to_string(d.join("refreshed")).unwrap(), "-f\n", "your own cache, forced, run once");
    // listed already: nothing is run
    let d = tmp("heal-fine");
    let (list, cache) = cache_standins(&d, "Lexend");
    assert_eq!(heal(&list, &cache, |p| p == noto || p == noto_mono), Healed::Fine);
    assert!(!d.join("refreshed").exists());
    // not on disk either (not installed): nothing is run
    assert_eq!(heal(&list, &cache, |_| false), Healed::Fine);
    assert!(!d.join("refreshed").exists());
    // still missing after the cache was made again: said so (the page then says "Not installed.")
    let d = tmp("heal-still");
    let (list, cache) = cache_standins(&d, "Something Else");
    let got = heal(&list, &cache, here);
    assert_eq!(got, Healed::Refreshed(vec![yggdrasil_conf::fonts::sans("lexend").unwrap()]));
    // no fontconfig at all: nothing to do
    assert_eq!(heal(&["/nonexistent/fc-list".to_string()], &cache, here), Healed::Fine);
    for t in ["heal", "heal-fine", "heal-still"] {
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("ygg-fontlib-{t}-{}", std::process::id())));
    }
}
