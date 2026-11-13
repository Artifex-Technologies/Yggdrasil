//! The install, step by step as the script took it: the same programs with the same arguments in the same order, the same
//! STEP/INFO/WARN lines, and the same files written into the new system.

use std::collections::HashSet;

use crate::catalog::{self, texts};
use crate::conf::{Os, GIB};
use crate::disks::{arg, free_space, get, index, iterate, part, to_int, Disk};
use crate::plan::MIB;
use crate::py::{self, read_text, tail_chars, write_text, Fail, R};
use crate::pyjson::{dict, loads, s, Int, Py, OTHER_NESTING};
use crate::sys::{Output, System};

pub const STEPS: [&str; 9] = [
    "Checking the computer",
    "Preparing the disk",
    "Encrypting",
    "Formatting",
    "Copying the system",
    "Setting it up",
    "Making the start-up image",
    "Installing the boot menu",
    "Finishing",
];
const MIN_ROOT: i128 = 20 * GIB;
const MIN_ESP: i128 = 260 * MIB;
const SUBVOLUMES: [(&str, &str); 4] = [("@", ""), ("@home", "home"), ("@cache", "var/cache"), ("@log", "var/log")];
const INSIDE_PATH: &str = "PATH=/usr/local/sbin:/usr/local/bin:/usr/bin:/usr/sbin";
/// Where NetworkManager keeps the networks it joins (with their passwords), on the live system and on the new one.
pub const NM_CONNECTIONS: &str = "/etc/NetworkManager/system-connections";

/// The live system's own places the program looks at (other than through programs): fixed on a live medium, elsewhere in
/// the tests.
#[derive(Clone, Debug)]
pub struct Host {
    pub integration_dirs: Vec<String>,
    pub os_release: String,
    pub zoneinfo: String,
    /// /sys/firmware/efi
    pub efi: String,
    /// the live system's /etc/resolv.conf
    pub resolv_conf: String,
    /// the live system's saved networks (NetworkManager's /etc/NetworkManager/system-connections)
    pub nm_connections: String,
}

impl Host {
    pub fn real() -> Host {
        Host {
            integration_dirs: crate::conf::INTEGRATION_DIRS.iter().map(|s| s.to_string()).collect(),
            os_release: "/etc/os-release".into(),
            zoneinfo: "/usr/share/zoneinfo".into(),
            efi: "/sys/firmware/efi".into(),
            resolv_conf: "/etc/resolv.conf".into(),
            nm_connections: NM_CONNECTIONS.into(),
        }
    }
}

pub struct Ctx<'a> {
    pub os: Os,
    pub host: Host,
    pub sys: &'a mut dyn System,
    /// Whether the live system's /dev or /run/udev has been bound into the new system (unmount_all() then takes care).
    pub bound: bool,
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn stop<T>(m: impl Into<String>) -> R<T> {
    Err(Fail::Stop(m.into()))
}

fn crash<T>(m: impl Into<String>) -> R<T> {
    Err(Fail::Crash(m.into()))
}

/// a // b as Python divides whole numbers (rounding down).
fn floor_div(a: i128, b: i128) -> R<i128> {
    if b == 0 {
        return crash("integer division or modulo by zero");
    }
    let q = a / b;
    Ok(if a % b != 0 && ((a < 0) != (b < 0)) { q - 1 } else { q })
}

/// text[:-n] by characters
fn drop_last(text: &str, n: usize) -> String {
    let count = text.chars().count();
    text.chars().take(count.saturating_sub(n)).collect()
}

/// Byte positions where a line starts (re.M's ^).
fn line_starts(text: &str) -> Vec<usize> {
    let mut v = vec![0];
    v.extend(text.match_indices('\n').map(|(i, _)| i + 1).filter(|i| *i <= text.len()));
    v
}

/// The end of a run of whitespace (Python's \s*) from byte i.
fn skip_space(text: &str, i: usize) -> usize {
    i + text[i..].chars().take_while(|c| pycompat::isspace(*c)).map(char::len_utf8).sum::<usize>()
}

/// The whitespace character (\s) at byte i, if there is one: its length.
fn space_at(text: &str, i: usize) -> Option<usize> {
    text[i..].chars().next().filter(|c| pycompat::isspace(*c)).map(char::len_utf8)
}

/// re.search(rf"^{loc}\s", text, re.M)
fn has_locale_line(text: &str, loc: &str) -> bool {
    line_starts(text).into_iter().any(|i| text[i..].starts_with(loc) && space_at(text, i + loc.len()).is_some())
}

/// ^#\s*(LOC\s) at line start i: where the group starts and where the match ends.
fn commented_locale(text: &str, i: usize, loc: &str) -> Option<(usize, usize)> {
    if !text[i..].starts_with('#') {
        return None;
    }
    let j = skip_space(text, i + 1);
    if !text[j..].starts_with(loc) {
        return None;
    }
    space_at(text, j + loc.len()).map(|n| (j, j + loc.len() + n))
}

/// re.sub(rf"^#\s*({loc}\s)", r"\1", text, flags=re.M): every such line uncommented.
fn uncomment_locale(text: &str, loc: &str) -> String {
    let starts: HashSet<usize> = line_starts(text).into_iter().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if starts.contains(&i) {
            if let Some((g, end)) = commented_locale(text, i, loc) {
                out.push_str(&text[g..end]);
                i = end;
                continue;
            }
        }
        let c = text[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// GRUB's setting `key` in /etc/default/grub set to `line` (re.sub(rf"^#?\s*{k}=.*$", line, text, count=1, flags=re.M), or the
/// line added at the end when there is none).
fn grub_set(text: &str, key: &str, line: &str) -> String {
    let want = format!("{key}=");
    for i in line_starts(text) {
        let from = if text[i..].starts_with('#') { i + 1 } else { i };
        let k = skip_space(text, from);
        if text[k..].starts_with(&want) {
            let after = k + want.len();
            let end = text[after..].find('\n').map_or(text.len(), |n| after + n);
            return format!("{}{}{}", &text[..i], line, &text[end..]);
        }
    }
    format!("{text}{line}\n")
}

/// Whether a file named `name`.map.gz or `name`.map is anywhere under `top` (as os.walk lists files: links to folders are not
/// followed, unreadable folders are passed over).
fn keymap_file_under(top: &str, name: &str) -> bool {
    let wanted = [format!("{name}.map.gz"), format!("{name}.map")];
    let Ok(rd) = std::fs::read_dir(top) else {
        return false;
    };
    let mut dirs = vec![];
    let mut found = false;
    for e in rd.flatten() {
        let path = e.path();
        if path.is_dir() {
            if !e.file_type().is_ok_and(|t| t.is_symlink()) {
                dirs.push(path);
            }
        } else if wanted.iter().any(|w| e.file_name().to_str() == Some(w)) {
            found = true;
        }
    }
    found || dirs.iter().any(|d| keymap_file_under(&d.to_string_lossy(), name))
}

/// A saved network's file for the new system: in its [connection] part, `permissions=user:live:;` (a profile only the live
/// account could use, as NetworkManager writes a private one) names the new account instead. Every other byte stays as it was.
pub fn network_for(data: &[u8], login: &str) -> Vec<u8> {
    let trim_end = |b: &[u8]| -> usize { b.iter().rposition(|c| !b" \t\r".contains(c)).map_or(0, |i| i + 1) };
    let trim = |b: &[u8]| -> Vec<u8> {
        let end = trim_end(b);
        let start = b[..end].iter().position(|c| !b" \t\r".contains(c)).unwrap_or(end);
        b[start..end].to_vec()
    };
    let mut section: Vec<u8> = vec![];
    let mut out: Vec<Vec<u8>> = vec![];
    for line in data.split(|&c| c == b'\n') {
        let s = trim(line);
        if s.starts_with(b"[") && s.ends_with(b"]") {
            section = s;
        } else if section == b"[connection]" {
            if let Some(eq) = line.iter().position(|&c| c == b'=') {
                let (key, value) = (&line[..eq], &line[eq + 1..]);
                if trim(key) == b"permissions" {
                    let items: Vec<Vec<u8>> = value
                        .split(|&c| c == b';')
                        .map(|item| {
                            let mut f: Vec<Vec<u8>> = item.split(|&c| c == b':').map(<[u8]>::to_vec).collect();
                            if f.len() >= 2 && trim(&f[0]) == b"user" && &f[1][..trim_end(&f[1])] == crate::plan::LIVE_USER.as_bytes() {
                                let rest = f[1][trim_end(&f[1])..].to_vec();
                                f[1] = [login.as_bytes(), &rest].concat();
                            }
                            f.join(&b':')
                        })
                        .collect();
                    out.push([key, b"=", &items.join(&b';')].concat());
                    continue;
                }
            }
        }
        out.push(line.to_vec());
    }
    out.join(&b'\n')
}

/// An error as the script words it (its OSError's strerror): no file name, which here could be a network's.
fn why(e: &std::io::Error) -> String {
    e.raw_os_error().map(py::strerror).unwrap_or_else(|| e.to_string())
}

/// A plain file's bytes, or None for anything else (a link is never followed; a pipe or a device is never opened).
fn read_plain(path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Ok(None);
    }
    let mut f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    if !f.metadata()?.is_file() {
        return Ok(None);
    }
    let mut data = vec![];
    f.read_to_end(&mut data)?;
    Ok(Some(data))
}

/// A folder only root may enter (0700, root's when this runs as root).
fn private_dir(dir: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    if unsafe { libc::geteuid() } == 0 {
        std::os::unix::fs::lchown(dir, Some(0), Some(0))?;
    }
    Ok(())
}

/// A file only root may read (0600, root's when this runs as root), written without following a link.
fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        std::fs::remove_file(path)?;
    }
    let mut f =
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(path)?;
    if unsafe { libc::geteuid() } == 0 {
        std::os::unix::fs::fchown(&f, Some(0), Some(0))?;
    }
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(data)
}

