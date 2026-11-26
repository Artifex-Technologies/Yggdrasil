//! Cloud drives, shared by Settings > Cloud Storage (which adds, mounts and removes them) and Minos (which lists
//! them under Cloud Storage in its sidebar).
//!
//! Linux has no single cloud API, so a drive is one of two things:
//!
//! - an **rclone remote** mounted as a folder (`~/Cloud/NAME` unless chosen otherwise) by `rclone mount --vfs-cache-mode full`,
//!   run by a systemd *user* unit, so the drive stays mounted when Minos and Settings are closed. The unit file
//!   (`~/.config/systemd/user/yggdrasil-cloud-NAME.service`) is written only when the person presses Mount or turns on
//!   "Mount at sign-in". Proton Drive's remote `proton` uses the unit that ships with Minos (`proton-drive.service`, mounted at
//!   `~/ProtonDrive`), so Minos's own Proton Drive sign-in and this list agree;
//! - a **folder** a sync app keeps (the Proton Drive or Nextcloud desktop app's folder): listed, never mounted.
//!
//! The sign-in is always the provider's own, through rclone (`rclone config create NAME TYPE`, which opens the browser); no
//! account of ours is involved. Removing a drive removes its rclone remote and its unit, never a file in the cloud.
//!
//! The list is `~/.config/yggdrasil/cloud-drives.conf` (`[remote NAME]` / `[folder NAME]` sections of `key = value` lines),
//! in the folder Minos already watches, so its sidebar follows Settings without a restart. Every command goes through [`Sys`],
//! so the tests run against a stand-in and never start rclone or systemctl.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The list's file name, in Yggdrasil's settings folder.
pub const LIST_FILE: &str = "cloud-drives.conf";
/// Proton Drive's remote: the name Minos's own sign-in and its shipped mount unit use.
pub const PROTON_REMOTE: &str = "proton";
/// rclone's type for Proton Drive.
pub const PROTON_TYPE: &str = "protondrive";
/// The user unit that ships with Minos and mounts the `proton` remote at ~/ProtonDrive.
pub const PROTON_UNIT: &str = "proton-drive.service";
/// The install command shown when rclone is missing (the Arch family's; nothing is installed for the person).
pub const INSTALL_HINT: &str = "sudo pacman -S rclone";

/// How a provider's sign-in works.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SignIn {
    /// `rclone config create NAME TYPE` opens the provider's page in the browser; its output is shown while it waits.
    Browser,
    /// The provider wants a user name, a password or keys: rclone's own questions (`rclone config create NAME TYPE --all`) in a
    /// terminal, where the password is typed into rclone and never passes through a command line.
    Terminal,
}

/// A provider that can be added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Provider {
    /// rclone's type.
    pub kind: &'static str,
    pub label: &'static str,
    /// The name a new drive of this kind gets (a number is added when it is taken).
    pub default_name: &'static str,
    pub sign_in: SignIn,
    pub about: &'static str,
}

pub const PROVIDERS: [Provider; 7] = [
    Provider {
        kind: PROTON_TYPE,
        label: "Proton Drive",
        default_name: PROTON_REMOTE,
        sign_in: SignIn::Terminal,
        about: "Your Proton user name, password and two-factor code, typed into rclone's own questions in a terminal",
    },
    Provider {
        kind: "drive",
        label: "Google Drive",
        default_name: "GoogleDrive",
        sign_in: SignIn::Browser,
        about: "Google's sign-in page opens in your browser",
    },
    Provider {
        kind: "onedrive",
        label: "OneDrive",
        default_name: "OneDrive",
        sign_in: SignIn::Browser,
        about: "Microsoft's sign-in page opens in your browser",
    },
    Provider {
        kind: "dropbox",
        label: "Dropbox",
        default_name: "Dropbox",
        sign_in: SignIn::Browser,
        about: "Dropbox's sign-in page opens in your browser",
    },
    Provider {
        kind: "box",
        label: "Box",
        default_name: "Box",
        sign_in: SignIn::Browser,
        about: "Box's sign-in page opens in your browser",
    },
    Provider {
        kind: "webdav",
        label: "WebDAV / Nextcloud",
        default_name: "WebDAV",
        sign_in: SignIn::Terminal,
        about: "The server's address, your user name and an app password, typed into rclone's own questions in a terminal",
    },
    Provider {
        kind: "s3",
        label: "S3",
        default_name: "S3",
        sign_in: SignIn::Terminal,
        about: "The provider, region and access keys, typed into rclone's own questions in a terminal",
    },
];

/// The provider of an rclone type, when it is one of those that can be added here.
pub fn provider(kind: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.kind == kind)
}

/// What to call an rclone type: the provider's name, or the type itself for one not in the list.
pub fn provider_label(kind: &str) -> &str {
    provider(kind).map(|p| p.label).unwrap_or(kind)
}

// ------------------------------------------------------------------------------------------------ the list

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An rclone remote, mounted at `mount`.
    Remote { provider: String, mount: PathBuf },
    /// A folder a sync app keeps.
    Folder { path: PathBuf },
}

/// One drive. For a remote, `name` is the rclone remote's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    pub name: String,
    pub kind: Kind,
}

impl Drive {
    /// Where its files are: the mount point or the folder.
    pub fn path(&self) -> &Path {
        match &self.kind {
            Kind::Remote { mount, .. } => mount,
            Kind::Folder { path } => path,
        }
    }

    /// rclone's type, for a remote.
    pub fn provider(&self) -> Option<&str> {
        match &self.kind {
            Kind::Remote { provider, .. } => Some(provider),
            Kind::Folder { .. } => None,
        }
    }

    /// Proton Drive's remote, mounted by the unit that ships with Minos.
    pub fn is_proton(&self) -> bool {
        self.name == PROTON_REMOTE && self.provider() == Some(PROTON_TYPE)
    }

