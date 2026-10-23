//! The Wi-Fi and VPN password agent: NetworkManager asks it when a connection needs a password it does not have (a new network, a
//! changed password, a VPN that asks each time). It opens the same box as the administrator password (one process per question),
//! in its "network" form, and hands the answer to NetworkManager. It stores nothing: NetworkManager keeps the passwords it is meant
//! to keep. Only NetworkManager itself may ask: a call from any other program is refused, so nothing can show a fake Wi-Fi box.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

use crate::agent::Bridge;

pub const OBJECT_PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
const NM: &str = "org.freedesktop.NetworkManager";
/// NetworkManager may show a prompt for this request.
const ALLOW_INTERACTION: u32 = 0x1;
/// The secrets NetworkManager had were refused: ask for new ones (and say so).
const REQUEST_NEW: u32 = 0x2;

type Settings = HashMap<String, HashMap<String, OwnedValue>>;

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.NetworkManager.SecretAgent")]
pub enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NoSecrets(String),
    UserCanceled(String),
    PermissionDenied(String),
}

/// What to ask for one request: the box's title and text, and the secrets as (key in the reply, label, shown while typing).
#[derive(Debug, PartialEq)]
pub struct Ask {
    pub title: String,
    pub summary: String,
    pub fields: Vec<(String, String, bool)>,
    /// VPN secrets go back inside the setting's "secrets" dictionary (a{ss}), everything else as plain strings.
    pub vpn: bool,
}

fn s(settings: &Settings, group: &str, key: &str) -> String {
    settings.get(group).and_then(|g| g.get(key)).and_then(|v| <&str>::try_from(v).ok()).unwrap_or("").to_string()
}

