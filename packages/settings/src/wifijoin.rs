//! Joining a Wi-Fi network with a password: NetworkManager's own D-Bus call (AddAndActivateConnection, what nmcli does inside),
//! so the password travels in a message on the system bus to NetworkManager alone. On nmcli's command line it would be in
//! /proc/PID/cmdline, which every program on the computer can read while nmcli waits for the network.
//!
//! The work is yggdrasil-netjoin's (packages/netjoin, shared with the installer's Network step, where its tests are): the
//! strongest access point with that name is chosen, NetworkManager works out the rest (WPA2, WPA3, the mixed modes), Settings
//! waits until it is connected, and a new profile that did not connect is deleted again (a mistyped password is not kept).

/// Joins `ssid` with `password` through NetworkManager on the system bus, and waits (up to 90 s) until it is connected.
pub fn connect(ssid: &str, password: &str) -> Result<(), String> {
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    yggdrasil_netjoin::join::connect(ssid, password)
}