    /// The systemd user unit that mounts it (None for a folder).
    pub fn unit(&self) -> Option<String> {
        match self.kind {
            Kind::Folder { .. } => None,
            _ if self.is_proton() => Some(PROTON_UNIT.into()),
            _ => Some(unit_name(&self.name)),
        }
    }

    /// The name shown: Proton Drive by its own name, the rest as they were named.
    pub fn label(&self) -> &str {
        if self.is_proton() {
            "Proton Drive"
        } else {
            &self.name
        }
    }
}

/// Proton Drive's remote as a drive (where the shipped unit mounts it).
pub fn proton_drive(home: &Path) -> Drive {
    Drive { name: PROTON_REMOTE.into(), kind: Kind::Remote { provider: PROTON_TYPE.into(), mount: home.join("ProtonDrive") } }
}

/// The unit for a remote: `yggdrasil-cloud-NAME.service`.
pub fn unit_name(name: &str) -> String {
    format!("yggdrasil-cloud-{name}.service")
}

/// A name a new remote may have here: 1 to 32 letters, digits, `-` and `_`, starting with a letter or digit. (rclone allows more,
/// but this one also becomes a unit name and a folder name, so it is kept plain.)
pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

/// A folder a drive may be mounted at or listed from: absolute, not `/`, no `..`, and none of the characters a unit file or the
/// list would have to escape (`"`, `\`, control characters).
pub fn valid_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    p.is_absolute()
        && p != Path::new("/")
        && !p.components().any(|c| c == std::path::Component::ParentDir)
        && !s.chars().any(|c| c.is_control() || c == '"' || c == '\\')
}

/// Where a new drive is mounted unless another folder is chosen: ~/Cloud/NAME.
pub fn default_mount(home: &Path, name: &str) -> PathBuf {
    home.join("Cloud").join(name)
}

/// The name a new drive of `p` gets: its default, or with 2, 3, ... after it when `taken` has that already.
pub fn default_name(p: &Provider, taken: &[String]) -> String {
    let free = |n: &str| !taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    if free(p.default_name) {
        return p.default_name.to_string();
    }
    (2..).map(|i| format!("{}{i}", p.default_name)).find(|n| free(n)).unwrap_or_default()
}

/// Reads the list. Tolerant: unknown keys and sections are skipped, a drive missing a field or with a name or path that is not
/// allowed is left out, and a second drive with a name already listed is ignored.
pub fn parse_list(text: &str) -> Vec<Drive> {
    let mut out: Vec<Drive> = Vec::new();
    let mut cur: Option<(bool, String, BTreeMap<String, String>)> = None;
    let finish = |cur: Option<(bool, String, BTreeMap<String, String>)>, out: &mut Vec<Drive>| {
        let Some((remote, name, keys)) = cur else { return };
        let path = |k: &str| keys.get(k).map(PathBuf::from).filter(|p| valid_path(p));
        let kind = if remote {
            if !valid_name(&name) {
                return;
            }
            match (keys.get("type").filter(|t| valid_name(t)), path("mount")) {
                (Some(t), Some(mount)) => Kind::Remote { provider: t.clone(), mount },
                _ => return,
            }
        } else {
            match path("path") {
                Some(path) if !name.is_empty() && !name.chars().any(char::is_control) => Kind::Folder { path },
                _ => return,
            }
        };
        if !out.iter().any(|d| d.name == name) {
            out.push(Drive { name, kind });
        }
    };
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(head) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            finish(cur.take(), &mut out);
            cur = match head.split_once(' ') {
                Some(("remote", n)) => Some((true, n.trim().to_string(), BTreeMap::new())),
                Some(("folder", n)) => Some((false, n.trim().to_string(), BTreeMap::new())),
                _ => None,
            };
            continue;
        }
        if let (Some((_, _, keys)), Some((k, v))) = (cur.as_mut(), line.split_once('=')) {
            keys.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    finish(cur, &mut out);
    out
}

/// The list as its file holds it.
pub fn write_list(drives: &[Drive]) -> String {
    let mut s = String::from(
        "# Cloud drives: written by Settings > Cloud Storage; Minos lists them under Cloud Storage.\n\
         # [remote NAME] is an rclone remote mounted at `mount`; [folder NAME] is a folder a sync app keeps.\n",
    );
    for d in drives {
        s.push('\n');
        match &d.kind {
            Kind::Remote { provider, mount } => {
                s.push_str(&format!("[remote {}]\ntype = {provider}\nmount = {}\n", d.name, mount.display()))
            }
            Kind::Folder { path } => s.push_str(&format!("[folder {}]\npath = {}\n", d.name, path.display())),
        }
    }
    s
}

/// Where the list and the units live, and the home folder new mounts go under.
#[derive(Clone, Debug)]
pub struct Paths {
    pub list: PathBuf,
    /// systemd's folder for the user's own units.
    pub units: PathBuf,
    pub home: PathBuf,
}

impl Paths {
    /// This account's: ~/.config/yggdrasil/cloud-drives.conf and ~/.config/systemd/user (XDG_CONFIG_HOME respected).
    pub fn user() -> Paths {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        let config =
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".config"));
        Paths { list: yggdrasil_conf::dirs::config().join(LIST_FILE), units: config.join("systemd/user"), home }
    }
}

/// The list as saved (empty when there is none).
pub fn load(paths: &Paths) -> Vec<Drive> {
    std::fs::read_to_string(&paths.list).map(|t| parse_list(&t)).unwrap_or_default()
}

/// Saves the list (whole, through a temporary file, so a reader never sees half of it).
pub fn save(paths: &Paths, drives: &[Drive]) -> std::io::Result<()> {
    write_file(&paths.list, &write_list(drives))
}

