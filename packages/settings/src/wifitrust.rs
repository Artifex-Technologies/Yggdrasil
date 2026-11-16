//! Heimdall's choices for one saved Wi-Fi network, shown on its page in Settings > Wi-Fi: its trust level (untrusted, trusted
//! or home) and whether others on the network may discover this computer there ("discoverable"). Heimdall's
//! docs/WIFI_TRUST.md, "The three levels" and "Discoverable".
//!
//! - **Read** without root: whether it is discoverable from `heimdall-wifi-guard discoverable --uuid UUID`
//!   (`NAME: discoverable on`, `on (home)` or `off`); the level from Heimdall's `/etc/heimdall/wifi-trusted.json` (readable by
//!   everyone, the file `heimdall-wifi-guard status` lists), and for the network connected now from the guard's
//!   `/run/heimdall-wifi-guard/state.json`, which knows when a trusted network does not count as trusted this time (another
//!   router for a home network, or a saved password that changed).
//! - **Change**: start `heimdall-wifi-discoverable-on@UUID.service` or `heimdall-wifi-discoverable-off@UUID.service` through
//!   systemd's D-Bus `StartUnit(name, "replace")`, allowing interactive authorization: polkit's own default for starting units
//!   (`org.freedesktop.systemd1.manage-units`, `auth_admin_keep`) asks for an administrator's password and keeps it for a few
//!   minutes (the owner's choice; Heimdall ships no rule of its own for it). The units run `heimdall-wifi-guard discoverable on|off --uuid UUID` as root; then the state is read again.
//!
//! A made-up system (fixture) answers the guard from its `run` entries, has Heimdall when its `tools` list the guard, has no
//! level, and refuses every change.

use std::time::{Duration, Instant};

/// Heimdall's guard, on PATH where Heimdall is installed.
pub const GUARD: &str = "heimdall-wifi-guard";
/// Where Heimdall keeps the networks it trusts, and the guard's state for the network connected now (both readable by all).
const TRUSTED: &str = "/etc/heimdall/wifi-trusted.json";
const STATE: &str = "/run/heimdall-wifi-guard/state.json";
/// Where the package puts the units (an administrator's own copy would be in /etc).
const UNIT_DIRS: [&str; 2] = ["/usr/lib/systemd/system", "/etc/systemd/system"];

pub const DISC_TITLE: &str = "Let this laptop be discovered on this network";
pub const DISC_ABOUT: &str = "Others on this network can find it for casting, printers, KDE Connect and Syncthing. Every other \
     protection stays on";

/// A network's trust level in Heimdall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Untrusted,
    Trusted,
    Home,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Untrusted => "Untrusted",
            Level::Trusted => "Trusted",
            Level::Home => "Home",
        }
    }

    /// The grey line under it: what the level means here.
    pub fn about(self) -> &'static str {
        match self {
            Level::Untrusted => "Every protection on, with a notice when you join",
            Level::Trusted => "No notices; every protection stays on (sudo heimdall-wifi-guard trust)",
            Level::Home => "Your home network: the device list and discovery are on (sudo heimdall-wifi-guard trust --home)",
        }
    }

    fn parse(word: &str) -> Option<Level> {
        match word {
            "untrusted" => Some(Level::Untrusted),
            "trusted" => Some(Level::Trusted),
            "home" => Some(Level::Home),
            _ => None,
        }
    }
}

/// Whether this computer may be discovered on the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disc {
    On,
    Off,
    /// A home network always is.
    Home,
}

/// Why the switch cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// No Heimdall on this computer.
    Heimdall,
    /// A Heimdall from before the per-network choice (no `discoverable` command, or no units).
    Newer,
}

impl Missing {
    pub fn note(self) -> &'static str {
        match self {
            Missing::Heimdall => "Needs Heimdall, which is not installed here.",
            Missing::Newer => "Needs a newer Heimdall (one with the per-network discoverable choice).",
        }
    }
}

/// What the page shows for one saved network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trust {
    pub uuid: String,
    /// None: not known (no Heimdall, or its file could not be read).
    pub level: Option<Level>,
    pub disc: Result<Disc, Missing>,
}

/// The guard's answer: `NAME: discoverable on`, `on (home)` or `off` (the name may itself hold ": discoverable ", so the end
/// decides). Anything else (an older guard's usage text, an error) is None.
pub fn parse_discoverable(text: &str) -> Option<Disc> {
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    let (_, word) = line.rsplit_once(": discoverable ")?;
    match word {
        "on" => Some(Disc::On),
        "off" => Some(Disc::Off),
        "on (home)" => Some(Disc::Home),
        _ => None,
    }
}

