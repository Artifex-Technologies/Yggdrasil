//! The Python script's tests (python/tests/test_labyrinth_appusage.py), carried over, and the service's own.

use super::*;
use crate::watch::{scan, Tracker, IDS_MAX_AGE};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn ids() -> BTreeSet<String> {
    ["firefox", "spotify-launcher", "com.microsoft.VSCode", "org.gimp.GIMP", "foo"].iter().map(|s| s.to_string()).collect()
}

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("appusage-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read(p: &Path) -> Value {
    json::loads(&fs::read_to_string(p).unwrap()).unwrap()
}

fn units(u: &[&str]) -> Vec<String> {
    u.iter().map(|s| s.to_string()).collect()
}

// ---- test_labyrinth_appusage.py

#[test]
fn test_units_map_to_desktop_ids() {
    let ids = ids();
    let m = |u: &str| match_unit(u, &ids);
    assert_eq!(m("lapp_spotify_launcher_123.scope").as_deref(), Some("spotify-launcher"));
    assert_eq!(m("app-firefox-4567.scope").as_deref(), Some("firefox"));
    assert_eq!(m("app-com.microsoft.VSCode-1915.scope").as_deref(), Some("com.microsoft.VSCode"));
    assert_eq!(m("app-spotify\\x2dlauncher-9.scope").as_deref(), Some("spotify-launcher"));
    assert_eq!(m("app-flatpak-org.gimp.GIMP-3.scope").as_deref(), Some("org.gimp.GIMP"));
    assert_eq!(m("app-foo@abc123.scope").as_deref(), Some("foo"));
    assert_eq!(m("session-3.scope"), None);
    assert_eq!(m("app-unknown-1.scope"), None);
}

#[test]
fn test_seconds_add_up_once_per_app() {
    let d = Tmp::new();
    let p = d.0.join("u").join("app-usage.json");
    let apps = sample(30, &ids(), &units(&["app-firefox-1.scope", "app-firefox-2.scope", "lapp_foo_9.scope"]), &p).unwrap();
    assert_eq!(apps, ["firefox", "foo"].iter().map(|s| s.to_string()).collect());
    sample(30, &ids(), &units(&["app-firefox-1.scope"]), &p).unwrap();
    let u = read(&p);
    assert_eq!(u["firefox"]["seconds"], 60);
    assert_eq!(u["foo"]["seconds"], 30);
    // launch counts written by labyrinth-launch are kept
    let mut u = u;
    u["firefox"]["launches"] = 5.into();
    fs::write(&p, json::dumps(&u)).unwrap();
    sample(30, &ids(), &units(&["app-firefox-1.scope"]), &p).unwrap();
    assert_eq!(read(&p)["firefox"], json::json!({"seconds": 90, "launches": 5}));
}

// ---- the matching, where the script's regular expressions decide

#[test]
fn the_patterns_decide_as_the_regular_expressions_did() {
    let ids: BTreeSet<String> = ["flatpak", "a-1", "a", "x_y", "dev.app", "app"].iter().map(|s| s.to_string()).collect();
    let m = |u: &str| match_unit(u, &ids);
    // flatpak- is taken off first; only when nothing matches that way is it part of the id
    assert_eq!(m("app-flatpak-1.scope").as_deref(), Some("flatpak"));
    // the shortest id that leaves -DIGITS.scope: "a-1" ("a" would leave -1-2)
    assert_eq!(m("app-a-1-2.scope").as_deref(), Some("a-1"));
    // lapp_: the split is at the last underscore, and the id's sanitize() must be what is left
    assert_eq!(m("lapp_x_y_12.scope").as_deref(), Some("x_y"));
    assert_eq!(m("lapp_dev_app_7.scope").as_deref(), Some("dev.app"));
    assert_eq!(m("lapp_x_y_12a.scope"), None);
    assert_eq!(m("lapp__1.scope"), None, "(.+) needs a character");
    // digits of any script, as \d; a hash only in lowercase hex
    assert_eq!(m("app-a-\u{0663}.scope").as_deref(), Some("a"));
    assert_eq!(m("app-a@ABC.scope"), None);
    assert_eq!(m("app-a@.scope"), None);
    // the dot does not cross a newline; nothing is matched past the end
    assert_eq!(m("app-a\nb-1.scope"), None);
    assert_eq!(m("app-a-1.scope\n"), None);
    assert_eq!(m("app-a-1.service"), None);
    assert_eq!(m("app--1.scope"), None);
    assert_eq!(m("app-app-1.scope").as_deref(), Some("app"));
}