fn write_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

// ------------------------------------------------------------------------------------------------ rclone's remotes

/// A remote in rclone's config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    /// rclone's type (`drive`, `onedrive`, ...).
    pub provider: String,
    /// False for a provider that signs in through the browser and has no token yet (a sign-in that was stopped).
    pub signed_in: bool,
}

/// rclone types that sign in through the browser and keep a token.
fn needs_token(kind: &str) -> bool {
    matches!(kind, "drive" | "onedrive" | "dropbox" | "box" | "pcloud" | "gphotos" | "yandex" | "hidrive" | "zoho" | "jottacloud")
}

/// `rclone config dump`: a JSON object of remotes, each with its `type` and settings. Only the names, types and whether a token
/// is there are kept; the settings (tokens, keys) are dropped as soon as they are read.
pub fn parse_config_dump(json: &str) -> Result<Vec<Remote>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("rclone's config could not be read: {e}"))?;
    let obj = v.as_object().ok_or("rclone's config is not a list of remotes")?;
    let mut out: Vec<Remote> = obj
        .iter()
        .filter_map(|(name, r)| {
            let provider = r.get("type")?.as_str()?.to_string();
            let token = r.get("token").and_then(|t| t.as_str()).is_some_and(|t| !t.trim().is_empty());
            Some(Remote { signed_in: token || !needs_token(&provider), name: name.clone(), provider })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// `rclone listremotes` (`name:` lines) or `rclone listremotes --long` (`name:   type`). Whether a remote is signed in is not
/// known from it, so each is taken as signed in.
pub fn parse_listremotes(text: &str) -> Vec<Remote> {
    text.lines()
        .filter_map(|l| {
            let (name, rest) = l.trim().split_once(':')?;
            (!name.is_empty()).then(|| Remote { name: name.to_string(), provider: rest.trim().to_string(), signed_in: true })
        })
        .collect()
}

/// The remotes rclone has: from `rclone config dump`, or `rclone listremotes --long` when that cannot be read. rclone never asks
/// for a config password here (`--ask-password=false`): an encrypted config is reported, not unlocked.
pub fn remotes(sys: &dyn Sys) -> Result<Vec<Remote>, String> {
    if let Some(r) = sys.run("rclone", &["config", "dump", "--ask-password=false"]).and_then(|j| parse_config_dump(&j).ok()) {
        return Ok(r);
    }
    sys.run("rclone", &["listremotes", "--long", "--ask-password=false"])
        .map(|t| parse_listremotes(&t))
        .ok_or_else(|| "rclone could not list its remotes (is its config locked with a password?)".to_string())
}

/// The drives Settings shows: Proton Drive first when rclone has its remote, then the list.
pub fn with_proton(list: &[Drive], remotes: &[Remote], home: &Path) -> Vec<Drive> {
    let mut out = Vec::new();
    if remotes.iter().any(|r| r.name == PROTON_REMOTE && r.provider == PROTON_TYPE) && !list.iter().any(Drive::is_proton) {
        out.push(proton_drive(home));
    }
    out.extend(list.iter().cloned());
    out
}

// ------------------------------------------------------------------------------------------------ the mount unit

/// A path or word as one argument in a unit file's command line: in double quotes, with systemd's specifiers (`%`) and variables
/// (`$`) doubled so they are taken as they are. (Paths with `"` or `\` are refused by `valid_path` before this.)
fn sd_arg(s: &str) -> String {
    format!("\"{}\"", s.replace('%', "%%").replace('$', "$$"))
}

/// The unit that mounts remote `name` at `mount`, with rclone and fusermount3 at the paths given. None for a name or path that is
/// not allowed.
pub fn unit_text(name: &str, mount: &Path, rclone: &Path, fusermount: &Path) -> Option<String> {
    if !valid_name(name) || !valid_path(mount) || !valid_path(rclone) || !valid_path(fusermount) {
        return None;
    }
    let m = sd_arg(&mount.to_string_lossy());
    let (rc, fm) = (sd_arg(&rclone.to_string_lossy()), sd_arg(&fusermount.to_string_lossy()));
    Some(format!(
        "# Written by Settings > Cloud Storage: mounts the rclone remote \"{name}\" at {mount}. Removing the drive there\n\
         # removes this file and the remote; the files in the cloud are never touched.\n\
         [Unit]\n\
         Description=Cloud drive {name} (rclone mount)\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         # a sign-in that fails (a lapsed token, no network yet) is not retried every few seconds\n\
         StartLimitIntervalSec=600\n\
         StartLimitBurst=3\n\
         \n\
         [Service]\n\
         Type=notify\n\
         ExecStartPre=/usr/bin/mkdir -p {m}\n\
         ExecStart={rc} mount {name}: {m} --vfs-cache-mode full --dir-cache-time 5m\n\
         ExecStop={fm} -u {m}\n\
         Restart=on-failure\n\
         RestartSec=30\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        mount = mount.display(),
    ))
}

/// Writes the drive's unit file into `paths.units` when it is missing or different; whether it was written (systemd then has to
/// read it again). Proton Drive's unit ships with Minos and a folder has none: nothing is written for them.
pub fn write_unit(paths: &Paths, d: &Drive, rclone: &Path, fusermount: &Path) -> Result<bool, String> {
    let Kind::Remote { mount, .. } = &d.kind else { return Ok(false) };
    if d.is_proton() {
        return Ok(false);
    }
    let text = unit_text(&d.name, mount, rclone, fusermount).ok_or("the drive's name or folder cannot be used")?;
    let file = paths.units.join(unit_name(&d.name));
    if std::fs::read_to_string(&file).ok().as_deref() == Some(text.as_str()) {
        return Ok(false);
    }
    write_file(&file, &text).map_err(|e| format!("could not write {}: {e}", file.display()))?;
    Ok(true)
}

// ------------------------------------------------------------------------------------------------ state

/// How a drive is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Mounted,
    NotMounted,
    /// Its unit is starting (signing in, mounting).
    Starting,
    /// Its unit failed (a lapsed sign-in, no network, a wrong folder).
    Failed,
    /// Its remote has no token: the browser sign-in was not finished.
    NotSignedIn,
    /// A sync app's folder that is there.
    Ready,
    /// A sync app's folder that is not there.
    Missing,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Mounted => "Mounted",
            State::NotMounted => "Not mounted",
            State::Starting => "Mounting...",
            State::Failed => "Error: could not mount",
            State::NotSignedIn => "Not signed in",
            State::Ready => "Ready",
            State::Missing => "Folder missing",
        }
    }

    /// Its files can be opened.
    pub fn usable(self) -> bool {
        matches!(self, State::Mounted | State::Ready)
    }

    pub fn is_error(self) -> bool {
        matches!(self, State::Failed | State::NotSignedIn | State::Missing)
    }
}