/// The network's name: the Wi-Fi SSID when there is one, else the connection's name.
fn network_name(c: &Settings) -> String {
    let ssid = c.get("802-11-wireless").and_then(|g| g.get("ssid")).and_then(|v| Vec::<u8>::try_from(v.try_clone().ok()?).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    if ssid.is_empty() { s(c, "connection", "id") } else { ssid }
}

/// Works out what to ask for (None: nothing this agent can answer).
pub fn plan(c: &Settings, setting: &str, hints: &[String], flags: u32) -> Option<Ask> {
    let name = network_name(c);
    let again = flags & REQUEST_NEW != 0;
    let summary = |what: &str| if again { format!("The password for “{name}” was not accepted. Enter it again to connect.") } else { format!("Enter the {what} for “{name}”.") };
    let one = |key: &str, label: &str| vec![(key.to_string(), label.to_string(), false)];
    match setting {
        "802-11-wireless-security" => {
            let key = match s(c, "802-11-wireless-security", "key-mgmt").as_str() {
                "none" => "wep-key0",
                "wpa-eap" | "wpa-eap-suite-b-192" => return None, // asked for under 802-1x
                _ => "psk",
            };
            Some(Ask { title: "Wi-Fi password needed".into(), summary: summary("Wi-Fi password"), fields: one(key, "Password"), vpn: false })
        }
        "802-1x" => {
            let mut fields = vec![];
            if s(c, "802-1x", "identity").is_empty() {
                fields.push(("identity".into(), "User name".into(), true));
            }
            fields.push(("password".into(), "Password".into(), false));
            Some(Ask { title: "Network sign-in".into(), summary: summary("user name and password"), fields, vpn: false })
        }
        "vpn" => {
            // the VPN plugin names what it needs in the hints (often just "password"); without hints, the usual one
            let keys: Vec<&str> = hints.iter().map(|h| h.as_str()).filter(|h| !h.starts_with("x-vpn-message:")).collect();
            let fields = if keys.is_empty() { one("password", "Password") } else { keys.iter().map(|k| (k.to_string(), label_for(k), false)).collect() };
            Some(Ask { title: "VPN password needed".into(), summary: summary("VPN password"), fields, vpn: true })
        }
        "gsm" | "cdma" => Some(Ask { title: "Mobile network password".into(), summary: summary("password"), fields: one("password", "Password"), vpn: false }),
        "pppoe" => Some(Ask { title: "Connection password".into(), summary: summary("password"), fields: one("password", "Password"), vpn: false }),
        _ => None,
    }
}

fn label_for(key: &str) -> String {
    match key {
        "password" => "Password".into(),
        "cert-pass" => "Certificate password".into(),
        "otp" | "challenge" => "One-time code".into(),
        k => k.replace(['-', '_'], " "),
    }
}

/// The reply NetworkManager expects: {setting: {key: value}}, or for a VPN {"vpn": {"secrets": {key: value}}}.
pub fn reply(setting: &str, ask: &Ask, answers: &[String]) -> Settings {
    let pairs = ask.fields.iter().zip(answers).map(|((k, _, _), v)| (k.clone(), v.clone()));
    let inner: HashMap<String, OwnedValue> = if ask.vpn {
        let secrets: HashMap<String, String> = pairs.collect();
        HashMap::from([("secrets".to_string(), Value::from(secrets).try_to_owned().expect("plain values"))])
    } else {
        pairs.map(|(k, v)| (k, OwnedValue::try_from(Value::from(v)).expect("a string"))).collect()
    };
    HashMap::from([(setting.to_string(), inner)])
}

pub struct NetAgent {
    /// The box of the question being answered (its pid), so NetworkManager's cancel can close it.
    child: Arc<Mutex<Option<u32>>>,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.SecretAgent")]
impl NetAgent {
    async fn get_secrets(&self, connection: Settings, _connection_path: zbus::zvariant::OwnedObjectPath, setting_name: String, hints: Vec<String>, flags: u32, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> Result<Settings, AgentError> {
        if !from_network_manager(conn, &hdr).await {
            return Err(AgentError::PermissionDenied("only NetworkManager may ask".into()));
        }
        if flags & ALLOW_INTERACTION == 0 {
            return Err(AgentError::NoSecrets("this agent keeps no passwords; it only asks".into()));
        }
        let Some(ask) = plan(&connection, &setting_name, &hints, flags) else {
            return Err(AgentError::NoSecrets(format!("nothing to ask for {setting_name}")));
        };
        let slot = self.child.clone();
        let answers = blocking::unblock(move || {
            let r = ask_in_box(&ask, &slot);
            (ask, r)
        })
        .await;
        match answers {
            (ask, Some(a)) => Ok(reply(&setting_name, &ask, &a)),
            (_, None) => Err(AgentError::UserCanceled("cancelled".into())),
        }
    }

    fn cancel_get_secrets(&self, _connection_path: zbus::zvariant::OwnedObjectPath, _setting_name: String) {
        if let Some(pid) = self.child.lock().unwrap().take() {
            unsafe { libc::kill(pid as i32, 15) };
        }
    }

    // NetworkManager keeps the secrets of its own connections; this agent stores nothing
    fn save_secrets(&self, _connection: Settings, _connection_path: zbus::zvariant::OwnedObjectPath) {}

    fn delete_secrets(&self, _connection: Settings, _connection_path: zbus::zvariant::OwnedObjectPath) {}
}

/// The caller is the program that owns NetworkManager's name on the system bus.
async fn from_network_manager(conn: &zbus::Connection, hdr: &zbus::message::Header<'_>) -> bool {
    let Some(sender) = hdr.sender() else { return false };
    let Ok(dbus) = zbus::fdo::DBusProxy::new(conn).await else { return false };
    let Ok(name) = zbus::names::BusName::try_from(NM) else { return false };
    dbus.get_name_owner(name).await.map(|o| o.as_str() == sender.as_str()).unwrap_or(false)
}

/// Opens the box in its network form and collects one answer per field; None when cancelled.
fn ask_in_box(ask: &Ask, slot: &Arc<Mutex<Option<u32>>>) -> Option<Vec<String>> {
    let mut bridge = Bridge::open(slot).ok()?;
    bridge.send(serde_json::json!({"t": "begin", "kind": "network", "title": ask.title, "message": ask.summary, "action_id": "", "user": ""}));
    let mut out = vec![];
    for (_, label, echo) in &ask.fields {
        match crate::helper::Ui::prompt(&mut bridge, label, *echo) {
            Some(a) => out.push(a),
            None => {
                bridge.close(slot);
                return None;
            }
        }
    }
    bridge.close(slot);
    Some(out)
}

/// Exports the agent on the system bus and registers it with NetworkManager. Keeps the connection alive for the caller.
pub fn register() -> Result<Connection, String> {
    let conn = Connection::system().map_err(|e| e.to_string())?;
    conn.object_server().at(OBJECT_PATH, NetAgent { child: Arc::new(Mutex::new(None)) }).map_err(|e| e.to_string())?;
    let p = zbus::blocking::Proxy::new(&conn, NM, "/org/freedesktop/NetworkManager/AgentManager", "org.freedesktop.NetworkManager.AgentManager").map_err(|e| e.to_string())?;
    p.call::<_, _, ()>("Register", &("org.yggdrasil.agent",)).map_err(|e| e.to_string())?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(pairs: &[(&str, &str, Value<'static>)]) -> Settings {
        let mut c: Settings = HashMap::new();
        for (g, k, v) in pairs {
            c.entry(g.to_string()).or_default().insert(k.to_string(), v.try_to_owned().unwrap());
        }
        c
    }

    #[test]
    fn wifi_asks_for_the_psk_and_names_the_network() {
        let c = conn(&[("connection", "id", Value::from("Home")), ("802-11-wireless", "ssid", Value::from(b"Cafe".to_vec())), ("802-11-wireless-security", "key-mgmt", Value::from("wpa-psk"))]);
        let a = plan(&c, "802-11-wireless-security", &[], ALLOW_INTERACTION).unwrap();
        assert_eq!(a.fields, vec![("psk".to_string(), "Password".to_string(), false)]);
        assert!(a.summary.contains("“Cafe”"), "{}", a.summary);
        assert!(plan(&c, "802-11-wireless-security", &[], ALLOW_INTERACTION | REQUEST_NEW).unwrap().summary.contains("not accepted"));
        let r = reply("802-11-wireless-security", &a, &["hunter22".into()]);
        assert_eq!(<&str>::try_from(&r["802-11-wireless-security"]["psk"]).unwrap(), "hunter22");
    }

    #[test]
    fn enterprise_wifi_is_asked_under_802_1x_and_vpn_secrets_are_nested() {
        let c = conn(&[("connection", "id", Value::from("Work")), ("802-11-wireless-security", "key-mgmt", Value::from("wpa-eap"))]);
        assert!(plan(&c, "802-11-wireless-security", &[], ALLOW_INTERACTION).is_none());
        let a = plan(&c, "802-1x", &[], ALLOW_INTERACTION).unwrap();
        assert_eq!(a.fields.len(), 2, "no identity in the profile: ask for it too");
        let v = conn(&[("connection", "id", Value::from("Office VPN"))]);
        let a = plan(&v, "vpn", &["password".into(), "x-vpn-message:Hello".into()], ALLOW_INTERACTION).unwrap();
        assert_eq!(a.fields.len(), 1);
        let r = reply("vpn", &a, &["s3cret".into()]);
        let secrets: HashMap<String, String> = r["vpn"]["secrets"].try_clone().unwrap().try_into().unwrap();
        assert_eq!(secrets["password"], "s3cret");
    }

    #[test]
    fn unknown_settings_are_not_answered() {
        assert!(plan(&HashMap::new(), "bluetooth", &[], ALLOW_INTERACTION).is_none());
    }
}
