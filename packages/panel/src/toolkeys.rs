//! The hotkeys of Settings > Tools (labyrinth_conf::tools): registered with KDE's global-shortcut daemon as the component
//! "labyrinth-tools" (changeable in System Settings > Shortcuts), and the tool's helper started when one is pressed. Only the tools
//! switched on have keys; switching one off in Settings takes its key away (checked every few seconds). The helpers run only while
//! they are used.

use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use labyrinth_conf::tools::ToolsConf;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::Type;
use zbus::MatchRule;

const COMPONENT: &str = "labyrinth-tools";
const META: i32 = 0x1000_0000;
const SHIFT: i32 = 0x0200_0000;

/// (action, label, default key, helper, on in this config?)
fn tools(c: &ToolsConf) -> Vec<(&'static str, &'static str, i32, &'static str, bool)> {
    vec![("color-picker", "Color Picker", META | SHIFT | 0x43, "labyrinth-color-picker", c.color_picker)]
}

fn id(action: &str, label: &str) -> Vec<String> {
    vec![COMPONENT.into(), action.into(), "Labyrinth Tools".into(), label.into()]
}

/// Gives each tool its key, or takes it away, as the config says. A key the user changed in System Settings is kept.
fn register(ga: &Proxy, c: &ToolsConf) {
    for (action, label, key, _, on) in tools(c) {
        let a = id(action, label);
        let _ = ga.call_method("doRegister", &(&a,));
        let have: Vec<i32> = ga.call("shortcut", &(&a,)).unwrap_or_default();
        if on && have.is_empty() {
            let _: Result<Vec<i32>, _> = ga.call("setShortcut", &(&a, vec![key], 6u32));
        } else if !on && !have.is_empty() {
            let _: Result<Vec<i32>, _> = ga.call("setShortcut", &(&a, Vec::<i32>::new(), 6u32));
        }
    }
}

fn start(helper: &str) {
    if let Ok(mut child) = Command::new(helper).stdin(Stdio::null()).stdout(Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Registers the keys, keeps them in step with the config, and starts helpers. Meant to run in its own thread.
pub fn run() -> Result<(), String> {
    let conn = Connection::session().map_err(|e| e.to_string())?;
    let ga = Proxy::new(&conn, "org.kde.kglobalaccel", "/kglobalaccel", "org.kde.KGlobalAccel").map_err(|e| e.to_string())?;
    register(&ga, &ToolsConf::load());
    // follow Settings: re-register when tools.conf changes
    {
        let conn = conn.clone();
        std::thread::spawn(move || {
            let mut seen: Option<SystemTime> = ToolsConf::modified();
            loop {
                std::thread::sleep(Duration::from_secs(3));
                let now = ToolsConf::modified();
                if now != seen {
                    seen = now;
                    if let Ok(ga) = Proxy::new(&conn, "org.kde.kglobalaccel", "/kglobalaccel", "org.kde.KGlobalAccel") {
                        register(&ga, &ToolsConf::load());
                    }
                }
            }
        });
    }
    let path: zbus::zvariant::OwnedObjectPath = ga.call("getComponent", &(COMPONENT,)).map_err(|e| format!("getComponent: {e}"))?;
    let rule = MatchRule::builder().msg_type(Type::Signal).interface("org.kde.kglobalaccel.Component").map_err(|e| e.to_string())?
        .member("globalShortcutPressed").map_err(|e| e.to_string())?.path(path.as_str()).map_err(|e| e.to_string())?.build();
    let it = MessageIterator::for_match_rule(rule, &conn, None).map_err(|e| e.to_string())?;
    for msg in it {
        let Ok(msg) = msg else { continue };
        let Ok((_component, action, _ts)) = msg.body().deserialize::<(String, String, i64)>() else { continue };
        let c = ToolsConf::load();
        if let Some((_, _, _, helper, true)) = tools(&c).into_iter().find(|t| t.0 == action) {
            start(helper);
        }
    }
    Ok(())
}
