//! A report from a made-up computer: everything personal on it (its name, the account, the home network, a paired device,
//! addresses and serial numbers) is out of the zip, and what a fix needs (make, model, parts, drivers, screens) is in it.

use super::*;
use std::collections::HashMap;
use std::io::Read;

struct Tree(PathBuf);

impl Tree {
    fn new(tag: &str) -> Tree {
        let d = std::env::temp_dir().join(format!("ygg-report-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tree(d)
    }

    fn put(&self, path: &str, data: impl AsRef<[u8]>) {
        let p = self.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A monitor's EDID: maker ACR, name XV345CUR, and a serial number that must never be read.
fn edid() -> Vec<u8> {
    let mut e = vec![0u8; 128];
    e[8..10].copy_from_slice(&[0x04, 0x72]);
    let mut serial = [0u8; 18];
    serial[3] = 0xff;
    serial[5..18].copy_from_slice(b"SN123456789\n ");
    e[54..72].copy_from_slice(&serial);
    let mut name = [0u8; 18];
    name[3] = 0xfc;
    name[5..18].copy_from_slice(b"XV345CUR\n    ");
    e[72..90].copy_from_slice(&name);
    e
}

fn computer(tag: &str) -> Tree {
    let t = Tree::new(tag);
    t.put("etc/hostname", "example-pc\n");
    t.put("proc/sys/kernel/hostname", "example-pc\n");
    t.put("etc/passwd", "root:x:0:0::/root:/bin/bash\nsam:x:1000:1000:Sam Sample,,,:/home/sam:/bin/bash\n");
    t.put("etc/os-release", "NAME=\"Example OS\"\nPRETTY_NAME=\"Example OS\"\nID=exampleos\nVERSION_ID=1.0.0\nBUILD_ID=7\n");
    t.put("proc/sys/kernel/osrelease", "7.3.5-2-cachyos\n");
    t.put("proc/uptime", "3600.5 100.0\n");
    t.put("proc/cpuinfo", "processor\t: 0\nmodel name\t: Example CPU 8000\n\nprocessor\t: 1\nmodel name\t: Example CPU 8000\n");
    t.put("proc/meminfo", "MemTotal:       32768000 kB\nSwapTotal:       8388608 kB\n");
    for (f, v) in [
        ("sys_vendor", "Micro-Star International Co., Ltd."),
        ("product_name", "MS-7D25"),
        ("product_serial", "SERIAL-SHOULD-NOT-APPEAR"),
        ("board_name", "MAG B660M"),
        ("chassis_type", "3"),
    ] {
        t.put(&format!("sys/class/dmi/id/{f}"), format!("{v}\n"));
    }
    let gpu = "sys/bus/pci/devices/0000:01:00.0";
    t.put(&format!("{gpu}/class"), "0x030000\n");
    t.put(&format!("{gpu}/vendor"), "0x10de\n");
    t.put(&format!("{gpu}/device"), "0x1e07\n");
    t.put(&format!("{gpu}/boot_vga"), "1\n");
    t.put(&format!("{gpu}/current_link_speed"), "8.0 GT/s PCIe\n");
    t.put(&format!("{gpu}/current_link_width"), "16\n");
    t.put(&format!("{gpu}/max_link_speed"), "8.0 GT/s PCIe\n");
    t.put(&format!("{gpu}/max_link_width"), "16\n");
    t.put(
        &format!("{gpu}/resource"),
        "0x00000000f6000000 0x00000000f6ffffff 0x0000000000040200\n0x0000000000000000 0x0000000000000000 0x0000000000000000\n0x00000000e0000000 0x00000000efffffff 0x000000000014220c\n0x00000000f0000000 0x00000000f1ffffff 0x000000000014220c\n",
    );
    std::fs::create_dir_all(t.0.join("sys/bus/pci/drivers/nvidia")).unwrap();
    std::os::unix::fs::symlink(t.0.join("sys/bus/pci/drivers/nvidia"), t.0.join(gpu).join("driver")).unwrap();
    t.put("usr/share/hwdata/pci.ids", "# pci.ids\n10de  NVIDIA Corporation\n\t1e04  TU102 [GeForce RTX 2080 Ti]\n\t1e07  TU102 [GeForce RTX 2080 Ti Rev. A]\n10df  Emulex Corporation\n");
    t.put("proc/driver/nvidia/version", "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  620.73.04\n");
    t.put("sys/class/drm/card1-DP-1/status", "connected\n");
    t.put("sys/class/drm/card1-DP-1/modes", "3440x1440\n2560x1440\n");
    t.put("sys/class/drm/card1-DP-1/edid", edid());
    t.put("sys/class/drm/card1-HDMI-A-1/status", "disconnected\n");
    t.put("home/sam/.local/state/yggdrasil/compositor.log", "joined Cosy Home 5G as sam from 192.168.1.23\nscreen DP-1 3440x1440@175\n");
    t.put("home/sam/Pictures/shot.png", b"\x89PNG not really");
    t
}

fn commands() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("nmcli -t -f NAME,TYPE connection show", "Cosy Home 5G:802-11-wireless\nlo:loopback\n"),
        ("bluetoothctl devices", "Device AA:BB:CC:DD:EE:FF Sam's Headphones\n"),
        (
            "journalctl -b 0 -p warning -o short-iso --utc --no-pager -n 3000",
            "2026-11-26T17:00:00+00:00 example-pc kernel: usb 1-2: SerialNumber: ABC123\n2026-11-26T17:00:01+00:00 example-pc NetworkManager: wlan0 192.168.1.23 aa:bb:cc:dd:ee:ff joined Cosy Home 5G\n2026-11-26T17:00:02+00:00 example-pc pulseaudio: Sam's Headphones lost; mail sam.sample@example.com\n2026-11-26T17:00:03+00:00 example-pc evince[812]: cannot open '/home/sam/Documents/Taxes 2026.pdf'\n2026-11-26T17:00:04+00:00 example-pc rclone[90]: GET https://drive-api.proton.me/drive/shares/AbC123/links?Page=0: 429\n2026-11-26T17:00:05+00:00 example-pc rclone[90]: GET https://drive-api.proton.me/drive/shares/AbC123/links?Page=1: 429\n2026-11-26T17:00:06+00:00 example-pc rclone[90]: GET https://drive-api.proton.me/drive/shares/AbC123/links?Page=2: 429\n2026-11-26T17:00:07+00:00 example-pc rclone[90]: GET https://drive-api.proton.me/drive/shares/AbC123/links?Page=3: 429\n2026-11-26T17:00:08+00:00 example-pc rclone[90]: GET https://drive-api.proton.me/drive/shares/AbC123/links?Page=4: 429\n",
        ),
        ("/usr/bin/selfcheck", "OK  the session for sam on example-pc\n"),
    ])
}

fn report(t: &Tree, req: &Request, support: Option<&Support>) -> Vec<File> {
    let cmds = commands();
    let run = |argv: &[&str]| cmds.get(argv.join(" ").as_str()).map(|s| s.to_string());
    let env = |k: &str| match k {
        "USER" => Some("sam".to_string()),
        "XDG_CURRENT_DESKTOP" => Some("Yggdrasil".to_string()),
        "XDG_SESSION_TYPE" => Some("wayland".to_string()),
        _ => None,
    };
    let m = Machine { root: t.0.clone(), run: &run, env: &env, home: PathBuf::from("/home/sam"), now: "2026-11-26 22:30 UTC".into() };
    gather(req, &m, support)
}

fn support() -> Support {
    Support {
        name: "Example OS".into(),
        email: Some("support@example.com".into()),
        github: None,
        includes: vec![("Self-check".into(), vec!["/usr/bin/selfcheck".into()])],
    }
}

fn text<'a>(files: &'a [File], name: &str) -> &'a str {
    std::str::from_utf8(&files.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no {name}")).data).unwrap()
}

