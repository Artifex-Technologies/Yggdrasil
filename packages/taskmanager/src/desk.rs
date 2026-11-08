//! The desktop around the processes: Hyprland's windows (through its socket, no program per question) and the apps' names
//! and icons from their menu entries.

use crate::procs::Window;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;

fn socket() -> Option<PathBuf> {
    let run = std::env::var_os("XDG_RUNTIME_DIR")?;
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    Some(PathBuf::from(run).join("hypr").join(sig).join(".socket.sock"))
}

/// One request to Hyprland ("j/clients", "dispatch ..."), its answer.
pub fn hypr(cmd: &str) -> Option<String> {
    let mut s = std::os::unix::net::UnixStream::connect(socket()?).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(2))).ok()?;
    s.write_all(cmd.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

pub fn parse_clients(json: &str) -> Vec<Window> {
    let Ok(serde_json::Value::Array(v)) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    v.iter()
        .filter(|c| c.get("mapped").and_then(|m| m.as_bool()).unwrap_or(true))
        .filter_map(|c| {
            Some(Window {
                address: c.get("address")?.as_str()?.to_string(),
                pid: c.get("pid")?.as_i64().filter(|&p| p > 0)? as u32,
                class: c.get("class").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                title: c.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            })
        })
        .collect()
}

pub fn windows() -> Vec<Window> {
    hypr("j/clients").map(|s| parse_clients(&s)).unwrap_or_default()
}

/// A Hyprland address as the Lua dispatchers want it (only hex digits get through).
fn address(a: &str) -> Option<String> {
    let hex = a.trim_start_matches("0x");
    (!hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("address:0x{hex}"))
}

/// Asks a window to close, as its close button would (the app may ask to save first).
pub fn close_window(addr: &str) -> bool {
    let Some(a) = address(addr) else { return false };
    hypr(&format!("dispatch hl.dsp.window.close({{ window = '{a}' }})")).is_some_and(|r| r.trim() == "ok")
}

/// Brings a window to the front.
pub fn focus_window(addr: &str) -> bool {
    let Some(a) = address(addr) else { return false };
    hypr(&format!("dispatch hl.dsp.focus({{ window = '{a}' }})")).is_some_and(|r| r.trim() == "ok")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppInfo {
    pub name: String,
    pub icon: String,
}

/// Menu entries by every name a window class may match: the file's name, StartupWMClass and the program's name.
#[derive(Default)]
pub struct Apps {
    by_key: HashMap<String, AppInfo>,
}

fn entry_keys(id: &str, m: &HashMap<String, String>) -> Vec<String> {
    let mut keys = vec![id.to_lowercase()];
    if let Some(w) = m.get("StartupWMClass") {
        keys.push(w.to_lowercase());
    }
    if let Some(exec) = m.get("Exec") {
        if let Some(prog) = exec.split_whitespace().find(|w| !w.contains('=') && *w != "env") {
            keys.push(prog.rsplit('/').next().unwrap_or(prog).to_lowercase());
        }
    }
    if let Some(last) = id.rsplit('.').next() {
        keys.push(last.to_lowercase());
    }
    keys
}

impl Apps {
    pub fn load() -> Apps {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
        let mut dirs = vec![PathBuf::from("/usr/share/applications"), PathBuf::from("/var/lib/flatpak/exports/share/applications"), data.join("flatpak/exports/share/applications"), data.join("applications")];
        dirs.dedup();
        let mut a = Apps::default();
        for d in dirs {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let Some(id) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".desktop")) else { continue };
                let Ok(t) = std::fs::read_to_string(&p) else { continue };
                a.add(id, &t);
            }
        }
        a
    }

    /// Later entries (yours) win over earlier ones (the system's) of the same key.
    pub fn add(&mut self, id: &str, text: &str) {
        let m = crate::startup::parse_desktop(text);
        if m.get("Type").is_some_and(|t| t != "Application") {
            return;
        }
        let Some(name) = m.get("Name").cloned() else { return };
        let info = AppInfo { name, icon: m.get("Icon").cloned().unwrap_or_default() };
        let keys = entry_keys(id, &m);
        // the file's own name and its window class are sure; the program's name only fills gaps
        for (i, k) in keys.into_iter().enumerate() {
            if i <= 1 || !self.by_key.contains_key(&k) {
                self.by_key.insert(k, info.clone());
            }
        }
    }

    pub fn find(&self, class: &str) -> Option<&AppInfo> {
        if class.is_empty() {
            return None;
        }
        let c = class.to_lowercase();
        self.by_key.get(&c).or_else(|| self.by_key.get(c.rsplit('.').next().unwrap_or(&c))).or_else(|| self.by_key.get(c.trim_end_matches("-stable")))
    }

    /// The menu entry for a window class, or else for the program's file name.
    pub fn find_for(&self, class: &str, exe: &str) -> Option<&AppInfo> {
        self.find(class).or_else(|| self.find(exe.rsplit('/').next().unwrap_or(exe)))
    }

    /// An app's name for its window class or program (the class itself, tidied, when no menu entry knows either).
    pub fn name(&self, class: &str, title: &str, exe: &str) -> String {
        if let Some(a) = self.find_for(class, exe) {
            return a.name.clone();
        }
        let last = class.rsplit('.').next().unwrap_or(class);
        if last.is_empty() {
            return title.to_string();
        }
        let mut c = last.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clients() {
        let j = r#"[{"address":"0x55d9","mapped":true,"pid":1234,"class":"code","title":"main.rs"},
                    {"address":"0x77","mapped":false,"pid":5,"class":"x","title":""},
                    {"address":"0x88","pid":-1,"class":"y","title":""}]"#;
        let w = parse_clients(j);
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].pid, w[0].class.as_str(), w[0].address.as_str()), (1234, "code", "0x55d9"));
        assert!(parse_clients("nope").is_empty());
        assert_eq!(address("0x55d9aa"), Some("address:0x55d9aa".into()));
        assert_eq!(address("0x12'); os.execute('x"), None, "nothing but hex reaches the Lua line");
    }

    #[test]
    fn names_from_menu_entries() {
        let mut a = Apps::default();
        a.add("code", "[Desktop Entry]\nType=Application\nName=Visual Studio Code\nExec=/usr/bin/code %F\nIcon=vscode\nStartupWMClass=Code\n");
        a.add("com.mitchellh.ghostty", "[Desktop Entry]\nType=Application\nName=Ghostty\nExec=ghostty\nIcon=com.mitchellh.ghostty\n");
        a.add("vivaldi-stable", "[Desktop Entry]\nType=Application\nName=Vivaldi\nExec=/usr/bin/vivaldi-stable %U\n");
        a.add("link", "[Desktop Entry]\nType=Link\nName=Web\n");
        assert_eq!(a.name("Code", "", ""), "Visual Studio Code");
        assert_eq!(a.name("com.microsoft.VSCode", "", "/usr/share/code/code"), "Visual Studio Code", "by the program when the class is unknown");
        assert_eq!(a.name("com.mitchellh.ghostty", "", ""), "Ghostty");
        assert_eq!(a.name("vivaldi-stable", "", ""), "Vivaldi");
        assert_eq!(a.find("Code").unwrap().icon, "vscode");
        assert_eq!(a.name("org.example.thing", "", ""), "Thing");
        assert_eq!(a.name("", "A title", ""), "A title");
        assert!(a.find("link").is_none(), "only applications");
    }
}
