//! The system tray: apps with a notification-area icon (Spotify, Steam, Solaar, Blueman ...) register on the session bus with the
//! StatusNotifierWatcher (org.kde.StatusNotifierWatcher, kept running by KDE's kded for now). This module is the *host* side: it
//! registers with the watcher, reads every item's icon and title, follows their changes, and tells the panel. The panel draws the
//! icons in the bar; a click calls the item's `Activate`, a right click its `ContextMenu`.
//!
//! Not done yet: the items' own menus when they only offer a dbusmenu (no `ContextMenu` of their own) and tooltips.

use std::collections::HashMap;

use resvg::tiny_skia::Pixmap;
use zbus::blocking::{fdo::PropertiesProxy, Connection, MessageIterator, Proxy};
use zbus::zvariant::OwnedValue;
use zbus::MatchRule;

#[derive(Clone, Debug)]
pub struct TrayItem {
    /// The registration string as the watcher lists it (the key).
    pub key: String,
    pub bus: String,
    pub path: String,
    pub id: String,
    pub title: String,
    pub icon_name: String,
    pub theme_path: String,
    /// The icon sent as pixels (premultiplied RGBA), when it has no name.
    pub pixmap: Option<Pixmap>,
    /// "Passive" items are hidden, "NeedsAttention" ones are shown.
    pub status: String,
}

impl TrayItem {
    pub fn shown(&self) -> bool {
        self.status != "Passive"
    }
}

#[derive(Debug)]
pub enum TrayMsg {
    Set(TrayItem),
    Remove(String),
}

/// "name/path" or just "name" -> (bus name, object path). libappindicator items register as ":1.23/org/ayatana/NotificationItem/x".
pub fn split_service(s: &str) -> (String, String) {
    match s.find('/') {
        Some(i) => (s[..i].to_string(), s[i..].to_string()),
        None => (s.to_string(), "/StatusNotifierItem".to_string()),
    }
}

/// The ARGB32 pixels the spec sends (network byte order, not premultiplied) as a premultiplied RGBA pixmap.
pub fn pixmap_from_argb(w: i32, h: i32, data: &[u8]) -> Option<Pixmap> {
    if w <= 0 || h <= 0 || data.len() < (w as usize) * (h as usize) * 4 {
        return None;
    }
    let mut pm = Pixmap::new(w as u32, h as u32)?;
    for (i, px) in pm.data_mut().chunks_exact_mut(4).enumerate() {
        let (a, r, g, b) = (data[i * 4] as u32, data[i * 4 + 1] as u32, data[i * 4 + 2] as u32, data[i * 4 + 3] as u32);
        px[0] = (r * a / 255) as u8;
        px[1] = (g * a / 255) as u8;
        px[2] = (b * a / 255) as u8;
        px[3] = a as u8;
    }
    Some(pm)
}

fn text(props: &HashMap<String, OwnedValue>, key: &str) -> String {
    props.get(key).and_then(|v| <&str>::try_from(v).ok()).unwrap_or("").to_string()
}

/// Reads one item's properties.
pub fn fetch(conn: &Connection, key: &str) -> Option<TrayItem> {
    let (bus, path) = split_service(key);
    let proxy = PropertiesProxy::builder(conn).destination(bus.clone()).ok()?.path(path.clone()).ok()?.build().ok()?;
    let props = proxy.get_all("org.kde.StatusNotifierItem".try_into().ok()?).ok()?;
    let pixmap = props
        .get("IconPixmap")
        .and_then(|v| Vec::<(i32, i32, Vec<u8>)>::try_from(v.try_clone().ok()?).ok())
        // the biggest one the item offers
        .and_then(|mut l| {
            l.sort_by_key(|(w, h, _)| w * h);
            l.pop()
        })
        .and_then(|(w, h, d)| pixmap_from_argb(w, h, &d));
    Some(TrayItem {
        key: key.to_string(),
        bus,
        path,
        id: text(&props, "Id"),
        title: text(&props, "Title"),
        icon_name: text(&props, "IconName"),
        theme_path: text(&props, "IconThemePath"),
        pixmap,
        status: if props.contains_key("Status") { text(&props, "Status") } else { "Active".into() },
    })
}