/// The text console's keymap for an X11 layout and variant ("gb" -> "uk", "us"+"dvorak" -> "dvorak"): from systemd's own
/// table (the first row that fits, as localectl does), else a console keymap of the same name, else "us".
pub fn console_keymap(layout: &str, variant: &str, root: &str) -> R<String> {
    let rows: Vec<Vec<String>> = match std::fs::read(format!("{root}/usr/share/systemd/kbd-model-map")) {
        Ok(bytes) => {
            let text = py::universal(&py::decode(&bytes)?);
            text.split_inclusive('\n')
                .filter(|l| !pycompat::strip(l).is_empty() && !l.starts_with('#'))
                .map(|l| pycompat::split(l).into_iter().map(str::to_string).collect())
                .collect()
        }
        Err(_) => vec![],
    };
    let row = |v: &str| rows.iter().find(|r| r.len() >= 4 && r[1] == layout && r[3] == v).map(|r| r[0].clone());
    let keymaps = format!("{root}/usr/share/kbd/keymaps");
    let keymap_file = |n: &str| keymap_file_under(&keymaps, n).then(|| n.to_string());
    // the exact pair, then a console keymap named after the variant (colemak), then the layout's own
    let found = [
        if variant.is_empty() { None } else { row(variant) },
        if variant.is_empty() { None } else { keymap_file(variant) },
        row("-"),
        keymap_file(layout),
    ];
    Ok(found.into_iter().flatten().find(|f| !f.is_empty()).unwrap_or_else(|| "us".into()))
}

/// A partition's use in the install: its device (a node sfdisk named, or the plan's own), mount point and file system to make.
type Use = (Py, String, Option<String>);

const MKFS: [(&str, &[&str]); 5] = [
    ("btrfs", &["mkfs.btrfs", "-f", "-L"]),
    ("ext4", &["mkfs.ext4", "-F", "-L"]),
    ("xfs", &["mkfs.xfs", "-f", "-L"]),
    ("vfat", &["mkfs.vfat", "-F", "32", "-n"]),
    ("swap", &["mkswap", "-L"]),
];
const LABELS: [(&str, &str); 6] =
    [("/home", "Home"), ("/boot/efi", "EFI"), ("swap", "swap"), ("/var", "Var"), ("/srv", "Srv"), ("/opt", "Opt")];

