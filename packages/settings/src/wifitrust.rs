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
//! - **Kind** of a trusted or home network ("This network is": home router, mesh, phone hotspot, travel, other; Heimdall's
//!   "What kind of network a trusted one is"): read with `heimdall-wifi-guard kind --uuid UUID` (`NAME: kind KIND`), set with
//!   `pkexec /usr/bin/heimdall-wifi-guard kind KIND --uuid UUID` (polkit's default for pkexec: the administrator's password).
//!   The guard refuses an untrusted network, so the page does not ask it then.
//!
//! - **A wired network** (Heimdall's docs/WIFI_TRUST.md, "Wired networks"): untrusted unless made home, which binds it to
//!   its router. Read from `heimdall-wifi-guard status` (no root): the list line `  NAME: home (wired), router ...` and, for the
//!   wired network connected now, `wired now: NAME, LEVEL; a hardware address of its own here: yes|no`. Made home with `pkexec
//!   /usr/bin/heimdall-wifi-guard trust --home [--real-address] --uuid UUID` (only while it is the one connected: the guard
//!   refuses another), back to untrusted with `pkexec /usr/bin/heimdall-wifi-guard untrust UUID`; then read again.
//!
//! A made-up system (fixture) answers the guard from its `run` entries, has Heimdall when its `tools` list the guard, has no
//! level, and refuses every change.

use std::time::{Duration, Instant};

/// Heimdall's guard, on PATH where Heimdall is installed.
pub const GUARD: &str = "heimdall-wifi-guard";
/// Where Heimdall keeps the networks it trusts, and the guard's state for the network connected now (both readable by all).
pub const TRUSTED: &str = "/etc/heimdall/wifi-trusted.json";
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

/// What kind of network a trusted one is (the owner's choice; Heimdall judges a different router on it by this).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetKind {
    HomeRouter,
    Mesh,
    PhoneHotspot,
    Travel,
    Other,
}

impl NetKind {
    pub const ALL: [NetKind; 5] = [NetKind::HomeRouter, NetKind::Mesh, NetKind::PhoneHotspot, NetKind::Travel, NetKind::Other];

    /// The guard's word for it.
    pub fn word(self) -> &'static str {
        match self {
            NetKind::HomeRouter => "home-router",
            NetKind::Mesh => "mesh",
            NetKind::PhoneHotspot => "phone-hotspot",
            NetKind::Travel => "travel",
            NetKind::Other => "other",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NetKind::HomeRouter => "Home router",
            NetKind::Mesh => "Mesh",
            NetKind::PhoneHotspot => "Phone hotspot",
            NetKind::Travel => "Travel / hotel",
            NetKind::Other => "Other",
        }
    }

    /// The grey line under the choice: what Heimdall makes of it.
    pub fn about(self) -> &'static str {
        match self {
            NetKind::HomeRouter => "Heimdall knows the router by its hardware address",
            NetKind::Mesh => "Several access points of one mesh are expected",
            NetKind::PhoneHotspot => "A new router address after the hotspot restarts is expected",
            NetKind::Travel | NetKind::Other => "Heimdall judges the router as it does a home router's",
        }
    }

    fn parse(word: &str) -> Option<NetKind> {
        NetKind::ALL.into_iter().find(|k| k.word() == word)
    }
}

/// The kind as the page shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindState {
    Set(NetKind),
    /// An untrusted network has no kind ("Trust this network first").
    NotTrusted,
    Missing(Missing),
}

pub const KIND_TITLE: &str = "This network is";
pub const KIND_NOT_TRUSTED: &str = "Trust this network first";

/// What the page shows for one saved network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trust {
    pub uuid: String,
    /// None: not known (no Heimdall, or its file could not be read).
    pub level: Option<Level>,
    pub disc: Result<Disc, Missing>,
    pub kind: KindState,
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

/// The guard's answer to `kind --uuid UUID`: `NAME: kind KIND` (the end decides, as for discoverable). Anything else (an older
/// guard's usage text, a refusal) is None.
pub fn parse_kind(text: &str) -> Option<NetKind> {
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    let (_, word) = line.rsplit_once(": kind ")?;
    NetKind::parse(word)
}

