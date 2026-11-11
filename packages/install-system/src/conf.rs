//! What belongs to the operating system: its installer integration file (the first of kind "installer", as the window picks
//! it) and /etc/os-release, read as the script read them, and the values they give (configure()).

use std::collections::HashSet;

use crate::py::{decode, universal, Fail, R};

pub const INTEGRATION_DIRS: [&str; 2] = [
    "/usr/share/yggdrasil/integrations",
    "/etc/yggdrasil/integrations",
];
pub const GIB: i128 = 1024 * 1024 * 1024;

/// key = value pairs in the order they first came (a dict: a key given again keeps its place and takes the new value).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conf(pub Vec<(String, String)>);

impl Conf {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    }

    /// conf.get(k) or "" (the files' values are text; an empty one counts as none)
    pub fn text(&self, k: &str) -> &str {
        self.get(k).unwrap_or("")
    }

    pub fn set(&mut self, k: &str, v: &str) {
        match self.0.iter_mut().find(|(key, _)| key == k) {
            Some(slot) => slot.1 = v.to_string(),
            None => self.0.push((k.to_string(), v.to_string())),
        }
    }

    pub fn from_pairs(pairs: &[(&str, &str)]) -> Conf {
        let mut c = Conf::default();
        for (k, v) in pairs {
            c.set(k, v);
        }
        c
    }
}

/// key = value lines; '#' starts a comment anywhere (as Yggdrasil's integration files are read).
pub fn parse_conf(text: &str) -> Conf {
    let mut values = Conf::default();
    for line in pycompat::splitlines(text) {
        let line = pycompat::strip(line.split('#').next().unwrap_or(""));
        if let Some((k, v)) = line.split_once('=') {
            values.set(pycompat::strip(k), pycompat::strip(v));
        }
    }
    values
}

/// A file's text as open(path).read() gave it, or None where the script caught the error (OSError, or not UTF-8).
fn read_conf_text(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    decode(&bytes).ok().map(|t| universal(&t))
}