#[test]
fn nothing_personal_leaves_the_computer() {
    let t = computer("all");
    let req = Request {
        area: "Sound".into(),
        text: "No sound after waking. My name is Sam Sample, at example-pc.".into(),
        pictures: vec![t.0.join("home/sam/Pictures/shot.png")],
    };
    let files = report(&t, &req, Some(&support()));
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "report.txt",
            "device.txt",
            "logs/this-start.txt",
            "logs/last-start.txt",
            "logs/crashes.txt",
            "logs/compositor.log.txt",
            "system/self-check.txt",
            "pictures/picture-1.png"
        ]
    );
    for f in files.iter().filter(|f| !f.name.starts_with("pictures/")) {
        let s = String::from_utf8_lossy(&f.data);
        for secret in [
            "example-pc",
            "Sam Sample",
            "/home/sam",
            " sam ",
            "Cosy Home",
            "192.168.1.23",
            "aa:bb:cc:dd:ee:ff",
            "AA:BB:CC",
            "ABC123",
            "SN123456789",
            "SERIAL-SHOULD-NOT-APPEAR",
            "Headphones",
            "sam.sample@",
            "Taxes",
            "AbC123",
            "-0700",
        ] {
            assert!(!s.contains(secret), "{} holds {secret:?}:\n{s}", f.name);
        }
    }
    assert_eq!(files.last().unwrap().data, b"\x89PNG not really", "a picture goes as it is");
    let r = text(&files, "report.txt");
    for want in [
        "Problem report for Example OS\n",
        "Made: 2026-11-26 22:30 UTC\n",
        "About: Sound\n",
        "What happened:\nNo sound after waking. My name is <name>, at <computer>.\n",
        "System: Example OS 1.0.0 (build 7)\n",
        "Desktop: Yggdrasil, session: Yggdrasil (wayland)\n",
        "Kernel: 7.3.5-2-cachyos\n",
        "Up for: 60 minutes\n",
    ] {
        assert!(r.contains(want), "report.txt lacks {want:?}:\n{r}");
    }
    let d = text(&files, "device.txt");
    for want in [
        "Make: Micro-Star International Co., Ltd.\n",
        "Model: MS-7D25\n",
        "Board: MAG B660M\n",
        "Kind: desktop\n",
        "Processor: Example CPU 8000 (2 threads)\n",
        "Memory: 31.2 GiB\n",
        "- NVIDIA Corporation TU102 [GeForce RTX 2080 Ti Rev. A] [10de:1e07], driver nvidia, the screen at start\n",
        "  memory windows (BARs): 16 MiB, 256 MiB, 32 MiB\n",
        "  PCIe link: 8.0 GT/s PCIe x16 (can do 8.0 GT/s PCIe x16)\n",
        "NVIDIA driver: NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  620.73.04\n",
        "- card1-DP-1: ACR XV345CUR, 3440x1440\n",
    ] {
        assert!(d.contains(want), "device.txt lacks {want:?}:\n{d}");
    }
    assert!(!d.contains("HDMI-A-1"), "a connector with no screen is left out");
    let j = text(&files, "logs/this-start.txt");
    assert!(j.contains("usb 1-2: SerialNumber: <serial>") && j.contains("wlan0 <ip> <mac> joined <network>"), "{j}");
    assert!(j.contains("cannot open '/home/<user>/Documents/<file>'"), "a file's name in an own folder goes: {j}");
    assert!(j.contains("GET https://drive-api.proton.me/… 429"), "a link keeps its site: {j}");
    assert_eq!(j.matches("drive-api.proton.me").count(), 4, "the repeat shown three times, then counted: {j}");
    assert!(j.contains("Repeated lines, shown three times above and left out after that:\n2 more times: <computer> rclone[##]: GET https://drive-api.proton.me/… ###\n"), "{j}");
    assert!(text(&files, "logs/last-start.txt").starts_with("(not available: journalctl -b -1"), "a missing command says so");
    assert_eq!(text(&files, "system/self-check.txt"), "OK  the session for <user> on <computer>\n");
}

