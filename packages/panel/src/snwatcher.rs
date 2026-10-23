//! The tray's registry (StatusNotifierWatcher): apps with a tray icon register here, and tray hosts (the panel's tray, tray.rs) ask it
//! which icons exist. The name is org.kde.StatusNotifierWatcher because that is what every app and toolkit looks for (the protocol
//! started at KDE); nothing of KDE is used. While another program holds the name (KDE's kded), this waits and takes over the moment
//! it is free; apps register again by themselves when the registry changes owner.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::object_server::SignalEmitter;

pub const NAME: &str = "org.kde.StatusNotifierWatcher";
pub const PATH: &str = "/StatusNotifierWatcher";

/// The key an item is listed under: "bus/path" when the app gave a path (libappindicator), else the bus name it gave (or the
/// sender's, when it gave nothing usable). tray.rs's `split_service` reads both forms.
pub fn item_key(sender: &str, service: &str) -> String {
    if service.starts_with('/') {
        format!("{sender}{service}")
    } else if service.is_empty() {
        sender.to_string()
    } else {
        service.to_string()
    }
}

/// The bus name part of a key (what disappears from the bus when the app quits).
pub fn key_bus(key: &str) -> &str {
    key.split('/').next().unwrap_or(key)
}

#[derive(Default)]
struct Lists {
    items: BTreeSet<String>,
    hosts: BTreeSet<String>,
}

#[derive(Clone, Default)]
struct Watcher {
    lists: Arc<Mutex<Lists>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(&self, service: String, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        let sender = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
        let key = item_key(&sender, &service);
        if key.is_empty() {
            return;
        }
        let added = self.lists.lock().unwrap().items.insert(key.clone());
        if added {
            let _ = Self::status_notifier_item_registered(&emitter, &key).await;
            let _ = self.registered_status_notifier_items_changed(&emitter).await;
        }
    }

    async fn register_status_notifier_host(&self, service: String, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        let first = {
            let mut l = self.lists.lock().unwrap();
            let first = l.hosts.is_empty();
            l.hosts.insert(service);
            first
        };
        if first {
            let _ = Self::status_notifier_host_registered(&emitter).await;
            let _ = self.is_status_notifier_host_registered_changed(&emitter).await;
        }
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.lists.lock().unwrap().items.iter().cloned().collect()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        !self.lists.lock().unwrap().hosts.is_empty()
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

impl Watcher {
    /// A bus name left the bus: drop its items and hosts. Returns the items removed.
    fn forget_bus(&self, bus: &str) -> Vec<String> {
        let mut l = self.lists.lock().unwrap();
        let gone: Vec<String> = l.items.iter().filter(|k| key_bus(k) == bus).cloned().collect();
        for k in &gone {
            l.items.remove(k);
        }
        l.hosts.retain(|h| h != bus);
        gone
    }
}

/// Serves the registry from its own thread (waits while another program holds the name). Returns at once.
pub fn spawn() {
    std::thread::spawn(|| {
        let mut told = false;
        loop {
            let w = Watcher::default();
            // no replace_existing_names: kded's watcher is left alone until it goes away by itself
            let built = zbus::blocking::connection::Builder::session().and_then(|b| b.name(NAME)).and_then(|b| b.serve_at(PATH, w.clone())).and_then(|b| b.build());
            match built {
                Ok(conn) => {
                    eprintln!("yggdrasil-panel: serving the tray registry");
                    watch_departures(&conn, &w);
                    return;
                }
                Err(_) => {
                    if !told {
                        eprintln!("yggdrasil-panel: another program serves the tray registry; taking over when it stops");
                        told = true;
                    }
                    std::thread::sleep(Duration::from_secs(10));
                }
            }
        }
    });
}

/// Apps that quit (their bus name disappears) are taken off the list, and hosts are told.
fn watch_departures(conn: &zbus::blocking::Connection, w: &Watcher) {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else { return };
    let Ok(changes) = dbus.receive_name_owner_changed() else { return };
    for sig in changes {
        let Ok(args) = sig.args() else { continue };
        if args.new_owner().is_some() {
            continue;
        }
        let gone = w.forget_bus(args.name().as_str());
        if gone.is_empty() {
            continue;
        }
        let Ok(iface) = conn.object_server().interface::<_, Watcher>(PATH) else { continue };
        let emitter = iface.signal_emitter().clone();
        for k in gone {
            let _ = zbus::block_on(Watcher::status_notifier_item_unregistered(&emitter, &k));
        }
        let _ = zbus::block_on(iface.get().registered_status_notifier_items_changed(&emitter));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_follow_what_the_app_registered() {
        assert_eq!(item_key(":1.42", "/org/ayatana/NotificationItem/steam"), ":1.42/org/ayatana/NotificationItem/steam");
        assert_eq!(item_key(":1.42", "org.kde.StatusNotifierItem-1234-1"), "org.kde.StatusNotifierItem-1234-1");
        assert_eq!(item_key(":1.42", ""), ":1.42");
        assert_eq!(key_bus(":1.42/org/ayatana/NotificationItem/steam"), ":1.42");
        assert_eq!(key_bus("org.kde.StatusNotifierItem-1234-1"), "org.kde.StatusNotifierItem-1234-1");
    }

    #[test]
    fn an_app_that_quits_is_taken_off_the_list() {
        let w = Watcher::default();
        {
            let mut l = w.lists.lock().unwrap();
            l.items.insert(":1.42/org/ayatana/NotificationItem/steam".into());
            l.items.insert(":1.43".into());
            l.hosts.insert(":1.42".into());
        }
        assert_eq!(w.forget_bus(":1.42"), vec![":1.42/org/ayatana/NotificationItem/steam".to_string()]);
        assert_eq!(w.registered_status_notifier_items(), vec![":1.43".to_string()]);
        assert!(!w.is_status_notifier_host_registered());
    }
}
