//! A laptop's rows: Displays > Tablet mode (a 2-in-1's rotation lock and on-screen keyboard) and Users > Fingerprint (enrol,
//! remove, and fingerprint for sudo, off until turned on). Both read their helper's `json` (packages/laptop) and change things
//! through it: `tablet lock|keyboard on|off` as you, `fingerprint enroll|delete` as you (fprintd asks for your password), and
//! `fingerprint sudo on|off` through pkexec (an administrator's password every time).

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;

use crate::ui;

pub const TABLET: &str = "/usr/lib/yggdrasil/tablet";
pub const FINGERPRINT: &str = "/usr/lib/yggdrasil/fingerprint";

/// `tablet json`: None on a computer with neither an accelerometer nor a tablet-mode switch (the group is left out).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tablet {
    pub accelerometer: bool,
    pub switch: bool,
    pub lock: bool,
    pub keyboard: bool,
    pub keyboard_installed: bool,
}

pub fn parse_tablet(json: &str) -> Option<Tablet> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let t = Tablet {
        accelerometer: v["accelerometer"].as_bool().unwrap_or(false),
        switch: v["tablet_switch"].as_bool().unwrap_or(false),
        lock: v["lock"].as_bool().unwrap_or(false),
        keyboard: v["keyboard"].as_bool().unwrap_or(true),
        keyboard_installed: v["keyboard_installed"].as_bool().unwrap_or(false),
    };
    (t.accelerometer || t.switch).then_some(t)
}

pub fn tablet() -> Option<Tablet> {
    crate::sys::run(TABLET, &["json"]).and_then(|s| parse_tablet(&s))
}

/// `fingerprint json`: None when fprintd has no reader (or is not installed: the group is left out, Agora offers it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fingerprint {
    pub reader: String,
    pub fingers: Vec<String>,
    pub sudo: bool,
}

pub fn parse_fingerprint(json: &str) -> Option<Fingerprint> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let reader = v["reader"].as_str()?.to_string();
    let fingers = v["fingers"].as_array().map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_string)).collect()).unwrap_or_default();
    Some(Fingerprint { reader, fingers, sudo: v["sudo"].as_bool().unwrap_or(false) })
}

pub fn fingerprint() -> Option<Fingerprint> {
    crate::sys::run(FINGERPRINT, &["json"]).and_then(|s| parse_fingerprint(&s))
}

