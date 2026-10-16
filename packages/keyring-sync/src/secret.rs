//! gnome-keyring's login keyring, through the Secret Service (org.freedesktop.secrets) and gnome-keyring's own "internal" interface,
//! the one Seahorse uses to change a keyring's password. Passwords are only ever held for the duration of a call.

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zeroize::Zeroize;

const SERVICE: &str = "org.freedesktop.secrets";
const ROOT: &str = "/org/freedesktop/secrets";
const INTERNAL: &str = "org.gnome.keyring.InternalUnsupportedGuiltRiddenInterface";

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    /// No secret service is running (nothing to fix).
    NoService,
    /// There is no default keyring yet.
    NoKeyring,
    Unlocked,
    Locked,
}

pub struct Keyring {
    conn: Connection,
    pub collection: OwnedObjectPath,
}

type SecretStruct = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

fn plain_secret(session: &OwnedObjectPath, password: &str) -> SecretStruct {
    (session.clone(), vec![], password.as_bytes().to_vec(), "text/plain".into())
}

impl Keyring {
    /// Connects to the secret service and finds the default (login) keyring.
    pub fn open() -> Result<Keyring, Status> {
        let conn = Connection::session().map_err(|_| Status::NoService)?;
        let svc = Proxy::new(&conn, SERVICE, ROOT, "org.freedesktop.Secret.Service").map_err(|_| Status::NoService)?;
        let default: OwnedObjectPath = svc.call("ReadAlias", &("default",)).map_err(|_| Status::NoService)?;
        if default.as_str() == "/" {
            return Err(Status::NoKeyring);
        }
        Ok(Keyring { conn, collection: default })
    }

    pub fn status(&self) -> Status {
        let p = Proxy::new(&self.conn, SERVICE, self.collection.as_str(), "org.freedesktop.DBus.Properties");
        let locked: Result<OwnedValue, _> = p.and_then(|p| p.call("Get", &("org.freedesktop.Secret.Collection", "Locked")));
        match locked.ok().and_then(|v| bool::try_from(v).ok()) {
            Some(true) => Status::Locked,
            Some(false) => Status::Unlocked,
            None => Status::NoService,
        }
    }

    fn session(&self) -> Result<OwnedObjectPath, String> {
        let svc = Proxy::new(&self.conn, SERVICE, ROOT, "org.freedesktop.Secret.Service").map_err(|e| e.to_string())?;
        let (_, session): (OwnedValue, OwnedObjectPath) = svc.call("OpenSession", &("plain", Value::from(""))).map_err(|e| e.to_string())?;
        Ok(session)
    }

    /// Opens the locked keyring with `password`.
    pub fn unlock(&self, password: &str) -> Result<(), String> {
        let session = self.session()?;
        let mut secret = plain_secret(&session, password);
        let internal = Proxy::new(&self.conn, SERVICE, ROOT, INTERNAL).map_err(|e| e.to_string())?;
        let r: Result<(), zbus::Error> = internal.call("UnlockWithMasterPassword", &(&self.collection, &secret));
        secret.2.zeroize();
        r.map_err(|e| e.to_string())?;
        if self.status() == Status::Unlocked {
            Ok(())
        } else {
            Err("wrong password".into())
        }
    }

    /// Changes the keyring's password from `old` to `new` (the keyring is opened with `old` first if it is locked).
    pub fn change_password(&self, old: &str, new: &str) -> Result<(), String> {
        let session = self.session()?;
        let mut a = plain_secret(&session, old);
        let mut b = plain_secret(&session, new);
        let internal = Proxy::new(&self.conn, SERVICE, ROOT, INTERNAL).map_err(|e| e.to_string())?;
        let r: Result<(), zbus::Error> = internal.call("ChangeWithMasterPassword", &(&self.collection, &a, &b));
        a.2.zeroize();
        b.2.zeroize();
        r.map_err(|e| {
            let t = e.to_string();
            if t.contains("Denied") || t.contains("denied") || t.contains("Invalid") {
                "the previous password is not right".to_string()
            } else {
                t
            }
        })?;
        // open it with the new password, so apps can use it at once (the change itself leaves it locked)
        if self.status() == Status::Locked {
            self.unlock(new)?;
        }
        Ok(())
    }

    /// How many items are in the keyring (readable only when it is unlocked; labels, not secrets).
    pub fn item_count(&self) -> Option<usize> {
        let p = Proxy::new(&self.conn, SERVICE, self.collection.as_str(), "org.freedesktop.DBus.Properties").ok()?;
        let v: OwnedValue = p.call("Get", &("org.freedesktop.Secret.Collection", "Items")).ok()?;
        Vec::<OwnedObjectPath>::try_from(v).ok().map(|l| l.len())
    }
}

/// Starts over with an empty login keyring protected by `password`: the old file is moved aside (not deleted), the daemon restarted.
pub fn start_fresh(password: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))).ok_or("no data folder")?.join("keyrings");
    let old = dir.join("login.keyring");
    let moved = dir.join(format!("login.keyring.old-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)));
    if old.exists() {
        std::fs::rename(&old, &moved).map_err(|e| e.to_string())?;
    }
    let _ = Command::new("pkill").args(["-x", "gnome-keyring-d"]).status();
    std::thread::sleep(std::time::Duration::from_millis(800));
    let mut child = Command::new("setsid").arg("gnome-keyring-daemon").arg("--login").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    child.stdin.take().ok_or("no stdin")?.write_all(password.as_bytes()).map_err(|e| e.to_string())?;
    let _ = child.wait();
    let _ = Command::new("setsid").args(["-f", "gnome-keyring-daemon", "--start", "--components=secrets"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    Ok(moved.display().to_string())
}
