//! The install on a made-up computer (tests/fake): the steps it says, the programs it runs, the files it leaves in the new
//! system, and where the passwords go. (parity/compare.py compares all of this with the Python script, program by program.)

mod fake;

use fake::{manual_plan, plan, plan_with, Disk, Live, ACCOUNT_PW, DISK_PW};

const DISK_TOOLS: [&str; 9] = ["wipefs", "sfdisk", "mkfs.btrfs", "mkfs.ext4", "mkfs.xfs", "mkfs.vfat", "mkswap", "cryptsetup", "mount"];

/// Every device a disk tool was given: only the chosen disk, its partitions and the encrypted system's mapper.
fn devices_touched(f: &fake::Fake) -> Vec<String> {
    f.calls
        .iter()
        .filter(|(a, _)| DISK_TOOLS.contains(&a[0].as_str()))
        .flat_map(|(a, _)| a.iter().filter(|x| x.starts_with("/dev/") && *x != "/dev").cloned().collect::<Vec<_>>())
        .collect()
}

#[test]
fn the_whole_disk_unencrypted() {
    let live = Live::new();
    let mut f = live.fake();
    assert_eq!(live.run(&mut f, &[], &plan().dumps()), 0, "{:?}", f.out);
    let steps: Vec<&String> = f.out.iter().filter(|l| l.starts_with("STEP")).collect();
    assert_eq!(steps.len(), 9);
    assert_eq!(steps[0], "STEP 1 9 Checking the computer");
    assert_eq!(f.out.last().unwrap(), "DONE");
    assert!(f.out.contains(&"INFO not encrypted (as chosen)".to_string()));
    assert!(f.out.contains(&"INFO copied 55%".to_string()));
    for d in devices_touched(&f) {
        assert!(d == "/dev/vdz" || d.starts_with("/dev/vdz"), "{d}");
    }
    assert_eq!(f.program_calls("wipefs")[0].0, ["wipefs", "-a", "/dev/vdz"]);
    let fstab = live.read("etc/fstab");
    assert!(fstab.starts_with("# Written by yggdrasil-install-system."));
    assert!(fstab.contains("UUID=UUID-OF-_dev_vdz2 / btrfs compress=zstd:1,noatime,subvol=/@ 0 0"));
    assert!(fstab.contains("UUID=UUID-OF-_dev_vdz2 /home btrfs compress=zstd:1,noatime,subvol=/@home 0 0"));
    assert!(fstab.contains("UUID=UUID-OF-_dev_vdz1 /boot/efi vfat umask=0077 0 2"));
    assert_eq!(live.read("etc/hostname"), "test-pc\n");
    assert_eq!(live.read("etc/hosts"), "127.0.0.1 localhost\n::1 localhost\n127.0.1.1 test-pc.localdomain test-pc\n");
    assert_eq!(live.read("etc/locale.conf"), "LANG=en_US.UTF-8\n");
    assert_eq!(live.read("etc/locale.gen"), "en_US.UTF-8 UTF-8\n#de_DE.UTF-8 UTF-8\n");
    assert_eq!(live.read("etc/vconsole.conf"), "KEYMAP=us\nXKBLAYOUT=us\n");
    assert!(live.read("etc/X11/xorg.conf.d/00-keyboard.conf").contains("Option \"XkbLayout\" \"us\""));
    assert_eq!(live.read("etc/machine-id"), "");
    assert_eq!(std::fs::read_link(format!("{}/etc/localtime", live.target())).unwrap().to_str(), Some("/usr/share/zoneinfo/Europe/Lisbon"));
    let sudoers = format!("{}/etc/sudoers.d/10-example-wheel", live.target());
    assert!(std::fs::read_to_string(&sudoers).unwrap().contains("%wheel ALL=(ALL:ALL) ALL"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&sudoers).unwrap().permissions().mode() & 0o777, 0o440);
    let grub = live.read("etc/default/grub");
    assert!(grub.contains("GRUB_DEFAULT=0\nGRUB_TIMEOUT=2\n"));
    assert!(grub.contains("GRUB_CMDLINE_LINUX_DEFAULT='quiet'\n"));
    assert!(grub.contains("GRUB_DISTRIBUTOR=Example\n"));
    assert!(!grub.lines().any(|l| l == "GRUB_ENABLE_CRYPTODISK=y"), "the commented line stays as it was");
    let useradd = &f.program_calls("useradd")[0].0;
    assert_eq!(&useradd[4..], ["useradd", "-m", "-G", "users,wheel,audio", "-s", "/bin/bash", "-c", "Ada Example", "ada"]);
    assert!(
        f.program_calls("grub-install").iter().any(|(a, _)| a.contains(&"--removable".to_string())),
        "a fallback copy on an EFI partition made now"
    );
    assert_eq!(f.calls.last().unwrap().0, ["cryptsetup", "close", "example-root"]);
}

