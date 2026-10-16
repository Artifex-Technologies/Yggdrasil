//! Restore points: Snapper's snapshots of the system (btrfs). Read, create and delete them without the command line.
//! Going back to one is not done from here (it changes what the computer boots into): the page says how, and the boot menu lists them (grub-btrfs).

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Snap {
    pub number: u64,
    /// "single", "pre" or "post".
    pub kind: String,
    /// "2026-10-16 08:00:19", local time.
    pub date: String,
    pub description: String,
    /// "timeline", "number", "" ... (how Snapper tidies it up later).
    pub cleanup: String,
    /// Marked important by whoever made it: never tidied away.
    pub important: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Snapper is not installed or the system is not btrfs.
    Unavailable(String),
    /// Snapper refuses this user (ALLOW_USERS is empty).
    NoPermission,
    Ready(Vec<Snap>),
}

/// The list out of `snapper --jsonout -c root list`: newest first, without the "current" pseudo entry (number 0).
pub fn parse_list(json: &str) -> Vec<Snap> {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let mut out: Vec<Snap> = v
        .get("root")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    let number = s.get("number")?.as_u64()?;
                    if number == 0 {
                        return None;
                    }
                    Some(Snap {
                        number,
                        kind: s.get("type").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        date: s.get("date").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        description: s.get("description").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        cleanup: s.get("cleanup").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        important: s.get("userdata").and_then(|u| u.get("important")).and_then(|x| x.as_str()) == Some("yes"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| b.number.cmp(&a.number));
    out
}

/// What to call a snapshot in the list.
pub fn title(s: &Snap) -> String {
    match (s.description.as_str(), s.kind.as_str(), s.cleanup.as_str()) {
        ("timeline", _, _) => "Automatic (hourly)".to_string(),
        ("", "pre", _) => "Before a change".to_string(),
        ("", "post", _) => "After a change".to_string(),
        ("", _, _) => format!("Restore point {}", s.number),
        (d, _, _) => d.to_string(),
    }
}

/// Reads the state of the restore points.
pub fn load() -> State {
    let out = std::process::Command::new("snapper").args(["--jsonout", "-c", "root", "list"]).output();
    match out {
        Err(_) => State::Unavailable("Snapper is not installed".into()),
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr).to_string();
            if o.status.success() {
                State::Ready(parse_list(&text))
            } else if err.contains("No permissions") || text.contains("No permissions") {
                State::NoPermission
            } else if err.contains("config") || err.contains("Unknown config") {
                State::Unavailable("There is no restore-point setup for this system (it needs a btrfs system disk)".into())
            } else {
                State::Unavailable(err.lines().next().unwrap_or("Snapper did not answer").to_string())
            }
        }
    }
}

/// Lets the signed-in user list, create and delete restore points (one administrator question).
pub fn allow_me() -> Result<(), String> {
    let user = crate::sys::run("id", &["-un"]).map(|s| s.trim().to_string()).unwrap_or_default();
    if user.is_empty() || !user.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
        return Err("cannot tell who you are".into());
    }
    crate::sys::act("pkexec", &["snapper", "-c", "root", "set-config", &format!("ALLOW_USERS={user}"), "SYNC_ACL=yes"])
}

pub fn create(description: &str) -> Result<(), String> {
    crate::sys::act("snapper", &["-c", "root", "create", "--description", description, "--userdata", "important=yes"])
}

pub fn delete(number: u64) -> Result<(), String> {
    crate::sys::act("snapper", &["-c", "root", "delete", &number.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"root":[
      {"subvolume":"/","number":0,"type":"single","date":"","description":"current","cleanup":"","userdata":null},
      {"subvolume":"/","number":1,"type":"single","date":"2026-10-14 05:51:56","description":"Fresh Labyrinth OS install","cleanup":"","userdata":{"important":"yes"}},
      {"subvolume":"/","number":22,"type":"single","date":"2026-10-16 01:00:19","description":"timeline","cleanup":"timeline","userdata":null},
      {"subvolume":"/","number":23,"type":"pre","date":"2026-10-16 02:00:00","description":"","cleanup":"number","userdata":null}
    ]}"#;

    #[test]
    fn the_list_is_read_newest_first_without_the_current_entry() {
        let l = parse_list(SAMPLE);
        assert_eq!(l.iter().map(|s| s.number).collect::<Vec<_>>(), vec![23, 22, 1]);
        assert!(l[2].important && !l[1].important);
        assert_eq!(l[2].date, "2026-10-14 05:51:56");
        assert!(parse_list("nonsense").is_empty());
    }

    #[test]
    fn titles_are_plain() {
        let l = parse_list(SAMPLE);
        assert_eq!(title(&l[0]), "Before a change");
        assert_eq!(title(&l[1]), "Automatic (hourly)");
        assert_eq!(title(&l[2]), "Fresh Labyrinth OS install");
    }
}
