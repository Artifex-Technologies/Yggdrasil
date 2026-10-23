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

    /// A path value (absolute only); `%u` becomes the user's name.
    pub fn path(&self, key: &str) -> Option<PathBuf> {
        let v = self.get(key)?;
        let user = std::env::var("USER").unwrap_or_default();
        let v = v.replace("%u", &user);
        v.starts_with('/').then(|| PathBuf::from(v))
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
        let only = all_in(&[a]);
        assert_eq!(only.iter().find(|i| i.kind == "assistant").unwrap().command("ask"), Some(vec!["/usr/bin/ask".into(), "--voice".into()]));
        let _ = std::fs::remove_dir_all(&d);
    }
}