#[test]
fn without_the_system_s_part_a_report_is_still_made() {
    let t = computer("plain");
    let files = report(&t, &Request { text: "x".into(), ..Request::default() }, None);
    let r = text(&files, "report.txt");
    assert!(r.starts_with("Problem report\n") && r.contains("About: not said\n"), "{r}");
    assert!(!files.iter().any(|f| f.name.starts_with("system/") || f.name.starts_with("pictures/")));
}

#[test]
fn the_zip_holds_every_file_in_one_folder() {
    let files =
        vec![File { name: "report.txt".into(), data: b"hello".to_vec() }, File { name: "logs/a.txt".into(), data: vec![b'x'; 5000] }];
    let data = zip(&files, "exampleos-report-2026-11-26-1530").unwrap();
    assert!(data.len() < 600, "compressed: {} bytes", data.len());
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(data)).unwrap();
    let mut seen = vec![];
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut s = Vec::new();
        f.read_to_end(&mut s).unwrap();
        seen.push((f.name().unwrap().to_string(), s));
    }
    assert_eq!(
        seen,
        [
            ("exampleos-report-2026-11-26-1530/report.txt".to_string(), b"hello".to_vec()),
            ("exampleos-report-2026-11-26-1530/logs/a.txt".to_string(), vec![b'x'; 5000])
        ]
    );
}