// ---- the file: whatever is in it

#[test]
fn a_broken_file_counts_as_empty_and_odd_entries_as_the_script_read_them() {
    let d = Tmp::new();
    let p = d.0.join("app-usage.json");
    fs::write(&p, "{not json").unwrap();
    sample(30, &ids(), &units(&["app-firefox-1.scope"]), &p).unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), r#"{"firefox": {"seconds": 30}}"#);
    // int() of what is there: a float is cut, text read as a number, true is 1; an empty entry starts again
    fs::write(&p, r#"{"firefox": {"seconds": 10.9, "last": 1.5}, "foo": {"seconds": " 7 "}, "spotify-launcher": 0, "org.gimp.GIMP": {"seconds": true}}"#).unwrap();
    sample(30, &ids(), &units(&["app-firefox-1.scope", "lapp_foo_1.scope", "lapp_spotify_launcher_2.scope", "app-flatpak-org.gimp.GIMP-3.scope"]), &p).unwrap();
    assert_eq!(
        fs::read_to_string(&p).unwrap(),
        r#"{"firefox": {"seconds": 40, "last": 1.5}, "foo": {"seconds": 37}, "spotify-launcher": {"seconds": 30}, "org.gimp.GIMP": {"seconds": 31}}"#
    );
}

#[test]
fn a_file_the_script_stopped_on_is_left_alone() {
    let d = Tmp::new();
    let p = d.0.join("app-usage.json");
    for text in [r#"[1, 2]"#, r#"{"firefox": {"seconds": "lots"}}"#, r#"{"firefox": 5}"#, r#"{"firefox": {"seconds": null}}"#] {
        fs::write(&p, text).unwrap();
        assert!(matches!(sample(30, &ids(), &units(&["app-firefox-1.scope"]), &p), Err(WriteError::Unusable(_))), "{text}");
        assert_eq!(fs::read_to_string(&p).unwrap(), text);
    }
    // nothing running: nothing written, not even a new file
    let q = d.0.join("none.json");
    sample(30, &ids(), &units(&["session-1.scope"]), &q).unwrap();
    assert!(!q.exists());
}

#[test]
fn the_file_is_written_as_python_writes_it() {
    let d = Tmp::new();
    let p = d.0.join("app-usage.json");
    fs::write(&p, "{\"caf\u{e9}\": {\"launches\": 2, \"last\": 1700000000}, \"firefox\": {}}").unwrap();
    sample(30, &ids(), &units(&["app-firefox-1.scope"]), &p).unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "{\"caf\\u00e9\": {\"launches\": 2, \"last\": 1700000000}, \"firefox\": {\"seconds\": 30}}");
    assert!(!d.0.join("app-usage.json.tmp").exists());
}

// ---- the service's counting

fn set(s: &[&str]) -> BTreeSet<String> {
    s.iter().map(|x| x.to_string()).collect()
}