impl Ctx<'_> {
    /// print(f"{kind} {text}".rstrip(), flush=True)
    pub fn say(&mut self, kind: &str, text: &str) -> R<()> {
        let line = format!("{kind} {text}");
        self.sys.out(pycompat::rstrip(&line))
    }

    fn step(&mut self, n: usize) -> R<()> {
        self.say("STEP", &format!("{n} {} {}", STEPS.len(), STEPS[n - 1]))
    }

    /// Runs a program; on failure (with `check`) stops the install with its error output.
    pub fn run(&mut self, argv: &[String], stdin: Option<&str>, check: bool) -> R<Output> {
        let r = self.sys.run(argv, stdin)?;
        if check && r.code != 0 {
            // the program, not the chroot and search path around it
            let shown: &[String] = if argv.first().map(String::as_str) == Some("chroot") { argv.get(4..).unwrap_or(&[]) } else { argv };
            let head = shown[..shown.len().min(3)].join(" ");
            let said = if r.stderr.is_empty() { &r.stdout } else { &r.stderr };
            return stop(format!("{head} failed: {}", tail_chars(pycompat::strip(said), 400)));
        }
        Ok(r)
    }

    pub fn capture(&mut self, argv: &[&str], check: bool) -> R<String> {
        Ok(self.run(&strings(argv), None, check)?.stdout)
    }

    /// Runs a program inside the new system (chroot), with the system's full search path: pkexec gives the installer one
    /// without /usr/local/bin, where an OS keeps its own helpers (a finishing program once found no menu theme there).
    pub fn inside(&mut self, argv: &[String], stdin: Option<&str>, check: bool) -> R<Output> {
        let mut full = strings(&["chroot", &self.os.target, "/usr/bin/env", INSIDE_PATH]);
        full.extend_from_slice(argv);
        self.run(&full, stdin, check)
    }

    fn inside_code(&mut self, argv: &[&str]) -> R<i32> {
        Ok(self.inside(&strings(argv), None, false)?.code)
    }

    fn settle(&mut self) -> R<()> {
        self.run(&strings(&["udevadm", "settle"]), None, false)?;
        self.sys.sleep(1);
        Ok(())
    }

    fn t(&self, rest: &str) -> String {
        format!("{}{rest}", self.os.target)
    }

    // ------------------------------------------------------------ the steps

    fn preflight(&mut self, p: &Py) -> R<()> {
        let name = self.os.name.clone();
        if !py::isdir(&self.host.efi) {
            return stop(format!("this computer started without UEFI; {name} installs on UEFI computers only (turn off Legacy/CSM)"));
        }
        if !py::isfile(&self.os.sfs) {
            return stop(format!("the system to copy was not found ({}); start the installer from the {name} USB", self.os.sfs));
        }
        let disk = p.at("disk").as_str().unwrap_or("").to_string();
        let choice = self.disks()?.into_iter().find(|d| d.path.as_str() == Some(disk.as_str()));
        let Some(choice) = choice else {
            return stop(format!("{disk} cannot be installed onto (not a disk, read-only, or the {name} USB itself)"));
        };
        if !choice.big_enough {
            return stop(format!("{disk} is too small: {name} needs at least {} GB", floor_div(self.os.min_bytes, GIB)?));
        }
        if p.at("mode").as_str() == Some("erase") {
            let mounted = self.capture(&["lsblk", "-nr", "-o", "MOUNTPOINTS", &disk], true)?;
            if !pycompat::split(&mounted).is_empty() {
                return stop(format!("something on {disk} is in use (mounted); close it first"));
            }
        } else if let Some(problem) = manual_fits(p, &choice)? {
            return stop(problem);
        }
        Ok(())
    }

    fn partition(&mut self, disk: &str) -> R<(String, String)> {
        self.run(&strings(&["wipefs", "-a", disk]), None, true)?;
        // GPT: 1 GiB EFI system partition, the rest for the system
        let layout = format!("label: gpt\nsize=1GiB, type=uefi, name=EFI\ntype=linux, name={}\n", self.os.name.replace(' ', "_"));
        self.run(&strings(&["sfdisk", "--wipe", "always", "--wipe-partitions", "always", disk]), Some(&layout), true)?;
        self.settle()?;
        Ok((part(disk, 1), part(disk, 2)))
    }

    /// sfdisk -J's partition table.
    fn sfdisk_table(&mut self, disk: &str) -> R<Py> {
        let text = self.capture(&["sfdisk", "-J", disk], true)?;
        let v = loads(&text, OTHER_NESTING).or_else(|_| {
            crash(if text.is_empty() { "Expecting value: line 1 column 1 (char 0)" } else { "sfdisk's output is not JSON" })
        })?;
        Ok(index(&v, "partitiontable")?.clone())
    }

    /// Carries out the partitioning of a manual plan: the partitions made, ("new:id", their device).
    fn manual_partition(&mut self, p: &Py) -> R<Vec<(String, Py)>> {
        let disk = p.at("disk").as_str().unwrap_or("").to_string();
        if p.at("new_table").truthy() {
            self.run(&strings(&["wipefs", "-a", &disk]), None, true)?;
            self.run(&strings(&["sfdisk", "--wipe", "always", &disk]), Some("label: gpt\n"), true)?;
        } else {
            // a number given twice is deleted once
            let mut numbers: Vec<Int> = p
                .at("delete")
                .as_list()
                .unwrap_or(&[])
                .iter()
                .filter_map(|n| if let Py::Int(i) = n { Some(i.clone()) } else { None })
                .collect();
            numbers.sort_by(|a, b| (b.value, b.text.len(), &b.text).cmp(&(a.value, a.text.len(), &a.text)));
            numbers.dedup();
            for n in numbers {
                self.run(&strings(&["sfdisk", "--delete", &disk, &n.text]), None, true)?;
            }
        }
        self.settle()?;
        let table = self.sfdisk_table(&disk)?;
        let ss = to_int(get(&table, "sectorsize")?.unwrap_or(&Py::Int(Int::new(512))))?.value;
        let mut made: Vec<(String, Py)> = vec![];
        for c in p.at("create").as_list().unwrap_or(&[]) {
            let start = floor_div(c.at("start").whole().unwrap_or(0), ss)?;
            let kind = match c.at("kind").as_str() {
                Some("efi") => "uefi",
                Some("swap") => "swap",
                _ => "linux",
            };
            let size = floor_div(c.at("size").whole().unwrap_or(0), ss)?;
            self.run(&strings(&["sfdisk", "--append", &disk]), Some(&format!("start={start}, size={size}, type={kind}\n")), true)?;
            self.settle()?;
            let table = self.sfdisk_table(&disk)?;
            let mut node = None;
            for q in iterate(get(&table, "partitions")?.unwrap_or(&Py::List(vec![])))? {
                if to_int(index(&q, "start")?)?.value == start {
                    node = Some(index(&q, "node")?.clone());
                    break;
                }
            }
            let Some(node) = node.filter(Py::truthy) else {
                return stop("the new partition could not be found after making it");
            };
            made.push((format!("new:{}", c.at("id").as_str().unwrap_or("")), node));
        }
        // an existing partition chosen as the EFI partition and formatted gets the EFI type, so firmware looks there
        for u in p.at("use").as_list().unwrap_or(&[]) {
            let partname = index(u, "part")?.as_str().unwrap_or("").to_string();
            if index(u, "mount")?.as_str() == Some("/boot/efi") && index(u, "format")?.truthy() && !partname.starts_with("new:") {
                let digits: String = {
                    let rev: Vec<char> = partname.chars().rev().take_while(|c| pycompat::decimal(*c).is_some()).collect();
                    rev.into_iter().rev().collect()
                };
                if digits.is_empty() {
                    return crash("'NoneType' object has no attribute 'group'");
                }
                self.run(&strings(&["sfdisk", "--part-type", &disk, &digits, "uefi"]), None, false)?;
            }
        }
        self.settle()?;
        Ok(made)
    }

    fn encrypt(&mut self, dev: &str, password: &str) -> R<String> {
        // LUKS2 with PBKDF2: GRUB (which reads /boot from inside) cannot open Argon2 keys
        let label = format!("{}-crypt", self.os.name.replace(' ', "_"));
        self.run(
            &strings(&[
                "cryptsetup",
                "luksFormat",
                "--batch-mode",
                "--type",
                "luks2",
                "--pbkdf",
                "pbkdf2",
                "--label",
                &label,
                "--key-file=-",
                dev,
            ]),
            Some(password),
            true,
        )?;
        let mapper = self.os.mapper.clone();
        self.run(&strings(&["cryptsetup", "open", "--key-file=-", dev, &mapper]), Some(password), true)?;
        Ok(format!("/dev/mapper/{mapper}"))
    }

    fn uuid_of(&mut self, dev: &str) -> R<String> {
        Ok(pycompat::strip(&self.capture(&["blkid", "-s", "UUID", "-o", "value", dev], true)?).to_string())
    }

    /// uses: (device, mount point, file system to make or None to keep). Formats, mounts / first and the rest under it. A new
    /// btrfs / gets the subvolumes (@home only when /home has no partition of its own). Returns the fstab lines.
    fn format_and_mount(&mut self, uses: &[Use]) -> R<Vec<String>> {
        let default_label: String = self.os.name.replace(' ', "_").chars().take(16).collect();
        for (dev, mount, fmt) in uses {
            if let Some(fmt) = fmt.as_deref().filter(|f| !f.is_empty()) {
                let mut argv = strings(MKFS.iter().find(|(k, _)| *k == fmt).map(|(_, a)| *a).unwrap_or(&[]));
                argv.push(LABELS.iter().find(|(m, _)| m == mount).map_or(default_label.clone(), |(_, l)| l.to_string()));
                argv.push(arg(dev)?);
                self.run(&argv, None, true)?;
            }
        }
        let (root_dev, _, root_fs) = uses.iter().find(|u| u.1 == "/").cloned().unwrap();
        let root_dev = arg(&root_dev)?;
        let own_home = uses.iter().any(|u| u.1 == "/home");
        let target = self.os.target.clone();
        py::makedirs(&target)?;
        let mut fstab = vec![];
        if root_fs.as_deref() == Some("btrfs") {
            let subs: Vec<(&str, &str)> = SUBVOLUMES.iter().copied().filter(|(_, w)| !(*w == "home" && own_home)).collect();
            self.run(&strings(&["mount", &root_dev, &target]), None, true)?;
            for (sub, _) in &subs {
                self.run(&strings(&["btrfs", "subvolume", "create", &format!("{target}/{sub}")]), None, true)?;
            }
            self.run(&strings(&["umount", &target]), None, true)?;
            let opts = "compress=zstd:1,noatime";
            let ru = self.uuid_of(&root_dev)?;
            for (sub, place) in &subs {
                let dst = py::join(&target, place);
                py::makedirs(&dst)?;
                self.run(&strings(&["mount", "-o", &format!("{opts},subvol={sub}"), &root_dev, &dst]), None, true)?;
                fstab.push(format!("UUID={ru} /{place} btrfs {opts},subvol=/{sub} 0 0"));
            }
        } else {
            self.run(&strings(&["mount", &root_dev, &target]), None, true)?;
            let u = self.uuid_of(&root_dev)?;
            fstab.push(format!("UUID={u} / {} noatime 0 1", root_fs.unwrap_or_default()));
        }
        let mut rest: Vec<&Use> = uses.iter().filter(|u| u.1 != "/" && u.1 != "swap").collect();
        rest.sort_by_key(|u| u.1.matches('/').count());
        for (dev, mount, fmt) in rest {
            let dev = arg(dev)?;
            let dst = format!("{target}{mount}");
            py::makedirs(&dst)?;
            self.run(&strings(&["mount", &dev, &dst]), None, true)?;
            let fs = match fmt.as_deref().filter(|f| !f.is_empty()) {
                Some(f) => f.to_string(),
                None => pycompat::strip(&self.capture(&["blkid", "-s", "TYPE", "-o", "value", &dev], true)?).to_string(),
            };
            let opts = if mount == "/boot/efi" {
                "umask=0077"
            } else if fs == "btrfs" {
                "compress=zstd:1,noatime"
            } else {
                "noatime"
            };
            let u = self.uuid_of(&dev)?;
            fstab.push(format!("UUID={u} {mount} {fs} {opts} 0 {}", if fs == "btrfs" { 0 } else { 2 }));
        }
        for (dev, mount, _) in uses {
            if mount == "swap" {
                let u = self.uuid_of(&arg(dev)?)?;
                fstab.push(format!("UUID={u} none swap defaults 0 0"));
            }
        }
        Ok(fstab)
    }

    /// unsquashfs, with its percentage turned into INFO lines every 10%.
    fn copy_system(&mut self) -> R<()> {
        let argv = strings(&["unsquashfs", "-f", "-percentage", "-d", &self.os.target, &self.os.sfs]);
        let mut last: i128 = -10;
        let (code, err) = self.sys.stream(&argv, &mut |line: &str| {
            let line = pycompat::strip(line);
            if pycompat::isdigit(line) {
                let n = pycompat::int(line)
                    .ok_or_else(|| Fail::Crash(format!("invalid literal for int() with base 10: {}", pycompat::repr(line))))?;
                if n >= last + 10 {
                    last = n;
                    return Ok(Some(format!("INFO copied {last}%")));
                }
            }
            Ok(None)
        })?;
        if code != 0 {
            return stop(format!("copying the system failed: {}", tail_chars(pycompat::strip(&err), 400)));
        }
        Ok(())
    }

    fn bind_mounts(&mut self) -> R<()> {
        // a fresh /run (the live system's would keep the new one busy at the end), with udev's part for the disk tools. The live
        // system's own are bound one way only, in the same call (--make-rslave): bind mounts are shared, and unmounting a shared
        // one at the end would unmount the live system's /dev/pts, /dev/shm ... too. A separate call afterwards could fail and
        // leave a shared one behind.
        let mounts: [(&str, &str, &[&str]); 5] = [
            ("proc", "proc", &["-t", "proc"]),
            ("sys", "sys", &["-t", "sysfs"]),
            ("/dev", "dev", &["--rbind", "--make-rslave"]),
            ("tmpfs", "run", &["-t", "tmpfs"]),
            ("/run/udev", "run/udev", &["--bind", "--make-rslave"]),
        ];
        for (src, dst, extra) in mounts {
            let d = py::join(&self.os.target, dst);
            py::makedirs(&d)?;
            if src.starts_with('/') {
                self.bound = true;
            }
            let mut argv = strings(&["mount"]);
            argv.extend(strings(extra));
            argv.extend(strings(&[src, &d]));
            self.run(&argv, None, true)?;
        }
        if py::isdir(&format!("{}/efivars", self.host.efi)) {
            let efivars = self.t("/sys/firmware/efi/efivars");
            self.run(&strings(&["mount", "-t", "efivarfs", "efivarfs", &efivars]), None, false)?;
        }
        Ok(())
    }

    fn write_fstab(&mut self, lines: &[String]) -> R<()> {
        let mut all = vec!["# Written by yggdrasil-install-system. <device> <mount point> <type> <options> <dump> <pass>".to_string()];
        all.extend_from_slice(lines);
        write_text(&self.t("/etc/fstab"), &(all.join("\n") + "\n"))
    }

    fn set_up(&mut self, p: &Py) -> R<()> {
        // the OS's own clean-up of everything that belongs to the live medium (live account, autologin, no-password rules ...),
        // and its kernels and start-up image presets put back
        match self.os.cleanup.clone() {
            Some(c) if py::exists(&self.t(&c)) => {
                self.inside(&[c], None, true)?;
            }
            _ => self.say(
                "WARN",
                "this system names no clean-up program for the live medium's settings (\"cleanup\" in its installer integration)",
            )?,
        }
        // a new machine id at first start (the medium's must not be shared)
        write_text(&self.t("/etc/machine-id"), "")?;
        // language
        let loc = p.at("locale").as_str().unwrap_or("").to_string();
        let gen = self.t("/etc/locale.gen");
        let text = if py::exists(&gen) { read_text(&gen)? } else { String::new() };
        if !has_locale_line(&text, &loc) {
            let charset = loc.split('.').nth(1).unwrap_or("UTF-8").to_string();
            let commented = line_starts(&text).into_iter().any(|i| commented_locale(&text, i, &loc).is_some());
            let text = if commented { uncomment_locale(&text, &loc) } else { format!("{text}{loc} {charset}\n") };
            write_text(&gen, &text)?;
        }
        self.inside(&strings(&["locale-gen"]), None, false)?;
        write_text(&self.t("/etc/locale.conf"), &format!("LANG={loc}\n"))?;
        // keyboard: the text console (its own keymap names), and the desktop (X11's setting, which Yggdrasil's session reads)
        let layout = p.at("keymap").as_str().unwrap_or("").to_string();
        let variant =
            if p.at("keymap_variant").truthy() { p.at("keymap_variant").as_str().unwrap_or("").to_string() } else { String::new() };
        let console = console_keymap(&layout, &variant, &self.os.target)?;
        let v_line = if variant.is_empty() { String::new() } else { format!("XKBVARIANT={variant}\n") };
        write_text(&self.t("/etc/vconsole.conf"), &format!("KEYMAP={console}\nXKBLAYOUT={layout}\n{v_line}"))?;
        py::makedirs(&self.t("/etc/X11/xorg.conf.d"))?;
        let v_opt = if variant.is_empty() { String::new() } else { format!("    Option \"XkbVariant\" \"{variant}\"\n") };
        write_text(
            &self.t("/etc/X11/xorg.conf.d/00-keyboard.conf"),
            &format!("Section \"InputClass\"\n    Identifier \"system-keyboard\"\n    MatchIsKeyboard \"on\"\n    Option \"XkbLayout\" \"{layout}\"\n{v_opt}EndSection\n"),
        )?;
        // time zone and the hardware clock in UTC
        let lt = self.t("/etc/localtime");
        if py::lexists(&lt) {
            py::remove(&lt)?;
        }
        py::symlink(&format!("/usr/share/zoneinfo/{}", p.at("timezone").as_str().unwrap_or("")), &lt)?;
        self.inside(&strings(&["hwclock", "--systohc", "--utc"]), None, false)?;
        // the computer's name
        let host = p.at("hostname").as_str().unwrap_or("").to_string();
        write_text(&self.t("/etc/hostname"), &format!("{host}\n"))?;
        write_text(&self.t("/etc/hosts"), &format!("127.0.0.1 localhost\n::1 localhost\n127.0.1.1 {host}.localdomain {host}\n"))?;
        // the account: an administrator through sudo (wheel); root stays locked
        let u = p.at("user");
        let login = u.at("login").as_str().unwrap_or("").to_string();
        let mut groups = vec![];
        for g in self.os.groups.clone() {
            if self.inside_code(&["getent", "group", &g])? == 0 {
                groups.push(g);
            }
        }
        let name = u.get("name").and_then(Py::as_str).unwrap_or("").to_string();
        self.inside(&strings(&["useradd", "-m", "-G", &groups.join(","), "-s", "/bin/bash", "-c", &name, &login]), None, true)?;
        let password = u.at("password").as_str().unwrap_or("");
        self.inside(&strings(&["chpasswd"]), Some(&format!("{login}:{password}\n")), true)?;
        self.inside(&strings(&["passwd", "-l", "root"]), None, false)?;
        py::makedirs(&self.t("/etc/sudoers.d"))?;
        let sudoers = self.t(&format!("/etc/sudoers.d/10-{}-wheel", self.os.id));
        write_text(&sudoers, "# Written by yggdrasil-install-system: administrators (the wheel group) use sudo with their own password.\n%wheel ALL=(ALL:ALL) ALL\n")?;
        py::chmod(&sudoers, 0o440)?;
        // the networks joined in the live session, so the new system is online at its first start
        self.carry_networks(&login)?;
        // the login screen: greetd with Yggdrasil's greeter; never leave the system without one
        if !py::exists(&self.t("/usr/lib/systemd/system/greetd.service")) {
            return stop("the system has no login screen (greetd is missing)");
        }
        self.inside(&strings(&["systemctl", "enable", "-f", "greetd.service"]), None, true)?;
        Ok(())
    }

    /// The networks saved in the live session (NetworkManager's files, the Wi-Fi joined there with its password) go to the new
    /// system as they are, so it is online at its first start without asking again (as Calamares' networkcfg does): the folder
    /// root's alone (0700), each file 0600. A profile only the live account could use becomes the new account's (network_for).
    /// Only plain files directly in the folder are taken: a link is never followed, and anything else is left behind. Nothing
    /// here stops the install, and no network's name or password is said, only how many were carried over.
    fn carry_networks(&mut self, login: &str) -> R<()> {
        use std::os::unix::ffi::OsStrExt;
        let live = self.host.nm_connections.clone();
        if py::islink(&live) || !py::isdir(&live) {
            return Ok(());
        }
        let mut names: Vec<std::ffi::OsString> = match std::fs::read_dir(&live) {
            Ok(rd) => rd.flatten().map(|e| e.file_name()).filter(|n| n.as_bytes().ends_with(b".nmconnection")).collect(),
            Err(e) => return self.say("WARN", &format!("the live session's saved networks could not be read ({})", why(&e))),
        };
        names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        let mut got = vec![];
        let mut left = 0;
        for n in names {
            match read_plain(&std::path::Path::new(&live).join(&n)) {
                Ok(Some(data)) => got.push((n, network_for(&data, login))),
                Ok(None) => left += 1,
                Err(e) => self.say("WARN", &format!("a saved network could not be read ({})", why(&e)))?,
            }
        }
        if left > 0 {
            self.say("WARN", &format!("left behind {left} of the live session's saved networks that are not plain files"))?;
        }
        if got.is_empty() {
            return Ok(());
        }
        let dir = self.t(NM_CONNECTIONS);
        if py::islink(&dir) {
            return self.say("WARN", "the new system's folder for saved networks is a link: none were carried over");
        }
        if let Err(e) = private_dir(&dir) {
            return self.say("WARN", &format!("the new system's folder for saved networks could not be made ({})", why(&e)));
        }
        let mut copied = 0;
        for (n, data) in got {
            let dst = std::path::Path::new(&dir).join(&n);
            match write_private(&dst, &data) {
                Ok(()) => copied += 1,
                Err(e) => {
                    let _ = std::fs::remove_file(&dst);
                    self.say("WARN", &format!("a saved network could not be written ({})", why(&e)))?;
                }
            }
        }
        if copied > 0 {
            self.say("INFO", &format!("the live session's saved networks carried over: {copied}"))?;
        }
        Ok(())
    }

    /// Runs `f` with the new system able to look up names: its /run is empty, so its resolv.conf (a link into /run) points
    /// nowhere; the live system's own answers are lent to it for the time being.
    fn with_dns(&mut self, f: &mut dyn FnMut(&mut Self) -> R<()>) -> R<()> {
        let link = self.t("/etc/resolv.conf");
        let live = match std::fs::read(&self.host.resolv_conf) {
            Ok(bytes) => py::universal(&py::decode(&bytes)?),
            Err(_) => String::new(),
        };
        let mut made = None;
        if py::islink(&link) {
            let to = std::fs::read_link(&link).map_err(|e| py::oserror(&e, &link))?;
            // normalised whether relative or not: a link with ".." in it must not lead outside the new system
            let dest = pycompat::normpath(&py::join("/etc", &to.to_string_lossy()));
            let path = self.t(&dest);
            if !py::exists(&path) {
                py::makedirs(&py::dirname(&path))?;
                write_text(&path, &live)?;
                made = Some(path);
            }
        }
        let result = f(self);
        if let Some(path) = made {
            py::remove(&path)?;
        }
        result
    }

    /// Removes packages from the new system (pacman -Rns). One that a program staying on the system needs (Sarina's bubblewrap
    /// and binutils, which Flatpak and the compilers use too) cannot go and is kept: that is no fault, so it is only noted.
    /// Returns the ones kept.
    pub fn remove_packages(&mut self, names: &[String]) -> R<Vec<String>> {
        let mut left: Vec<String> = vec![];
        for n in names {
            if !left.contains(n) {
                left.push(n.clone());
            }
        }
        if left.is_empty() {
            return Ok(vec![]);
        }
        let mut argv = strings(&["pacman", "-Rns", "--noconfirm"]);
        argv.extend(left.iter().cloned());
        if self.inside(&argv, None, false)?.code == 0 {
            return Ok(vec![]);
        }
        // one at a time, until a round removes nothing more (a package another one on the list needed goes in a later round)
        loop {
            let mut still = vec![];
            for x in &left {
                if self.inside_code(&["pacman", "-Q", x])? == 0 {
                    still.push(x.clone());
                }
            }
            left = still;
            let mut kept = vec![];
            for x in &left {
                if self.inside_code(&["pacman", "-Rns", "--noconfirm", x])? != 0 {
                    kept.push(x.clone());
                }
            }
            if kept.is_empty() || kept.len() == left.len() {
                return Ok(kept);
            }
            left = kept;
        }
    }

    /// The browsers chosen in the installer: the others that came on the medium are removed, the rest installed (from the
    /// repositories, or Flathub; both need the internet, and a browser that cannot be had is left for Agora: never a reason to
    /// stop the install), and the default one opens web links and sits in the dock.
    fn set_up_browsers(&mut self, p: &Py) -> R<()> {
        let Some(chosen) = p.get("browsers") else {
            return Ok(());
        };
        let chosen: Vec<String> = chosen.as_list().unwrap_or(&[]).iter().filter_map(|b| b.as_str().map(str::to_string)).collect();
        let catalog = catalog::entries(self.os.browsers_file.as_deref(), "browsers");
        let plan = catalog::browser_plan(&catalog, &chosen);
        let mut have = vec![];
        for x in &plan.remove {
            if self.inside_code(&["pacman", "-Q", x])? == 0 {
                have.push(x.clone());
            }
        }
        if !have.is_empty() {
            self.say("INFO", &format!("removing the browsers not chosen: {}", have.join(", ")))?;
            let kept = self.remove_packages(&have)?;
            if !kept.is_empty() {
                self.say("INFO", &format!("kept, as other programs need them: {}", kept.join(", ")))?;
            }
        }
        let mut missing = vec![];
        for x in &plan.install {
            if self.inside_code(&["pacman", "-Q", x])? != 0 {
                missing.push(x.clone());
            }
        }
        let flat = plan.flatpaks.clone();
        if !missing.is_empty() || !flat.is_empty() {
            self.with_dns(&mut |me: &mut Self| {
                if !missing.is_empty() {
                    me.say("INFO", &format!("installing {} (from the internet)", missing.join(", ")))?;
                    let mut argv = strings(&["pacman", "-S", "--needed", "--noconfirm"]);
                    argv.extend(missing.iter().cloned());
                    let mut code = me.inside(&argv, None, false)?.code;
                    if code != 0 {
                        // the medium's package lists may be older than the mirrors: bring the system up to date with them
                        let mut argv = strings(&["pacman", "-Syu", "--needed", "--noconfirm"]);
                        argv.extend(missing.iter().cloned());
                        code = me.inside(&argv, None, false)?.code;
                    }
                    if code != 0 {
                        me.say("WARN", &format!("could not install {} (no internet?); Agora can install them later", missing.join(", ")))?;
                    }
                }
                for app in &flat {
                    me.inside(
                        &strings(&[
                            "flatpak",
                            "remote-add",
                            "--system",
                            "--if-not-exists",
                            "flathub",
                            "https://dl.flathub.org/repo/flathub.flatpakrepo",
                        ]),
                        None,
                        false,
                    )?;
                    me.say("INFO", &format!("installing {app} from Flathub"))?;
                    if me.inside(&strings(&["flatpak", "install", "--system", "-y", "--noninteractive", "flathub", app]), None, false)?.code
                        != 0
                    {
                        me.say("WARN", &format!("could not install {app} from Flathub (no internet?); Agora can install it later"))?;
                    }
                }
                Ok(())
            })?;
        }
        // the default: the one picked, else the first chosen; only one that is really there now
        let mut order: Vec<String> = vec![];
        if p.at("default_browser").truthy() {
            order.push(p.at("default_browser").as_str().unwrap_or("").to_string());
        }
        order.extend(chosen.iter().cloned());
        let dirs = [self.t("/usr/share/applications"), self.t("/var/lib/flatpak/exports/share/applications")];
        let mut default: Option<String> = None;
        for bid in &order {
            let b = catalog.iter().find(|x| catalog::id(x) == *bid);
            for d in b.map(|b| texts(b.at("desktop"))).unwrap_or_default() {
                if dirs.iter().any(|dd| py::exists(&py::join(dd, &d))) {
                    default = Some(d);
                    break;
                }
            }
            if default.as_deref().is_some_and(|d| !d.is_empty()) {
                break;
            }
        }
        let default = default.filter(|d| !d.is_empty());
        if chosen.is_empty() {
            self.say("INFO", "no browser chosen: Agora has them all")?;
        } else if let Some(d) = &default {
            self.say("INFO", &format!("default browser: {d}"))?;
            let mf = self.t("/etc/xdg/mimeapps.list");
            let text = if py::exists(&mf) { read_text(&mf)? } else { String::new() };
            write_text(&mf, &catalog::mimeapps_with_browser(&text, d))?;
        } else {
            self.say("WARN", "none of the chosen browsers could be installed; Agora can install one later")?;
        }
        let dock = self.t("/etc/yggdrasil/dock");
        if py::exists(&dock) {
            let ids: HashSet<String> =
                catalog.iter().flat_map(|b| texts(b.at("desktop"))).map(|d| drop_last(&d, ".desktop".len())).collect();
            let text = read_text(&dock)?;
            let default_id = default.as_deref().map(|d| drop_last(d, ".desktop".len()));
            write_text(&dock, &catalog::dock_with_browser(&text, &ids, default_id.as_deref()))?;
        }
        Ok(())
    }

    /// The optional parts chosen in the installer (an assistant, a security monitor): those on the medium are kept, with their
    /// services on, or removed when not chosen; chosen ones the medium does not carry are written down for the new user's
    /// first login (the integration's "first_login" file, which the OS's own program reads), or left for Agora when the OS
    /// names no such file. Never a reason to stop the install.
    fn set_up_extras(&mut self, p: &Py) -> R<()> {
        let Some(chosen) = p.get("extras") else {
            return Ok(());
        };
        let chosen: Vec<String> = chosen.as_list().unwrap_or(&[]).iter().filter_map(|e| e.as_str().map(str::to_string)).collect();
        let catalog = catalog::entries(self.os.extras_file.as_deref(), "extras");
        let comps = catalog::components(self.os.components_file.as_deref());
        let names: Vec<String> = comps.values().flat_map(catalog::component_packages).collect();
        let mut installed = HashSet::new();
        for x in &names {
            if self.inside_code(&["pacman", "-Q", x])? == 0 {
                installed.insert(x.clone());
            }
        }
        let plan = catalog::extras_plan(&catalog, &chosen, &comps, &installed);
        if !plan.remove.is_empty() {
            self.say("INFO", &format!("removing what was not chosen: {}", plan.remove.join(", ")))?;
            let kept = self.remove_packages(&plan.remove)?;
            if !kept.is_empty() {
                self.say("INFO", &format!("kept, as other programs need them: {}", kept.join(", ")))?;
            }
        }
        let login = p.at("user").at("login").as_str().unwrap_or("").to_string();
        for cid in &plan.keep {
            let c = comps.get(cid).cloned().unwrap_or(Py::None);
            for unit in texts(c.at("enable_system")) {
                self.inside(&strings(&["systemctl", "enable", &unit]), None, false)?;
            }
            let global = texts(c.at("enable_global"));
            if !global.is_empty() {
                let mut argv = strings(&["systemctl", "--global", "enable"]);
                argv.extend(global);
                self.inside(&argv, None, false)?;
            }
            let groups = texts(c.at("add_groups"));
            if !groups.is_empty() && !login.is_empty() {
                self.inside(&strings(&["usermod", "-aG", &groups.join(","), &login]), None, false)?;
            }
        }
        let names_of = |ids: &[String]| ids.iter().map(|c| catalog::component_name(&comps, c)).collect::<Vec<_>>().join(", ");
        if !plan.keep.is_empty() {
            self.say("INFO", &format!("kept: {}", names_of(&plan.keep)))?;
        }
        match (&self.os.first_login_file.clone(), plan.queue.is_empty()) {
            (_, true) => {}
            (None, false) => self.say("INFO", &format!("not on the installer, to install from Agora: {}", names_of(&plan.queue)))?,
            (Some(file), false) => {
                let f = self.t(file);
                py::makedirs(&py::dirname(&f))?;
                let list = dict(vec![("user", s(&login)), ("components", Py::List(plan.queue.iter().map(|c| s(c)).collect()))]);
                write_text(&f, &list.dumps())?;
                // the user's first login removes it once everything is installed
                self.inside(&strings(&["chown", &login, file]), None, false)?;
                self.say(
                    "INFO",
                    &format!("not on the installer, installed at the first login (needs the internet): {}", names_of(&plan.queue)),
                )?;
            }
        }
        Ok(())
    }

    /// Encrypted system: the start-up image opens it with a key file, so the password is typed once (in GRUB).
    fn encryption_setup(&mut self) -> R<String> {
        let key = self.t("/crypto_keyfile.bin");
        let bytes = self.sys.random(4096);
        std::fs::write(&key, bytes).map_err(|e| py::oserror(&e, &key))?;
        py::chmod(&key, 0o000)?;
        Ok(key)
    }

    /// Whether the new system's start-up image is built with systemd (Arch's default since mkinitcpio 39) rather than busybox.
    fn systemd_initramfs(&mut self) -> R<bool> {
        let bytes = match std::fs::read(self.t("/etc/mkinitcpio.conf")) {
            Ok(b) => b,
            Err(_) => return Ok(true),
        };
        let text = py::universal(&py::decode(&bytes)?);
        let hooks = pycompat::splitlines(&text).into_iter().find(|l| pycompat::strip(l).starts_with("HOOKS=")).unwrap_or("");
        Ok(format!(" {} ", hooks.replace(['(', ')'], " ")).contains(" systemd "))
    }

    fn initramfs(&mut self, encrypted: bool) -> R<()> {
        py::makedirs(&self.t("/etc/mkinitcpio.conf.d"))?;
        if encrypted {
            let hook = if self.systemd_initramfs()? { "sd-encrypt" } else { "encrypt" };
            let conf = format!(
                "# Written by yggdrasil-install-system: the system disk is encrypted. \"{hook}\" opens it (before resume and filesystems)
# with the key file inside this image, so the password is asked once, by GRUB.
if [[ \" ${{HOOKS[*]}} \" != *\" encrypt \"* && \" ${{HOOKS[*]}} \" != *\" sd-encrypt \"* ]]; then
  _ly=()
  for _h in \"${{HOOKS[@]}}\"; do
    [[ $_h == resume || $_h == filesystems ]] && [[ \" ${{_ly[*]}} \" != *\" {hook} \"* ]] && _ly+=({hook})
    _ly+=(\"$_h\")
  done
  HOOKS=(\"${{_ly[@]}}\")
  unset _ly _h
fi
FILES+=(/crypto_keyfile.bin)
"
            );
            write_text(&self.t(&format!("/etc/mkinitcpio.conf.d/{}-encrypt.conf", self.os.id)), &conf)?;
        }
        self.inside(&strings(&["mkinitcpio", "-P"]), None, true)?;
        // the image holds the key: only root may read it
        let boot = self.t("/boot");
        for img in py::listdir(&boot)? {
            if img.starts_with("initramfs-") {
                py::chmod(&format!("{boot}/{img}"), 0o600)?;
            }
        }
        Ok(())
    }

    fn bootloader(&mut self, encrypted: bool, crypt_uuid: &str, fallback: bool) -> R<()> {
        let path = self.t("/etc/default/grub");
        let mut text = if py::exists(&path) { read_text(&path)? } else { String::new() };
        let mut options = self.os.kernel_options.clone();
        let mapper = self.os.mapper.clone();
        if encrypted {
            if self.systemd_initramfs()? {
                options.extend([
                    format!("rd.luks.name={crypt_uuid}={mapper}"),
                    format!("rd.luks.key={crypt_uuid}=/crypto_keyfile.bin"),
                    format!("root=/dev/mapper/{mapper}"),
                ]);
            } else {
                options.extend([
                    format!("cryptdevice=UUID={crypt_uuid}:{mapper}"),
                    "cryptkey=rootfs:/crypto_keyfile.bin".to_string(),
                    format!("root=/dev/mapper/{mapper}"),
                ]);
            }
        }
        let mut values = self.os.grub_defaults.clone();
        values.push(("GRUB_CMDLINE_LINUX_DEFAULT".into(), format!("'{}'", options.join(" "))));
        if encrypted {
            values.push(("GRUB_ENABLE_CRYPTODISK".into(), "y".into()));
        }
        for (k, v) in &values {
            text = grub_set(&text, k, &format!("{k}={v}"));
        }
        write_text(&path, &text)?;
        let id = format!("--bootloader-id={}", self.os.name.replace(' ', "_"));
        self.inside(&strings(&["grub-install", "--target=x86_64-efi", "--efi-directory=/boot/efi", &id, "--recheck"]), None, true)?;
        // a second copy at the firmware's fallback path, for computers that forget boot entries; not on an EFI partition that
        // was there before (Windows' own): its fallback is another system's
        if fallback {
            self.inside(&strings(&["grub-install", "--target=x86_64-efi", "--efi-directory=/boot/efi", "--removable"]), None, false)?;
        }
        // the plain menu first (the theme's installer keeps a copy of it to fall back on), then the OS's own look, if it has one
        self.inside(&strings(&["grub-mkconfig", "-o", "/boot/grub/grub.cfg"]), None, true)?;
        if let Some(theme) = self.os.boot_theme.clone().filter(|t| py::exists(&self.t(t))) {
            let r = self.inside(&[theme], None, false)?;
            if r.code != 0 {
                let said = if r.stderr.is_empty() { &r.stdout } else { &r.stderr };
                let name = self.os.name.clone();
                self.say(
                    "WARN",
                    &format!(
                        "the {name} menu theme could not be installed; the plain menu is used: {}",
                        tail_chars(pycompat::strip(said), 200)
                    ),
                )?;
            }
        }
        Ok(())
    }

    /// Unmounts the new system and closes its encryption. Once the live system's /dev was bound into it, everything under it is
    /// made a slave first, so that umount -R can never reach the live system's own mounts (whatever happened to the bind); when
    /// that cannot be done nothing is unmounted, and false is returned (a restart unmounts it).
    pub fn unmount_all(&mut self) -> R<bool> {
        let target = self.os.target.clone();
        if self.bound && self.run(&strings(&["mount", "--make-rslave", &target]), None, false)?.code != 0 {
            return Ok(false);
        }
        if self.run(&strings(&["umount", "-R", &target]), None, false)?.code != 0 {
            self.run(&strings(&["umount", "-R", "-l", &target]), None, false)?;
        }
        let mapper = self.os.mapper.clone();
        self.run(&strings(&["cryptsetup", "close", &mapper]), None, false)?;
        Ok(true)
    }

    pub fn install(&mut self, p: &Py) -> R<()> {
        self.step(1)?;
        self.preflight(p)?;
        self.step(2)?;
        let disk = p.at("disk").as_str().unwrap_or("").to_string();
        let (mut plan, root_part): (Vec<Use>, Py) = if p.at("mode").as_str() == Some("erase") {
            self.say("INFO", &format!("installing onto {disk}; everything on it is erased"))?;
            let (esp, root_part) = self.partition(&disk)?;
            (vec![(s(&esp), "/boot/efi".into(), Some("vfat".into())), (s(&root_part), "/".into(), Some("btrfs".into()))], s(&root_part))
        } else {
            self.say("INFO", &format!("installing onto the partitions chosen on {disk}"))?;
            let made = self.manual_partition(p)?;
            let mut plan = vec![];
            for u in iterate(index(p, "use")?)? {
                let partname = index(&u, "part")?.clone();
                let dev = made.iter().find(|(k, _)| Some(k.as_str()) == partname.as_str()).map_or(partname.clone(), |(_, v)| v.clone());
                let mount = index(&u, "mount")?.as_str().unwrap_or("").to_string();
                let fmt = index(&u, "format")?.as_str().map(str::to_string);
                plan.push((dev, mount, fmt));
            }
            let root_part = plan.iter().find(|u| u.1 == "/").map(|u| u.0.clone()).unwrap_or(Py::None);
            (plan, root_part)
        };
        self.step(3)?;
        let encrypted = p.at("encrypt").truthy();
        let password = p.at("encrypt_password").as_str().unwrap_or("").to_string();
        let mut root = root_part.clone();
        if encrypted {
            root = s(&self.encrypt(&arg(&root_part)?, &password)?);
            for u in plan.iter_mut() {
                if u.1 == "/" {
                    u.0 = root.clone();
                }
            }
        } else {
            self.say("INFO", "not encrypted (as chosen)")?;
        }
        self.step(4)?;
        let fstab = self.format_and_mount(&plan)?;
        self.step(5)?;
        self.copy_system()?;
        self.step(6)?;
        self.bind_mounts()?;
        self.write_fstab(&fstab)?;
        self.set_up(p)?;
        self.set_up_browsers(p)?;
        self.set_up_extras(p)?;
        if encrypted {
            let key = self.encryption_setup()?;
            self.run(
                &strings(&["cryptsetup", "luksAddKey", "--batch-mode", "--key-file=-", &arg(&root_part)?, &key]),
                Some(&password),
                true,
            )?;
        }
        self.step(7)?;
        self.initramfs(encrypted)?;
        self.step(8)?;
        // the firmware's fallback copy only on an EFI partition made for this system (erased or formatted now)
        let esp_formatted = plan.iter().find(|u| u.1 == "/boot/efi").is_some_and(|u| u.2.is_some());
        let crypt_uuid = self.uuid_of(&arg(&root_part)?)?;
        self.uuid_of(&arg(&root)?)?;
        self.bootloader(encrypted, &crypt_uuid, esp_formatted)?;
        self.step(9)?;
        if let Some(finish) = self.os.finish.clone().filter(|f| py::exists(&self.t(f))) {
            let r = self.inside(&[finish], None, false)?;
            for line in pycompat::splitlines(&r.stdout) {
                if line.contains("WARNING") {
                    let note = line.split_once("WARNING:").map_or(line, |(_, after)| after);
                    self.say("WARN", pycompat::strip(note))?;
                }
            }
            if r.code != 0 {
                let said = if r.stdout.is_empty() { &r.stderr } else { &r.stdout };
                return stop(format!("the last checks of the new system failed: {}", tail_chars(pycompat::strip(said), 400)));
            }
        }
        if !self.unmount_all()? {
            let target = self.os.target.clone();
            self.say("WARN", &format!("the new system stays mounted at {target} until the computer restarts"))?;
        }
        self.say("DONE", "")
    }
}

/// A manual plan against the disk as it is now: the partitions it names exist and are not in use, the new ones fit in free
/// space, and / and the EFI partition are big enough. An error, or None.
pub fn manual_fits(p: &Py, d: &Disk) -> R<Option<String>> {
    for q in &d.partitions {
        if matches!(q.path, Py::List(_) | Py::Dict(_)) {
            return crash(format!("unhashable type: '{}'", q.path.type_name()));
        }
    }
    // {q["number"]: q}: a number given twice keeps the last partition
    let by_num = |n: i128| d.partitions.iter().rev().find(|q| q.number.value == n);
    let new_table = p.at("new_table").truthy();
    let mut deleted: Vec<Int> = if new_table {
        d.partitions.iter().map(|q| q.number.clone()).collect()
    } else {
        p.at("delete").as_list().unwrap_or(&[]).iter().filter_map(|n| if let Py::Int(i) = n { Some(i.clone()) } else { None }).collect()
    };
    deleted.sort_by(|a, b| (a.value, a.text.len(), &a.text).cmp(&(b.value, b.text.len(), &b.text)));
    deleted.dedup();
    let is_deleted = |n: i128| deleted.iter().any(|x| x.value == n && x.value.abs() < crate::pyjson::CLAMP);
    if !new_table && !d.table.truthy() {
        return Ok(Some("the disk has no partition table: start with a new one".into()));
    }
    for n in &deleted {
        match by_num(n.value).filter(|_| n.value.abs() < crate::pyjson::CLAMP) {
            None => return Ok(Some(format!("there is no partition {} to delete", n.text))),
            Some(q) if q.mounted => return Ok(Some(format!("{} is in use (mounted)", q.path.py_str()))),
            Some(_) => {}
        }
    }
    let kept: Vec<(i128, i128)> =
        d.partitions.iter().filter(|q| !is_deleted(q.number.value)).map(|q| (q.start.value, q.size.value)).collect();
    let gaps = free_space(&kept, MIB, d.size.value - MIB);
    let mut taken: Vec<(i128, i128)> = vec![];
    let creates = p.at("create").as_list().unwrap_or(&[]);
    for c in creates {
        let (start, size) = (c.at("start").whole().unwrap_or(0), c.at("size").whole().unwrap_or(0));
        let inside_gap = gaps.iter().any(|g| g.0 <= start && start + size <= g.0 + g.1);
        let overlaps = taken.iter().any(|t| start < t.0 + t.1 && t.0 < start + size);
        if !inside_gap || overlaps || start.rem_euclid(MIB) != 0 {
            return Ok(Some("a new partition does not fit in the free space".into()));
        }
        taken.push((start, size));
    }
    let mut sizes: Vec<(String, i128)> =
        creates.iter().map(|c| (format!("new:{}", c.at("id").as_str().unwrap_or("")), c.at("size").whole().unwrap_or(0))).collect();
    let set_size = |sizes: &mut Vec<(String, i128)>, k: &str, v: i128| match sizes.iter_mut().find(|(key, _)| key == k) {
        Some(slot) => slot.1 = v,
        None => sizes.push((k.to_string(), v)),
    };
    for u in p.at("use").as_list().unwrap_or(&[]) {
        let partname = index(u, "part")?.as_str().unwrap_or("").to_string();
        let mount = index(u, "mount")?.as_str().unwrap_or("").to_string();
        if !partname.starts_with("new:") {
            let q = d.partitions.iter().rev().find(|q| q.path.as_str() == Some(partname.as_str()));
            let Some(q) = q.filter(|q| !is_deleted(q.number.value)) else {
                return Ok(Some(format!("{partname} is not a partition on {} (or it is being deleted)", d.path.py_str())));
            };
            if q.mounted {
                return Ok(Some(format!("{partname} is in use (mounted)")));
            }
            let keep = *index(u, "format")? == Py::None;
            if keep && mount == "/boot/efi" && q.fstype.as_str() != Some("vfat") {
                return Ok(Some(format!("{partname} has no FAT file system to use as the EFI partition as it is: format it")));
            }
            if keep && mount != "/boot/efi" && !q.fstype.truthy() {
                return Ok(Some(format!("{partname} has no file system to keep: format it")));
            }
            set_size(&mut sizes, &partname, q.size.value);
        }
        let size = sizes.iter().find(|(k, _)| *k == partname).map_or(0, |(_, v)| *v);
        if mount == "/" && size < MIN_ROOT {
            return Ok(Some(format!("/ needs at least {} GB", MIN_ROOT / GIB)));
        }
        if mount == "/boot/efi" && size < MIN_ESP {
            return Ok(Some(format!("the EFI partition needs at least {} MB", MIN_ESP / MIB)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grub_settings_replace_the_first_line_or_are_added() {
        let text = "GRUB_DEFAULT=saved\n#GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX_DEFAULT=\"loglevel=3\"\n";
        let t = grub_set(text, "GRUB_TIMEOUT", "GRUB_TIMEOUT=2");
        assert_eq!(t, "GRUB_DEFAULT=saved\nGRUB_TIMEOUT=2\nGRUB_CMDLINE_LINUX_DEFAULT=\"loglevel=3\"\n");
        assert_eq!(
            grub_set(&t, "GRUB_DEFAULT", "GRUB_DEFAULT=0"),
            "GRUB_DEFAULT=0\nGRUB_TIMEOUT=2\nGRUB_CMDLINE_LINUX_DEFAULT=\"loglevel=3\"\n"
        );
        assert_eq!(grub_set("X=1\n", "GRUB_X", "GRUB_X=y"), "X=1\nGRUB_X=y\n");
        assert_eq!(grub_set("X=1", "GRUB_X", "GRUB_X=y"), "X=1GRUB_X=y\n", "as the script did: no line break added first");
        assert_eq!(grub_set("#\n  GRUB_X=1\nY\n", "GRUB_X", "GRUB_X=2"), "GRUB_X=2\nY\n", "\\s* crosses line breaks");
        assert_eq!(grub_set("GRUB_DEFAULTS=1\n", "GRUB_DEFAULT", "GRUB_DEFAULT=0"), "GRUB_DEFAULTS=1\nGRUB_DEFAULT=0\n");
    }

    #[test]
    fn locale_gen_lines() {
        let text = "# en_US.UTF-8 UTF-8\n#en_US ISO-8859-1\nde_DE.UTF-8 UTF-8\n";
        assert!(has_locale_line(text, "de_DE.UTF-8"));
        assert!(!has_locale_line(text, "en_US.UTF-8"));
        assert!(!has_locale_line(text, "de_DE"));
        assert_eq!(uncomment_locale(text, "en_US.UTF-8"), "en_US.UTF-8 UTF-8\n#en_US ISO-8859-1\nde_DE.UTF-8 UTF-8\n");
        assert_eq!(uncomment_locale("#\nen_GB.UTF-8 UTF-8\n", "en_GB.UTF-8"), "en_GB.UTF-8 UTF-8\n", "\\s* crosses line breaks");
        assert_eq!(uncomment_locale("#a x\n#a\n#a y\n", "a"), "a x\na\na y\n", "as re.sub does it");
    }

    #[test]
    fn python_division() {
        assert_eq!(floor_div(7, 2).unwrap(), 3);
        assert_eq!(floor_div(-7, 2).unwrap(), -4);
        assert_eq!(floor_div(7, -2).unwrap(), -4);
        assert!(floor_div(1, 0).is_err());
        assert_eq!(drop_last("vivaldi-stable.desktop", 8), "vivaldi-stable");
        assert_eq!(drop_last("x", 8), "");
    }
}