/// mountinfo writes a space, tab, newline and backslash in a path as \040, \011, \012 and \134.
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The mount points in /proc/self/mountinfo's text.
pub fn mount_points(mountinfo: &str) -> Vec<PathBuf> {
    mountinfo.lines().filter_map(|l| l.split(' ').nth(4)).map(|p| PathBuf::from(unescape_mount(p))).collect()
}

/// A unit as systemd reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitInfo {
    /// `active`, `activating`, `inactive`, `failed`, ...
    pub active: String,
    /// Started at sign-in (`UnitFileState=enabled`).
    pub enabled: bool,
}

/// `systemctl --user show -p Id -p ActiveState -p UnitFileState U1 U2 ...`: one block per unit, blank lines between.
pub fn parse_unit_show(text: &str) -> BTreeMap<String, UnitInfo> {
    let mut out = BTreeMap::new();
    for block in text.split("\n\n") {
        let mut id = None;
        let mut info = UnitInfo::default();
        for (k, v) in block.lines().filter_map(|l| l.split_once('=')) {
            match k.trim() {
                "Id" => id = Some(v.trim().to_string()),
                "ActiveState" => info.active = v.trim().to_string(),
                "UnitFileState" => info.enabled = matches!(v.trim(), "enabled" | "enabled-runtime"),
                _ => {}
            }
        }
        if let Some(id) = id.filter(|i| !i.is_empty()) {
            out.insert(id, info);
        }
    }
    out
}

/// A drive's state from the mount points, its unit (when known) and, for a remote, whether rclone has its token.
pub fn state_of(
    d: &Drive,
    mounts: &[PathBuf],
    unit: Option<&UnitInfo>,
    signed_in: Option<bool>,
    folder_there: &dyn Fn(&Path) -> bool,
) -> State {
    match &d.kind {
        Kind::Folder { path } => {
            if folder_there(path) {
                State::Ready
            } else {
                State::Missing
            }
        }
        Kind::Remote { mount, .. } => {
            if mounts.iter().any(|m| m == mount) {
                State::Mounted
            } else if unit.is_some_and(|u| matches!(u.active.as_str(), "activating" | "reloading")) {
                State::Starting
            } else if unit.is_some_and(|u| u.active == "failed") {
                State::Failed
            } else if signed_in == Some(false) {
                State::NotSignedIn
            } else {
                State::NotMounted
            }
        }
    }
}

/// The drives' units as systemd has them, asked in one `systemctl --user show` (empty when there are none or it cannot say).
pub fn units(sys: &dyn Sys, drives: &[Drive]) -> BTreeMap<String, UnitInfo> {
    let names: Vec<String> = drives.iter().filter_map(Drive::unit).collect();
    if names.is_empty() {
        return BTreeMap::new();
    }
    let mut args = vec!["--user", "show", "-p", "Id", "-p", "ActiveState", "-p", "UnitFileState"];
    args.extend(names.iter().map(String::as_str));
    sys.run("systemctl", &args).map(|t| parse_unit_show(&t)).unwrap_or_default()
}

// ------------------------------------------------------------------------------------------------ doing things

/// The commands the drives need, so the tests can stand in for them.
pub trait Sys {
    /// A program's standard output, when it ran and succeeded.
    fn run(&self, cmd: &str, args: &[&str]) -> Option<String>;
    /// A program run for its effect; Err carries what it said.
    fn act(&self, cmd: &str, args: &[&str]) -> Result<(), String>;
    /// /proc/self/mountinfo's text.
    fn mountinfo(&self) -> String;
    /// Where a program is on PATH.
    fn find(&self, program: &str) -> Option<PathBuf>;
}

/// The real system.
pub struct Real;

