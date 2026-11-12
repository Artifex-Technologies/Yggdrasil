//! Random plans on the made-up computer (tests/fake): the checks never panic, a refused plan runs no program at all, and an
//! install, whatever the plan, never gives a disk tool anything but the chosen disk, its partitions and the encrypted system's
//! mapper: another disk with Windows on it (/dev/vdy) and the live USB (/dev/sdy) are never named.

mod fake;

use fake::{Disk, Live, MIB};
use install_system::pyjson::{s, Py};
use proptest::prelude::*;

const KEYS: [&str; 27] = [
    "mode",
    "disk",
    "user",
    "login",
    "password",
    "name",
    "hostname",
    "timezone",
    "locale",
    "keymap",
    "keymap_variant",
    "encrypt",
    "encrypt_password",
    "browsers",
    "default_browser",
    "extras",
    "new_table",
    "delete",
    "create",
    "use",
    "id",
    "start",
    "size",
    "kind",
    "part",
    "mount",
    "format",
];
const WORDS: [&str; 30] = [
    "/dev/vdz",
    "/dev/vdy",
    "/dev/vdz1",
    "/dev/vdy1",
    "/dev/sdy",
    "erase",
    "manual",
    "new:a",
    "new:",
    "/",
    "/boot/efi",
    "/home",
    "swap",
    "btrfs",
    "vfat",
    "ext4",
    "ada",
    "root",
    "UTC",
    "us",
    "dvorak",
    "en_US.UTF-8",
    "firefox",
    "monitor",
    "x\n",
    "a:b",
    "\u{0}",
    "../x",
    "é\u{2028}",
    "true",
];

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(WORDS.to_vec()).prop_map(String::from),
        "[ -~]{0,10}",
        any::<String>().prop_map(|t| t.chars().take(8).collect())
    ]
}

/// Any JSON value, as text (some of Python's own: NaN, Infinity, big numbers).
fn json() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        prop::sample::select(vec![
            "null",
            "true",
            "false",
            "0",
            "-0",
            "1",
            "-1",
            "NaN",
            "-Infinity",
            "1e400",
            "1.5",
            "1180591620717411303424"
        ])
        .prop_map(String::from),
        any::<i64>().prop_map(|n| n.to_string()),
        text().prop_map(|t| s(&t).dumps()),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(|v| format!("[{}]", v.join(", "))),
            prop::collection::vec((prop::sample::select(KEYS.to_vec()), inner), 0..7)
                .prop_map(|kv| format!("{{{}}}", kv.iter().map(|(k, v)| format!("\"{k}\": {v}")).collect::<Vec<_>>().join(", "))),
        ]
    })
}

