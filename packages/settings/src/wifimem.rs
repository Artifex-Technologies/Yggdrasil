//! Wi-Fi & Network: what Settings remembers between visits, for a network's own page. NetworkManager keeps its leases where
//! only the system can read them, and the kernel forgets an access point half a minute after it was last heard, so Settings
//! notes for itself:
//! - each network's last lease (address, subnet, router, DNS) while it is connected, for its page when it is not;
//! - when each access point was first and last heard, for "first seen" and "last seen".
//!
//! One file, `$XDG_STATE_HOME/yggdrasil/wifi-memory.json` (~/.local/state/...), readable only by its owner. It stays on this
//! computer. Access points not heard for 30 days are dropped, and it never holds more than `MAX_SEEN` of them; a forgotten
//! network's lease goes with it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

const KEEP_SECS: i64 = 30 * 86_400;
const MAX_SEEN: usize = 2000;

/// A connected network's lease as it was last seen.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Lease {
    /// When it was noted (seconds since 1970).
    pub time: i64,
    #[serde(default)]
    pub v4: Vec<String>,
    #[serde(default)]
    pub subnet: String,
    #[serde(default)]
    pub router: String,
    #[serde(default)]
    pub dns: Vec<String>,
    #[serde(default)]
    pub v6: Vec<String>,
    #[serde(default)]
    pub lease_secs: Option<u64>,
    #[serde(default)]
    pub dhcp_server: String,
}

impl Lease {
    pub fn from_live(l: &crate::netconf::Live, now: i64) -> Lease {
        Lease {
            time: now,
            v4: l.v4.clone(),
            subnet: l.v4.first().and_then(|a| crate::netconf::subnet(a)).unwrap_or_default(),
            router: l.router.clone(),
            dns: l.dns.clone(),
            v6: l.v6.clone(),
            lease_secs: l.lease_secs,
            dhcp_server: l.dhcp_server.clone(),
        }
    }

    /// The same addresses and servers (the time aside).
    fn same(&self, o: &Lease) -> bool {
        (&self.v4, &self.subnet, &self.router, &self.dns, &self.v6, self.lease_secs, &self.dhcp_server) == (&o.v4, &o.subnet, &o.router, &o.dns, &o.v6, o.lease_secs, &o.dhcp_server)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Memory {
    /// Access point address ("AA:BB:CC:DD:EE:FF") -> (first heard, last heard), seconds since 1970.
    #[serde(default)]
    pub seen: BTreeMap<String, (i64, i64)>,
    /// Saved network (NetworkManager profile id) -> its last lease.
    #[serde(default)]
    pub leases: BTreeMap<String, Lease>,
}

impl Memory {
    /// Notes an access point heard at `at`. True when the file should be written (a new one, or last heard moved by more than
    /// ten minutes: not on every look).
    pub fn note_seen(&mut self, bssid: &str, at: i64) -> bool {
        match self.seen.get_mut(bssid) {
            Some((first, last)) => {
                let moved = at - *last > 600;
                *first = (*first).min(at);
                *last = (*last).max(at);
                moved
            }
            None => {
                self.seen.insert(bssid.to_string(), (at, at));
                true
            }
        }
    }

    /// Notes a connected network's lease. True when it changed, or the last note is over an hour old.
    pub fn note_lease(&mut self, uuid: &str, lease: Lease) -> bool {
        match self.leases.get(uuid) {
            Some(old) if old.same(&lease) && lease.time - old.time < 3600 => false,
            _ => {
                self.leases.insert(uuid.to_string(), lease);
                true
            }
        }
    }

    /// Drops access points not heard for 30 days (and the oldest past `MAX_SEEN`), and the leases of networks no longer saved.
    /// True when anything went.
    pub fn prune(&mut self, now: i64, saved: &[&str]) -> bool {
        let before = (self.seen.len(), self.leases.len());
        self.seen.retain(|_, (_, last)| now - *last <= KEEP_SECS);
        if self.seen.len() > MAX_SEEN {
            let mut by_age: Vec<(i64, String)> = self.seen.iter().map(|(k, (_, l))| (*l, k.clone())).collect();
            by_age.sort();
            for (_, k) in by_age.into_iter().take(self.seen.len() - MAX_SEEN) {
                self.seen.remove(&k);
            }
        }
        self.leases.retain(|u, _| saved.contains(&u.as_str()));
        before != (self.seen.len(), self.leases.len())
    }

    /// When an access point was first and last heard.
    pub fn heard(&self, bssid: &str) -> Option<(i64, i64)> {
        self.seen.get(bssid).copied()
    }
}

pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
    });
    base.join("yggdrasil/wifi-memory.json")
}

pub fn load() -> Memory {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Writes it whole (a new file renamed over the old), readable only by its owner.
pub fn save(m: &Memory) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let p = path();
    let dir = p.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".wifi-memory.{}.tmp", std::process::id()));
    let text = serde_json::to_string(m).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
    f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seen_first_and_last() {
        let mut m = Memory::default();
        assert!(m.note_seen("02:00:00:00:00:01", 1000));
        assert!(!m.note_seen("02:00:00:00:00:01", 1300), "five minutes later: not worth a write");
        assert!(m.note_seen("02:00:00:00:00:01", 2000));
        assert_eq!(m.heard("02:00:00:00:00:01"), Some((1000, 2000)));
        m.note_seen("02:00:00:00:00:01", 500);
        assert_eq!(m.heard("02:00:00:00:00:01"), Some((500, 2000)), "an earlier sighting moves first seen back");
    }

    #[test]
    fn leases_kept_while_saved_and_old_sightings_dropped() {
        let mut m = Memory::default();
        let l = Lease { time: 10_000, v4: vec!["192.168.1.20/24".into()], subnet: "192.168.1.0/24".into(), router: "192.168.1.1".into(), ..Default::default() };
        assert!(m.note_lease("u1", l.clone()));
        assert!(!m.note_lease("u1", Lease { time: 10_100, ..l.clone() }), "the same lease a minute later");
        assert!(m.note_lease("u1", Lease { time: 10_100, router: "192.168.1.254".into(), ..l.clone() }), "a new router");
        m.note_seen("02:00:00:00:00:01", 0);
        m.note_seen("02:00:00:00:00:02", 40 * 86_400);
        assert!(m.prune(40 * 86_400, &["u1"]));
        assert_eq!(m.seen.len(), 1, "the one not heard for 40 days went");
        assert!(m.prune(40 * 86_400, &[]), "a forgotten network's lease goes");
        assert!(m.leases.is_empty());
        for i in 0..(MAX_SEEN + 5) {
            m.note_seen(&format!("02:00:00:00:{:02X}:{:02X}", i / 256, i % 256), 40 * 86_400 + i as i64);
        }
        m.prune(40 * 86_400 + 10_000, &[]);
        assert_eq!(m.seen.len(), MAX_SEEN);
        assert!(!m.seen.contains_key("02:00:00:00:00:00"), "the oldest went first");
    }

    #[test]
    fn written_for_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("wifimem-test-{}", std::process::id()));
        let old = std::env::var_os("XDG_STATE_HOME");
        std::env::set_var("XDG_STATE_HOME", &dir);
        let mut m = Memory::default();
        m.note_seen("02:00:00:00:00:01", 5);
        save(&m).unwrap();
        assert_eq!(load(), m);
        assert_eq!(std::fs::metadata(path()).unwrap().permissions().mode() & 0o777, 0o600);
        match old {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
