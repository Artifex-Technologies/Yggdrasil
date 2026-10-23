//! The Bluetooth pairing agent: when a device pairs, BlueZ asks it to show or confirm a code, or to type one. It opens the same box
//! as the other questions (one process per question). Only BlueZ itself may ask: a call from any other program is refused.
//! Already paired devices may use their services without asking; anything else is asked about.

use std::sync::{Arc, Mutex};

use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

use crate::agent::Bridge;

pub const OBJECT_PATH: &str = "/org/yggdrasil/BluetoothAgent";
const BLUEZ: &str = "org.bluez";

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
pub enum BtError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
    Canceled(String),
}

/// A six-digit passkey as people read it ("012345").
pub fn passkey_text(p: u32) -> String {
    format!("{:06}", p % 1_000_000)
}

/// A PIN typed for an old device: digits or letters, 1 to 16 characters.
pub fn valid_pin(s: &str) -> bool {
    (1..=16).contains(&s.chars().count()) && s.chars().all(|c| c.is_ascii_alphanumeric())
}

pub struct BtAgent {
    /// The box being shown (its pid), so BlueZ's Cancel can close it.
    child: Arc<Mutex<Option<u32>>>,
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl BtAgent {
    fn release(&self) {}

    async fn request_pin_code(&self, device: OwnedObjectPath, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> Result<String, BtError> {
        check(conn, &hdr).await?;
        let name = device_name(conn, &device).await;
        let slot = self.child.clone();
        let pin = blocking::unblock(move || ask_text(&slot, &format!("Pairing with “{name}”"), "Type the PIN shown on the device, or the one in its manual (often 0000).", "PIN")).await;
        pin.filter(|p| valid_pin(p)).ok_or_else(|| BtError::Rejected("no PIN".into()))
    }

    async fn display_pin_code(&self, device: OwnedObjectPath, pincode: String, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> Result<(), BtError> {
        check(conn, &hdr).await?;
        let name = device_name(conn, &device).await;
        let slot = self.child.clone();
        blocking::unblock(move || show(&slot, &format!("Pairing with “{name}”"), &format!("Type {pincode} on “{name}”, then Enter."))).await;
        Ok(())
    }

    async fn request_passkey(&self, device: OwnedObjectPath, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> Result<u32, BtError> {
        check(conn, &hdr).await?;
        let name = device_name(conn, &device).await;
        let slot = self.child.clone();
        let t = blocking::unblock(move || ask_text(&slot, &format!("Pairing with “{name}”"), "Type the six-digit code shown on the device.", "Code")).await;
        t.and_then(|t| t.trim().parse::<u32>().ok()).filter(|n| *n <= 999_999).ok_or_else(|| BtError::Rejected("no code".into()))
    }

    async fn display_passkey(&self, device: OwnedObjectPath, passkey: u32, entered: u16, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> Result<(), BtError> {
        check(conn, &hdr).await?;
        // called again for every key typed on the device; the box is shown once
        if entered == 0 {
            let name = device_name(conn, &device).await;
            let slot = self.child.clone();
            let code = passkey_text(passkey);
            blocking::unblock(move || show(&slot, &format!("Pairing with “{name}”"), &format!("Type {code} on “{name}”, then Enter."))).await;
        }
        Ok(())
    }

    async fn request_confirmation(&self, device: OwnedObjectPath, passkey: u32, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> Result<(), BtError> {
        check(conn, &hdr).await?;
        let name = device_name(conn, &device).await;
        let slot = self.child.clone();
        let code = passkey_text(passkey);
        let yes = blocking::unblock(move || confirm(&slot, &format!("Pair with “{name}”?"), &format!("Check that “{name}” shows {code}."), "Pair")).await;
        if yes { Ok(()) } else { Err(BtError::Rejected("not confirmed".into())) }
    }

    async fn request_authorization(&self, device: OwnedObjectPath, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> Result<(), BtError> {
        check(conn, &hdr).await?;
        let name = device_name(conn, &device).await;
        let slot = self.child.clone();
        let yes = blocking::unblock(move || confirm(&slot, &format!("Pair with “{name}”?"), &format!("“{name}” wants to pair with this computer."), "Pair")).await;
        if yes { Ok(()) } else { Err(BtError::Rejected("not allowed".into())) }
    }

    async fn authorize_service(&self, device: OwnedObjectPath, _uuid: String, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> Result<(), BtError> {
        check(conn, &hdr).await?;
        // a device you paired (or marked trusted) may use its services; anything else is asked about
        if device_flag(conn, &device, "Paired").await || device_flag(conn, &device, "Trusted").await {
            return Ok(());
        }
        let name = device_name(conn, &device).await;
        let slot = self.child.clone();
        let yes = blocking::unblock(move || confirm(&slot, &format!("Allow “{name}”?"), &format!("“{name}”, which is not paired, wants to connect to this computer."), "Allow")).await;
        if yes { Ok(()) } else { Err(BtError::Rejected("not allowed".into())) }
    }

    fn cancel(&self) {
        if let Some(pid) = self.child.lock().unwrap().take() {
            unsafe { libc::kill(pid as i32, 15) };
        }
    }
}

/// Only BlueZ (the owner of org.bluez on the system bus) may ask.
async fn check(conn: &zbus::Connection, hdr: &zbus::message::Header<'_>) -> Result<(), BtError> {
    let ok = async {
        let sender = hdr.sender()?;
        let dbus = zbus::fdo::DBusProxy::new(conn).await.ok()?;
        let owner = dbus.get_name_owner(zbus::names::BusName::try_from(BLUEZ).ok()?).await.ok()?;
        Some(owner.as_str() == sender.as_str())
    };
    if ok.await == Some(true) { Ok(()) } else { Err(BtError::Rejected("only BlueZ may ask".into())) }
}

async fn device_prop(conn: &zbus::Connection, device: &OwnedObjectPath, prop: &str) -> Option<zbus::zvariant::OwnedValue> {
    let p = zbus::fdo::PropertiesProxy::builder(conn).destination(BLUEZ).ok()?.path(device.as_ref()).ok()?.build().await.ok()?;
    p.get(zbus::names::InterfaceName::try_from("org.bluez.Device1").ok()?, prop).await.ok()
}

async fn device_name(conn: &zbus::Connection, device: &OwnedObjectPath) -> String {
    let n = device_prop(conn, device, "Alias").await.and_then(|v| String::try_from(v).ok()).unwrap_or_default();
    let n: String = n.chars().filter(|c| !c.is_control()).take(60).collect();
    if n.trim().is_empty() { "a Bluetooth device".into() } else { n }
}

async fn device_flag(conn: &zbus::Connection, device: &OwnedObjectPath, prop: &str) -> bool {
    device_prop(conn, device, prop).await.and_then(|v| bool::try_from(v).ok()).unwrap_or(false)
}

fn ask_text(slot: &Arc<Mutex<Option<u32>>>, title: &str, text: &str, label: &str) -> Option<String> {
    let mut b = Bridge::open(slot).ok()?;
    b.send(serde_json::json!({"t": "begin", "kind": "bluetooth", "title": title, "message": text, "action_id": "", "user": ""}));
    let a = crate::helper::Ui::prompt(&mut b, label, true);
    b.close(slot);
    a
}

fn confirm(slot: &Arc<Mutex<Option<u32>>>, title: &str, text: &str, button: &str) -> bool {
    let Ok(mut b) = Bridge::open(slot) else { return false };
    b.send(serde_json::json!({"t": "begin", "kind": "bluetooth", "title": title, "message": text, "action_id": "", "user": ""}));
    let yes = b.confirm(button);
    b.close(slot);
    yes
}

/// Shows a code until BlueZ cancels (pairing finished or failed) or the person closes the box.
fn show(slot: &Arc<Mutex<Option<u32>>>, title: &str, text: &str) {
    let Ok(mut b) = Bridge::open(slot) else { return };
    b.send(serde_json::json!({"t": "begin", "kind": "bluetooth", "title": title, "message": text, "action_id": "", "user": ""}));
    b.confirm("Close");
    b.close(slot);
}

/// Exports the agent on the system bus and makes it BlueZ's default agent. Keeps the connection alive for the caller.
pub fn register() -> Result<Connection, String> {
    let conn = Connection::system().map_err(|e| e.to_string())?;
    conn.object_server().at(OBJECT_PATH, BtAgent { child: Arc::new(Mutex::new(None)) }).map_err(|e| e.to_string())?;
    let p = zbus::blocking::Proxy::new(&conn, BLUEZ, "/org/bluez", "org.bluez.AgentManager1").map_err(|e| e.to_string())?;
    let path = zbus::zvariant::ObjectPath::try_from(OBJECT_PATH).map_err(|e| e.to_string())?;
    p.call::<_, _, ()>("RegisterAgent", &(&path, "KeyboardDisplay")).map_err(|e| e.to_string())?;
    let _ = p.call::<_, _, ()>("RequestDefaultAgent", &(&path,));
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_read_like_codes() {
        assert_eq!(passkey_text(42), "000042");
        assert_eq!(passkey_text(999_999), "999999");
        assert!(valid_pin("0000") && valid_pin("abc123"));
        assert!(!valid_pin("") && !valid_pin("12 34") && !valid_pin(&"1".repeat(17)));
    }
}
