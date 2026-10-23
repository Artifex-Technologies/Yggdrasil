//! The multimedia keys (volume, microphone, playback, screen brightness) while KWin is the compositor: KWin hands them to KDE's
//! global-shortcut daemon, which announces each press for the component that registered the key. Those components used to be KDE's
//! kded (kmix, mediacontrol) and PowerDevil; once those stop, the panel registers the same actions under the same names, so the keys,
//! and any change the user made to them, stay as they are, and does the work with `yggdrasil-mediakey`.
//!
//! The panel claims a component only while the KDE program that owns it is not running, so a key never acts twice. Yggdrasil's own
//! compositor binds these keys itself (compositor/src/shortcuts.rs); this module is the bridge until then.

use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::MatchRule;

use crate::osd::{OsdKind, OsdState};

/// (component, its name, KDE program that owns it on the bus, [(action, action name, Qt key code for a fresh setup, mediakey argument)])
type Group = (&'static str, &'static str, &'static str, &'static [(&'static str, &'static str, i32, &'static str)]);

pub const GROUPS: [Group; 3] = [
    ("kmix", "Audio Volume", "org.kde.kded6", &[
        ("increase_volume", "Increase Volume", 0x0100_0072, "volume-up"),
        ("decrease_volume", "Decrease Volume", 0x0100_0070, "volume-down"),
        ("mute", "Mute", 0x0100_0071, "mute"),
        ("mic_mute", "Mute Microphone", 0x0100_0113, "mic-mute"),
    ]),
    ("mediacontrol", "Media Controller", "org.kde.kded6", &[
        ("playpausemedia", "Play/Pause media playback", 0x0100_0080, "play-pause"),
        ("pausemedia", "Pause media playback", 0x0100_0085, "play-pause"),
        ("stopmedia", "Stop media playback", 0x0100_0081, "stop"),
        ("nextmedia", "Media playback next", 0x0100_0083, "next"),
        ("previousmedia", "Media playback previous", 0x0100_0082, "previous"),
    ]),
    ("org_kde_powerdevil", "Power Management", "org.kde.Solid.PowerManagement", &[
        ("Increase Screen Brightness", "Increase Screen Brightness", 0x0100_00b2, "brightness-up"),
        ("Decrease Screen Brightness", "Decrease Screen Brightness", 0x0100_00b3, "brightness-down"),
    ]),
];

/// The mediakey argument for a pressed (component, action), if it is one of ours.
pub fn command_for(component: &str, action: &str) -> Option<&'static str> {
    GROUPS.iter().find(|g| g.0 == component)?.3.iter().find(|a| a.0 == action).map(|a| a.3)
}

/// `brightnessctl -m` ("device,class,current,percent%,max"): the percent.
pub fn brightness_percent(machine: &str) -> Option<u32> {
    machine.lines().next()?.split(',').nth(3)?.trim_end_matches('%').parse().ok()
}

fn owner_running(conn: &Connection, name: &str) -> bool {
    zbus::blocking::fdo::DBusProxy::new(conn).ok().and_then(|d| d.name_has_owner(zbus::names::BusName::try_from(name).ok()?).ok()).unwrap_or(false)
}

/// Registers the actions of every group whose KDE owner is not running. Returns the components claimed.
fn claim(conn: &Connection, ga: &Proxy) -> Vec<&'static str> {
    let mut claimed = vec![];
    for (component, cname, owner, actions) in GROUPS {
        if owner_running(conn, owner) {
            continue;
        }
        for (action, aname, key, _) in actions.iter() {
            let id = vec![component.to_string(), action.to_string(), cname.to_string(), aname.to_string()];
            if ga.call_method("doRegister", &(&id,)).is_err() {
                continue;
            }
            // a fresh setup has no key for it yet: give it the usual one (SetPresent); a key the user chose is kept
            let have: Vec<i32> = ga.call("shortcut", &(&id,)).unwrap_or_default();
            if have.is_empty() {
                let _: Result<Vec<i32>, _> = ga.call("setShortcut", &(&id, vec![*key], 2u32));
            }
        }
        claimed.push(component);
    }
    claimed
}

fn run_key(arg: &str, osd: &calloop::channel::Sender<OsdState>) {
    let _ = std::process::Command::new("yggdrasil-mediakey").arg(arg).stdin(std::process::Stdio::null()).status();
    // the volume pop-up follows PipeWire by itself; brightness has no such signal here, so it is shown from what was just set
    if arg.starts_with("brightness") {
        if let Ok(o) = std::process::Command::new("brightnessctl").arg("-m").output() {
            if let Some(percent) = brightness_percent(&String::from_utf8_lossy(&o.stdout)) {
                let _ = osd.send(OsdState { kind: OsdKind::Brightness, percent, muted: false });
            }
        }
    }
}

/// Claims the keys now (for what is not owned by a running KDE program) and again whenever kded or PowerDevil leaves the bus, and
/// acts on the presses. Runs on its own threads; returns at once.
pub fn spawn(osd: calloop::channel::Sender<OsdState>) {
    std::thread::spawn(move || {
        let Ok(conn) = Connection::session() else { return };
        let Ok(ga) = Proxy::new(&conn, "org.kde.kglobalaccel", "/kglobalaccel", "org.kde.KGlobalAccel") else { return };
        let mine: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>> = Default::default();
        *mine.lock().unwrap() = claim(&conn, &ga);
        // kded or PowerDevil stopped: claim what they had
        {
            let (c2, mine2) = (conn.clone(), mine.clone());
            std::thread::spawn(move || {
                let Ok(ga) = Proxy::new(&c2, "org.kde.kglobalaccel", "/kglobalaccel", "org.kde.KGlobalAccel") else { return };
                let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&c2) else { return };
                let Ok(changes) = dbus.receive_name_owner_changed() else { return };
                for sig in changes {
                    let Ok(args) = sig.args() else { continue };
                    let name = args.name().to_string();
                    if args.new_owner().is_none() && GROUPS.iter().any(|g| g.2 == name) {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        let now = claim(&c2, &ga);
                        let mut m = mine2.lock().unwrap();
                        for c in now {
                            if !m.contains(&c) {
                                m.push(c);
                            }
                        }
                    }
                }
            });
        }
        let Ok(rule) = MatchRule::builder().msg_type(zbus::message::Type::Signal).interface("org.kde.kglobalaccel.Component").and_then(|b| b.member("globalShortcutPressed")).map(|b| b.build()) else { return };
        let Ok(iter) = MessageIterator::for_match_rule(rule, &conn, None) else { return };
        for msg in iter.flatten() {
            let Ok((component, action, _)) = msg.body().deserialize::<(String, String, i64)>() else { continue };
            // only components the panel claimed: while kded owns them, kded acts and the panel stays out of it
            if !mine.lock().unwrap().iter().any(|c| *c == component) {
                continue;
            }
            if let Some(arg) = command_for(&component, &action) {
                run_key(arg, &osd);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presses_map_to_mediakey_commands() {
        assert_eq!(command_for("kmix", "increase_volume"), Some("volume-up"));
        assert_eq!(command_for("mediacontrol", "playpausemedia"), Some("play-pause"));
        assert_eq!(command_for("org_kde_powerdevil", "Decrease Screen Brightness"), Some("brightness-down"));
        assert_eq!(command_for("kmix", "push_to_talk"), None);
        assert_eq!(command_for("kwin", "Window Close"), None);
    }

    #[test]
    fn brightness_is_read_from_brightnessctl() {
        assert_eq!(brightness_percent("intel_backlight,backlight,48000,50%,96000\n"), Some(50));
        assert_eq!(brightness_percent(""), None);
    }
}