/// "right-index-finger" -> "Right index finger".
pub fn finger_name(f: &str) -> String {
    let s = f.replace('-', " ");
    let mut c = s.chars();
    c.next().map(|first| first.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// What a row asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// tablet lock on|off, tablet keyboard on|off
    Tablet(&'static str, bool),
    Enroll,
    DeleteFingers,
    Sudo(bool),
}

impl Action {
    /// The helper and its arguments.
    pub fn command(&self) -> (&'static str, Vec<String>) {
        let w = |b: bool| if b { "on" } else { "off" }.to_string();
        match self {
            Action::Tablet(what, on) => (TABLET, vec![what.to_string(), w(*on)]),
            Action::Enroll => (FINGERPRINT, vec!["enroll".into()]),
            Action::DeleteFingers => (FINGERPRINT, vec!["delete".into()]),
            Action::Sudo(on) => ("pkexec", vec![FINGERPRINT.into(), "sudo".into(), w(*on)]),
        }
    }

    pub fn run(&self) -> Result<(), String> {
        let (cmd, args) = self.command();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        crate::sys::act(cmd, &args)
    }
}

/// Displays > Tablet mode.
pub fn tablet_group(ui: &mut egui::Ui, pal: &Palette, t: &Tablet) -> Option<Action> {
    let mut out = None;
    ui::group(ui, pal, Some("Tablet mode"), |ui| {
        let mut first = true;
        if t.accelerometer {
            let mut lock = t.lock;
            let note = if t.switch {
                "Folded into a tablet, the screen turns with the device; this keeps it as it is"
            } else {
                "The screen turns with the device; this keeps it as it is"
            };
            ui::row(ui, pal, true, "Lock rotation", Some(note), |ui| {
                if ui::switch(ui, pal, &mut lock).changed() {
                    out = Some(Action::Tablet("lock", lock));
                }
            });
            first = false;
        }
        if t.switch {
            let mut kb = t.keyboard;
            let note = if t.keyboard_installed {
                "Shown when the screen is folded back and no keyboard is plugged in"
            } else {
                "Add \"On-screen keyboard\" from Agora first"
            };
            ui::row(ui, pal, first, "On-screen keyboard", Some(note), |ui| {
                if ui.add_enabled_ui(t.keyboard_installed, |ui| ui::switch(ui, pal, &mut kb)).inner.changed() {
                    out = Some(Action::Tablet("keyboard", kb));
                }
            });
        }
    });
    out
}

/// Users > Fingerprint, for your own account. `busy`: an enrolment is waiting for your finger.
pub fn fingerprint_group(ui: &mut egui::Ui, pal: &Palette, fp: &Fingerprint, busy: bool) -> Option<Action> {
    let mut out = None;
    ui::group(ui, pal, Some("Fingerprint"), |ui| {
        let enrolled = if fp.fingers.is_empty() {
            "No finger enrolled".to_string()
        } else {
            fp.fingers.iter().map(|f| finger_name(f)).collect::<Vec<_>>().join(", ")
        };
        ui::row(ui, pal, true, &fp.reader, Some(&enrolled), |ui| {
            if busy {
                ui.label(RichText::new("Touch the reader until it is done").color(pal.dim));
            } else {
                if ui::action(ui, pal, "Add a finger").clicked() {
                    out = Some(Action::Enroll);
                }
                if !fp.fingers.is_empty() && ui::quiet(ui, pal, "Remove all").clicked() {
                    out = Some(Action::DeleteFingers);
                }
            }
        });
        let mut sudo = fp.sudo;
        let note = if fp.fingers.is_empty() && !fp.sudo {
            "Enrol a finger first. Off until you turn it on; the password always works too"
        } else {
            "A finger instead of your password in the terminal; the password always works too (an administrator's password to change)"
        };
        ui::row(ui, pal, false, "Use for sudo", Some(note), |ui| {
            let can = !busy && (fp.sudo || !fp.fingers.is_empty());
            if ui.add_enabled_ui(can, |ui| ui::switch(ui, pal, &mut sudo)).inner.changed() {
                out = Some(Action::Sudo(sudo));
            }
        });
        ui::row(ui, pal, false, "Lock screen and password boxes", Some("Still the password (planned)"), |_| {});
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_answers_are_read() {
        let t = parse_tablet(r#"{"accelerometer":true,"tablet_switch":true,"lock":false,"keyboard":true,"keyboard_installed":false}"#);
        assert_eq!(t, Some(Tablet { accelerometer: true, switch: true, lock: false, keyboard: true, keyboard_installed: false }));
        assert_eq!(parse_tablet(r#"{"accelerometer":false,"tablet_switch":false}"#), None, "a desktop: no group");
        assert_eq!(parse_tablet("off"), None);
        let f = parse_fingerprint(r#"{"installed":true,"reader":"Synaptics Sensors","fingers":["right-index-finger"],"sudo":false}"#);
        assert_eq!(f, Some(Fingerprint { reader: "Synaptics Sensors".into(), fingers: vec!["right-index-finger".into()], sudo: false }));
        assert_eq!(parse_fingerprint(r#"{"installed":true,"reader":null,"fingers":[],"sudo":false}"#), None, "no reader: no group");
        assert_eq!(parse_fingerprint(r#"{"installed":false,"reader":null}"#), None);
        assert_eq!(finger_name("right-index-finger"), "Right index finger");
        assert_eq!(finger_name(""), "");
    }

    #[test]
    fn only_sudo_goes_through_pkexec() {
        assert_eq!(Action::Sudo(true).command(), ("pkexec", vec![FINGERPRINT.to_string(), "sudo".into(), "on".into()]));
        assert_eq!(Action::Enroll.command(), (FINGERPRINT, vec!["enroll".to_string()]));
        assert_eq!(Action::DeleteFingers.command().0, FINGERPRINT);
        assert_eq!(Action::Tablet("lock", true).command(), (TABLET, vec!["lock".to_string(), "on".into()]));
        assert_eq!(Action::Tablet("keyboard", false).command(), (TABLET, vec!["keyboard".to_string(), "off".into()]));
    }
}