/// The installer integration file's keys: of the files in the folders (a later folder's file of the same name replaces an
/// earlier one), the first by name whose kind is "installer". Empty when there is none.
pub fn installer_integration(dirs: &[&str]) -> Conf {
    let mut found: Vec<(String, Conf)> = vec![];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name();
            let n = name.to_string_lossy();
            let Some(stem) = n.strip_suffix(".conf") else {
                continue;
            };
            let Some(text) = read_conf_text(&std::path::Path::new(d).join(&name)) else {
                continue;
            };
            let values = parse_conf(&text);
            if !values.text("kind").is_empty() {
                match found.iter_mut().find(|(s, _)| s == stem) {
                    Some(slot) => slot.1 = values,
                    None => found.push((stem.to_string(), values)),
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, values) in found {
        if values.text("kind") == "installer" {
            return Conf(
                values
                    .0
                    .into_iter()
                    .filter(|(_, v)| !v.is_empty())
                    .collect(),
            );
        }
    }
    Conf::default()
}

/// /etc/os-release's keys, their quotes taken off.
pub fn os_release(path: &str) -> Conf {
    match read_conf_text(std::path::Path::new(path)) {
        Some(text) => Conf(
            parse_conf(&text)
                .0
                .into_iter()
                .map(|(k, v)| (k, v.trim_matches(|c| c == '"' || c == '\'').to_string()))
                .collect(),
        ),
        None => Conf::default(),
    }
}

/// An absolute path without '..', or None.
pub fn absolute(v: Option<&str>) -> Option<String> {
    v.filter(|v| !v.is_empty() && v.starts_with('/') && !v.split('/').any(|p| p == ".."))
        .map(str::to_string)
}

/// The operating system's values. The defaults (Os::default) are what a system that says nothing gets.
#[derive(Clone, Debug, PartialEq)]
pub struct Os {
    pub name: String,
    pub id: String,
    pub sfs: String,
    pub target: String,
    pub mapper: String,
    pub cleanup: Option<String>,
    pub finish: Option<String>,
    pub boot_theme: Option<String>,
    pub kernel_options: Vec<String>,
    pub groups: Vec<String>,
    pub reserved_extra: HashSet<String>,
    pub min_bytes: i128,
    pub browsers_file: Option<String>,
    pub extras_file: Option<String>,
    pub components_file: Option<String>,
    pub first_login_file: Option<String>,
    /// What GRUB is set up with, in order: a short menu and no submenu or recovery entries.
    pub grub_defaults: Vec<(String, String)>,
}

impl Default for Os {
    fn default() -> Os {
        Os {
            name: "Linux".into(),
            id: "linux".into(),
            sfs: "/run/archiso/bootmnt/linux/x86_64/airootfs.sfs".into(),
            target: "/mnt/linux-install".into(),
            mapper: "linux-root".into(),
            cleanup: None,
            finish: None,
            boot_theme: None,
            kernel_options: vec!["quiet".into()],
            groups: [
                "users", "lp", "video", "network", "storage", "wheel", "audio", "kvm",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            reserved_extra: HashSet::new(),
            min_bytes: 30 * GIB,
            browsers_file: None,
            extras_file: None,
            components_file: None,
            first_login_file: None,
            grub_defaults: [
                ("GRUB_TIMEOUT", "2"),
                // the first entry, not the last one chosen: remembering means GRUB writing its grubenv file, which it cannot do
                // on btrfs ("sparse file not allowed", then a "press any key" stop at every start)
                ("GRUB_DEFAULT", "0"),
                ("GRUB_SAVEDEFAULT", "false"),
                ("GRUB_DISABLE_SUBMENU", "true"),
                ("GRUB_TERMINAL_OUTPUT", "console"),
                ("GRUB_DISABLE_RECOVERY", "true"),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        }
    }
}

/// re.sub(r"\s+OS$", "", name): " OS" (after any whitespace) taken off the end.
fn without_os(name: &str) -> String {
    if let Some(before) = name.strip_suffix("OS") {
        let trimmed = before.trim_end_matches(pycompat::isspace);
        if trimmed.len() < before.len() {
            return trimmed.to_string();
        }
    }
    name.to_string()
}

/// The OS's values from its integration file (`conf`) and os-release (`release`); `env_sfs` and `env_target` are
/// YGGDRASIL_INSTALL_SFS and YGGDRASIL_INSTALL_TARGET. A Crash where the script stopped with a traceback.
pub fn configure(
    conf: &Conf,
    release: &Conf,
    env_sfs: Option<&str>,
    env_target: Option<&str>,
) -> R<Os> {
    let mut os = Os::default();
    let name = [
        conf.text("name").to_string(),
        without_os(release.text("NAME")),
    ]
    .into_iter()
    .find(|n| !n.is_empty())
    .unwrap_or_else(|| "Linux".into());
    let kept: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || " ._-".contains(*c))
        .collect();
    os.name = Some(pycompat::strip(&kept).to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Linux".into());
    let id_source = if release.text("ID").is_empty() {
        os.name.clone()
    } else {
        release.text("ID").to_string()
    };
    let id: String = id_source
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '-')
        .collect();
    os.id = if id.is_empty() { "linux".into() } else { id };
    os.sfs = env_sfs
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| absolute(conf.get("medium")))
        .unwrap_or_else(|| format!("/run/archiso/bootmnt/{}/x86_64/airootfs.sfs", os.id));
    os.target = env_target
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("/mnt/{}-install", os.id));
    os.mapper = format!("{}-root", os.id);
    os.cleanup = absolute(conf.get("cleanup"));
    os.finish = absolute(conf.get("finish"));
    os.boot_theme = absolute(conf.get("boot_menu_theme"));
    if !conf.text("kernel_options").is_empty() {
        os.kernel_options = pycompat::split(conf.text("kernel_options"))
            .into_iter()
            .map(str::to_string)
            .collect();
    }
    if !conf.text("groups").is_empty() {
        os.groups = pycompat::split(conf.text("groups"))
            .into_iter()
            .map(str::to_string)
            .collect();
    }
    os.reserved_extra = pycompat::split(conf.text("reserved_logins"))
        .into_iter()
        .map(str::to_string)
        .collect();
    let min = conf.text("min_size_gb");
    if pycompat::isdigit(min) {
        let gb = pycompat::int(min).ok_or_else(|| {
            Fail::Crash(format!(
                "invalid literal for int() with base 10: {}",
                pycompat::repr(min)
            ))
        })?;
        os.min_bytes = gb.saturating_mul(GIB);
    }
    os.browsers_file = absolute(conf.get("browsers"));
    os.extras_file = absolute(conf.get("extras"));
    os.components_file = absolute(conf.get("components"));
    os.first_login_file = absolute(conf.get("first_login"));
    os.grub_defaults
        .push(("GRUB_DISTRIBUTOR".into(), os.name.clone()));
    Ok(os)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[(&str, &str)]) -> tempdir::Dir {
        let d = tempdir::Dir::new();
        for (name, text) in files {
            std::fs::write(d.path().join(name), text).unwrap();
        }
        d
    }

    #[test]
    fn the_first_installer_integration_by_name_as_the_window_picks_it() {
        let share = folder(&[
            ("a-branding.conf", "kind = branding\nname = X\n"),
            ("b-installer.conf", "kind = installer\ncommand = /usr/bin/yggdrasil-install-system\nname = Bee # the OS\nempty =\n"),
            ("c-installer.conf", "kind = installer\nname = Cee\n"),
            ("notes.txt", "kind = installer\n"),
        ]);
        let s = share.path().to_str().unwrap();
        let conf = installer_integration(&[s]);
        assert_eq!(conf.get("name"), Some("Bee"), "a comment is cut off");
        assert_eq!(conf.get("empty"), None);
        let etc = folder(&[("b-installer.conf", "kind = installer\nname = Admin\n")]);
        assert_eq!(
            installer_integration(&[s, etc.path().to_str().unwrap()]).get("name"),
            Some("Admin"),
            "the administrator's file of the same name wins"
        );
        let empty = folder(&[]);
        assert_eq!(
            installer_integration(&[empty.path().to_str().unwrap()]),
            Conf::default()
        );
        let broken = folder(&[
            ("a-installer.conf", "kind = installer\nname = \u{0}"),
            ("b-installer.conf", "kind = installer\nname = Bee\n"),
        ]);
        std::fs::write(
            broken.path().join("a-installer.conf"),
            b"kind = installer\nname = \xff\n",
        )
        .unwrap();
        assert_eq!(
            installer_integration(&[broken.path().to_str().unwrap()]).get("name"),
            Some("Bee"),
            "a file that is not UTF-8 is skipped"
        );
    }

    #[test]
    fn names_paths_and_labels_follow_the_os() {
        let d = folder(&[("os-release", "NAME=\"LykanOS\"\nID=lykanos\n")]);
        let release = os_release(d.path().join("os-release").to_str().unwrap());
        let conf = Conf::from_pairs(&[
            ("cleanup", "/usr/local/bin/lykanos-install-cleanup"),
            ("finish", "relative/finish"),
            ("boot_menu_theme", "/a/../b"),
            ("kernel_options", "quiet splash"),
            ("groups", "users wheel"),
            ("min_size_gb", "40"),
            ("first_login", "/var/lib/n/first.json"),
        ]);
        let os = configure(&conf, &release, None, None).unwrap();
        assert_eq!((os.name.as_str(), os.id.as_str()), ("LykanOS", "lykanos"));
        assert_eq!(os.sfs, "/run/archiso/bootmnt/lykanos/x86_64/airootfs.sfs");
        assert_eq!(
            (os.target.as_str(), os.mapper.as_str()),
            ("/mnt/lykanos-install", "lykanos-root")
        );
        assert_eq!(
            os.cleanup.as_deref(),
            Some("/usr/local/bin/lykanos-install-cleanup")
        );
        assert_eq!(os.finish, None, "only absolute paths");
        assert_eq!(os.boot_theme, None, "no '..'");
        assert_eq!(os.kernel_options, ["quiet", "splash"]);
        assert_eq!(os.groups, ["users", "wheel"]);
        assert_eq!(os.min_bytes, 40 * GIB);
        assert_eq!(
            os.first_login_file.as_deref(),
            Some("/var/lib/n/first.json")
        );
        assert_eq!(
            os.grub_defaults.last().unwrap(),
            &("GRUB_DISTRIBUTOR".to_string(), "LykanOS".to_string())
        );
        let os = configure(
            &Conf::default(),
            &Conf::from_pairs(&[("NAME", "Labyrinth OS"), ("ID", "labyrinth")]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(os.name, "Labyrinth", "os-release's name without the ' OS'");
        let os = configure(
            &Conf::from_pairs(&[
                ("name", "Labyrinth"),
                ("medium", "/run/archiso/bootmnt/lab/x86_64/airootfs.sfs"),
            ]),
            &Conf::from_pairs(&[("NAME", "Labyrinth OS"), ("ID", "labyrinth")]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            (os.name.as_str(), os.sfs.as_str()),
            ("Labyrinth", "/run/archiso/bootmnt/lab/x86_64/airootfs.sfs")
        );
        let os = configure(
            &Conf::default(),
            &Conf::default(),
            Some("/x.sfs"),
            Some("/t"),
        )
        .unwrap();
        assert_eq!((os.sfs.as_str(), os.target.as_str()), ("/x.sfs", "/t"));
    }

    #[test]
    fn a_system_that_says_nothing() {
        let os = configure(&Conf::default(), &Conf::default(), None, None).unwrap();
        assert_eq!((os.name.as_str(), os.id.as_str()), ("Linux", "linux"));
        assert_eq!(os.cleanup, None);
        assert_eq!(os.browsers_file, None);
        assert_eq!(
            configure(
                &Conf::from_pairs(&[("min_size_gb", "\u{b2}")]),
                &Conf::default(),
                None,
                None
            ),
            Err(Fail::Crash(
                "invalid literal for int() with base 10: '\u{b2}'".into()
            ))
        );
        assert_eq!(
            configure(
                &Conf::from_pairs(&[("min_size_gb", "\u{663}\u{660}")]),
                &Conf::default(),
                None,
                None
            )
            .unwrap()
            .min_bytes,
            30 * GIB,
            "Arabic-Indic 30"
        );
        assert_eq!(without_os("A \u{85}OS"), "A");
        assert_eq!(without_os("A OS OS"), "A OS");
        assert_eq!(without_os("LykanOS"), "LykanOS");
    }
}

/// A scratch folder for the tests, removed afterwards.
#[cfg(test)]
pub mod tempdir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new() -> Dir {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "install-system-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Default for Dir {
        fn default() -> Dir {
            Dir::new()
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