/// What to show for the kind, from the level and the guard's answer (None: no answer). A network known to be untrusted is
/// not asked; one the guard did not answer for while it may be trusted needs a newer Heimdall (the `kind` command came with
/// the network kinds) unless it turned out untrusted after all.
pub fn kind_state(level: Option<Level>, answer: Option<NetKind>) -> KindState {
    match (level, answer) {
        (Some(Level::Untrusted), _) => KindState::NotTrusted,
        (_, Some(k)) => KindState::Set(k),
        (_, None) => KindState::Missing(Missing::Newer),
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
        return Trust { uuid, level: None, disc: Err(Missing::Heimdall), kind: KindState::Missing(Missing::Heimdall) };
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
    let answer = if level == Some(Level::Untrusted) {
        None
    } else {
        crate::sys::run(GUARD, &["kind", "--uuid", &uuid]).and_then(|t| parse_kind(&t))
    };
    Trust { uuid, level, disc, kind: kind_state(level, answer) }
}

/// Set the kind: the guard as root through pkexec (polkit asks for the administrator's password). Then `load` and
/// `check_kind` tell whether it took.
pub fn set_kind(uuid: &str, kind: NetKind) -> Result<(), String> {
    as_root(uuid, &["kind", kind.word(), "--uuid", uuid])
}

/// The guard as root through pkexec, for the profile UUID (checked first: the guard takes only a profile id there).
fn as_root(uuid: &str, args: &[&str]) -> Result<(), String> {
    if unit_name(uuid, true).is_none() {
        return Err("This network's profile id is not one Heimdall accepts.".into());
    }
    let guard = format!("/usr/bin/{GUARD}");
    let mut all = vec![guard.as_str()];
    all.extend_from_slice(args);
    crate::sys::act("pkexec", &all).map_err(|e| {
        // pkexec's own answers when the password box was closed or the password was wrong
        if e.is_empty() || e.contains("Not authorized") || e.contains("dismissed") {
            "Not changed: the administrator's password was not given.".to_string()
        } else {
            e
        }
    })
}

/// A wired network's state in Heimdall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WiredState {
    /// Made home (bound to its router).
    pub home: bool,
    /// For the wired network connected now: whether this computer uses a hardware address of its own there (false: the real
    /// one). None when it is not the one connected.
    pub own_address: Option<bool>,
}

/// What the page shows for one saved wired network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wired {
    pub uuid: String,
    pub state: Result<WiredState, Missing>,
}

pub const WIRED_HOME_TITLE: &str = "This is my home network";
pub const WIRED_HOME_ABOUT: &str = "The device list may look around here. Heimdall knows its router by its hardware address: \
     plugged in elsewhere, it is untrusted again";
pub const WIRED_UNTRUSTED_ABOUT: &str = "Untrusted: a hardware address of its own and no computer name sent";
pub const WIRED_REAL_TITLE: &str = "Keep my real hardware address";
pub const WIRED_REAL_ABOUT: &str = "The router reserves an address for this computer";
pub const WIRED_NEEDS_LINK: &str = "Connect to it first: Heimdall makes the network connected now home.";
pub const WIRED_NEWER: &str = "Needs a newer Heimdall (one with wired home networks).";

/// A wired network's state from the guard's `status`, for the profile named NAME (`connected`: it is the wired network up now).
/// The list line `  NAME: home (wired)...` says home; for the connected one the `wired now: NAME, LEVEL; a hardware address of
/// its own here: yes|no` line says both. None when this is not the guard's status, or when the wired network connected now
/// has no `wired now` line (a Heimdall from before wired networks).
pub fn parse_wired_status(text: &str, name: &str, connected: bool) -> Option<WiredState> {
    if !text.lines().any(|l| l.starts_with("networks you trust")) {
        return None;
    }
    let listed_home = text.lines().any(|l| {
        l.strip_prefix("  ")
            .and_then(|r| r.strip_prefix(name))
            .and_then(|r| r.strip_prefix(": "))
            .is_some_and(|r| r.starts_with("home (wired)"))
    });
    let now = text.lines().find_map(|l| {
        let rest = l.strip_prefix("wired now: ")?;
        let (left, own) = rest.rsplit_once("; a hardware address of its own here: ")?;
        let (n, level) = left.rsplit_once(", ")?;
        let own = match own.trim() {
            "yes" => true,
            "no" => false,
            _ => return None,
        };
        Some((n, level, own))
    });
    if !connected {
        return Some(WiredState { home: listed_home, own_address: None });
    }
    match now {
        Some((n, level, own)) if n == name => Some(WiredState { home: level == "home", own_address: Some(own) }),
        // another wired network is the one up (two cables): this one is known only from the list
        Some(_) => Some(WiredState { home: listed_home, own_address: None }),
        None => None,
    }
}

