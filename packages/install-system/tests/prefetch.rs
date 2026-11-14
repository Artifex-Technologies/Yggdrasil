//! --prefetch on a made-up computer (tests/fake): what it would download for the choices, one package at a time and at low
//! priority, into its own folder with a private database; the fallback to fresh package lists; no room, no network, a stop;
//! and the install taking what was fetched. Nothing is downloaded: the mirrors are made up too.

mod fake;

use fake::{plan_with, Live, GB};

/// A browser the medium does not carry, beside the two it does.
fn with_librewolf(live: &Live) {
    live.dir.write(
        "os/browsers.json",
        r#"{"browsers": [{"id": "vivaldi", "packages": ["vivaldi"], "desktop": ["vivaldi-stable.desktop"]}, {"id": "firefox", "packages": ["firefox"], "desktop": ["firefox.desktop"]}, {"id": "librewolf", "packages": ["librewolf"], "desktop": ["librewolf.desktop"]}]}"#,
    );
}

const CHOICES: &str = r#"{"browsers": ["vivaldi", "librewolf"], "extras": ["monitor"]}"#;

fn pacman_calls(f: &fake::Fake) -> Vec<Vec<String>> {
    f.calls.iter().filter(|(a, _)| a[0] == "nice").map(|(a, _)| a.clone()).collect()
}

#[test]
fn the_chosen_packages_one_at_a_time_into_its_own_folder() {
    let live = Live::new();
    with_librewolf(&live);
    let mut f = live.fake();
    assert_eq!(live.prefetch(&mut f, CHOICES, &|| false), 0, "{:?}", f.out);
    assert_eq!(f.out, ["PREFETCH 0 3", "PREFETCH 1 3", "PREFETCH 2 3", "PREFETCH 3 3", "DONE "].map(|s| s.trim_end().to_string()));
    let calls = pacman_calls(&f);
    let dir = live.dir.path("prefetch");
    for a in &calls {
        assert_eq!(&a[..7], ["nice", "-n", "19", "ionice", "-c", "3", "pacman"], "low priority");
        assert!(a.windows(2).any(|w| w == [String::from("--dbpath"), format!("{dir}/db")]), "a private database: {a:?}");
        assert!(a.windows(2).any(|w| w == [String::from("--cachedir"), format!("{dir}/pkg")]));
        assert!(!a.iter().any(|x| x == "-S" || x == "-U" || x.starts_with("-Su") && x != "-Sup"), "nothing is installed: {a:?}");
    }
    // what the install would get: the browser the medium lacks and the optional part's package (vivaldi is on the medium)
    let listed = &calls[0];
    assert!(listed.contains(&"-Sp".to_string()) && listed.ends_with(&["librewolf".to_string(), "monitor-git".to_string()]), "{listed:?}");
    let got: Vec<&String> = calls[1..].iter().map(|a| a.last().unwrap()).collect();
    assert_eq!(got, ["extra/librewolf", "extra/monitor-git", "core/libexample"]);
    assert!(calls[1..].iter().all(|a| a.contains(&"-Swdd".to_string())), "one package each time, its dependencies listed apart");
    assert!(std::fs::read_to_string(format!("{dir}/pacman.conf")).unwrap().contains("ParallelDownloads = 1\n"));
    assert_eq!(std::fs::read_link(format!("{dir}/db/local")).unwrap().to_str(), Some(live.dir.path("pacman/local").as_str()));
    assert_eq!(std::fs::read_to_string(format!("{dir}/db/sync/core.db")).unwrap(), "the medium's core list", "the medium's lists first");
    assert_eq!(std::fs::read_dir(format!("{dir}/pkg")).unwrap().count(), 3);
    // again with the same choices: the same, pacman skipping what it has
    let mut g = live.fake();
    assert_eq!(live.prefetch(&mut g, CHOICES, &|| false), 0);
    assert_eq!(g.out.last().unwrap(), "DONE");
}