/// A manual plan for /dev/vdz with Windows on it: mostly the window's kinds of choices (an EFI partition, a /, perhaps a /home
/// or swap, perhaps Windows deleted), with wrong values mixed in.
fn manual() -> impl Strategy<Value = String> {
    let gb = fake::GB;
    let root_start = prop::sample::select(vec![300 * gb, 300 * gb, 300 * gb, 300 * gb, 317 * MIB, 200 * gb, 300 * gb + 512]);
    let root_size = prop::sample::select(vec![30 * gb, 30 * gb, 100 * gb, 100 * gb, 140 * gb, 10 * gb, 300 * gb]);
    let second = prop::option::weighted(
        0.4,
        (
            prop::sample::select(vec![450 * gb, 460 * gb, 301 * MIB]),
            prop::sample::select(vec![16 * gb, 30 * gb, 512 * MIB]),
            prop::sample::select(vec!["linux", "swap", "efi"]),
        ),
    );
    let esp = prop::sample::select(vec![
        ("/dev/vdz1", "null"),
        ("/dev/vdz1", "null"),
        ("/dev/vdz1", "null"),
        ("/dev/vdz1", "\"vfat\""),
        ("/dev/vdz1", "\"vfat\""),
        ("new:b", "\"vfat\""),
        ("/dev/vdz3", "null"),
        ("/dev/vdy1", "null"),
    ]);
    let root = prop::sample::select(vec![
        ("new:a", "\"btrfs\""),
        ("new:a", "\"btrfs\""),
        ("new:a", "\"ext4\""),
        ("new:a", "\"xfs\""),
        ("new:a", "\"btrfs\""),
        ("/dev/vdz3", "\"btrfs\""),
        ("new:a", "null"),
        ("/dev/vdz2", "\"ext4\""),
    ]);
    let extra = prop::option::weighted(
        0.3,
        prop::sample::select(vec![
            ("new:b", "/home", "\"ext4\""),
            ("new:b", "swap", "\"swap\""),
            ("/dev/vdz3", "/srv", "null"),
            ("/dev/vdz2", "/home", "null"),
            ("new:b", "/var", "\"btrfs\""),
        ]),
    );
    let delete = prop::sample::select(vec![vec![], vec![], vec![], vec![], vec![3], vec![3, 2], vec![2], vec![9], vec![1]]);
    let new_table = prop::sample::select(vec!["false", "false", "false", "false", "false", "true", "null", "\"false\""]);
    (root_start, root_size, second, esp, root, extra, delete, new_table, any::<bool>()).prop_map(
        |(rs, rz, second, esp, root, extra, delete, new_table, encrypt)| {
            let mut create = vec![format!(r#"{{"id": "a", "start": {rs}, "size": {rz}, "kind": "linux"}}"#)];
            if let Some((st, sz, k)) = second {
                create.push(format!(r#"{{"id": "b", "start": {st}, "size": {sz}, "kind": "{k}"}}"#));
            }
            let mut uses = vec![
                format!(r#"{{"part": "{}", "mount": "/boot/efi", "format": {}}}"#, esp.0, esp.1),
                format!(r#"{{"part": "{}", "mount": "/", "format": {}}}"#, root.0, root.1),
            ];
            if let Some((p, m, f)) = extra {
                uses.push(format!(r#"{{"part": "{p}", "mount": "{m}", "format": {f}}}"#));
            }
            fake::plan_with(&[
                ("mode", "\"manual\""),
                ("new_table", new_table),
                ("delete", &format!("{delete:?}")),
                ("create", &format!("[{}]", create.join(", "))),
                ("use", &format!("[{}]", uses.join(", "))),
                ("encrypt", if encrypt { "true" } else { "false" }),
                ("encrypt_password", "\"pw-DISK-secret-2\""),
            ])
        },
    )
}

/// The whole-disk plan with one value changed to something random.
fn changed() -> impl Strategy<Value = String> {
    (prop::sample::select(KEYS[..16].to_vec()), json()).prop_map(|(k, v)| fake::plan_with(&[(k, &v)]))
}

const DISK_TOOLS: [&str; 9] = ["wipefs", "sfdisk", "mkfs.btrfs", "mkfs.ext4", "mkfs.xfs", "mkfs.vfat", "mkswap", "cryptsetup", "mount"];

/// Runs a plan on a computer with Windows on /dev/vdz (the chosen disk) and on /dev/vdy, and checks what it may and may not do.
fn check(plan: &str) -> Result<(), TestCaseError> {
    let live = Live::new();
    let mut f = live.fake();
    f.disks = vec![Disk::windows("/dev/vdz"), Disk::windows("/dev/vdy"), Disk::empty("/dev/sdy", 16 * fake::GB)];
    let before: Vec<u32> = f.disks[0].parts.iter().map(|q| q.n).collect();
    let code = live.run(&mut f, &[], plan);
    prop_assert!([0, 1, 2].contains(&code));
    prop_assert!(!f.out.is_empty(), "every run says something");
    let last = f.out.last().unwrap();
    prop_assert!(last == "DONE" || last.starts_with("ERROR "), "{}", last);
    if code == 2 {
        prop_assert!(f.calls.is_empty(), "a refused plan ran {:?}", f.calls.first());
        return Ok(());
    }
    for (argv, _) in &f.calls {
        if !DISK_TOOLS.contains(&argv[0].as_str()) {
            continue;
        }
        for a in argv.iter().filter(|a| a.starts_with("/dev")) {
            prop_assert!(a == "/dev" || a.starts_with("/dev/vdz") || a == "/dev/mapper/example-root", "{:?} touched {}", argv, a);
        }
    }
    prop_assert_eq!(f.disks[1].parts.len(), 3, "the other disk is as it was");
    let p = install_system::read_plan(plan.as_bytes()).unwrap();
    let wiped = f.calls.iter().any(|(a, _)| a[0] == "wipefs" || (a[0] == "sfdisk" && a[1] == "--wipe"));
    let whole = p.at("mode").as_str() == Some("erase") || p.at("new_table") == &Py::Bool(true);
    prop_assert!(!wiped || whole, "the table was wiped for a plan that keeps it");
    if !whole {
        // only the partitions the plan names for deletion are gone
        let deleted: Vec<u32> =
            f.calls.iter().filter(|(a, _)| a[0] == "sfdisk" && a[1] == "--delete").map(|(a, _)| a[3].parse().unwrap()).collect();
        let named: Vec<i128> = p.at("delete").as_list().unwrap_or(&[]).iter().filter_map(Py::whole).collect();
        for n in &deleted {
            prop_assert!(named.contains(&(*n as i128)), "deleted {} not named", n);
        }
        for n in before {
            prop_assert!(deleted.contains(&n) || f.disks[0].parts.iter().any(|q| q.n == n), "partition {} disappeared", n);
        }
    }
    Ok(())
}

proptest! {
    // 400 cases each, or PROPTEST_CASES
    #![proptest_config(ProptestConfig { cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(400), ..ProptestConfig::default() })]

    #[test]
    fn any_bytes_are_answered(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
        let _ = install_system::read_plan(&bytes);
        check(&String::from_utf8_lossy(&bytes))?;
    }

    #[test]
    fn any_json_is_answered(plan in json()) {
        check(&plan)?;
    }

    #[test]
    fn a_whole_disk_plan_changed_in_one_place(plan in changed()) {
        check(&plan)?;
    }

    #[test]
    fn manual_plans_touch_the_chosen_disk_only(plan in manual()) {
        check(&plan)?;
    }
}

#[test]
fn some_random_manual_plans_get_installed() {
    // the property above is only worth something if random plans get past the checks: count them on a fixed sample
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let mut done = 0;
    for _ in 0..400 {
        let plan = manual().new_tree(&mut runner).unwrap().current();
        let live = Live::new();
        let mut f = live.fake();
        f.disks = vec![Disk::windows("/dev/vdz"), Disk::windows("/dev/vdy")];
        if live.run(&mut f, &[], &plan) == 0 {
            done += 1;
        }
    }
    eprintln!("{done} of 400 random manual plans were installed");
    assert!(done >= 30, "only {done} of 400 random manual plans were installed");
}

#[test]
#[ignore]
fn why_random_manual_plans_stop() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let mut why: std::collections::BTreeMap<String, usize> = Default::default();
    for _ in 0..400 {
        let plan = manual().new_tree(&mut runner).unwrap().current();
        let live = Live::new();
        let mut f = live.fake();
        f.disks = vec![Disk::windows("/dev/vdz"), Disk::windows("/dev/vdy")];
        live.run(&mut f, &[], &plan);
        *why.entry(f.out.last().unwrap().clone()).or_default() += 1;
    }
    for (k, v) in why {
        eprintln!("{v:4} {k}");
    }
}
