//! yggdrasil-netjoin: joining a network and reading what NetworkManager sees, shared by Settings (Wi-Fi & Network) and the
//! installer (its Network step). Light on purpose: zbus (already in every egui app's tree through its accessibility) for the
//! join, nmcli's terse output for reading. A password goes to NetworkManager in a D-Bus message, never on a command line.

pub mod join;
pub mod list;
