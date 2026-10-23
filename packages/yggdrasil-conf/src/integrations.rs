//! Integration files: how an operating system plugs its own features into Yggdrasil (an assistant, a package backend, a security
//! monitor, a cloud drive, its branding) without Yggdrasil naming it. See docs/INTEGRATIONS.md.
//!
//! `/usr/share/yggdrasil/integrations/*.conf` (the OS's packages), then `/etc/yggdrasil/integrations/*.conf` (the administrator; the
//! same file name wins). Plain `key = value` lines; the first key is `kind`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integration {
    pub kind: String,
    /// The file it came from (its name is the integration's id).
    pub file: PathBuf,
    values: BTreeMap<String, String>,
}

impl Integration {
    pub fn parse(file: &Path, text: &str) -> Option<Integration> {
        let mut values = BTreeMap::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if let Some((k, v)) = line.split_once('=') {
                values.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        let kind = values.get("kind")?.clone();
        Some(Integration { kind, file: file.to_path_buf(), values })
    }

    pub fn id(&self) -> String {
        self.file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }

    /// A command: the program (an absolute path, or nothing) and its arguments, split on spaces. Run directly, never through a shell.
    pub fn command(&self, key: &str) -> Option<Vec<String>> {
        let parts: Vec<String> = self.get(key)?.split_whitespace().map(str::to_string).collect();
        if parts.first().is_some_and(|p| p.starts_with('/')) { Some(parts) } else { None }
    }

    /// A local web address (`http://127.0.0.1:PORT`, `http://localhost:PORT` or `http://[::1]:PORT`) as `host:port`. Anything that
    /// is not on this computer is refused: the desktop never sends a request elsewhere because a file said so.
    pub fn loopback(&self, key: &str) -> Option<String> {
        let rest = self.get(key)?.strip_prefix("http://")?.trim_end_matches('/');
        let (host, port) = rest.rsplit_once(':')?;
        let local = matches!(host, "127.0.0.1" | "localhost" | "[::1]");
        (local && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit())).then(|| format!("{}:{port}", if host == "localhost" { "127.0.0.1" } else { host }))
    }

    /// A path value (absolute only); `%u` becomes the user's name.
    pub fn path(&self, key: &str) -> Option<PathBuf> {
        let v = self.get(key)?;
        let user = std::env::var("USER").unwrap_or_default();
        let v = v.replace("%u", &user);
        v.starts_with('/').then(|| PathBuf::from(v))
    }
}

/// A Settings control an integration declares: `switch.ID`, `choice.ID` or `action.ID = label | description | /path/command`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub kind: ControlKind,
    pub id: String,
    pub label: String,
    pub description: String,
    /// The program and its arguments; Settings adds `status`/`on`/`off`, `list`/`get`/`set X`, or nothing.
    pub command: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    Switch,
    Choice,
    Action,
}

impl Integration {
    /// The Settings controls it declares, in the order of their IDs. One without an absolute command is left out.
    pub fn controls(&self) -> Vec<Control> {
        let mut out = vec![];
        for (k, v) in &self.values {
            let (kind, id) = match k.split_once('.') {
                Some(("switch", id)) => (ControlKind::Switch, id),
                Some(("choice", id)) => (ControlKind::Choice, id),
                Some(("action", id)) => (ControlKind::Action, id),
                _ => continue,
            };
            let mut parts = v.splitn(3, '|').map(str::trim);
            let (Some(label), Some(description), Some(cmd)) = (parts.next(), parts.next(), parts.next()) else { continue };
            let command: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
            if label.is_empty() || !command.first().is_some_and(|c| c.starts_with('/')) {
                continue;
            }
            out.push(Control { kind, id: id.to_string(), label: label.to_string(), description: description.to_string(), command });
        }
        out
    }
}

fn read_dir(dir: &Path, into: &mut BTreeMap<String, Integration>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "conf") {
            if let Some(i) = std::fs::read_to_string(&p).ok().and_then(|t| Integration::parse(&p, &t)) {
                into.insert(i.id(), i);
            }
        }
    }
}

/// Every integration, from the given folders in order (a later one replaces an earlier one of the same name).
pub fn all_in(dirs: &[PathBuf]) -> Vec<Integration> {
    let mut m = BTreeMap::new();
    for d in dirs {
        read_dir(d, &mut m);
    }
    m.into_values().collect()
}

pub fn all() -> Vec<Integration> {
    all_in(&[crate::dirs::system_data().join("integrations"), crate::dirs::system_config().join("integrations")])
}

/// The integrations of one kind ("assistant", "packages", "security", "cloud", "branding", "settings-page").
pub fn of_kind(kind: &str) -> Vec<Integration> {
    all().into_iter().filter(|i| i.kind == kind).collect()
}

pub fn first(kind: &str) -> Option<Integration> {
    of_kind(kind).into_iter().next()
}