/// A network's level from Heimdall's trusted file, as the guard reads an entry: none is untrusted, an entry from before the
/// levels (no `level`) is home, and any `level` but "trusted" or "home" is untrusted. None when the file is not JSON.
pub fn level_in_file(text: &str, uuid: &str) -> Option<Level> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let Some(e) = v.as_object()?.get(uuid).and_then(|e| e.as_object()) else { return Some(Level::Untrusted) };
    Some(match e.get("level") {
        None => Level::Home,
        Some(l) => l.as_str().and_then(Level::parse).unwrap_or(Level::Untrusted),
    })
}

/// The guard's level for the network connected now (its state's `wifi` is this profile), which may be lower than the file's.
pub fn level_in_state(text: &str, uuid: &str) -> Option<Level> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("wifi")?.as_str()? != uuid {
        return None;
    }
    Level::parse(v.get("level")?.as_str()?)
}

/// The unit that turns it on or off for this profile; None for anything that is not a NetworkManager profile id (the guard
/// takes only lowercase hex digits and dashes there).
pub fn unit_name(uuid: &str, on: bool) -> Option<String> {
    let ok = uuid.len() == 36 && uuid.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-');
    ok.then(|| format!("heimdall-wifi-discoverable-{}@{uuid}.service", if on { "on" } else { "off" }))
}

fn units_present() -> bool {
    if crate::fixture::get().is_some() {
        return true;
    }
    ["on", "off"]
        .iter()
        .all(|w| UNIT_DIRS.iter().any(|d| std::path::Path::new(&format!("{d}/heimdall-wifi-discoverable-{w}@.service")).is_file()))
}

/// Read everything for one saved Wi-Fi network (on a background thread: it starts the guard).
pub fn load(uuid: &str) -> Trust {
    let uuid = uuid.to_string();
    if !crate::privpack::installed(GUARD) {
        return Trust { uuid, level: None, disc: Err(Missing::Heimdall) };
    }
    let level = if crate::fixture::get().is_some() {
        None
    } else {
        let now = std::fs::read_to_string(STATE).ok().and_then(|t| level_in_state(&t, &uuid));
        now.or_else(|| std::fs::read_to_string(TRUSTED).ok().and_then(|t| level_in_file(&t, &uuid)))
            // no file yet: Heimdall trusts nothing
            .or_else(|| (!std::path::Path::new(TRUSTED).exists()).then_some(Level::Untrusted))
    };
    let read = crate::sys::run(GUARD, &["discoverable", "--uuid", &uuid]).and_then(|t| parse_discoverable(&t));
    let disc = match read {
        Some(d) if units_present() => Ok(d),
        _ => Err(Missing::Newer),
    };
    Trust { uuid, level, disc }
}