#[test]
fn when_the_mirrors_moved_on_fresh_lists_and_the_updates() {
    let live = Live::new();
    with_librewolf(&live);
    let mut f = live.fake();
    f.mirror.medium_old = true;
    assert_eq!(live.prefetch(&mut f, CHOICES, &|| false), 0, "{:?}", f.out);
    let calls = pacman_calls(&f);
    let ops: Vec<&str> = calls.iter().map(|a| a.iter().find(|x| x.starts_with("-S")).unwrap().as_str()).collect();
    assert_eq!(ops, ["-Sp", "-Swdd", "-Sy", "-Sup", "-Swdd", "-Swdd", "-Swdd", "-Swdd"], "as the install's pacman -Syu would");
    assert!(f.out.contains(&"PREFETCH 4 4".to_string()) && f.out.last().unwrap() == "DONE");
    assert_eq!(calls.last().unwrap().last().unwrap(), "core/systemd-update");
}

#[test]
fn no_network_no_room_or_nothing_to_get() {
    let live = Live::new();
    with_librewolf(&live);
    let mut f = live.fake();
    f.mirror.offline = true;
    assert_eq!(live.prefetch(&mut f, CHOICES, &|| false), 3);
    assert_eq!(f.out.last().unwrap(), "WARN the package mirrors could not be reached; the install downloads as usual");
    let mut f = live.fake();
    f.mirror.avail = GB / 4;
    assert_eq!(live.prefetch(&mut f, CHOICES, &|| false), 3);
    assert!(f.out.last().unwrap().starts_with("WARN not enough room"));
    assert!(!pacman_calls(&f).iter().any(|a| a.contains(&"-Swdd".to_string())), "nothing fetched without room");
    // what the medium has needs nothing
    let mut f = live.fake();
    assert_eq!(live.prefetch(&mut f, r#"{"browsers": ["vivaldi"], "extras": []}"#, &|| false), 0);
    assert_eq!(f.out, ["PREFETCH 0 0", "DONE"]);
    assert!(pacman_calls(&f).is_empty());
    // not root, and choices that are not JSON
    let mut f = live.fake();
    f.euid = 1000;
    assert_eq!(live.prefetch(&mut f, CHOICES, &|| false), 1);
    let mut f = live.fake();
    assert_eq!(live.prefetch(&mut f, "{not json", &|| false), 2);
    assert_eq!(f.out, ["ERROR the choices are not JSON"]);
}

#[test]
fn a_stop_between_two_packages() {
    let live = Live::new();
    with_librewolf(&live);
    let mut f = live.fake();
    let asked = std::cell::Cell::new(0);
    // the window closes standard input after the first package
    let stop = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };
    assert_eq!(live.prefetch(&mut f, CHOICES, &stop), 0);
    assert_eq!(f.out, ["PREFETCH 0 3", "PREFETCH 1 3", "STOPPED"]);
}

#[test]
fn the_install_takes_what_was_fetched() {
    let live = Live::new();
    for (n, t) in [
        ("prefetch/pkg/librewolf-1-1-x86_64.pkg.tar.zst", "package"),
        ("prefetch/pkg/librewolf-1-1-x86_64.pkg.tar.zst.sig", "signature"),
        ("prefetch/pkg/half-1-1-x86_64.pkg.tar.zst.part", "half"),
        ("elsewhere/secret", "not a package"),
    ] {
        live.dir.write(n, t);
    }
    std::os::unix::fs::symlink(live.dir.path("elsewhere/secret"), live.dir.path("prefetch/pkg/link-1-1-x86_64.pkg.tar.zst")).unwrap();
    let mut f = live.fake();
    let p = plan_with(&[("browsers", r#"["firefox"]"#), ("default_browser", "\"firefox\"")]);
    assert_eq!(live.run(&mut f, &[], &p), 0, "{:?}", f.out);
    assert!(f.out.contains(&"INFO 1 package was fetched beforehand".to_string()), "{:?}", f.out);
    let cache = |n: &str| std::path::Path::new(&format!("{}/var/cache/pacman/pkg/{n}", live.target())).exists();
    assert!(cache("librewolf-1-1-x86_64.pkg.tar.zst") && cache("librewolf-1-1-x86_64.pkg.tar.zst.sig"));
    assert!(!cache("half-1-1-x86_64.pkg.tar.zst.part"), "a download that did not finish is left");
    assert!(!cache("link-1-1-x86_64.pkg.tar.zst"), "never a link out of the folder");
    // without a prefetch folder, the install says nothing about it
    let live = Live::new();
    let mut f = live.fake();
    assert_eq!(live.run(&mut f, &[], &p), 0);
    assert!(!f.out.iter().any(|l| l.contains("beforehand")));
}