/// The security monitor, if the operating system has one (read once per program).
pub fn security() -> Option<&'static Integration> {
    static SECURITY: std::sync::OnceLock<Option<Integration>> = std::sync::OnceLock::new();
    SECURITY.get_or_init(|| first("security")).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_overrides_and_refuses_relative_commands() {
        let d = std::env::temp_dir().join(format!("ygg-int-{}", std::process::id()));
        let (a, b) = (d.join("share"), d.join("etc"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("sarina.conf"), "kind = assistant\nname = Sarina\nask = /usr/bin/ask --voice # hold to talk\n").unwrap();
        std::fs::write(a.join("cloud.conf"), "kind = cloud\nname = Drive\nmount = /home/%u/Drive\nsign_in = sign-in proton\n").unwrap();
        std::fs::write(a.join("broken.conf"), "name = no kind\n").unwrap();
        std::fs::write(a.join("notes.txt"), "kind = assistant\n").unwrap();
        std::fs::write(b.join("sarina.conf"), "kind = assistant\nname = Ada\nask = /opt/ask\n").unwrap();
        let all = all_in(&[a.clone(), b.clone()]);
        assert_eq!(all.len(), 2);
        let s = all.iter().find(|i| i.kind == "assistant").unwrap();
        assert_eq!(s.get("name"), Some("Ada"), "the administrator's file wins");
        assert_eq!(s.command("ask"), Some(vec!["/opt/ask".to_string()]));
        let c = all.iter().find(|i| i.kind == "cloud").unwrap();
        assert_eq!(c.command("sign_in"), None, "a command must be an absolute path");
        std::env::set_var("USER", "zl");
        assert_eq!(c.path("mount"), Some(PathBuf::from("/home/zl/Drive")));
        let api = |v: &str| Integration::parse(Path::new("x.conf"), &format!("kind = security\napi = {v}\n")).unwrap().loopback("api");
        assert_eq!(api("http://127.0.0.1:8930"), Some("127.0.0.1:8930".into()));
        assert_eq!(api("http://localhost:8930/"), Some("127.0.0.1:8930".into()));
        assert_eq!(api("http://[::1]:80"), Some("[::1]:80".into()));
        for bad in ["https://127.0.0.1:8930", "http://example.com:80", "http://127.0.0.1.evil.com:80", "http://127.0.0.1", "http://127.0.0.1:80/x"] {
            assert_eq!(api(bad), None, "{bad}");
        }
        let only = all_in(&[a]);
        assert_eq!(only.iter().find(|i| i.kind == "assistant").unwrap().command("ask"), Some(vec!["/usr/bin/ask".into(), "--voice".into()]));
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// The accent Yggdrasil falls back to when neither the user nor the operating system chose one.
pub const FALLBACK_ACCENT: [u8; 3] = [124, 84, 214];

fn hex(v: &str) -> Option<[u8; 3]> {
    let v = v.trim().trim_start_matches('#');
    if v.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(v, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

static BRANDING: std::sync::OnceLock<Option<Integration>> = std::sync::OnceLock::new();

fn branding() -> Option<&'static Integration> {
    BRANDING.get_or_init(|| first("branding")).as_ref()
}

/// The default accent: the operating system's branding, else Yggdrasil's fallback. Read once per program.
pub fn default_accent() -> [u8; 3] {
    branding().and_then(|b| b.get("accent")).and_then(hex).unwrap_or(FALLBACK_ACCENT)
}

/// The operating system's default wallpaper, if its branding names one that exists.
pub fn default_wallpaper() -> Option<PathBuf> {
    branding().and_then(|b| b.path("wallpaper")).filter(|p| p.is_file())
}

/// Folders with wallpapers to offer: the standard one (operating systems put theirs in a subfolder), Yggdrasil's own, and the
/// folder of the branding's default.
pub fn wallpaper_dirs() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("/usr/share/wallpapers"), crate::dirs::system_data().join("wallpapers")];
    if let Some(d) = default_wallpaper().and_then(|p| p.parent().map(Path::to_path_buf)) {
        if !v.contains(&d) {
            v.push(d);
        }
    }
    v
}

#[cfg(test)]
mod control_tests {
    use super::*;

    #[test]
    fn controls_are_read_in_order_and_need_absolute_commands() {
        let i = Integration::parse(Path::new("/x/sarina.conf"), "kind = assistant\nswitch.b_learn = Learn | From talks | /usr/bin/learn --x\nchoice.a_voice = Voice | Which voice | /usr/bin/voice\naction.c = Bad | no path | relative\nswitch.d = | empty label | /bin/x\n").unwrap();
        let c = i.controls();
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].kind, c[0].id.as_str(), c[0].label.as_str()), (ControlKind::Choice, "a_voice", "Voice"));
        assert_eq!(c[1].command, vec!["/usr/bin/learn".to_string(), "--x".to_string()]);
    }
}

#[cfg(test)]
mod branding_tests {
    #[test]
    fn hex_colours() {
        assert_eq!(super::hex("#7c54d6"), Some([124, 84, 214]));
        assert_eq!(super::hex("7C54D6"), Some([124, 84, 214]));
        assert_eq!(super::hex("#7c54"), None);
        assert_eq!(super::hex("purple"), None);
    }
}