/// Registers as a host and keeps the panel told about the items. Runs on its own threads; returns at once.
pub fn spawn(tx: calloop::channel::Sender<TrayMsg>) {
    std::thread::spawn(move || {
        let Ok(conn) = Connection::session() else { return };
        let host = format!("org.kde.StatusNotifierHost-{}-1", std::process::id());
        if conn.request_name(host.as_str()).is_err() {
            return;
        }
        // the watcher may start later than we do (it lives in kded): keep trying for a while
        let mut registered = false;
        let mut items: Vec<String> = vec![];
        for _ in 0..60 {
            if let Ok(w) = Proxy::new(&conn, "org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher") {
                if w.call::<_, _, ()>("RegisterStatusNotifierHost", &(host.as_str(),)).is_ok() {
                    items = w.get_property("RegisteredStatusNotifierItems").unwrap_or_default();
                    registered = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        if !registered {
            eprintln!("labyrinth-panel: no StatusNotifierWatcher: the tray stays empty");
            return;
        }
        for k in items {
            if let Some(it) = fetch(&conn, &k) {
                let _ = tx.send(TrayMsg::Set(it));
            }
        }
        // items coming and going
        let c2 = conn.clone();
        let tx2 = tx.clone();
        std::thread::spawn(move || {
            let Ok(rule) = MatchRule::builder().msg_type(zbus::message::Type::Signal).interface("org.kde.StatusNotifierWatcher").map(|b| b.build()) else { return };
            let Ok(iter) = MessageIterator::for_match_rule(rule, &c2, None) else { return };
            for msg in iter.flatten() {
                let hdr = msg.header();
                let member = hdr.member().map(|m| m.to_string()).unwrap_or_default();
                let Ok(key) = msg.body().deserialize::<String>() else { continue };
                match member.as_str() {
                    "StatusNotifierItemRegistered" => {
                        if let Some(it) = fetch(&c2, &key) {
                            let _ = tx2.send(TrayMsg::Set(it));
                        }
                    }
                    "StatusNotifierItemUnregistered" => {
                        let _ = tx2.send(TrayMsg::Remove(key));
                    }
                    _ => {}
                }
            }
        });
        // an item changed its icon, title or status: read it again (the signal's sender tells which one)
        let known: std::sync::Arc<std::sync::Mutex<HashMap<String, String>>> = Default::default();
        let Ok(rule) = MatchRule::builder().msg_type(zbus::message::Type::Signal).interface("org.kde.StatusNotifierItem").map(|b| b.build()) else { return };
        let Ok(iter) = MessageIterator::for_match_rule(rule, &conn, None) else { return };
        let _ = &known;
        for msg in iter.flatten() {
            let hdr = msg.header();
            let Some(sender) = hdr.sender().map(|s| s.to_string()) else { continue };
            let path = hdr.path().map(|p| p.to_string()).unwrap_or_default();
            // the registration key is "sender/path" for appindicator items and "sender" or the well-known name for the rest
            let w = Proxy::new(&conn, "org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher").ok();
            let list: Vec<String> = w.and_then(|w| w.get_property("RegisteredStatusNotifierItems").ok()).unwrap_or_default();
            for key in list {
                let (bus, p) = split_service(&key);
                if bus == sender && (p == path || p == "/StatusNotifierItem") {
                    if let Some(it) = fetch(&conn, &key) {
                        let _ = tx.send(TrayMsg::Set(it));
                    }
                }
            }
        }
    });
}

/// An icon the item ships in its own theme folder (`IconThemePath`): NAME.png or NAME.svg, scaled to `px`.
pub fn icon_in(dir: &str, name: &str, px: u32) -> Option<Pixmap> {
    for ext in ["png", "svg"] {
        let p = std::path::Path::new(dir).join(format!("{name}.{ext}"));
        if p.is_file() {
            if let Some(pm) = crate::fylgja::load_image(&p.display().to_string(), px, px, 0.0) {
                return Some(pm);
            }
        }
    }
    None
}

/// Tells an item it was clicked (`kind` is "Activate", "ContextMenu" or "SecondaryActivate"); x and y are screen coordinates.
pub fn call(item: &TrayItem, kind: &'static str, x: i32, y: i32) {
    let (bus, path) = (item.bus.clone(), item.path.clone());
    std::thread::spawn(move || {
        if let Ok(conn) = Connection::session() {
            if let Ok(p) = Proxy::new(&conn, bus, path, "org.kde.StatusNotifierItem") {
                let _ = p.call::<_, _, ()>(kind, &(x, y));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_strings_split_into_bus_and_path() {
        assert_eq!(split_service(":1.53/org/ayatana/NotificationItem/indicator_solaar"), (":1.53".into(), "/org/ayatana/NotificationItem/indicator_solaar".into()));
        assert_eq!(split_service("org.kde.StatusNotifierItem-12-1"), ("org.kde.StatusNotifierItem-12-1".into(), "/StatusNotifierItem".into()));
        assert_eq!(split_service(":1.9/StatusNotifierItem"), (":1.9".into(), "/StatusNotifierItem".into()));
    }

    #[test]
    fn argb_becomes_premultiplied_rgba() {
        // two pixels: opaque red, half-transparent green (ARGB, not premultiplied)
        let data = [255, 255, 0, 0, 128, 0, 255, 0];
        let pm = pixmap_from_argb(2, 1, &data).unwrap();
        assert_eq!(&pm.data()[0..4], &[255, 0, 0, 255]);
        assert_eq!(&pm.data()[4..8], &[0, 128, 0, 128]);
        assert!(pixmap_from_argb(2, 1, &data[..4]).is_none(), "too little data");
        assert!(pixmap_from_argb(0, 1, &data).is_none());
    }

    #[test]
    fn passive_items_are_hidden() {
        let mut it = TrayItem { key: "k".into(), bus: "b".into(), path: "/p".into(), id: String::new(), title: String::new(), icon_name: String::new(), theme_path: String::new(), pixmap: None, status: "Active".into() };
        assert!(it.shown());
        it.status = "Passive".into();
        assert!(!it.shown());
    }
}