#[test]
fn an_app_counts_once_from_its_first_scope_to_its_last() {
    let t0 = Instant::now();
    let mut t = Tracker::new(ids(), t0);
    let mut no_ids = || -> BTreeSet<String> { panic!("not read again") };
    assert!(!t.update(&set(&["app-firefox-1.scope"]), t0, &mut no_ids));
    assert!(!t.update(&set(&["app-firefox-1.scope", "app-firefox-2.scope"]), t0 + Duration::from_secs(10), &mut no_ids));
    assert!(!t.update(&set(&["app-firefox-2.scope"]), t0 + Duration::from_secs(20), &mut no_ids), "a window closed, the app runs on");
    assert!(t.update(&set(&[]), t0 + Duration::from_millis(45_500), &mut no_ids), "the last one: it stopped");
    assert_eq!(t.take(t0 + Duration::from_secs(50)), [("firefox".to_string(), 45)].into());
    // the half second is kept for next time
    t.update(&set(&["app-firefox-3.scope"]), t0 + Duration::from_secs(100), &mut no_ids);
    assert_eq!(t.take(t0 + Duration::from_millis(100_600)), [("firefox".to_string(), 1)].into());
    assert!(t.any_running());
}

#[test]
fn time_that_could_not_be_written_is_written_later() {
    let t0 = Instant::now();
    let mut t = Tracker::new(ids(), t0);
    let mut no_ids = || -> BTreeSet<String> { BTreeSet::new() };
    t.update(&set(&["lapp_foo_1.scope"]), t0, &mut no_ids);
    let secs = t.take(t0 + Duration::from_secs(30));
    t.give_back(secs);
    assert_eq!(t.take(t0 + Duration::from_secs(60)), [("foo".to_string(), 60)].into());
}

#[test]
fn the_desktop_entries_are_read_again_when_old() {
    let t0 = Instant::now();
    let mut t = Tracker::new(BTreeSet::new(), t0);
    let reads = std::cell::Cell::new(0);
    let mut fresh = || {
        reads.set(reads.get() + 1);
        ids()
    };
    // an unknown app a moment after the entries were read: not yet
    t.update(&set(&["app-firefox-1.scope"]), t0 + Duration::from_secs(5), &mut fresh);
    assert_eq!(reads.get(), 0);
    // a minute later another scope that matches nothing: read again, and the running firefox counts from then
    t.update(&set(&["app-firefox-1.scope", "app-new-2.scope"]), t0 + Duration::from_secs(70), &mut fresh);
    assert_eq!(reads.get(), 1);
    assert_eq!(t.take(t0 + Duration::from_secs(100)), [("firefox".to_string(), 30)].into());
    // known apps only: read again after ten minutes
    t.update(&set(&["app-firefox-1.scope", "app-new-2.scope", "app-foo-3.scope"]), t0 + Duration::from_secs(100), &mut fresh);
    assert_eq!(reads.get(), 1);
    t.update(&set(&["app-foo-4.scope"]), t0 + Duration::from_secs(70) + IDS_MAX_AGE + Duration::from_secs(1), &mut fresh);
    assert_eq!(reads.get(), 2);
}

#[test]
fn scopes_are_found_in_slices_at_any_depth() {
    let d = Tmp::new();
    for p in ["init.scope", "app.slice/app-firefox-1.scope", "app.slice/lapp_foo.slice/lapp_foo_9.scope", "app.slice/x.service", "session.slice/a.slice/b.slice/app-foo-2.scope", "app.slice/app-firefox-1.scope/sub"] {
        fs::create_dir_all(d.0.join(p)).unwrap();
    }
    fs::write(d.0.join("app.slice/not-a-dir.scope"), "").unwrap();
    let mut watched = vec![];
    let found = scan(&d.0, |p| watched.push(p.strip_prefix(&d.0).unwrap().to_path_buf()));
    assert_eq!(found, set(&["init.scope", "app-firefox-1.scope", "lapp_foo_9.scope", "app-foo-2.scope"]));
    watched.sort();
    let want: Vec<PathBuf> = ["", "app.slice", "app.slice/lapp_foo.slice", "session.slice", "session.slice/a.slice", "session.slice/a.slice/b.slice"].iter().map(PathBuf::from).collect();
    assert_eq!(watched, want);
}