#[test]
fn the_system_s_integration_names_the_address_and_its_checks() {
    let i = yggdrasil_conf::integrations::Integration::parse(
        Path::new("/x/example-report.conf"),
        "kind = report\nname = Example OS\nemail = support@example.com\ninclude.b_packages = Packages | /usr/bin/pacman -Q\ninclude.a_check = Self-check | /usr/bin/selfcheck\ninclude.c_bad = Bad | relative/path\ninclude.d_untitled = | /usr/bin/x\n",
    )
    .unwrap();
    let s = Support::from(&i);
    assert_eq!(s.name, "Example OS");
    assert_eq!(s.email.as_deref(), Some("support@example.com"));
    assert_eq!(
        s.includes,
        [
            ("Self-check".to_string(), vec!["/usr/bin/selfcheck".to_string()]),
            ("Packages".to_string(), vec!["/usr/bin/pacman".to_string(), "-Q".to_string()])
        ],
        "in their ids' order; a relative command or an untitled one left out"
    );
    assert_eq!(s.github, None);
    let odd = yggdrasil_conf::integrations::Integration::parse(Path::new("/x/r.conf"), "kind = report\nemail = not an address\n").unwrap();
    assert_eq!(Support::from(&odd).email, None);
    let gh = |v: &str| {
        let i =
            yggdrasil_conf::integrations::Integration::parse(Path::new("/x/r.conf"), &format!("kind = report\ngithub = {v}\n")).unwrap();
        Support::from(&i).github
    };
    assert_eq!(gh("https://github.com/Example-Org/Example_OS/").as_deref(), Some("https://github.com/Example-Org/Example_OS"));
    for bad in [
        "http://github.com/a/b",
        "https://gitlab.com/a/b",
        "https://github.com/a",
        "https://github.com/a/b/issues",
        "https://github.com/a/b?x=1",
    ] {
        assert_eq!(gh(bad), None, "{bad}");
    }
}

#[test]
fn the_message_to_paste_says_what_happened_without_names() {
    let t = computer("message");
    let cmds = commands();
    let run = |argv: &[&str]| cmds.get(argv.join(" ").as_str()).map(|s| s.to_string());
    let env = |k: &str| (k == "USER").then(|| "sam".to_string());
    let m = Machine { root: t.0.clone(), run: &run, env: &env, home: PathBuf::from("/home/sam"), now: String::new() };
    let req = Request { area: "Sound".into(), text: "  No sound on example-pc after sleep.\n".into(), pictures: vec![] };
    let (subject, message) = message(&req, &m, "exampleos-report-2026-11-26-2230.zip");
    assert_eq!(subject, "Problem report: Sound");
    assert_eq!(
        message,
        "No sound on <computer> after sleep.\n\nAbout: Sound\nSystem: Example OS 1.0.0\n\nThe full report is the attached file exampleos-report-2026-11-26-2230.zip."
    );
}

#[test]
fn a_long_log_keeps_its_end_from_a_whole_line() {
    let long: String = (0..60_000).map(|i| format!("line {i}\n")).collect();
    let t = tail(long.clone());
    assert!(t.len() <= LOG_TAIL + 30 && t.starts_with("(only the last part)\nline "), "{}", &t[..40]);
    assert!(t.ends_with("line 59999\n"));
    assert_eq!(tail("short\n".into()), "short\n");
}

#[test]
fn a_mail_link_is_encoded_and_a_slow_command_is_given_up() {
    assert_eq!(encode("Problem report: Sound (Example OS 1.0)"), "Problem%20report%3A%20Sound%20%28Example%20OS%201.0%29");
    assert_eq!(run(&["echo", "hi"], Duration::from_secs(5)).as_deref(), Some("hi\n"));
    let start = Instant::now();
    assert_eq!(run(&["sleep", "5"], Duration::from_millis(200)), None);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(run(&["/nonexistent/program"], Duration::from_secs(1)), None);
}
