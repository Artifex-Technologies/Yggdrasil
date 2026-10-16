//! Drives coming and going: a notification when a USB stick or SD card is plugged in (click it to open the drive in Minos) and a quiet one
//! when it goes. This replaces KDE's "device notifier" for the one thing it did for us. It listens to udisks2 on the system bus
//! (`InterfacesAdded` / `InterfacesRemoved` of the block devices) and never mounts anything by itself: opening the notification mounts the
//! drive (through `labyrinth-mount-open`, which asks udisks2, so polkit decides whether it is allowed) and shows it in Minos.

use std::collections::HashMap;

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::MatchRule;

use crate::notify::{self, NMsg, Notif};

type Props = HashMap<String, HashMap<String, OwnedValue>>;

/// What the panel needs to know about a drive that appeared.
#[derive(Debug, PartialEq, Eq)]
pub struct Drive {
    pub device: String,
    pub label: String,
    pub size: u64,
}

fn text(p: &HashMap<String, OwnedValue>, k: &str) -> String {
    p.get(k).and_then(|v| <&str>::try_from(v).ok()).unwrap_or("").to_string()
}

/// A device path such as `/dev/sdb1` from udisks2's `Device` property (bytes with a trailing 0).
fn device_path(p: &HashMap<String, OwnedValue>) -> String {
    p.get("Device").and_then(|v| Vec::<u8>::try_from(v.try_clone().ok()?).ok()).map(|b| String::from_utf8_lossy(&b).trim_end_matches('\0').to_string()).unwrap_or_default()
}

/// Is this a user's drive (something to tell them about)? Not a system disk, not a hidden device, and it has a file system.
pub fn drive_from(ifaces: &Props) -> Option<Drive> {
    let block = ifaces.get("org.freedesktop.UDisks2.Block")?;
    ifaces.get("org.freedesktop.UDisks2.Filesystem")?;
    let flag = |k: &str| block.get(k).and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
    if flag("HintSystem") || flag("HintIgnore") {
        return None;
    }
    let device = device_path(block);
    if device.is_empty() {
        return None;
    }
    let size = block.get("Size").and_then(|v| u64::try_from(v).ok()).unwrap_or(0);
    Some(Drive { device, label: text(block, "IdLabel"), size })
}

pub fn size_text(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.0} GB", b / GB)
    } else {
        format!("{:.0} MB", b / 1_000_000.0)
    }
}

fn announce(tx: &calloop::channel::Sender<NMsg>, d: &Drive, arrived: bool) {
    let name = if d.label.is_empty() { d.device.clone() } else { d.label.clone() };
    let n = Notif {
        id: notify::next_id(),
        app: "Drives".into(),
        entry: String::new(),
        icon: "drive-removable-media".into(),
        summary: if arrived { "Drive connected".into() } else { "Drive disconnected".into() },
        body: if d.size > 0 { format!("{name} ({})", size_text(d.size)) } else { name },
        // a quiet one when it goes: kept in the list, no pop-up
        urgency: if arrived { 1 } else { 0 },
        time: notify::now_secs(),
        actions: vec![],
        link: if arrived { format!("mount:{}", d.device) } else { String::new() },
    };
    let _ = tx.send(NMsg::New(n));
}

pub fn spawn(tx: calloop::channel::Sender<NMsg>) {
    std::thread::spawn(move || {
        let Ok(conn) = Connection::system() else { return };
        let Ok(rule) = MatchRule::builder().msg_type(zbus::message::Type::Signal).sender("org.freedesktop.UDisks2").and_then(|b| b.interface("org.freedesktop.DBus.ObjectManager")).map(|b| b.build()) else { return };
        let Ok(iter) = MessageIterator::for_match_rule(rule, &conn, None) else { return };
        // what is plugged in, by object path, so a removal can say which drive went
        let mut known: HashMap<String, Drive> = HashMap::new();
        for msg in iter.flatten() {
            let hdr = msg.header();
            match hdr.member().map(|m| m.as_str()).unwrap_or("") {
                "InterfacesAdded" => {
                    let Ok((path, ifaces)) = msg.body().deserialize::<(OwnedObjectPath, Props)>() else { continue };
                    if let Some(d) = drive_from(&ifaces) {
                        announce(&tx, &d, true);
                        known.insert(path.to_string(), d);
                    }
                }
                "InterfacesRemoved" => {
                    let Ok((path, _)) = msg.body().deserialize::<(OwnedObjectPath, Vec<String>)>() else { continue };
                    if let Some(d) = known.remove(path.as_str()) {
                        announce(&tx, &d, false);
                    }
                }
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn ov<'a>(v: impl Into<Value<'a>>) -> OwnedValue {
        OwnedValue::try_from(v.into()).unwrap()
    }

    fn props(system: bool, with_fs: bool) -> Props {
        let mut block = HashMap::new();
        block.insert("HintSystem".to_string(), ov(system));
        block.insert("HintIgnore".to_string(), ov(false));
        block.insert("Device".to_string(), ov(b"/dev/sdb1\0".to_vec()));
        block.insert("IdLabel".to_string(), ov("BACKUP"));
        block.insert("Size".to_string(), ov(32_000_000_000u64));
        let mut p: Props = HashMap::new();
        p.insert("org.freedesktop.UDisks2.Block".into(), block);
        if with_fs {
            p.insert("org.freedesktop.UDisks2.Filesystem".into(), HashMap::new());
        }
        p
    }

    #[test]
    fn a_usb_stick_is_announced_a_system_disk_is_not() {
        assert_eq!(drive_from(&props(false, true)), Some(Drive { device: "/dev/sdb1".into(), label: "BACKUP".into(), size: 32_000_000_000 }));
        assert_eq!(drive_from(&props(true, true)), None, "system disks are not announced");
        assert_eq!(drive_from(&props(false, false)), None, "no file system: nothing to open");
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(size_text(32_000_000_000), "32 GB");
        assert_eq!(size_text(500_000_000), "500 MB");
    }
}