/// Turn it on or off: start the unit over systemd's D-Bus, letting polkit ask for the password, and wait for its job to end.
/// Then `load` tells whether it took.
pub fn set(uuid: &str, on: bool) -> Result<(), String> {
    use zbus::blocking::{Connection, Proxy};
    use zbus::proxy::{CacheProperties, MethodFlags};
    use zbus::zvariant::OwnedObjectPath;
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    let unit = unit_name(uuid, on).ok_or("This network's profile id is not one Heimdall accepts.")?;
    let e = |e: zbus::Error| e.to_string();
    let conn = Connection::system().map_err(e)?;
    let systemd =
        Proxy::new(&conn, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager").map_err(e)?;
    let job: OwnedObjectPath =
        match systemd.call_with_flags("StartUnit", MethodFlags::AllowInteractiveAuth.into(), &(unit.as_str(), "replace")) {
            Ok(Some(j)) => j,
            Ok(None) => return Err("systemd gave no answer.".into()),
            Err(zbus::Error::MethodError(name, msg, _)) => return Err(start_error(name.as_str(), msg.as_deref().unwrap_or_default())),
            Err(err) => return Err(err.to_string()),
        };
    // the unit is a short oneshot: its job goes away when it has run (a minute at the most, then the page just reads again)
    let job = zbus::blocking::proxy::Builder::<Proxy>::new(&conn)
        .destination("org.freedesktop.systemd1")
        .and_then(|b| b.path(job))
        .and_then(|b| b.interface("org.freedesktop.systemd1.Job"))
        .map(|b| b.cache_properties(CacheProperties::No))
        .and_then(|b| b.build())
        .map_err(e)?;
    let until = Instant::now() + Duration::from_secs(60);
    while Instant::now() < until && job.get_property::<String>("State").is_ok() {
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// What a refused StartUnit means to the person.
fn start_error(name: &str, msg: &str) -> String {
    match name {
        "org.freedesktop.DBus.Error.AccessDenied" | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired" => {
            "Not changed: the administrator's password was not given.".into()
        }
        "org.freedesktop.systemd1.NoSuchUnit" => Missing::Newer.note().into(),
        _ if msg.is_empty() => name.to_string(),
        _ => msg.to_string(),
    }
}

/// After a change: an error when the state read again is not the one asked for.
pub fn check(t: &Trust, on: bool) -> Result<(), String> {
    match t.disc {
        Ok(Disc::On) if on => Ok(()),
        Ok(Disc::Off) if !on => Ok(()),
        Ok(Disc::Home) => Ok(()),
        Err(m) => Err(m.note().into()),
        Ok(_) => Err(format!(
            "Heimdall did not change it. journalctl -u {} says why.",
            unit_name(&t.uuid, on).unwrap_or_else(|| "heimdall-wifi-discoverable-*".into())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0d6f1c8e-3b2a-4c5d-9e8f-7a6b5c4d3e2f";

    #[test]
    fn the_guards_answer_parses() {
        assert_eq!(parse_discoverable("Birchwood Library: discoverable on\n"), Some(Disc::On));
        assert_eq!(parse_discoverable("Birchwood Library: discoverable off\n"), Some(Disc::Off));
        assert_eq!(parse_discoverable("Maple Street: discoverable on (home)\n"), Some(Disc::Home));
        assert_eq!(parse_discoverable("a: discoverable off: discoverable on"), Some(Disc::On), "the end decides");
        assert_eq!(parse_discoverable("usage: heimdall-wifi-guard pre-up|up|down ..."), None, "an older guard");
        assert_eq!(parse_discoverable(""), None);
        assert_eq!(parse_discoverable("x: discoverable maybe"), None);
    }

    #[test]
    fn levels_from_the_file_as_the_guard_reads_them() {
        let f = format!(
            r#"{{"{ID}": {{"ssid": "Birchwood Library", "level": "trusted"}}, "home-id": {{"ssid": "Maple Street"}},
               "odd": {{"level": 3}}, "disc": {{"ssid": "Cafe", "level": "untrusted", "discoverable": true}}}}"#
        );
        assert_eq!(level_in_file(&f, ID), Some(Level::Trusted));
        assert_eq!(level_in_file(&f, "home-id"), Some(Level::Home), "an entry from before the levels");
        assert_eq!(level_in_file(&f, "odd"), Some(Level::Untrusted));
        assert_eq!(level_in_file(&f, "disc"), Some(Level::Untrusted));
        assert_eq!(level_in_file(&f, "absent"), Some(Level::Untrusted));
        assert_eq!(level_in_file("not json", ID), None);
    }

    #[test]
    fn the_connected_networks_level_from_the_guards_state() {
        let s = format!(r#"{{"wifi": "{ID}", "level": "trusted", "trusted": false}}"#);
        assert_eq!(level_in_state(&s, ID), Some(Level::Trusted));
        assert_eq!(level_in_state(&s, "another"), None, "another network is connected");
        assert_eq!(level_in_state(r#"{"wifi": null, "level": null}"#, ID), None);
    }

    #[test]
    fn units_only_for_profile_ids() {
        assert_eq!(unit_name(ID, true).as_deref(), Some("heimdall-wifi-discoverable-on@0d6f1c8e-3b2a-4c5d-9e8f-7a6b5c4d3e2f.service"));
        assert_eq!(unit_name(ID, false).as_deref(), Some("heimdall-wifi-discoverable-off@0d6f1c8e-3b2a-4c5d-9e8f-7a6b5c4d3e2f.service"));
        assert_eq!(unit_name("../../etc/x", true), None);
        assert_eq!(unit_name(&ID.to_uppercase(), true), None, "the guard takes lowercase only");
        assert_eq!(unit_name("", true), None);
    }

    #[test]
    fn a_change_is_checked_against_what_is_read_again() {
        let t = |disc| Trust { uuid: ID.into(), level: None, disc };
        assert_eq!(check(&t(Ok(Disc::On)), true), Ok(()));
        assert_eq!(check(&t(Ok(Disc::Off)), false), Ok(()));
        assert!(check(&t(Ok(Disc::Off)), true).unwrap_err().contains("journalctl -u heimdall-wifi-discoverable-on@"));
        assert_eq!(check(&t(Err(Missing::Newer)), true), Err(Missing::Newer.note().into()));
        assert_eq!(start_error("org.freedesktop.DBus.Error.AccessDenied", "x"), "Not changed: the administrator's password was not given.");
    }
}
