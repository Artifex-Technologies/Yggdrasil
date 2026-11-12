//! A made-up computer for the tests, in the test process itself: every program the install runs is answered here and written
//! down (nothing is started), and its disks are /dev/vdz and the like. The new system goes into a scratch folder, and the live
//! system's places (integration files, zoneinfo, /sys/firmware/efi, resolv.conf) are scratch folders too.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use install_system::install::Host;
use install_system::py::R;
use install_system::pyjson::{dict, int, s, Py};
use install_system::sys::{Output, System};
use install_system::Env;

pub const GB: i128 = 1024 * 1024 * 1024;
pub const MIB: i128 = 1024 * 1024;
pub const ACCOUNT_PW: &str = "pw-ACCOUNT-1";
pub const DISK_PW: &str = "pw-DISK-secret-2";

/// A scratch folder, removed afterwards.
pub struct Dir(pub PathBuf);

impl Dir {
    pub fn new() -> Dir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("install-system-it-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }

    pub fn path(&self, rest: &str) -> String {
        format!("{}/{}", self.0.display(), rest.trim_start_matches('/'))
    }

    pub fn write(&self, rest: &str, text: &str) {
        let p = self.path(rest);
        std::fs::create_dir_all(Path::new(&p).parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Debug)]
pub struct Partition {
    pub n: u32,
    pub start: i128,
    pub sectors: i128,
    pub fstype: String,
    pub efi: bool,
    pub mounted: bool,
}

#[derive(Clone, Debug)]
pub struct Disk {
    pub path: String,
    pub size: i128,
    pub table: String,
    pub parts: Vec<Partition>,
}

pub fn part_name(disk: &str, n: u32) -> String {
    if disk.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{disk}p{n}")
    } else {
        format!("{disk}{n}")
    }
}

impl Disk {
    pub fn empty(path: &str, size: i128) -> Disk {
        Disk { path: path.into(), size, table: String::new(), parts: vec![] }
    }

