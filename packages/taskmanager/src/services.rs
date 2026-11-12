//! Services (systemd units of type service), the system's and your own, with what they do now and whether they start by
//! themselves. Starting, stopping and switching one on or off goes through systemctl; for the system's ones it asks for your
//! password (the desktop's polkit agent), as Windows asks for an administrator.

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    System,
    User,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::System => "System",
            Scope::User => "Yours",
        }
    }
    fn flag(self) -> Option<&'static str> {
        match self {
            Scope::System => None,
            Scope::User => Some("--user"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Service {
    pub name: String,
    pub description: String,
    /// active / inactive / failed / activating ...
    pub active: String,
    /// running / dead / exited ...
    pub sub: String,
    /// enabled / disabled / static / masked / indirect ... ("" when it has no unit file of its own)
    pub file_state: String,
    pub scope: Option<Scope>,
}

impl Service {
    pub fn status(&self) -> &'static str {
        match (self.active.as_str(), self.sub.as_str()) {
            ("active", "running") => "Running",
            ("active", "exited") => "Done (ran once)",
            ("active", _) => "Active",
            ("activating", _) => "Starting",
            ("deactivating", _) => "Stopping",
            ("failed", _) => "Failed",
            ("reloading", _) => "Reloading",
            _ => "Stopped",
        }
    }
    pub fn startup(&self) -> &'static str {
        match self.file_state.as_str() {
            "enabled" | "enabled-runtime" => "Automatic",
            "disabled" => "Manual",
            "static" | "indirect" | "generated" | "transient" => "When needed",
            "masked" | "masked-runtime" => "Blocked",
            "alias" => "Another name",
            _ => "",
        }
    }
    /// The name as people read it: without ".service", and systemd's escapes (\x2d for "-") turned back.
    pub fn display(&self) -> String {
        unescape(self.name.trim_end_matches(".service"))
    }
    pub fn scope(&self) -> Scope {
        self.scope.unwrap_or(Scope::System)
    }
    pub fn running(&self) -> bool {
        matches!(self.active.as_str(), "active" | "activating" | "reloading")
    }
}

/// systemd's \xNN escapes in unit names, back to the characters.
pub fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' {
            if let Ok(v) = u8::from_str_radix(&s[i + 2..i + 4], 16) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Merges `systemctl list-units --type=service --all --output=json` (what runs) and `list-unit-files` (what can).
pub fn parse(units: &str, files: &str, scope: Scope) -> Vec<Service> {
    let mut out: std::collections::BTreeMap<String, Service> = Default::default();
    if let Ok(Value::Array(v)) = serde_json::from_str::<Value>(files) {
        for f in v {
            let Some(name) = f.get("unit_file").and_then(|x| x.as_str()) else { continue };
            if name.contains("@.") {
                continue; // a template: its instances are listed by name when they exist
            }
            out.insert(
                name.to_string(),
                Service {
                    name: name.to_string(),
                    file_state: f.get("state").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    scope: Some(scope),
                    active: "inactive".into(),
                    sub: "dead".into(),
                    ..Default::default()
                },
            );
        }
    }
    if let Ok(Value::Array(v)) = serde_json::from_str::<Value>(units) {
        for u in v {
            let Some(name) = u.get("unit").and_then(|x| x.as_str()) else { continue };
            let load = u.get("load").and_then(|x| x.as_str()).unwrap_or("");
            if load == "not-found" && !out.contains_key(name) {
                continue; // named somewhere, installed nowhere
            }
            let s =
                out.entry(name.to_string()).or_insert_with(|| Service { name: name.to_string(), scope: Some(scope), ..Default::default() });
            let g = |k: &str| u.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            s.active = g("active");
            s.sub = g("sub");
            let d = g("description");
            if d != name {
                s.description = d;
            }
        }
    }
    out.into_values().collect()
}