#[test]
fn passwords_only_on_standard_input() {
    let live = Live::new();
    let mut f = live.fake();
    assert_eq!(
        live.run(&mut f, &[], &plan_with(&[("encrypt", "true"), ("encrypt_password", &format!("\"{DISK_PW}\""))])),
        0,
        "{:?}",
        f.out
    );
    let mut seen = vec![];
    for (argv, stdin) in &f.calls {
        for pw in [ACCOUNT_PW, DISK_PW] {
            assert!(!argv.iter().any(|a| a.contains(pw)), "{pw} in {argv:?}");
        }
        if let Some(text) = stdin {
            let prog = if argv[0] == "chroot" { argv[4].as_str() } else { argv[0].as_str() };
            if text.contains(ACCOUNT_PW) {
                assert_eq!(prog, "chpasswd");
                assert_eq!(text, &format!("ada:{ACCOUNT_PW}\n"));
                seen.push("chpasswd".to_string());
            }
            if text.contains(DISK_PW) {
                assert_eq!(prog, "cryptsetup");
                assert_eq!(text, DISK_PW, "exactly the password, no line break (a key file read whole)");
                seen.push(argv[1].clone());
            }
        }
    }
    assert_eq!(seen, ["luksFormat", "open", "chpasswd", "luksAddKey"]);
    for line in f.out.iter().chain(f.err.iter()) {
        assert!(!line.contains(ACCOUNT_PW) && !line.contains(DISK_PW), "{line}");
    }
    let mut files = vec![std::path::PathBuf::from(live.target())];
    while let Some(p) = files.pop() {
        if p.is_dir() && !p.is_symlink() {
            files.extend(std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()));
        } else if let Ok(bytes) = std::fs::read(&p) {
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains(ACCOUNT_PW) && !text.contains(DISK_PW), "{}", p.display());
        }
    }
    // the key file: random bytes, readable by no one
    use std::os::unix::fs::PermissionsExt;
    let key = format!("{}/crypto_keyfile.bin", live.target());
    assert_eq!(std::fs::metadata(&key).unwrap().len(), 4096);
    assert_eq!(std::fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0);
    assert_eq!(f.random_calls, 1);
    let grub = live.read("etc/default/grub");
    assert!(grub.contains(
        "rd.luks.name=UUID-OF-_dev_vdz2=example-root rd.luks.key=UUID-OF-_dev_vdz2=/crypto_keyfile.bin root=/dev/mapper/example-root"
    ));
    assert!(grub.contains("GRUB_ENABLE_CRYPTODISK=y\n"));
    assert!(live.read("etc/mkinitcpio.conf.d/example-encrypt.conf").contains("\"sd-encrypt\" opens it"));
}

#[test]
fn beside_windows_nothing_else_is_touched() {
    let live = Live::new();
    let mut f = live.fake();
    f.disks[0] = Disk::windows("/dev/vdz");
    assert_eq!(live.run(&mut f, &[], &manual_plan(&[])), 0, "{:?}", f.out);
    assert!(f.program_calls("wipefs").is_empty(), "the table is kept");
    let sfdisk: Vec<&Vec<String>> = f.program_calls("sfdisk").iter().map(|(a, _)| a).collect();
    assert!(sfdisk.iter().all(|a| a[1] == "-J" || a[1] == "--append"), "{sfdisk:?}");
    assert!(f.program_calls("mkfs.vfat").is_empty(), "Windows' EFI partition is used as it is");
    let mkfs = f.program_calls("mkfs.btrfs");
    assert_eq!(mkfs.len(), 1);
    assert_eq!(mkfs[0].0.last().unwrap(), "/dev/vdz4");
    assert!(
        !f.program_calls("grub-install").iter().any(|(a, _)| a.contains(&"--removable".to_string())),
        "Windows' fallback is not replaced"
    );
    assert_eq!(f.disks[0].parts.iter().map(|q| q.n).collect::<Vec<_>>(), [1, 2, 3, 4]);
    for d in devices_touched(&f) {
        assert!(d.starts_with("/dev/vdz"), "{d}");
    }
}