impl Sys for Real {
    fn run(&self, cmd: &str, args: &[&str]) -> Option<String> {
        let out = std::process::Command::new(cmd).args(args).stdin(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn act(&self, cmd: &str, args: &[&str]) -> Result<(), String> {
        match std::process::Command::new(cmd).args(args).stdin(std::process::Stdio::null()).output() {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            Err(e) => Err(format!("{cmd}: {e}")),
        }
    }

    fn mountinfo(&self) -> String {
        std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default()
    }

    fn find(&self, program: &str) -> Option<PathBuf> {
        let paths = std::env::var_os("PATH")?;
        std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| p.is_file())
    }
}

/// The programs a mount unit runs, found on PATH (rclone must be there; fusermount3 is taken from /usr/bin when it is not).
fn programs(sys: &dyn Sys) -> Result<(PathBuf, PathBuf), String> {
    let rclone = sys.find("rclone").ok_or("rclone is not installed")?;
    let fusermount = sys.find("fusermount3").unwrap_or_else(|| PathBuf::from("/usr/bin/fusermount3"));
    Ok((rclone, fusermount))
}

fn systemctl(sys: &dyn Sys, args: &[&str]) -> Result<(), String> {
    let mut all = vec!["--user"];
    all.extend_from_slice(args);
    sys.act("systemctl", &all)
}

/// Mounts a remote: writes its unit (the first time), and starts it. Returns when systemd has started it (the mount itself
/// may take a few seconds more; the state says when it is there).
pub fn mount(sys: &dyn Sys, paths: &Paths, d: &Drive) -> Result<(), String> {
    let unit = d.unit().ok_or("a folder is not mounted")?;
    let (rclone, fusermount) = programs(sys)?;
    if write_unit(paths, d, &rclone, &fusermount)? {
        systemctl(sys, &["daemon-reload"])?;
    }
    // a mount that kept failing may have hit its retry limit: cleared, so this start is a fresh one
    let _ = systemctl(sys, &["reset-failed", &unit]);
    systemctl(sys, &["start", &unit]).map_err(|e| format!("could not mount {}: {e}", d.label()))
}

/// Unmounts a remote (its unit stops; the files stay in the cloud and in rclone's cache).
pub fn unmount(sys: &dyn Sys, d: &Drive) -> Result<(), String> {
    let unit = d.unit().ok_or("a folder is not mounted")?;
    systemctl(sys, &["stop", &unit]).map_err(|e| format!("could not unmount {}: {e}", d.label()))
}

/// "Mount at sign-in": the unit is enabled (written first when needed) or disabled. It is not started or stopped by this.
pub fn set_at_login(sys: &dyn Sys, paths: &Paths, d: &Drive, on: bool) -> Result<(), String> {
    let unit = d.unit().ok_or("a folder is not mounted")?;
    if on {
        let (rclone, fusermount) = programs(sys)?;
        if write_unit(paths, d, &rclone, &fusermount)? {
            systemctl(sys, &["daemon-reload"])?;
        }
    }
    systemctl(sys, &[if on { "enable" } else { "disable" }, &unit])
}

/// Removes a drive from this computer: for a remote its unit is stopped, disabled and (when it is ours) deleted, its rclone remote
/// is deleted and its empty mount folder removed; then it leaves the list. Files in the cloud, and files in a folder, are never
/// touched (an empty folder is removed with rmdir, which cannot remove anything in it).
pub fn remove(sys: &dyn Sys, paths: &Paths, d: &Drive) -> Result<(), String> {
    if let Kind::Remote { mount, .. } = &d.kind {
        let unit = d.unit().unwrap_or_default();
        let _ = systemctl(sys, &["stop", &unit]);
        let _ = systemctl(sys, &["disable", &unit]);
        if !d.is_proton() {
            let file = paths.units.join(&unit);
            if file.exists() {
                std::fs::remove_file(&file).map_err(|e| format!("could not remove {}: {e}", file.display()))?;
                let _ = systemctl(sys, &["daemon-reload"]);
            }
        }
        if mount_points(&sys.mountinfo()).iter().any(|m| m == mount) {
            return Err(format!("{} is still mounted (a program may have a file open in it); close it and try again", d.label()));
        }
        sys.act("rclone", &["config", "delete", &d.name, "--ask-password=false"])
            .map_err(|e| format!("rclone could not forget {}: {e}", d.label()))?;
        let _ = std::fs::remove_dir(mount);
    }
    let mut list = load(paths);
    list.retain(|x| x.name != d.name);
    save(paths, &list).map_err(|e| format!("could not save the list: {e}"))
}

/// Adds a drive to the list (one with the same name is replaced).
pub fn add(paths: &Paths, d: Drive) -> Result<(), String> {
    let mut list = load(paths);
    list.retain(|x| x.name != d.name);
    list.push(d);
    save(paths, &list).map_err(|e| format!("could not save the list: {e}"))
}

/// rclone's own sign-in for a new remote: `rclone config create NAME TYPE`, with `--all` (every question asked) for a provider
/// signed in to in a terminal.
pub fn sign_in_args(name: &str, p: &Provider) -> Vec<String> {
    let mut v: Vec<String> = ["config", "create", name, p.kind].map(String::from).to_vec();
    if p.sign_in == SignIn::Terminal {
        v.push("--all".into());
    }
    v
}

/// Terminals a terminal sign-in can run in: (program, the arguments before the command). `$TERMINAL` first when it is one.
const TERMINALS: [(&str, &[&str]); 7] = [
    ("ghostty", &["-e"]),
    ("foot", &[]),
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("konsole", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("xterm", &["-e"]),
];

/// The command line that runs `argv` in a terminal window, which stays open afterwards so its last words can be read. Every word
/// of `argv` is an argument to sh, never script text.
pub fn in_terminal(argv: &[String], terminal_env: Option<&str>, installed: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let preferred = terminal_env.and_then(|t| TERMINALS.iter().find(|(n, _)| *n == t)).filter(|(n, _)| installed(n));
    let (term, before) = preferred.or_else(|| TERMINALS.iter().find(|(n, _)| installed(n)))?;
    let mut cmd: Vec<String> = std::iter::once(term.to_string()).chain(before.iter().map(|s| s.to_string())).collect();
    cmd.extend(
        ["sh", "-c", "\"$@\"; code=$?; echo; echo \"Finished (exit code $code). Press Enter to close.\"; read -r _", "sh"]
            .map(String::from),
    );
    cmd.extend(argv.iter().cloned());
    Some(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A stand-in system: answers from a table, records every command, starts nothing.
    #[derive(Default)]
    struct Fake {
        answers: BTreeMap<String, String>,
        fail: Vec<String>,
        mountinfo: String,
        log: RefCell<Vec<String>>,
        no_rclone: bool,
    }

    impl Sys for Fake {
        fn run(&self, cmd: &str, args: &[&str]) -> Option<String> {
            let line = format!("{cmd} {}", args.join(" "));
            self.log.borrow_mut().push(line.clone());
            self.answers.get(&line).cloned()
        }
        fn act(&self, cmd: &str, args: &[&str]) -> Result<(), String> {
            let line = format!("{cmd} {}", args.join(" "));
            self.log.borrow_mut().push(line.clone());
            if self.fail.contains(&line) {
                Err("refused".into())
            } else {
                Ok(())
            }
        }
        fn mountinfo(&self) -> String {
            self.mountinfo.clone()
        }
        fn find(&self, program: &str) -> Option<PathBuf> {
            (program != "rclone" || !self.no_rclone).then(|| PathBuf::from(format!("/usr/bin/{program}")))
        }
    }

    /// A temporary home with its own config folders (never the real ones).
    fn temp_paths(tag: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("ygg-clouddrives-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Paths { list: root.join("config/yggdrasil").join(LIST_FILE), units: root.join("config/systemd/user"), home: root.join("home") }
    }

    fn gdrive(home: &Path) -> Drive {
        Drive { name: "GoogleDrive".into(), kind: Kind::Remote { provider: "drive".into(), mount: default_mount(home, "GoogleDrive") } }
    }

    #[test]
    fn the_list_is_read_back_as_written_and_bad_entries_are_left_out() {
        let home = Path::new("/home/me");
        let drives = vec![
            gdrive(home),
            Drive { name: "Nextcloud".into(), kind: Kind::Folder { path: home.join("Nextcloud") } },
            Drive { name: "My Box".into(), kind: Kind::Folder { path: home.join("Box Sync") } },
        ];
        assert_eq!(parse_list(&write_list(&drives)), drives);
        let messy = "[remote ok]\ntype = dropbox\nmount = /home/me/Cloud/ok\nextra = 1\n\
                     [remote bad name]\ntype = drive\nmount = /home/me/x\n\
                     [remote rel]\ntype = drive\nmount = Cloud/rel\n\
                     [remote up]\ntype = drive\nmount = /home/me/../etc\n\
                     [remote notype]\nmount = /home/me/Cloud/n\n\
                     [remote ok]\ntype = box\nmount = /home/me/Cloud/again\n\
                     [something else]\nkey = value\n\
                     [folder q]\npath = /home/me/\"quoted\"\n";
        let got = parse_list(messy);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].provider(), Some("dropbox"));
        assert!(parse_list("").is_empty());
    }

    #[test]
    fn names_and_paths_that_may_be_used() {
        assert!(valid_name("GoogleDrive") && valid_name("work-2") && valid_name("a_b"));
        for bad in ["", "-x", "_x", "has space", "dot.name", "slash/x", "ünï", &"x".repeat(33)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert!(valid_path(Path::new("/home/me/Cloud/My Drive")));
        for bad in ["relative", "/", "/home/me/../x", "/home/\"q\"", "/home/back\\slash", "/home/new\nline"] {
            assert!(!valid_path(Path::new(bad)), "{bad:?}");
        }
        let p = provider("drive").unwrap();
        assert_eq!(default_name(p, &[]), "GoogleDrive");
        assert_eq!(default_name(p, &["googledrive".into(), "GoogleDrive2".into()]), "GoogleDrive3");
        assert_eq!(provider_label("onedrive"), "OneDrive");
        assert_eq!(provider_label("sftp"), "sftp");
    }

    #[test]
    fn rclone_config_dump_gives_names_types_and_whether_signed_in() {
        let json = r#"{
            "work": {"type": "onedrive", "token": "{\"access_token\":\"made-up\"}", "drive_type": "business"},
            "half": {"type": "drive", "scope": "drive"},
            "proton": {"type": "protondrive", "username": "someone"},
            "nas": {"type": "webdav", "url": "https://example.org/dav", "vendor": "nextcloud"},
            "broken": {"no_type": true}
        }"#;
        let r = parse_config_dump(json).unwrap();
        let names: Vec<&str> = r.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["half", "nas", "proton", "work"], "sorted, the one without a type left out");
        assert!(!r[0].signed_in, "a browser sign-in with no token was not finished");
        assert!(r[1].signed_in && r[2].signed_in && r[3].signed_in);
        assert_eq!(r[3].provider, "onedrive");
        assert!(parse_config_dump("[]").is_err());
        assert!(parse_config_dump("not json").is_err());
        assert_eq!(parse_config_dump("{}").unwrap(), vec![]);
    }

    #[test]
    fn rclone_listremotes_short_and_long() {
        let short = parse_listremotes("proton:\nwork:\n\n");
        assert_eq!(short.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["proton", "work"]);
        assert_eq!(short[0].provider, "");
        let long = parse_listremotes("GoogleDrive: drive\nnas:         webdav\nnonsense line\n");
        assert_eq!(long.len(), 2);
        assert_eq!((long[1].name.as_str(), long[1].provider.as_str()), ("nas", "webdav"));
    }

    #[test]
    fn remotes_fall_back_to_listremotes_when_the_dump_cannot_be_read() {
        let mut f = Fake::default();
        f.answers.insert("rclone config dump --ask-password=false".into(), r#"{"a":{"type":"box","token":"t"}}"#.into());
        assert_eq!(remotes(&f).unwrap(), vec![Remote { name: "a".into(), provider: "box".into(), signed_in: true }]);
        let mut f = Fake::default();
        f.answers.insert("rclone listremotes --long --ask-password=false".into(), "b: s3\n".into());
        assert_eq!(remotes(&f).unwrap()[0].provider, "s3");
        assert!(remotes(&Fake::default()).is_err());
    }

    #[test]
    fn proton_drive_is_shown_first_when_rclone_has_its_remote() {
        let home = Path::new("/home/me");
        let list = vec![gdrive(home)];
        let proton = Remote { name: PROTON_REMOTE.into(), provider: PROTON_TYPE.into(), signed_in: true };
        let all = with_proton(&list, &[proton], home);
        assert_eq!(all.len(), 2);
        assert!(all[0].is_proton());
        assert_eq!(all[0].unit().as_deref(), Some(PROTON_UNIT));
        assert_eq!(all[0].path(), home.join("ProtonDrive"));
        assert_eq!(all[0].label(), "Proton Drive");
        assert_eq!(with_proton(&list, &[], home), list);
        let other_proton = Remote { name: PROTON_REMOTE.into(), provider: "drive".into(), signed_in: true };
        assert_eq!(with_proton(&list, &[other_proton], home).len(), 1, "a remote called proton of another type is not Proton Drive");
    }

    #[test]
    fn the_mount_unit_runs_rclone_mount_with_the_full_cache() {
        let t = unit_text(
            "GoogleDrive",
            Path::new("/home/me/Cloud/Google Drive"),
            Path::new("/usr/bin/rclone"),
            Path::new("/usr/bin/fusermount3"),
        )
        .unwrap();
        assert!(t.contains(
            "ExecStart=\"/usr/bin/rclone\" mount GoogleDrive: \"/home/me/Cloud/Google Drive\" --vfs-cache-mode full --dir-cache-time 5m\n"
        ));
        assert!(t.contains("ExecStartPre=/usr/bin/mkdir -p \"/home/me/Cloud/Google Drive\"\n"));
        assert!(t.contains("ExecStop=\"/usr/bin/fusermount3\" -u \"/home/me/Cloud/Google Drive\"\n"));
        assert!(t.contains("Type=notify\n") && t.contains("Restart=on-failure\n") && t.contains("WantedBy=default.target\n"));
        // systemd's specifiers and variables in a folder's name stay as they are
        let odd = unit_text("x", Path::new("/home/me/100%$HOME"), Path::new("/usr/bin/rclone"), Path::new("/usr/bin/fusermount3")).unwrap();
        assert!(odd.contains("\"/home/me/100%%$$HOME\""));
        assert!(unit_text("bad name", Path::new("/home/me/x"), Path::new("/usr/bin/rclone"), Path::new("/f")).is_none());
        assert!(unit_text("x", Path::new("rel"), Path::new("/usr/bin/rclone"), Path::new("/f")).is_none());
    }

    #[test]
    fn state_comes_from_the_mounts_the_unit_and_the_token() {
        let home = Path::new("/home/me");
        let d = gdrive(home);
        let mi = "36 35 98:0 / /mnt1 rw - ext4 /dev/x rw\n\
                  99 30 0:77 / /home/me/Cloud/GoogleDrive rw,nosuid - fuse.rclone GoogleDrive: rw\n\
                  100 30 0:78 / /home/me/My\\040Files rw - fuse.rclone x: rw\n";
        let mounts = mount_points(mi);
        assert!(mounts.contains(&PathBuf::from("/home/me/My Files")), "escaped spaces read back");
        let no = |_: &Path| false;
        assert_eq!(state_of(&d, &mounts, None, Some(true), &no), State::Mounted);
        let failed = UnitInfo { active: "failed".into(), enabled: false };
        let starting = UnitInfo { active: "activating".into(), enabled: true };
        assert_eq!(state_of(&d, &[], Some(&failed), Some(true), &no), State::Failed);
        assert_eq!(state_of(&d, &[], Some(&starting), Some(true), &no), State::Starting);
        assert_eq!(state_of(&d, &[], None, Some(false), &no), State::NotSignedIn);
        assert_eq!(state_of(&d, &[], None, None, &no), State::NotMounted);
        let f = Drive { name: "Nextcloud".into(), kind: Kind::Folder { path: home.join("Nextcloud") } };
        assert_eq!(state_of(&f, &[], None, None, &|_| true), State::Ready);
        assert_eq!(state_of(&f, &[], None, None, &no), State::Missing);
        assert!(State::Mounted.usable() && State::Ready.usable() && !State::NotMounted.usable());
    }

    #[test]
    fn systemctl_show_is_read_per_unit() {
        let text = "Id=yggdrasil-cloud-GoogleDrive.service\nActiveState=active\nUnitFileState=enabled\n\n\
                    Id=proton-drive.service\nActiveState=failed\nUnitFileState=disabled\n\n\
                    Id=missing.service\nActiveState=inactive\nUnitFileState=\n";
        let u = parse_unit_show(text);
        assert_eq!(u.len(), 3);
        assert!(u["yggdrasil-cloud-GoogleDrive.service"].enabled);
        assert_eq!(u["proton-drive.service"].active, "failed");
        assert!(!u["missing.service"].enabled);
        let mut f = Fake::default();
        let home = Path::new("/home/me");
        let drives = vec![gdrive(home), Drive { name: "n".into(), kind: Kind::Folder { path: home.join("n") } }];
        f.answers
            .insert("systemctl --user show -p Id -p ActiveState -p UnitFileState yggdrasil-cloud-GoogleDrive.service".into(), text.into());
        assert_eq!(units(&f, &drives).len(), 3, "one systemctl for every unit, none for a folder");
        assert!(units(&f, &drives[1..]).is_empty());
    }

    #[test]
    fn mounting_writes_the_unit_once_and_starts_it() {
        let paths = temp_paths("mount");
        let d = gdrive(&paths.home);
        let f = Fake::default();
        mount(&f, &paths, &d).unwrap();
        let file = paths.units.join("yggdrasil-cloud-GoogleDrive.service");
        assert!(std::fs::read_to_string(&file).unwrap().contains("mount GoogleDrive:"));
        assert_eq!(
            *f.log.borrow(),
            [
                "systemctl --user daemon-reload",
                "systemctl --user reset-failed yggdrasil-cloud-GoogleDrive.service",
                "systemctl --user start yggdrasil-cloud-GoogleDrive.service"
            ]
        );
        f.log.borrow_mut().clear();
        mount(&f, &paths, &d).unwrap();
        assert!(!f.log.borrow().iter().any(|l| l.contains("daemon-reload")), "the same unit is not written or reloaded again");
        unmount(&f, &d).unwrap();
        assert_eq!(f.log.borrow().last().unwrap(), "systemctl --user stop yggdrasil-cloud-GoogleDrive.service");
        // Proton Drive's unit ships with Minos: never written
        let p = proton_drive(&paths.home);
        mount(&f, &paths, &p).unwrap();
        assert!(!paths.units.join(PROTON_UNIT).exists());
        assert_eq!(f.log.borrow().last().unwrap(), "systemctl --user start proton-drive.service");
        // without rclone nothing is written or started
        let none = Fake { no_rclone: true, ..Default::default() };
        let other = Drive { name: "Other".into(), kind: Kind::Remote { provider: "box".into(), mount: paths.home.join("Cloud/Other") } };
        assert!(mount(&none, &paths, &other).is_err());
        assert!(none.log.borrow().is_empty() && !paths.units.join(unit_name("Other")).exists());
        let _ = std::fs::remove_dir_all(paths.list.ancestors().nth(3).unwrap());
    }

    #[test]
    fn mount_at_sign_in_enables_or_disables_without_starting() {
        let paths = temp_paths("login");
        let d = gdrive(&paths.home);
        let f = Fake::default();
        set_at_login(&f, &paths, &d, true).unwrap();
        assert!(paths.units.join(unit_name("GoogleDrive")).exists());
        assert_eq!(f.log.borrow().last().unwrap(), "systemctl --user enable yggdrasil-cloud-GoogleDrive.service");
        set_at_login(&f, &paths, &d, false).unwrap();
        assert_eq!(f.log.borrow().last().unwrap(), "systemctl --user disable yggdrasil-cloud-GoogleDrive.service");
        assert!(!f.log.borrow().iter().any(|l| l.contains(" start ")));
        let _ = std::fs::remove_dir_all(paths.list.ancestors().nth(3).unwrap());
    }

    #[test]
    fn removing_forgets_the_remote_and_the_unit_but_never_a_file() {
        let paths = temp_paths("remove");
        let d = gdrive(&paths.home);
        let keep = Drive { name: "Nextcloud".into(), kind: Kind::Folder { path: paths.home.join("Nextcloud") } };
        add(&paths, d.clone()).unwrap();
        add(&paths, keep.clone()).unwrap();
        let f = Fake::default();
        mount(&f, &paths, &d).unwrap();
        // a file left in the mount folder (it is not mounted: something written there by hand) stays, and so does the folder
        std::fs::create_dir_all(d.path()).unwrap();
        std::fs::write(d.path().join("mine.txt"), "keep me").unwrap();
        f.log.borrow_mut().clear();
        remove(&f, &paths, &d).unwrap();
        assert_eq!(
            *f.log.borrow(),
            [
                "systemctl --user stop yggdrasil-cloud-GoogleDrive.service",
                "systemctl --user disable yggdrasil-cloud-GoogleDrive.service",
                "systemctl --user daemon-reload",
                "rclone config delete GoogleDrive --ask-password=false"
            ]
        );
        assert!(!paths.units.join(unit_name("GoogleDrive")).exists());
        assert_eq!(std::fs::read_to_string(d.path().join("mine.txt")).unwrap(), "keep me");
        assert_eq!(load(&paths), vec![keep.clone()]);
        // a folder drive only leaves the list: its folder and files stay
        std::fs::create_dir_all(keep.path()).unwrap();
        remove(&f, &paths, &keep).unwrap();
        assert!(keep.path().is_dir() && load(&paths).is_empty());
        // still mounted (busy): the remote is kept, and it says why
        add(&paths, d.clone()).unwrap();
        let busy =
            Fake { mountinfo: format!("99 30 0:77 / {} rw - fuse.rclone GoogleDrive: rw\n", d.path().display()), ..Default::default() };
        assert!(remove(&busy, &paths, &d).unwrap_err().contains("still mounted"));
        assert!(!busy.log.borrow().iter().any(|l| l.starts_with("rclone")));
        assert_eq!(load(&paths).len(), 1);
        let _ = std::fs::remove_dir_all(paths.list.ancestors().nth(3).unwrap());
    }

    #[test]
    fn sign_in_is_rclones_own_and_the_terminal_keeps_words_as_arguments() {
        let drive = provider("drive").unwrap();
        assert_eq!(sign_in_args("GoogleDrive", drive), ["config", "create", "GoogleDrive", "drive"]);
        let s3 = provider("s3").unwrap();
        assert_eq!(sign_in_args("S3", s3), ["config", "create", "S3", "s3", "--all"]);
        let argv: Vec<String> = ["rclone", "config", "create", "x; rm -rf ~", "s3", "--all"].map(String::from).to_vec();
        let only = |names: &'static [&'static str]| move |p: &str| names.contains(&p);
        let c = in_terminal(&argv, None, &only(&["xterm", "foot"])).unwrap();
        assert_eq!(&c[..3], ["foot", "sh", "-c"]);
        assert_eq!(&c[c.len() - 6..], argv.as_slice(), "the words follow the script as its arguments");
        assert!(!c[3].contains("rm -rf"));
        assert_eq!(in_terminal(&argv, Some("xterm"), &only(&["xterm", "foot"])).unwrap()[..2], ["xterm", "-e"]);
        assert!(in_terminal(&argv, None, &only(&[])).is_none());
    }
}