fn systemctl(scope: Scope, args: &[&str]) -> Option<String> {
    let mut c = std::process::Command::new("systemctl");
    if let Some(f) = scope.flag() {
        c.arg(f);
    }
    let out = c.args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Every service, the system's and yours.
pub fn list() -> Vec<Service> {
    let mut v = vec![];
    for scope in [Scope::System, Scope::User] {
        let units = systemctl(scope, &["list-units", "--type=service", "--all", "--output=json", "--no-pager"]).unwrap_or_default();
        let files = systemctl(scope, &["list-unit-files", "--type=service", "--output=json", "--no-pager"]).unwrap_or_default();
        v.extend(parse(&units, &files, scope));
    }
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

impl Action {
    pub fn verb(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Restart => "restart",
            Action::Enable => "enable",
            Action::Disable => "disable",
        }
    }
}

/// The systemctl command for an action (the system's services ask for the password through polkit by themselves).
pub fn command(s: &Service, a: Action) -> Vec<String> {
    let mut v = vec!["systemctl".to_string()];
    if let Some(f) = s.scope().flag() {
        v.push(f.into());
    }
    v.push(a.verb().into());
    v.push(s.name.clone());
    v
}

/// Runs it in the background; the result shows at the next refresh. Returns the error text when systemctl says no.
pub fn run(s: &Service, a: Action) -> Result<(), String> {
    let cmd = command(s, a);
    let out = std::process::Command::new(&cmd[0]).args(&cmd[1..]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if e.is_empty() { format!("systemctl {} {} did not work", a.verb(), s.name) } else { e })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_files_merge() {
        let units = r#"[{"unit":"sshd.service","load":"loaded","active":"active","sub":"running","description":"OpenSSH Daemon"},
            {"unit":"apparmor.service","load":"not-found","active":"inactive","sub":"dead","description":"apparmor.service"},
            {"unit":"getty@tty1.service","load":"loaded","active":"active","sub":"running","description":"Getty on tty1"},
            {"unit":"oneshot.service","load":"loaded","active":"active","sub":"exited","description":"oneshot.service"}]"#;
        let files = r#"[{"unit_file":"sshd.service","state":"enabled","preset":"disabled"},
            {"unit_file":"getty@.service","state":"enabled","preset":"enabled"},
            {"unit_file":"cups.service","state":"disabled","preset":"disabled"},
            {"unit_file":"oneshot.service","state":"static","preset":null}]"#;
        let v = parse(units, files, Scope::System);
        let names: Vec<&str> = v.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["cups.service", "getty@tty1.service", "oneshot.service", "sshd.service"]);
        let ssh = &v[3];
        assert_eq!((ssh.status(), ssh.startup(), ssh.description.as_str()), ("Running", "Automatic", "OpenSSH Daemon"));
        assert_eq!((v[0].status(), v[0].startup()), ("Stopped", "Manual"));
        assert_eq!(
            (v[2].status(), v[2].startup(), v[2].description.as_str()),
            ("Done (ran once)", "When needed", ""),
            "a description that is only the name is dropped"
        );
        assert!(parse("not json", "[]", Scope::User).is_empty());
        assert_eq!(unescape(r"app-yggdrasil\x2dpanel@autostart"), "app-yggdrasil-panel@autostart");
        assert_eq!(unescape(r"odd\x"), r"odd\x");
        let s = Service { name: r"app-gnome\x2dkeyring.service".into(), ..Default::default() };
        assert_eq!(s.display(), "app-gnome-keyring");
    }

    #[test]
    fn commands() {
        let s = Service { name: "x.service".into(), scope: Some(Scope::User), ..Default::default() };
        assert_eq!(command(&s, Action::Restart), ["systemctl", "--user", "restart", "x.service"]);
        let s = Service { name: "y.service".into(), scope: Some(Scope::System), ..Default::default() };
        assert_eq!(command(&s, Action::Disable), ["systemctl", "disable", "y.service"]);
    }
}
