//! The Python script's tests (helpers/tests/test_game_fix.py), carried over: a pretend Steam library with Dead Space 2 (a
//! file made from catalogue lines, kv) and Fallout 4 (ini files made from the game's templates, an existing file backed up),
//! apply and undo, other lines kept, a game never started. And more: odd bytes kept, the {name} rule, the screen.

use super::*;
use crate::testutil::{env_lock, TempDir};
use std::sync::MutexGuard;

const CATALOGUE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../settings/data/game-fixes.json");

struct Lib {
    home: TempDir,
    sa: String,
    fixes: BTreeMap<String, Value>,
    games: BTreeMap<String, Game>,
    _lock: MutexGuard<'static, ()>,
}

fn write(p: &str, text: &str) {
    fs::create_dir_all(Path::new(p).parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn setup() -> Lib {
    let lock = env_lock();
    let home = TempDir::new();
    let sa = home.p(".local/share/Steam/steamapps");
    fs::create_dir_all(&sa).unwrap();
    for (appid, name, folder) in [("47780", "Dead Space 2", "Dead Space 2"), ("377160", "Fallout 4", "Fallout 4")] {
        write(
            &format!("{sa}/appmanifest_{appid}.acf"),
            &format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{folder}\"\n}}\n"),
        );
        fs::create_dir_all(format!("{sa}/common/{folder}")).unwrap();
        fs::create_dir_all(format!("{sa}/compatdata/{appid}/pfx/drive_c/users/steamuser")).unwrap();
    }
    let fo = format!("{sa}/common/Fallout 4");
    write(
        &format!("{fo}/Fallout4/Fallout4Prefs.ini"),
        "[Display]\r\niPresentInterval=1\r\nbBorderless=1\r\nbFull Screen=0\r\niSize H=720\r\niSize W=1280\r\n\r\n[Imagespace]\r\nbDoDepthOfField=1\r\n",
    );
    write(&format!("{fo}/Fallout4_Default.ini"), "[General]\nsLanguage=en\n");
    std::env::set_var("HOME", &home.0);
    std::env::set_var("YGGDRASIL_SCREEN", "3440x1440");
    let fixes = load_catalogue(CATALOGUE).unwrap().unwrap().into_iter().map(|f| (f["appid"].as_str().unwrap().to_string(), f)).collect();
    let games = installed(&home.p("")).unwrap();
    Lib { home, sa, fixes, games, _lock: lock }
}

impl Lib {
    fn ds2_file(&self) -> String {
        format!("{}/compatdata/47780/pfx/drive_c/users/steamuser/AppData/Local/EA Games/Dead Space 2/settings.txt", self.sa)
    }
    fn prefs(&self) -> String {
        format!("{}/compatdata/377160/pfx/drive_c/users/steamuser/Documents/My Games/Fallout4/Fallout4Prefs.ini", self.sa)
    }
    fn state(&self, appid: &str) -> &'static str {
        status(&self.fixes[appid], &self.games).unwrap().0
    }
    fn apply(&self, appid: &str) {
        assert!(apply(&self.fixes[appid], &self.games).is_ok());
    }
    fn undo(&self, appid: &str) {
        assert!(undo(&self.fixes[appid], &self.games).is_ok());
    }
}

impl Drop for Lib {
    fn drop(&mut self) {
        std::env::remove_var("YGGDRASIL_SCREEN");
    }
}

fn read(p: &str) -> String {
    String::from_utf8(fs::read(p).unwrap()).unwrap()
}

#[test]
fn test_the_catalogue_reads() {
    let l = setup();
    assert_eq!(l.fixes.keys().cloned().collect::<Vec<_>>(), vec!["377160".to_string(), "47780".to_string()]);
    for f in l.fixes.values() {
        assert!(!f["title"].as_str().unwrap().is_empty() && !f["files"].as_array().unwrap().is_empty());
    }
}

#[test]
fn test_dead_space_2_made_applied_and_undone() {
    let l = setup();
    assert_eq!(l.state("47780"), "not applied");
    l.apply("47780");
    let text = read(&l.ds2_file());
    assert!(text.contains("Window.Width = 3440\r\n"));
    assert!(text.contains("Window.Height = 1440\r\n"));
    assert!(text.contains("Audio.Output = 0\r\n"), "the rest of the defaults are there");
    assert_eq!(l.state("47780"), "applied");
    l.undo("47780");
    assert!(!Path::new(&l.ds2_file()).exists(), "a file it made is removed again");
}

#[test]
fn test_an_existing_file_keeps_its_other_lines_and_comes_back() {
    let l = setup();
    write(&l.ds2_file(), "Audio.Output = 2\r\nWindow.Width = 1024\r\n");
    l.apply("47780");
    let text = read(&l.ds2_file());
    assert!(text.contains("Audio.Output = 2\r\n"), "the player's own settings stay");
    assert!(text.contains("Window.Width = 3440\r\n"));
    assert!(text.contains("Window.VSync = False\r\n"), "a missing key is added");
    l.apply("47780");
    l.undo("47780");
    assert_eq!(
        read(&l.ds2_file()),
        "Audio.Output = 2\r\nWindow.Width = 1024\r\n",
        "undo gives the file back as it was, even after two applies"
    );
}