/// Read a saved wired network's state (on a background thread: it starts the guard).
pub fn load_wired(uuid: &str, name: &str, connected: bool) -> Wired {
    let uuid = uuid.to_string();
    if !crate::privpack::installed(GUARD) {
        return Wired { uuid, state: Err(Missing::Heimdall) };
    }
    let read = crate::sys::run(GUARD, &["status"]).and_then(|t| parse_wired_status(&t, name, connected));
    Wired { uuid, state: read.ok_or(Missing::Newer) }
}

/// Make it home (with the real hardware address there when `real`), or back to untrusted: the guard as root through pkexec.
/// Then `load_wired` and `check_wired` tell whether it took.
pub fn set_wired(uuid: &str, home: bool, real: bool) -> Result<(), String> {
    match (home, real) {
        (true, true) => as_root(uuid, &["trust", "--home", "--real-address", "--uuid", uuid]),
        (true, false) => as_root(uuid, &["trust", "--home", "--uuid", uuid]),
        (false, _) => as_root(uuid, &["untrust", uuid]),
    }
}

/// After a change: an error when the state read again is not the one asked for.
pub fn check_wired(w: &Wired, home: bool) -> Result<(), String> {
    match w.state {
        Ok(s) if s.home == home => Ok(()),
        Ok(_) => Err("Heimdall did not change it.".into()),
        Err(Missing::Newer) => Err(WIRED_NEWER.into()),
        Err(m) => Err(m.note().into()),
    }
}