    /// 500 GB with Windows: its EFI partition (1), its reserved partition (2), Windows (3), and 200 GB free at the end.
    pub fn windows(path: &str) -> Disk {
        let p = |n, start_mib: i128, size_mib: i128, fs: &str, efi| Partition {
            n,
            start: start_mib * 2048,
            sectors: size_mib * 2048,
            fstype: fs.into(),
            efi,
            mounted: false,
        };
        Disk {
            path: path.into(),
            size: 500 * GB,
            table: "gpt".into(),
            parts: vec![p(1, 1, 300, "vfat", true), p(2, 301, 16, "", false), p(3, 317, 299 * 1024, "ntfs", false)],
        }
    }
}

/// The made-up computer.
pub struct Fake {
    pub calls: Vec<(Vec<String>, Option<String>)>,
    pub out: Vec<String>,
    pub err: Vec<String>,
    pub euid: u32,
    pub accounts: Vec<String>,
    pub disks: Vec<Disk>,
    pub live: Option<(String, String)>,
    pub fs: HashMap<String, String>,
    pub installed: Vec<String>,
    pub groups: Vec<String>,
    /// argv prefixes that fail (inside the new system: the program's own argv)
    pub fail: Vec<Vec<String>>,
    pub target: String,
    pub medium: Vec<(String, String)>,
    pub random_calls: usize,
    /// the call (counted from 0) that fails, whatever it is
    pub fail_at: Option<usize>,
    /// for each call, whether it failed
    pub failed: Vec<bool>,
}

pub fn medium() -> Vec<(String, String)> {
    [
        ("etc/locale.gen", "#en_US.UTF-8 UTF-8\n#de_DE.UTF-8 UTF-8\n"),
        (
            "etc/default/grub",
            "GRUB_DEFAULT=saved\nGRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX_DEFAULT=\"loglevel=3 quiet\"\n#GRUB_ENABLE_CRYPTODISK=y\n",
        ),
        ("etc/mkinitcpio.conf", "HOOKS=(base systemd autodetect modconf kms keyboard sd-vconsole block filesystems fsck)\n"),
        ("etc/xdg/mimeapps.list", "[Default Applications]\ntext/html=vivaldi-stable.desktop\n"),
        ("boot/initramfs-linux.img", "image\n"),
        ("usr/lib/systemd/system/greetd.service", "[Unit]\n"),
        ("usr/share/systemd/kbd-model-map", "uk\tgb\tpc105\t-\tx\nde-latin1\tde\tpc105\t-\tx\n"),
        ("usr/local/bin/example-install-cleanup", "#!/bin/sh\n"),
        ("usr/local/bin/example-install-finish", "#!/bin/sh\n"),
        ("usr/share/applications/firefox.desktop", "[Desktop Entry]\n"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}

fn out(code: i32, stdout: &str, stderr: &str) -> Output {
    Output { code, stdout: stdout.into(), stderr: stderr.into() }
}

impl Fake {
    pub fn new(target: &str) -> Fake {
        Fake {
            calls: vec![],
            out: vec![],
            err: vec![],
            euid: 0,
            accounts: ["root", "nobody", "taken"].iter().map(|s| s.to_string()).collect(),
            disks: vec![Disk::empty("/dev/vdz", 500 * GB), Disk::empty("/dev/sdy", 16 * GB)],
            live: Some(("/dev/sdy1".into(), "sdy".into())),
            fs: HashMap::new(),
            installed: ["vivaldi", "firefox"].iter().map(|s| s.to_string()).collect(),
            groups: ["users", "wheel", "audio"].iter().map(|s| s.to_string()).collect(),
            fail: vec![],
            target: target.into(),
            medium: medium(),
            random_calls: 0,
            fail_at: None,
            failed: vec![],
        }
    }

    fn disk(&mut self, path: &str) -> Option<&mut Disk> {
        self.disks.iter_mut().find(|d| d.path == path)
    }

    fn lsblk(&mut self, a: &[String]) -> Output {
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        match a.as_slice() {
            ["-J", "-b", "-d", "-o", _] => {
                let list = self
                    .disks
                    .iter()
                    .map(|d| {
                        dict(vec![
                            ("name", s(&d.path[5..])),
                            ("path", s(&d.path)),
                            ("size", int(d.size)),
                            ("model", s("TEST DISK 0001")),
                            ("tran", Py::None),
                            ("ro", Py::Bool(false)),
                            ("type", s("disk")),
                            ("rm", Py::Bool(false)),
                        ])
                    })
                    .collect();
                out(0, &dict(vec![("blockdevices", Py::List(list))]).dumps(), "")
            }
            ["-J", "-b", "-o", _, disk] => {
                let Some(d) = self.disks.iter().find(|d| d.path == *disk) else {
                    return out(32, "", "not a block device\n");
                };
                let children = d
                    .parts
                    .iter()
                    .map(|q| {
                        let path = part_name(&d.path, q.n);
                        let fstype = self.fs.get(&path).cloned().unwrap_or_else(|| q.fstype.clone());
                        dict(vec![
                            ("path", s(&path)),
                            ("start", int(q.start)),
                            ("size", int(q.sectors * 512)),
                            ("fstype", if fstype.is_empty() { Py::None } else { s(&fstype) }),
                            ("label", Py::None),
                            ("partlabel", Py::None),
                            (
                                "parttype",
                                s(if q.efi { "c12a7328-f81f-11d2-ba4b-00a0c93ec93b" } else { "0fc63daf-8483-4772-8e79-3d69d8477de4" }),
                            ),
                            ("partn", int(q.n as i128)),
                            ("pttype", s(&d.table)),
                            ("mountpoints", Py::List(vec![if q.mounted { s("/x") } else { Py::None }])),
                            ("type", s("part")),
                        ])
                    })
                    .collect();
                let top = dict(vec![
                    ("path", s(&d.path)),
                    ("start", Py::None),
                    ("size", int(d.size)),
                    ("pttype", if d.table.is_empty() { Py::None } else { s(&d.table) }),
                    ("type", s("disk")),
                    ("children", Py::List(children)),
                ]);
                out(0, &dict(vec![("blockdevices", Py::List(vec![top]))]).dumps(), "")
            }
            ["-nr", "-o", "MOUNTPOINTS", disk] => {
                let Some(d) = self.disks.iter().find(|d| d.path == *disk) else {
                    return out(32, "", "not a block device\n");
                };
                out(0, &(String::from("\n") + &d.parts.iter().map(|q| if q.mounted { "/x\n" } else { "\n" }).collect::<String>()), "")
            }
            ["-no", "PKNAME", src] => out(0, &self.live.clone().filter(|(s, _)| s == src).map(|(_, p)| p + "\n").unwrap_or_default(), ""),
            _ => out(1, "", "lsblk: unexpected\n"),
        }
    }

    fn sfdisk(&mut self, a: &[String], stdin: Option<&str>) -> Output {
        let disk = a.iter().find(|x| x.starts_with("/dev/")).cloned().unwrap_or_default();
        let Some(d) = self.disk(&disk) else {
            return out(1, "", "no such disk\n");
        };
        let first = a.first().map(String::as_str);
        match first {
            Some("-J") => {
                let parts = d
                    .parts
                    .iter()
                    .map(|q| dict(vec![("node", s(&part_name(&d.path, q.n))), ("start", int(q.start)), ("size", int(q.sectors))]))
                    .collect();
                out(
                    0,
                    &dict(vec![(
                        "partitiontable",
                        dict(vec![("label", s("gpt")), ("sectorsize", int(512)), ("partitions", Py::List(parts))]),
                    )])
                    .dumps(),
                    "",
                )
            }
            Some("--delete") => {
                let n: u32 = a[2].parse().unwrap_or(0);
                if !d.parts.iter().any(|q| q.n == n) {
                    return out(1, "", "no such partition\n");
                }
                d.parts.retain(|q| q.n != n);
                out(0, "", "")
            }
            Some("--part-type") => out(0, "", ""),
            Some("--append") => {
                let spec: HashMap<String, String> = stdin
                    .unwrap_or("")
                    .trim()
                    .split(',')
                    .filter_map(|kv| kv.trim().split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                let mut n = 1;
                while d.parts.iter().any(|q| q.n == n) {
                    n += 1;
                }
                d.parts.push(Partition {
                    n,
                    start: spec["start"].parse().unwrap(),
                    sectors: spec["size"].parse().unwrap(),
                    fstype: String::new(),
                    efi: spec["type"] == "uefi",
                    mounted: false,
                });
                out(0, "", "")
            }
            Some("--wipe") => {
                d.table = "gpt".into();
                d.parts.clear();
                if a.iter().any(|x| x == "--wipe-partitions") {
                    let esp = GB / 512;
                    d.parts.push(Partition { n: 1, start: 2048, sectors: esp, fstype: String::new(), efi: true, mounted: false });
                    d.parts.push(Partition {
                        n: 2,
                        start: 2048 + esp,
                        sectors: d.size / 512 - 2048 - esp - 34,
                        fstype: String::new(),
                        efi: false,
                        mounted: false,
                    });
                }
                out(0, "", "")
            }
            _ => out(1, "", "sfdisk: unexpected\n"),
        }
    }

    fn make_medium(&self) {
        for (path, text) in &self.medium {
            let p = format!("{}/{path}", self.target);
            std::fs::create_dir_all(Path::new(&p).parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let _ = std::os::unix::fs::symlink("../run/systemd/resolve/stub-resolv.conf", format!("{}/etc/resolv.conf", self.target));
    }

    fn inside(&mut self, a: &[String]) -> Output {
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        match a.as_slice() {
            ["pacman", "-Q", x] => {
                if self.installed.iter().any(|i| i == x) {
                    out(0, "", "")
                } else {
                    out(1, "", "not found\n")
                }
            }
            ["pacman", "-Rns", "--noconfirm", rest @ ..] => {
                self.installed.retain(|i| !rest.contains(&i.as_str()));
                out(0, "", "")
            }
            ["getent", "group", g] => {
                if self.groups.iter().any(|x| x == g) {
                    out(0, "", "")
                } else {
                    out(2, "", "")
                }
            }
            _ => out(0, "", ""),
        }
    }

    /// The calls of one program (by name, or the program run inside the new system).
    pub fn program_calls(&self, name: &str) -> Vec<&(Vec<String>, Option<String>)> {
        self.calls.iter().filter(|(a, _)| a[0] == name || (a[0] == "chroot" && a.get(4).map(String::as_str) == Some(name))).collect()
    }
}

impl System for Fake {
    fn run(&mut self, argv: &[String], stdin: Option<&str>) -> R<Output> {
        self.calls.push((argv.to_vec(), stdin.map(str::to_string)));
        let failing = self.fail_at == Some(self.calls.len() - 1);
        self.failed.push(failing);
        let own = if argv[0] == "chroot" { &argv[4..] } else { argv };
        if failing || self.fail.iter().any(|f| own.starts_with(f)) {
            *self.failed.last_mut().unwrap() = true;
            return Ok(out(1, "", "failed (made up)\n"));
        }
        let rest = &argv[1..];
        Ok(match argv[0].as_str() {
            "lsblk" => self.lsblk(rest),
            "findmnt" => match &self.live {
                Some((src, _)) => out(0, &format!("{src}\n"), ""),
                None => out(1, "", ""),
            },
            "sfdisk" => self.sfdisk(rest, stdin),
            "wipefs" => {
                if let Some(d) = self.disk(&argv[2]) {
                    d.table.clear();
                    d.parts.clear();
                }
                out(0, "", "")
            }
            "blkid" => {
                let dev = argv.last().unwrap().clone();
                if argv[2] == "TYPE" {
                    out(0, &format!("{}\n", self.fs.get(&dev).cloned().unwrap_or_else(|| "ext4".into())), "")
                } else {
                    out(0, &format!("UUID-OF-{}\n", dev.replace('/', "_")), "")
                }
            }
            p if p.starts_with("mkfs.") || p == "mkswap" => {
                let kind = if p == "mkswap" { "swap" } else { &p[5..] };
                self.fs.insert(argv.last().unwrap().clone(), kind.to_string());
                out(0, "", "")
            }
            "chroot" => self.inside(&argv[4..]),
            _ => out(0, "", ""),
        })
    }

    fn stream(&mut self, argv: &[String], line: &mut dyn FnMut(&str) -> R<Option<String>>) -> R<(i32, String)> {
        self.calls.push((argv.to_vec(), None));
        let failing = self.fail_at == Some(self.calls.len() - 1);
        self.failed.push(failing);
        if failing {
            return Ok((1, "unsquashfs: failed (made up)\n".into()));
        }
        self.make_medium();
        for p in ["0", "10", "55", "100"] {
            if let Some(said) = line(&format!("{p}\n"))? {
                self.out.push(said);
            }
        }
        Ok((0, String::new()))
    }

    fn sleep(&mut self, _seconds: u64) {}

    fn random(&mut self, n: usize) -> Vec<u8> {
        self.random_calls += 1;
        vec![0x5a; n]
    }

    fn account_exists(&mut self, login: &str) -> bool {
        self.accounts.iter().any(|a| a == login)
    }

    fn euid(&mut self) -> u32 {
        self.euid
    }

    fn out(&mut self, line: &str) -> R<()> {
        self.out.push(line.to_string());
        Ok(())
    }

    fn err(&mut self, line: &str) {
        self.err.push(line.to_string());
    }
}

/// The live system's places, in a scratch folder: an integration file naming the OS "Example", a zoneinfo with UTC and
/// Europe/Lisbon, UEFI with efivars, and the system to copy.
pub struct Live {
    pub dir: Dir,
    pub host: Host,
    pub env: Env,
}

impl Live {
    pub fn new() -> Live {
        let dir = Dir::new();
        dir.write(
            "share/integrations/example-installer.conf",
            "kind = installer\nname = Example\ncleanup = /usr/local/bin/example-install-cleanup\nfinish = /usr/local/bin/example-install-finish\nbrowsers = BROWSERS\nextras = EXTRAS\ncomponents = COMPONENTS\n",
        );
        let conf = std::fs::read_to_string(dir.path("share/integrations/example-installer.conf")).unwrap();
        let conf = conf
            .replace("BROWSERS", &dir.path("os/browsers.json"))
            .replace("EXTRAS", &dir.path("os/extras.json"))
            .replace("COMPONENTS", &dir.path("os/components.json"));
        dir.write("share/integrations/example-installer.conf", &conf);
        dir.write("os/browsers.json", r#"{"browsers": [{"id": "vivaldi", "packages": ["vivaldi"], "desktop": ["vivaldi-stable.desktop"]}, {"id": "firefox", "packages": ["firefox"], "desktop": ["firefox.desktop"]}]}"#);
        dir.write("os/extras.json", r#"{"extras": [{"id": "monitor", "components": ["monitor"]}]}"#);
        dir.write("os/components.json", r#"{"components": [{"id": "monitor", "packages": ["monitor-git"]}]}"#);
        dir.write("os-release", "NAME=\"Example OS\"\nID=example\n");
        dir.write("zoneinfo/UTC", "TZif");
        dir.write("zoneinfo/Europe/Lisbon", "TZif");
        dir.write("firmware/efi/efivars/x", "");
        dir.write("resolv.conf", "# made up\n");
        dir.write("medium/airootfs.sfs", "squashfs\n");
        std::fs::create_dir_all(dir.path("target")).unwrap();
        let host = Host {
            integration_dirs: vec![dir.path("share/integrations"), dir.path("etc/integrations")],
            os_release: dir.path("os-release"),
            zoneinfo: dir.path("zoneinfo"),
            efi: dir.path("firmware/efi"),
            resolv_conf: dir.path("resolv.conf"),
        };
        let env = Env { sfs: Some(dir.path("medium/airootfs.sfs")), target: Some(dir.path("target")) };
        Live { dir, host, env }
    }

    pub fn target(&self) -> String {
        self.dir.path("target")
    }

    pub fn fake(&self) -> Fake {
        Fake::new(&self.target())
    }

    /// Runs the program: (exit code, the made-up computer afterwards).
    pub fn run(&self, fake: &mut Fake, args: &[&str], plan: &str) -> i32 {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        install_system::main_with(&args, &mut plan.as_bytes(), &self.env, self.host.clone(), fake)
    }

    pub fn read(&self, rest: &str) -> String {
        std::fs::read_to_string(format!("{}/{rest}", self.target())).unwrap_or_else(|e| panic!("{rest}: {e}"))
    }
}

/// A plan the window could send: the whole of /dev/vdz.
pub fn plan() -> Py {
    install_system::pyjson::loads(
        &format!(
            r#"{{"disk": "/dev/vdz", "mode": "erase", "encrypt": false, "encrypt_password": "",
               "user": {{"name": "Ada Example", "login": "ada", "password": "{ACCOUNT_PW}"}},
               "hostname": "test-pc", "timezone": "Europe/Lisbon", "locale": "en_US.UTF-8", "keymap": "us", "keymap_variant": ""}}"#
        ),
        20,
    )
    .unwrap()
}

/// The plan with some values changed (JSON text for each).
pub fn plan_with(changes: &[(&str, &str)]) -> String {
    let mut p = plan();
    if let Py::Dict(d) = &mut p {
        for (k, v) in changes {
            d.insert(k.to_string(), install_system::pyjson::loads(v, 20).unwrap());
        }
    }
    p.dumps()
}

/// A manual plan beside Windows (Disk::windows): its EFI partition kept, a new btrfs / in the free space.
pub fn manual_plan(extra: &[(&str, &str)]) -> String {
    let mut changes = vec![
        ("mode", r#""manual""#),
        ("new_table", "false"),
        ("delete", "[]"),
        ("create", r#"[{"id": "r", "start": 322122547200, "size": 161061273600, "kind": "linux"}]"#),
        ("use", r#"[{"part": "/dev/vdz1", "mount": "/boot/efi", "format": null}, {"part": "new:r", "mount": "/", "format": "btrfs"}]"#),
    ];
    changes.extend_from_slice(extra);
    plan_with(&changes)
}