#[test]
fn test_fallout_4_files_from_its_templates() {
    let l = setup();
    l.apply("377160");
    let text = read(&l.prefs());
    assert!(text.contains("iSize W=3440\r\n"));
    assert!(text.contains("iSize H=1440\r\n"));
    assert!(text.contains("iPresentInterval=1\r\n"), "the template's other lines are kept");
    assert!(text.contains("[Imagespace]\r\nbDoDepthOfField=1"));
    assert!(Path::new(&l.prefs()).parent().unwrap().join("Fallout4.ini").is_file(), "made from Fallout4_Default.ini");
    assert_eq!(l.state("377160"), "applied");
    l.undo("377160");
    assert!(!Path::new(&l.prefs()).exists());
}

#[test]
fn test_fallout_4_offers_its_launch_option() {
    let l = setup();
    let launch = l.fixes["377160"]["launch"].as_str().unwrap();
    assert!(launch.contains("Fallout4Launcher.exe/Fallout4.exe"));
    assert!(launch.ends_with("%command%"));
    assert_eq!(l.fixes["47780"].get("launch").and_then(|v| v.as_str()).unwrap_or(""), "", "Dead Space 2 needs none");
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_ini_keys_go_into_their_section() {
    let l = lines(&["[Display]", "iSize W=1280", "", "[Other]", "x=1"]);
    let l = ini_set(l, Some("Display"), "bBorderless", "1").unwrap();
    assert_eq!(l[..3], lines(&["[Display]", "iSize W=1280", "bBorderless=1"])[..]);
    let l = ini_set(l, Some("New"), "k", "v").unwrap();
    assert_eq!(l[l.len() - 2..], lines(&["[New]", "k=v"])[..]);
    assert_eq!(ini_get(&l, Some("display"), "isize w").unwrap().as_deref(), Some("1280"), "names compare without case, as the games do");
}

#[test]
fn test_a_game_never_started_waits() {
    let l = setup();
    fs::remove_dir_all(format!("{}/compatdata/47780", l.sa)).unwrap();
    assert_eq!(l.state("47780"), "start once");
}

// ---- more

#[test]
fn bytes_that_are_not_text_are_kept_through_an_edit() {
    let l = setup();
    let f = l.ds2_file();
    fs::create_dir_all(Path::new(&f).parent().unwrap()).unwrap();
    fs::write(&f, b"Name = caf\xe9\r\nWindow.Width = 1024\r\n\xff\xfe = x\r\n").unwrap();
    l.apply("47780");
    let got = fs::read(&f).unwrap();
    assert!(got.starts_with(b"Name = caf\xe9\r\nWindow.Width = 3440\r\n\xff\xfe = x\r\n"), "{:?}", String::from_utf8_lossy(&got));
    l.undo("47780");
    assert_eq!(fs::read(&f).unwrap(), b"Name = caf\xe9\r\nWindow.Width = 1024\r\n\xff\xfe = x\r\n");
}

#[test]
fn names_in_braces_are_filled_as_the_regular_expression_did() {
    let v: Vec<(String, Value)> = vec![("game".into(), "/g".into()), ("width".into(), 3440.into()), ("n\u{e9}".into(), "x".into())];
    assert_eq!(fill("{game}/a/{width}", &v), "/g/a/3440");
    assert_eq!(fill("{unknown} {} {game {width}}", &v), "{unknown} {} {game 3440}");
    assert_eq!(fill("{{game}}", &v), "{/g}");
    assert_eq!(fill("{n\u{e9}}", &v), "x", "\\w takes letters of any script");
    assert_eq!(fill("{a-b}", &v), "{a-b}");
}

#[test]
fn values_are_written_as_python_wrote_str_of_them() {
    let spec = json::json!({"format": "kv", "set": {"A": 1, "B": true, "C": null, "D": 1.5, "E": [1, "x"], "F": "{width}"}});
    let v: Vec<(String, Value)> = vec![("width".into(), 7.into())];
    let w = wanted(&spec, &v).unwrap();
    let got: Vec<(&str, &str)> = w.iter().map(|(_, k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(got, vec![("A", "1"), ("B", "True"), ("C", "None"), ("D", "1.5"), ("E", "[1, 'x']"), ("F", "7")]);
}

#[test]
fn the_screen_from_the_variable_or_none() {
    let _l = env_lock();
    std::env::set_var("YGGDRASIL_SCREEN", "2560x1440");
    assert_eq!(screen(), Some((2560, 1440)));
    std::env::set_var("YGGDRASIL_SCREEN", "\u{0662}x1");
    assert_eq!(screen(), Some((2, 1)), "digits of any script, as \\d and int()");
    std::env::remove_var("YGGDRASIL_SCREEN");
}

#[test]
fn a_second_library_and_a_link_to_the_same_one() {
    let l = setup();
    let other = l.home.p("Games/SteamLibrary");
    write(&format!("{other}/steamapps/appmanifest_1.acf"), "\"appid\" \"1\"\n\"name\" \"One\"\n\"installdir\" \"one\"\n");
    std::os::unix::fs::symlink(&other, l.home.p("link")).unwrap();
    write(
        &format!("{}/libraryfolders.vdf", l.sa),
        &format!(
            "\"libraryfolders\"\n{{\n \"0\" {{ \"path\" \"{}\" }}\n \"1\" {{ \"path\" \"{}\" }}\n \"2\" {{ \"path\"\t\"{}\" }}\n}}\n",
            other,
            l.home.p("link"),
            other.replace('/', "\\\\/")
        ),
    );
    let games = installed(&l.home.p("")).unwrap();
    assert_eq!(games["1"].steamapps, format!("{other}/steamapps"), "the first path to the library is the one kept");
    assert_eq!(games.len(), 3);
}
