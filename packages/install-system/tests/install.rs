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

/// No internet at all (every download fails): the install still finishes from the medium alone. A browser that is not on it is
/// left for Agora with a warning; device software that is not on it waits in the first-login list for the new user, who gets it
/// once online (Labyrinth's labyrinth-first-components, Yggdrasil's yggdrasil-device-offers).
#[test]
fn no_internet_never_stops_the_install() {
    let live = Live::new();
    let conf = live.dir.path("share/integrations/example-installer.conf");
    let text = std::fs::read_to_string(&conf).unwrap();
    std::fs::write(&conf, format!("{text}first_login = /var/lib/example/first-login.json\n")).unwrap();
    live.dir.write(
        "os/extras.json",
        r#"{"extras": [{"id": "monitor", "components": ["monitor"]}, {"id": "razer", "group": "Devices", "components": ["razer"], "chosen_if_usb": ["1532"]}]}"#,
    );
    live.dir.write(
        "os/components.json",
        r#"{"components": [{"id": "monitor", "packages": ["monitor-git"]}, {"id": "razer", "packages": ["openrazer-daemon"]}]}"#,
    );
    let mut f = live.fake();
    f.installed = vec!["vivaldi".into()];
    for p in [&["pacman", "-S"][..], &["pacman", "-Syu"], &["flatpak"]] {
        f.fail.push(p.iter().map(|s| s.to_string()).collect());
    }
    let p = plan_with(&[("browsers", r#"["firefox"]"#), ("default_browser", "\"firefox\""), ("extras", r#"["razer"]"#)]);
    assert_eq!(live.run(&mut f, &[], &p), 0, "{:?}", f.out);
    assert_eq!(f.out.last().unwrap(), "DONE");
    assert!(f.out.contains(&"WARN could not install firefox (no internet?); Agora can install them later".to_string()), "{:?}", f.out);
    assert!(
        f.out.contains(&"INFO not on the installer, installed at the first login (needs the internet): razer".to_string()),
        "{:?}",
        f.out
    );
    assert_eq!(live.read("var/lib/example/first-login.json"), r#"{"user": "ada", "components": ["razer"]}"#);
    assert!(f.failed.iter().filter(|x| **x).count() >= 2, "the downloads were tried and failed");
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

const WIFI_PW: &str = "pw-WIFI-3";

/// The live session's saved networks in Live's scratch folder: two plain profiles (one only the live account may use), and a
/// link, a folder, a pipe and a file of another kind that must all stay behind.
fn live_networks(live: &Live) -> String {
    use std::os::unix::fs::PermissionsExt;
    let home = format!(
        "[connection]\nid=Home Net\ntype=wifi\npermissions=user:live:;\n\n[wifi]\nssid=Home Net\n\n[wifi-security]\nkey-mgmt=wpa-psk\npsk={WIFI_PW}\n"
    );
    live.dir.write("nm/system-connections/Home Net.nmconnection", &home);
    live.dir.write("nm/system-connections/Cafe.nmconnection", "[connection]\nid=Cafe\ntype=wifi\n\n[wifi]\nssid=Cafe\n");
    live.dir.write("nm/system-connections/notes.txt", "not a profile\n");
    live.dir.write("outside/Elsewhere.nmconnection", "[connection]\nid=Elsewhere\n");
    std::os::unix::fs::symlink(live.dir.path("outside/Elsewhere.nmconnection"), live.dir.path("nm/system-connections/Link.nmconnection"))
        .unwrap();
    std::fs::create_dir(live.dir.path("nm/system-connections/Folder.nmconnection")).unwrap();
    let fifo = std::ffi::CString::new(live.dir.path("nm/system-connections/Pipe.nmconnection")).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    std::fs::set_permissions(live.dir.path("nm/system-connections"), std::fs::Permissions::from_mode(0o700)).unwrap();
    home
}

#[test]
fn the_live_sessions_networks_are_carried_over() {
    use std::os::unix::fs::PermissionsExt;
    let live = Live::new();
    let home = live_networks(&live);
    let mut f = live.fake();
    assert_eq!(live.run(&mut f, &[], &plan().dumps()), 0, "{:?}", f.out);
    assert!(f.out.contains(&"INFO the live session's saved networks carried over: 2".to_string()), "{:?}", f.out);
    assert!(f.out.contains(&"WARN left behind 3 of the live session's saved networks that are not plain files".to_string()));
    for line in &f.out {
        assert!(!line.contains("Home Net") && !line.contains("Cafe") && !line.contains(WIFI_PW), "a network named: {line}");
    }
    let dir = format!("{}/etc/NetworkManager/system-connections", live.target());
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o7777, 0o700);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["Cafe.nmconnection", "Home Net.nmconnection"], "plain .nmconnection files only");
    for n in &names {
        let m = std::fs::symlink_metadata(format!("{dir}/{n}")).unwrap();
        assert!(m.file_type().is_file());
        assert_eq!(m.permissions().mode() & 0o7777, 0o600, "{n}");
    }
    assert_eq!(live.read("etc/NetworkManager/system-connections/Home Net.nmconnection"), home.replace("user:live:", "user:ada:"));
    assert_eq!(
        live.read("etc/NetworkManager/system-connections/Cafe.nmconnection"),
        "[connection]\nid=Cafe\ntype=wifi\n\n[wifi]\nssid=Cafe\n"
    );
    // the password went from file to file: never into a program's arguments or standard input
    for (argv, stdin) in &f.calls {
        assert!(!argv.iter().any(|a| a.contains(WIFI_PW)) && !stdin.as_deref().unwrap_or("").contains(WIFI_PW), "{argv:?}");
    }
}

#[test]
fn no_saved_networks_nothing_to_do() {
    // no folder at all (a live system without NetworkManager), an empty one, and a link in its place: nothing said, nothing made
    for how in ["missing", "empty", "link"] {
        let live = Live::new();
        match how {
            "empty" => std::fs::create_dir_all(live.dir.path("nm/system-connections")).unwrap(),
            "link" => {
                live.dir.write("elsewhere/Home.nmconnection", "[connection]\nid=Home\n");
                std::fs::create_dir_all(live.dir.path("nm")).unwrap();
                std::os::unix::fs::symlink(live.dir.path("elsewhere"), live.dir.path("nm/system-connections")).unwrap();
            }
            _ => {}
        }
        let mut f = live.fake();
        assert_eq!(live.run(&mut f, &[], &plan().dumps()), 0, "{how}: {:?}", f.out);
        assert!(!f.out.iter().any(|l| l.contains("saved network")), "{how}: {:?}", f.out);
        assert!(!std::path::Path::new(&format!("{}/etc/NetworkManager", live.target())).exists(), "{how}");
    }
}

#[test]
fn a_link_in_the_new_system_is_not_written_through() {
    let live = Live::new();
    live_networks(&live);
    let mut f = live.fake();
    // the medium (made by the fake unsquashfs) with its saved-networks folder a link out of the new system
    live.dir.write("victim/keep", "");
    std::fs::create_dir_all(format!("{}/etc/NetworkManager", live.target())).unwrap();
    std::os::unix::fs::symlink(live.dir.path("victim"), format!("{}/etc/NetworkManager/system-connections", live.target())).unwrap();
    assert_eq!(live.run(&mut f, &[], &plan().dumps()), 0, "{:?}", f.out);
    assert!(f.out.contains(&"WARN the new system's folder for saved networks is a link: none were carried over".to_string()));
    assert_eq!(std::fs::read_dir(live.dir.path("victim")).unwrap().count(), 1, "nothing written through the link");
}

#[test]
fn only_the_live_accounts_permission_changes_hands() {
    use install_system::install::network_for;
    let f = |t: &str| String::from_utf8(network_for(t.as_bytes(), "ada")).unwrap();
    assert_eq!(f("[connection]\npermissions=user:live:;\n"), "[connection]\npermissions=user:ada:;\n");
    assert_eq!(f("[connection]\npermissions=user:live;\n"), "[connection]\npermissions=user:ada;\n");
    assert_eq!(f("[connection]\npermissions=user:bob:;user:live:;\n"), "[connection]\npermissions=user:bob:;user:ada:;\n");
    assert_eq!(f("[connection]\r\npermissions = user:live\r\n"), "[connection]\r\npermissions = user:ada\r\n");
    // another account, a name that only starts with "live", another part of the file, other keys: as they were
    for same in [
        "[connection]\npermissions=user:bob:;\n",
        "[connection]\npermissions=user:livewire:;\n",
        "[connection]\npermissions=group:live:;\n",
        "[wifi]\npermissions=user:live:;\n",
        "[connection]\nid=user:live\nuuid=x\n",
        "permissions=user:live:;\n",
        "",
        "[connection]\n\n[vpn]\npermissions=user:live:;",
    ] {
        assert_eq!(f(same), same);
    }
    // bytes that are not UTF-8 (an SSID can be anything) stay as they were
    let odd = b"[connection]\nid=\xff\xfe\npermissions=user:live:;\n";
    assert_eq!(network_for(odd, "ada"), b"[connection]\nid=\xff\xfe\npermissions=user:ada:;\n");
}