#[test]
fn a_refused_plan_runs_nothing() {
    let live = Live::new();
    for (p, said) in [
        ("{not json", "ERROR the plan is not JSON"),
        ("[]", "ERROR the plan is not a JSON object"),
        (
            &plan_with(&[("hostname", "\"-x\"")]) as &str,
            "ERROR the computer's name may use letters, digits and - (not at the start or end)",
        ),
        (&plan_with(&[("user", r#"{"name": "A", "login": "taken", "password": "x"}"#)]), "ERROR that account name is taken by the system"),
        (
            &plan_with(&[("user", r#"{"name": "A", "login": "ada", "password": "x\nroot:y"}"#)]),
            "ERROR the account's password may not contain a line break",
        ),
        (&plan_with(&[("timezone", "\"/etc/hostname\"")]), "ERROR unknown time zone"),
        (&manual_plan(&[("new_table", "\"false\"")]), "ERROR a new partition table is chosen with true or false"),
        (&manual_plan(&[("delete", "[true]")]), "ERROR a partition to delete is not a number"),
        (&plan_with(&[("browsers", r#"["opera"]"#)]), "ERROR unknown browser chosen"),
    ] {
        let mut f = live.fake();
        assert_eq!(live.run(&mut f, &[], p), 2, "{p}");
        assert_eq!(f.out, [said], "{p}");
        assert!(f.calls.is_empty(), "{p}: {:?}", f.calls);
    }
}

#[test]
fn check_and_not_root_run_nothing() {
    let live = Live::new();
    let mut f = live.fake();
    assert_eq!(live.run(&mut f, &["--check"], &plan().dumps()), 0);
    assert_eq!(f.out, ["DONE"]);
    assert!(f.calls.is_empty());
    let mut f = live.fake();
    f.euid = 1000;
    assert_eq!(live.run(&mut f, &[], &plan().dumps()), 1);
    assert_eq!(f.out, ["ERROR yggdrasil-install-system needs root"]);
    assert!(f.calls.is_empty());
}

#[test]
fn the_disks_it_offers() {
    let live = Live::new();
    let mut f = live.fake();
    f.disks.push(Disk::windows("/dev/vdx"));
    f.disks.push(Disk::empty("/dev/vdw", 10 * fake::GB));
    assert_eq!(live.run(&mut f, &["--disks"], ""), 0);
    let v = install_system::pyjson::loads(&f.out[0], 20).unwrap();
    let list = v.as_list().unwrap();
    let paths: Vec<&str> = list.iter().map(|d| d.at("path").as_str().unwrap()).collect();
    assert_eq!(paths, ["/dev/vdz", "/dev/vdx", "/dev/vdw"], "never the live USB");
    assert_eq!(list[2].at("big_enough"), &install_system::pyjson::Py::Bool(false));
    let win = &list[1];
    assert_eq!(win.at("table").as_str(), Some("gpt"));
    assert_eq!(win.at("partitions").as_list().unwrap().len(), 3);
    assert_eq!(win.at("partitions").as_list().unwrap()[0].at("efi"), &install_system::pyjson::Py::Bool(true));
    let free = win.at("free").as_list().unwrap();
    assert_eq!(free.len(), 1);
    assert!(f.out[0].starts_with("[{\"path\": \"/dev/vdz\", \"size\": 536870912000, \"model\": \"TEST DISK 0001\""), "{}", f.out[0]);
}

#[test]
fn a_failure_stops_and_unmounts() {
    let live = Live::new();
    let mut f = live.fake();
    f.fail.push(vec!["mkfs.btrfs".into()]);
    assert_eq!(live.run(&mut f, &[], &plan().dumps()), 1);
    assert_eq!(f.out.last().unwrap(), "ERROR mkfs.btrfs -f -L failed: failed (made up)");
    let tail: Vec<&Vec<String>> = f.calls.iter().rev().take(2).map(|(a, _)| a).collect();
    assert_eq!(tail[1], &["umount", "-R", &live.target()]);
    assert_eq!(tail[0], &["cryptsetup", "close", "example-root"]);
}

#[test]
fn browsers_and_extras() {
    let live = Live::new();
    let mut f = live.fake();
    f.installed.push("monitor-git".into());
    let p = plan_with(&[("browsers", r#"["firefox"]"#), ("default_browser", "\"firefox\""), ("extras", "[]")]);
    assert_eq!(live.run(&mut f, &[], &p), 0, "{:?}", f.out);
    assert!(f.out.contains(&"INFO removing the browsers not chosen: vivaldi".to_string()));
    assert!(f.out.contains(&"INFO default browser: firefox.desktop".to_string()));
    assert!(f.out.contains(&"INFO removing what was not chosen: monitor-git".to_string()));
    assert!(live.read("etc/xdg/mimeapps.list").contains("text/html=firefox.desktop"));
    assert_eq!(f.installed, ["firefox"]);
}

/// What may never happen to the live system's mounts in one run: a bind of its /dev or /run/udev that is not made a slave in
/// the same call, or umount -R of the new system after such a bind without a successful `mount --make-rslave TARGET` first
/// (umount -R would carry over to the live system's /dev/pts, /dev/shm ... through a shared bind).
fn live_mounts_safe(f: &fake::Fake, target: &str) -> Result<(), String> {
    let (mut bound, mut slave) = (false, false);
    for ((argv, _), failed) in f.calls.iter().zip(&f.failed) {
        let a: Vec<&str> = argv.iter().map(String::as_str).collect();
        if a[0] == "mount" && (a.contains(&"--rbind") || a.contains(&"--bind")) {
            if !a.contains(&"--make-rslave") {
                return Err(format!("a bind without --make-rslave: {a:?}"));
            }
            bound = true;
        }
        if a == ["mount", "--make-rslave", target] && !failed {
            slave = true;
        }
        if a.starts_with(&["umount", "-R"]) && bound && !slave {
            return Err(format!("umount -R after a bind of the live /dev, with no successful mount --make-rslave first: {a:?}"));
        }
    }
    Ok(())
}

#[test]
fn a_failure_anywhere_never_unmounts_the_live_dev() {
    let encrypted = plan_with(&[("encrypt", "true"), ("encrypt_password", &format!("\"{DISK_PW}\""))]);
    let live = Live::new();
    let mut whole = live.fake();
    assert_eq!(live.run(&mut whole, &[], &encrypted), 0);
    live_mounts_safe(&whole, &live.target()).unwrap();
    let binds = whole.calls.iter().filter(|(a, _)| a[0] == "mount" && a.iter().any(|x| x == "--rbind" || x == "--bind")).count();
    assert_eq!(binds, 2, "/dev and /run/udev");
    assert!(
        !whole.calls.iter().any(|(a, _)| a.len() == 3 && a[..2] == ["mount", "--make-rslave"] && a[2] != live.target()),
        "no separate call"
    );
    // each call of the install fails in turn, with the cleanup's own `mount --make-rslave TARGET` working or failing too
    for k in 0..whole.calls.len() {
        for slave_fails in [false, true] {
            let live = Live::new();
            let mut f = live.fake();
            f.fail_at = Some(k);
            if slave_fails {
                f.fail.push(vec!["mount".into(), "--make-rslave".into()]);
            }
            let code = live.run(&mut f, &[], &encrypted);
            live_mounts_safe(&f, &live.target())
                .unwrap_or_else(|e| panic!("call {k} failing ({:?}), make-rslave failing {slave_fails}: {e}", whole.calls[k].0));
            assert!(code == 1 || (code == 0 && f.out.last().unwrap() == "DONE"), "call {k}: {code} {:?}", f.out.last());
        }
    }
}

#[test]
fn left_mounted_rather_than_unmounted_unsafely() {
    let live = Live::new();
    let mut f = live.fake();
    f.fail.push(vec!["mount".into(), "--make-rslave".into()]);
    assert_eq!(live.run(&mut f, &[], &plan().dumps()), 0);
    let n = f.out.len();
    assert_eq!(
        f.out[n - 2..],
        [format!("WARN the new system stays mounted at {} until the computer restarts", live.target()), "DONE".to_string()]
    );
    assert!(
        !f.calls.iter().any(|(a, _)| a[..2] == ["umount", "-R"] || a[..2] == ["cryptsetup", "close"]),
        "nothing unmounted, nothing closed"
    );
    // a failure before the live /dev is bound: nothing to make a slave
    let mut f = live.fake();
    f.fail.push(vec!["mkfs.btrfs".into()]);
    assert_eq!(live.run(&mut f, &[], &plan().dumps()), 1);
    assert!(!f.calls.iter().any(|(a, _)| a[..2] == ["mount", "--make-rslave"]));
}
