//! Hold-to-talk: the global shortcut (Super+S) as KDE's global-shortcut daemon reports it, press AND release.
//!
//! A shortcut that only starts a program (what Super+S was) cannot tell when the keys come up. An action registered through
//! `org.kde.KGlobalAccel` is announced with `globalShortcutPressed` and `globalShortcutReleased` signals, which is what this
//! thread listens for. The shortcut is registered once, as the component "yggdrasil-panel" (changeable in System Settings >
//! Shortcuts); if the user already changed it, theirs is kept.

use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::Type;
use zbus::MatchRule;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Down,
    Up,
}

const COMPONENT: &str = "yggdrasil-panel";
const ACTION: &str = "ask-sarina";
/// Qt::MetaModifier | Qt::Key_S
const META_S: i32 = 0x1000_0000 | 0x53;
/// Qt::ControlModifier | Qt::AltModifier | Qt::Key_S: a second way in, for a keyboard whose Super key does not reach the desktop (some gaming keyboards lock it)
const CTRL_ALT_S: i32 = 0x0400_0000 | 0x0800_0000 | 0x53;

fn id() -> Vec<String> {
    vec![COMPONENT.into(), ACTION.into(), "Yggdrasil Panel".into(), "Ask Sarina (hold to talk)".into()]
}

/// Registers the shortcut and listens. Returns when the bus goes away. Meant to run in its own thread.
pub fn run(tx: calloop::channel::Sender<Key>) -> Result<(), String> {
    let conn = Connection::session().map_err(|e| e.to_string())?;
    let ga = Proxy::new(&conn, "org.kde.kglobalaccel", "/kglobalaccel", "org.kde.KGlobalAccel").map_err(|e| e.to_string())?;
    let action = id();
    ga.call_method("doRegister", &(&action,)).map_err(|e| format!("doRegister: {e}"))?;
    let have: Vec<i32> = ga.call("shortcut", &(&action,)).unwrap_or_default();
    // flags: SetPresent (2) makes it the active shortcut. Only the defaults are ever set here: a change made in System Settings is kept (it is then
    // neither empty nor exactly the old default).
    if have.is_empty() {
        let _: Vec<i32> = ga.call("setShortcut", &(&action, vec![META_S, CTRL_ALT_S], 2u32)).map_err(|e| format!("setShortcut: {e}"))?;
    } else if have == vec![META_S] {
        // 2 | 4: set it even though a saved one exists (without the 4, kglobalacceld hands back what was saved)
        let _: Vec<i32> = ga.call("setShortcut", &(&action, vec![META_S, CTRL_ALT_S], 6u32)).map_err(|e| format!("setShortcut: {e}"))?;
    }
    // the old way (a desktop file started by Meta+S) would also fire on the same keys: take its shortcut away
    let _: Result<bool, _> = ga.call("unregister", &("labyrinth-sarina-ask.desktop", "_launch"));
    let path: zbus::zvariant::OwnedObjectPath = ga.call("getComponent", &(COMPONENT,)).map_err(|e| format!("getComponent: {e}"))?;
    let rule = MatchRule::builder().msg_type(Type::Signal).interface("org.kde.kglobalaccel.Component").map_err(|e| e.to_string())?.path(path.as_str()).map_err(|e| e.to_string())?.build();
    let it = MessageIterator::for_match_rule(rule, &conn, None).map_err(|e| e.to_string())?;
    for msg in it {
        let Ok(msg) = msg else { continue };
        let hdr = msg.header();
        let key = match hdr.member().map(|m| m.as_str()) {
            Some("globalShortcutPressed") => Key::Down,
            Some("globalShortcutReleased") => Key::Up,
            _ => continue,
        };
        // (componentUnique, actionUnique, timestamp): only our action
        let Ok((_, action, _)): Result<(String, String, i64), _> = msg.body().deserialize() else { continue };
        if action == ACTION && tx.send(key).is_err() {
            break;
        }
    }
    Ok(())
}