/// After a change of kind: an error when the kind read again is not the one asked for.
pub fn check_kind(t: &Trust, want: NetKind) -> Result<(), String> {
    match t.kind {
        KindState::Set(k) if k == want => Ok(()),
        KindState::Set(_) => Err("Heimdall did not change it.".into()),
        KindState::NotTrusted => Err(format!("{KIND_NOT_TRUSTED}.")),
        KindState::Missing(m) => Err(m.note().into()),
    }
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
    fn the_guards_kind_parses() {
        assert_eq!(parse_kind("Birchwood Library: kind home-router\n"), Some(NetKind::HomeRouter));
        assert_eq!(parse_kind("Maple Street: kind mesh\n"), Some(NetKind::Mesh));
        assert_eq!(parse_kind("Pocket: kind phone-hotspot"), Some(NetKind::PhoneHotspot));
        assert_eq!(parse_kind("Harbor Inn: kind travel\n\n"), Some(NetKind::Travel));
        assert_eq!(parse_kind("x: kind mesh: kind other"), Some(NetKind::Other), "the end decides");
        assert_eq!(parse_kind("Pocket: kind phone-hotspot. Heimdall judges a different router on it by this."), None, "a change's answer");
        assert_eq!(parse_kind("x: kind Mesh"), None);
        assert_eq!(parse_kind("usage: heimdall-wifi-guard pre-up|up|down ..."), None, "an older guard");
        assert_eq!(parse_kind(""), None);
        for k in NetKind::ALL {
            assert_eq!(NetKind::parse(k.word()), Some(k));
        }
    }

    #[test]
    fn the_kind_only_for_a_trusted_network() {
        assert_eq!(kind_state(Some(Level::Untrusted), Some(NetKind::Mesh)), KindState::NotTrusted);
        assert_eq!(kind_state(Some(Level::Untrusted), None), KindState::NotTrusted);
        assert_eq!(kind_state(Some(Level::Trusted), Some(NetKind::Mesh)), KindState::Set(NetKind::Mesh));
        assert_eq!(kind_state(Some(Level::Home), None), KindState::Missing(Missing::Newer), "a guard without the kind command");
        assert_eq!(kind_state(None, Some(NetKind::Travel)), KindState::Set(NetKind::Travel), "level unknown: the guard decides");
        let t = |kind| Trust { uuid: ID.into(), level: Some(Level::Home), disc: Ok(Disc::Home), kind };
        assert_eq!(check_kind(&t(KindState::Set(NetKind::Mesh)), NetKind::Mesh), Ok(()));
        assert!(check_kind(&t(KindState::Set(NetKind::HomeRouter)), NetKind::Mesh).is_err());
        assert_eq!(check_kind(&t(KindState::NotTrusted), NetKind::Mesh), Err("Trust this network first.".into()));
        assert_eq!(set_kind("../x", NetKind::Mesh), Err("This network's profile id is not one Heimdall accepts.".into()));
    }

    // heimdall-wifi-guard status, as it prints a wired home network (Heimdall packaging/net/heimdall-wifi-guard, cmd_status)
    const STATUS: &str = "networks you trust:\n  Maple Street: home (WPA2-Personal), router 00:00:5e:00:53:01\n  \
        Wired connection 1: home (wired), router 00:00:5e:00:53:02\nVPN: not set up (heimdall-wifi-guard vpn-import FILE.conf)\n\
        wired now: Wired connection 1, home; a hardware address of its own here: no\ndevices reaching this computer: none\n";

    #[test]
    fn a_wired_networks_state_from_the_guards_status() {
        let s = |home, own| Some(WiredState { home, own_address: own });
        assert_eq!(parse_wired_status(STATUS, "Wired connection 1", true), s(true, Some(false)));
        assert_eq!(parse_wired_status(STATUS, "Wired connection 1", false), s(true, None), "from the list when not connected");
        assert_eq!(parse_wired_status(STATUS, "Wired connection 2", false), s(false, None));
        assert_eq!(parse_wired_status(STATUS, "Maple Street", false), s(false, None), "a Wi-Fi network of that name is not it");
        assert_eq!(parse_wired_status(STATUS, "Wired", false), s(false, None), "a name is matched whole");
        let away = "networks you trust: none\nVPN: not set up\nwired now: Office, untrusted; a hardware address of its own here: yes\n";
        assert_eq!(parse_wired_status(away, "Office", true), s(false, Some(true)));
        assert_eq!(parse_wired_status(away, "Lab", true), s(false, None), "two cables: another one is up");
        let odd = "networks you trust: none\nwired now: A, b, home; a hardware address of its own here: yes\n";
        assert_eq!(parse_wired_status(odd, "A, b", true), s(true, Some(true)), "a comma in the name: the end decides");
        let older = "networks you trust: none\nVPN: not set up\nnow on: Cafe, untrusted (open)\n";
        assert_eq!(parse_wired_status(older, "Wired connection 1", true), None, "a Heimdall from before wired networks");
        assert_eq!(parse_wired_status("usage: heimdall-wifi-guard ...", "x", false), None);
    }

    #[test]
    fn a_wired_change_is_checked_and_only_for_a_profile_id() {
        let w = |state| Wired { uuid: ID.into(), state };
        let st = |home| Ok(WiredState { home, own_address: None });
        assert_eq!(check_wired(&w(st(true)), true), Ok(()));
        assert_eq!(check_wired(&w(st(false)), false), Ok(()));
        assert_eq!(check_wired(&w(st(false)), true), Err("Heimdall did not change it.".into()));
        assert_eq!(check_wired(&w(Err(Missing::Newer)), true), Err(WIRED_NEWER.into()));
        assert_eq!(set_wired("--home", true, true), Err("This network's profile id is not one Heimdall accepts.".into()));
        assert_eq!(set_wired("x; rm", false, false), Err("This network's profile id is not one Heimdall accepts.".into()));
    }

    #[test]
    fn a_change_is_checked_against_what_is_read_again() {
        let t = |disc| Trust { uuid: ID.into(), level: None, disc, kind: KindState::NotTrusted };
        assert_eq!(check(&t(Ok(Disc::On)), true), Ok(()));
        assert_eq!(check(&t(Ok(Disc::Off)), false), Ok(()));
        assert!(check(&t(Ok(Disc::Off)), true).unwrap_err().contains("journalctl -u heimdall-wifi-discoverable-on@"));
        assert_eq!(check(&t(Err(Missing::Newer)), true), Err(Missing::Newer.note().into()));
        assert_eq!(start_error("org.freedesktop.DBus.Error.AccessDenied", "x"), "Not changed: the administrator's password was not given.");
    }
}
