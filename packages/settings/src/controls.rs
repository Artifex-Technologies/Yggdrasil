//! Settings controls an operating system declares in its integration files (see Yggdrasil's docs/INTEGRATIONS.md): switches,
//! choices and buttons, each backed by a small command of the OS's. Settings draws them; the OS decides what they do. A control whose
//! command fails is not shown (Settings never guesses its state).

use std::process::{Command, Stdio};

use yggdrasil_conf::integrations::{Control, ControlKind};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Switch(bool),
    Choice { options: Vec<String>, current: String },
    Action,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub control: Control,
    pub value: Value,
}

fn run(cmd: &[String], extra: &[&str]) -> Option<String> {
    let out = Command::new(&cmd[0]).args(&cmd[1..]).args(extra).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The current state of each control (run on a background thread: each is a small command).
pub fn load(controls: Vec<Control>) -> Vec<State> {
    controls
        .into_iter()
        .filter_map(|control| {
            let value = match control.kind {
                ControlKind::Switch => match run(&control.command, &["status"])?.as_str() {
                    "on" => Value::Switch(true),
                    "off" => Value::Switch(false),
                    _ => return None,
                },
                ControlKind::Choice => {
                    let options: Vec<String> = run(&control.command, &["list"])?.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
                    if options.is_empty() {
                        return None;
                    }
                    let current = run(&control.command, &["get"]).unwrap_or_default();
                    Value::Choice { options, current }
                }
                ControlKind::Action => Value::Action,
            };
            Some(State { control, value })
        })
        .collect()
}

/// Turns a switch on or off.
pub fn set_switch(c: &Control, on: bool) -> Result<(), String> {
    run(&c.command, &[if on { "on" } else { "off" }]).map(|_| ()).ok_or_else(|| format!("{}: could not change it", c.label))
}

/// Picks an option of a choice.
pub fn set_choice(c: &Control, option: &str) -> Result<(), String> {
    run(&c.command, &["set", option]).map(|_| ()).ok_or_else(|| format!("{}: could not change it", c.label))
}

/// Runs a button's command; what it prints (if anything) is returned to show.
pub fn act(c: &Control) -> Result<String, String> {
    run(&c.command, &[]).ok_or_else(|| format!("{}: it did not work", c.label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn control(kind: ControlKind, cmd: &str) -> Control {
        Control { kind, id: "x".into(), label: "X".into(), description: String::new(), command: vec![cmd.to_string()] }
    }

    #[test]
    fn states_come_from_the_commands_and_failures_hide_controls() {
        let d = std::env::temp_dir().join(format!("ygg-controls-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let script = |name: &str, body: &str| {
            let p = d.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p.to_string_lossy().into_owned()
        };
        let sw = script("sw", r#"case "$1" in status) echo on;; on|off) exit 0;; esac"#);
        let ch = script("ch", r#"case "$1" in list) printf 'amy\njenny\n';; get) echo jenny;; set) [ "$2" = amy ];; esac"#);
        let bad = script("bad", "exit 3");
        let weird = script("weird", "echo maybe");
        let s = load(vec![control(ControlKind::Switch, &sw), control(ControlKind::Choice, &ch), control(ControlKind::Switch, &bad), control(ControlKind::Switch, &weird), control(ControlKind::Action, &bad)]);
        assert_eq!(s.len(), 3, "a failing or unclear switch is hidden; a button is always shown");
        assert!(matches!(s[0].value, Value::Switch(true)));
        assert!(matches!(&s[1].value, Value::Choice { options, current } if options == &["amy", "jenny"] && current == "jenny"));
        assert!(set_choice(&s[1].control, "amy").is_ok());
        assert!(set_choice(&s[1].control, "nobody").is_err());
        assert!(act(&control(ControlKind::Action, &bad)).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
