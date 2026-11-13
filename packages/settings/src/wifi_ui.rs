//! Wi-Fi & Network, laid out as a phone's Wi-Fi settings: the Wi-Fi switch with the network this computer is on under it,
//! then Known Networks (saved ones in range) and Other Networks (the rest in range), each with a padlock, signal bars and an
//! (i). The (i) opens the network's own page: for any network, what it broadcasts (read from the kernel's last scan without
//! sending anything: yggdrasil-wifiscan); for a saved one also what NetworkManager keeps and its settings; for the connected
//! one also what it gave this computer and This Network (its devices, Wi-Fi checks). Below the lists, Previously connected
//! networks: every saved Wi-Fi network, the most recent first.

use crate::{devices, loading, netconf, sys, ui, wifihealth, App};
use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;
use yggdrasil_wifiscan as scan;

/// A network that is not saved, by its name; a hidden one (no name heard) by its access point's address.
#[derive(Clone, Debug, PartialEq)]
pub struct AirKey {
    pub ssid: String,
    pub bssid: Option<[u8; 6]>,
}

/// The main list's parts: the network in use, Known Networks (saved and in range), Other Networks (the rest in range, by
/// NetworkManager's list) and the access points that hide their name (heard by the radio, not in NetworkManager's list).
pub(crate) struct Sections<'a> {
    pub connected: Option<&'a sys::Wifi>,
    pub known: Vec<(&'a sys::Wifi, &'a netconf::Profile)>,
    pub other: Vec<&'a sys::Wifi>,
    pub hidden: Vec<&'a scan::Network>,
}

pub(crate) fn sections(n: &sys::Network) -> Sections<'_> {
    let saved_for = |ssid: &str| n.saved.iter().find(|p| p.wifi && p.ssid == ssid);
    Sections {
        connected: n.networks.iter().find(|w| w.in_use),
        known: n.networks.iter().filter(|w| !w.in_use).filter_map(|w| saved_for(&w.ssid).map(|p| (w, p))).collect(),
        other: n.networks.iter().filter(|w| !w.in_use && saved_for(&w.ssid).is_none()).collect(),
        // not the hidden, encrypted twin of an open guest network in range (Enhanced Open in transition mode): a phone shows that
        // network once, and so does this list
        hidden: n
            .air
            .iter()
            .filter(|a| a.hidden())
            .filter(|a| !(a.best().security().kind == scan::SecKind::Owe && n.air.iter().any(|o| o.best().security().owe_twin)))
            .collect(),
    }
}

/// Every saved Wi-Fi network, the most recently connected first.
pub(crate) fn history(n: &sys::Network) -> Vec<&netconf::Profile> {
    let mut list: Vec<&netconf::Profile> = n.saved.iter().filter(|p| p.wifi).collect();
    list.sort_by_key(|p| std::cmp::Reverse(p.last_connected));
    list
}

/// "12 h", "30 min", "2 days".
fn span(secs: u64) -> String {
    match secs {
        0..=5399 => format!("{} min", (secs / 60).max(1)),
        5400..=172_799 => format!("{} h", (secs + 1800) / 3600),
        _ => format!("{} days", (secs + 43_200) / 86_400),
    }
}

/// One fact on a details card: the name on the left (with a line under it), the value on the right, selectable.
fn fact(ui: &mut egui::Ui, pal: &Palette, first: &mut bool, k: &str, v: &str, sub: Option<&str>) {
    if v.is_empty() {
        return;
    }
    ui::row(ui, pal, *first, k, sub, |ui| {
        ui.add(egui::Label::new(RichText::new(v).color(pal.dim)).selectable(true).extend());
    });
    *first = false;
}

/// A short note in dim text on a card of its own.
fn note(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui::group(ui, pal, None, |ui| {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(text).color(pal.dim)).wrap());
        ui.add_space(4.0);
    });
}

/// The line under a page's title (the title and Back are the header above the page, nav.rs).
fn header(ui: &mut egui::Ui, pal: &Palette, sub: &str) {
    if !sub.is_empty() {
        ui.label(RichText::new(sub).size(12.0).color(pal.dim));
    }
    ui.add_space(8.0);
}

/// The password box for a secured Wi-Fi network: Some(true) to connect (the Connect button, or Enter, with a password typed),
/// Some(false) to give up (Cancel, or Escape), None while it stays open. The box keeps the keyboard while the window is open,
/// so Enter is read from the frame's keys: it never "loses focus" (it takes it back every frame), which is why Enter used to
/// do nothing here.
pub(crate) fn password_window(ctx: &egui::Context, pal: &Palette, ssid: &str, pw: &mut String) -> Option<bool> {
    let mut done = None;
    egui::Window::new(format!("Connect to “{ssid}”"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label("Password");
            ui.add(egui::TextEdit::singleline(pw).password(true).desired_width(280.0)).request_focus();
            let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if (ui::action(ui, pal, "Connect").clicked() || enter) && !pw.is_empty() {
                    done = Some(true);
                }
                if ui::quiet(ui, pal, "Cancel").clicked() || escape {
                    done = Some(false);
                }
            });
        });
    done
}

impl App {
    pub(crate) fn page_network(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(n) = self.data.network.clone() else { return loading(ui) };
        // `yggdrasil-settings network:wifi=NAME` opens that network's page once the list is in
        if let Some(name) = self.wifi_start.take() {
            match n.saved.iter().find(|p| p.wifi && p.ssid == name) {
                Some(p) => self.net_open = Some(p.uuid.clone()),
                None => self.wifi_air = Some(AirKey { ssid: name, bssid: None }),
            }
        }
        // the connected network's page opens Devices and Wi-Fi checks; their back buttons return to it
        if self.lan_open {
            return self.devices_page(ui, pal, ctx, &n);
        }
        if self.wifi_check_open {
            return self.wifi_check_page(ui, pal, ctx);
        }
        if let Some(uuid) = self.net_open.clone() {
            return self.network_page(ui, pal, ctx, &n, &uuid);
        }
        if let Some(k) = self.wifi_air.clone() {
            return self.air_page(ui, pal, ctx, &n, &k);
        }
        if self.wifi_history {
            return self.history_page(ui, pal, &n);
        }
        let mut open: Option<String> = None;
        let mut air_open: Option<AirKey> = None;
        // Wired shows only while a cable is plugged in
        let wired: Vec<&sys::NetDevice> = n.devices.iter().filter(|d| d.kind == "ethernet" && sys::cable_in(&d.state)).collect();
        if !wired.is_empty() {
            ui::group(ui, pal, Some("Wired"), |ui| {
                for (i, d) in wired.iter().enumerate() {
                    let on = d.state.starts_with("connected");
                    let sub = if on { "Connected".to_string() } else { "Cable plugged in, not connected".into() };
                    let title = format!("Ethernet ({})", d.device);
                    match n.saved.iter().find(|p| !p.wifi && p.name == d.connection) {
                        Some(p) => {
                            if ui::nav_row(ui, pal, i == 0, &title, Some(&sub), |_| {}) {
                                open = Some(p.uuid.clone());
                            }
                        }
                        None => ui::row(ui, pal, i == 0, &title, Some(&sub), |_| {}),
                    }
                }
            });
        }
        let mut wifi_on = n.wifi_on;
        let has_wifi = n.devices.iter().any(|d| d.kind == "wifi");
        let mut toggled = false;
        let listing = n.wifi_on && has_wifi;
        let saved_for = |ssid: &str| n.saved.iter().find(|p| p.wifi && p.ssid == ssid);
        let parts = sections(&n);
        let mut connect: Option<(String, bool)> = None;
        let mut join: Option<String> = None;
        // the switch, and the network this computer is on right under it, on one card
        ui::group(ui, pal, Some("Wi-Fi"), |ui| {
            ui::row(ui, pal, true, "Wi-Fi", Some(if has_wifi { "Connect to wireless networks" } else { "No Wi-Fi adapter found" }), |ui| {
                if has_wifi && ui::switch(ui, pal, &mut wifi_on).changed() {
                    toggled = true;
                }
            });
            if let Some(w) = parts.connected.filter(|_| listing) {
                if ui::net_row(ui, pal, false, true, &w.ssid, None, w.secured, Some(w.signal)) != ui::NetClick::None {
                    match saved_for(&w.ssid) {
                        Some(p) => open = Some(p.uuid.clone()),
                        None => air_open = Some(AirKey { ssid: w.ssid.clone(), bssid: None }),
                    }
                }
            }
        });
        if toggled {
            let arg = if wifi_on { "on" } else { "off" };
            self.change(ctx, move || sys::act("nmcli", &["radio", "wifi", arg]));
        }
        if listing {
            // Known Networks: saved, in range, not the one in use. A click joins; the (i) opens its page.
            let Sections { known, other, hidden, .. } = parts;
            if !known.is_empty() {
                ui::group(ui, pal, Some("Known Networks"), |ui| {
                    for (i, (w, p)) in known.iter().enumerate() {
                        match ui::net_row(ui, pal, i == 0, false, &w.ssid, None, w.secured, Some(w.signal)) {
                            ui::NetClick::Row => join = Some(p.uuid.clone()),
                            ui::NetClick::Info => open = Some(p.uuid.clone()),
                            ui::NetClick::None => {}
                        }
                    }
                });
            }
            // Other Networks: the rest in range, then the ones that hide their name
            ui::group(ui, pal, Some("Other Networks"), |ui| {
                if other.is_empty() && hidden.is_empty() {
                    ui::row(ui, pal, true, if known.is_empty() { "No networks found" } else { "No other networks found" }, None, |_| {});
                }
                for (i, w) in other.iter().enumerate() {
                    match ui::net_row(ui, pal, i == 0, false, &w.ssid, None, w.secured, Some(w.signal)) {
                        ui::NetClick::Row => connect = Some((w.ssid.clone(), w.secured)),
                        ui::NetClick::Info => air_open = Some(AirKey { ssid: w.ssid.clone(), bssid: None }),
                        ui::NetClick::None => {}
                    }
                }
                for (i, a) in hidden.iter().enumerate() {
                    let ap = a.best();
                    let maker = n.maker(&ap.bss.bssid);
                    let sub = [maker, ap.band().label()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
                    // nothing to join without its name: the row opens its page too
                    if ui::net_row(
                        ui,
                        pal,
                        other.is_empty() && i == 0,
                        false,
                        "Hidden network",
                        Some(&sub),
                        ap.security().kind.secured(),
                        Some(ap.quality()),
                    ) != ui::NetClick::None
                    {
                        air_open = Some(AirKey { ssid: String::new(), bssid: Some(ap.bss.bssid) });
                    }
                }
            });
            ui.add_space(8.0);
            if ui::quiet(ui, pal, "Scan again").clicked() {
                self.change(ctx, || {
                    let _ = sys::act("nmcli", &["device", "wifi", "rescan"]);
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    Ok(())
                });
            }
        }
        // every saved Wi-Fi network, in range or not (to look at, change or forget)
        let saved_wifi = n.saved.iter().filter(|p| p.wifi).count();
        let mut history = false;
        if saved_wifi > 0 {
            ui::group(ui, pal, None, |ui| {
                let sub = format!("{saved_wifi} saved, the most recent first");
                history = ui::nav_row(ui, pal, true, "Previously connected networks", Some(&sub), |_| {});
            });
        }
        if let Some((ssid, ask)) = connect {
            if ask {
                self.wifi_password = Some((ssid, String::new()));
            } else {
                self.change(ctx, move || sys::act("nmcli", &["device", "wifi", "connect", &ssid]));
            }
        }
        if let Some(uuid) = join {
            self.change(ctx, move || sys::act("nmcli", &["connection", "up", &uuid]));
        }
        if history {
            self.wifi_history = true;
            self.status.clear();
        }
        if let Some(k) = air_open {
            self.wifi_air = Some(k);
            self.status.clear();
        }
        if let Some(u) = open {
            self.net_open = Some(u);
            self.net_from_history = false;
            self.net_draft = None;
            self.net_forget = false;
            self.status.clear();
        }
    }

    /// Previously connected networks: every saved Wi-Fi network, newest connection first, each with its security and when it
    /// was last joined; the (i) opens its page.
    fn history_page(&mut self, ui: &mut egui::Ui, pal: &Palette, n: &sys::Network) {
        header(ui, pal, "Every saved Wi-Fi network, the most recent first");
        let now = sys::unix_now();
        let list = history(n);
        let mut open: Option<String> = None;
        if list.is_empty() {
            note(ui, pal, "No Wi-Fi networks are saved.");
        }
        ui::group(ui, pal, None, |ui| {
            for (i, p) in list.iter().enumerate() {
                let title = if p.ssid.is_empty() { &p.name } else { &p.ssid };
                let when = if n.live.contains_key(&p.uuid) {
                    "connected now".to_string()
                } else if p.last_connected > 0 {
                    format!("last connected {}", devices::ago(now, p.last_connected))
                } else {
                    "never connected".to_string()
                };
                let range = if !n.live.contains_key(&p.uuid) && n.networks.iter().any(|w| w.ssid == p.ssid) { " · in range" } else { "" };
                let sub = format!("{} · {when}{range}", netconf::key_mgmt_label(&p.key_mgmt));
                if ui::net_row(ui, pal, i == 0, false, title, Some(&sub), false, None) != ui::NetClick::None {
                    open = Some(p.uuid.clone());
                }
            }
        });
        if let Some(u) = open {
            self.net_open = Some(u);
            self.net_from_history = true;
            self.net_draft = None;
            self.net_forget = false;
            self.status.clear();
        }
    }

    /// A network that is not saved: what it broadcasts, and joining it.
    fn air_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, n: &sys::Network, k: &AirKey) {
        // joined (and so saved) since: its saved page instead
        if let Some(p) = n.saved.iter().find(|p| p.wifi && !k.ssid.is_empty() && p.ssid == k.ssid) {
            self.wifi_air = None;
            self.net_open = Some(p.uuid.clone());
            self.net_from_history = false;
            self.net_draft = None;
            return;
        }
        let net = match k.bssid {
            Some(b) => n.air.iter().find(|a| a.aps.iter().any(|ap| ap.bss.bssid == b)),
            None => n.air_for(&k.ssid),
        };
        let listed = n.networks.iter().find(|w| !k.ssid.is_empty() && w.ssid == k.ssid);
        let state =
            if listed.is_some() || net.is_some() { "Wi-Fi · Not saved · in range" } else { "Wi-Fi · Not saved · not heard now" };
        header(ui, pal, state);
        if let Some(w) = listed {
            if ui::action(ui, pal, "Join this network").clicked() {
                if w.secured {
                    self.wifi_password = Some((w.ssid.clone(), String::new()));
                } else {
                    let ssid = w.ssid.clone();
                    self.change(ctx, move || sys::act("nmcli", &["device", "wifi", "connect", &ssid]));
                }
            }
        }
        note(ui, pal, "Its IP addresses, router and DNS servers are known only after joining: a network does not broadcast them.");
        match net {
            Some(a) => air_sections(ui, pal, n, a),
            None if !n.air_error.is_empty() => note(ui, pal, &format!("What it broadcasts could not be read: {}.", n.air_error)),
            None => note(ui, pal, "Not heard in the last scan. Scan again on the Wi-Fi page, or move closer."),
        }
    }

    /// One saved network's own page: joining or leaving it, what it gave the computer (or the last lease Settings saw), This
    /// Network, auto-join, what it broadcasts, private address, IPv4, IPv6, DNS, proxy, forget.
    fn network_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, n: &sys::Network, uuid: &str) {
        let Some(saved) = n.saved.iter().find(|p| p.uuid == uuid).cloned() else {
            // forgotten, or gone: back to the list
            self.net_open = None;
            self.net_draft = None;
            return;
        };
        if self.net_draft.as_ref().is_none_or(|d| d.uuid != uuid) {
            self.net_draft = Some(saved.clone());
        }
        let Some(mut d) = self.net_draft.take() else { return };
        let now = sys::unix_now();
        let live = n.live.get(uuid);
        let in_range = saved.wifi && n.networks.iter().any(|w| w.ssid == saved.ssid);
        let air = if saved.wifi { n.air_for(&saved.ssid) } else { None };
        let state = match (saved.wifi, live.is_some(), in_range) {
            (false, true, _) => "Wired · Connected",
            (false, false, _) => "Wired · Saved",
            (true, true, _) => "Wi-Fi · Connected",
            (true, false, true) => "Wi-Fi · Saved · in range",
            (true, false, false) => "Wi-Fi · Saved · not in range",
        };
        header(ui, pal, state);
        let mut back = false;
        let mut forget = false;
        // join (or disconnect) and Delete side by side at the top, where they are seen; Delete asks once more
        ui.horizontal(|ui| {
            if saved.wifi {
                if live.is_some() {
                    if ui::quiet(ui, pal, "Disconnect").clicked() {
                        let id = uuid.to_string();
                        self.change(ctx, move || sys::act("nmcli", &["connection", "down", &id]));
                    }
                } else if in_range && ui::action(ui, pal, "Join this network").clicked() {
                    let id = uuid.to_string();
                    self.change(ctx, move || sys::act("nmcli", &["connection", "up", &id]));
                }
            }
            if !self.net_forget {
                if ui.button(RichText::new("Delete").color(pal.red)).on_hover_text("Delete this saved network").clicked() {
                    self.net_forget = true;
                }
            } else {
                ui.label(if saved.wifi { "Its password and settings go; joining again needs the password." } else { "Its settings go." });
                if ui.button(RichText::new("Delete").color(pal.red)).clicked() {
                    forget = true;
                }
                if ui::quiet(ui, pal, "Keep it").clicked() {
                    self.net_forget = false;
                }
            }
        });
        if let Some(l) = live {
            ui::group(ui, pal, Some("Connection"), |ui| {
                let mut first = true;
                fact(ui, pal, &mut first, if l.v4.len() > 1 { "IP addresses" } else { "IP address" }, &l.v4.join("\n"), None);
                fact(ui, pal, &mut first, "Subnet", &l.v4.first().and_then(|a| netconf::subnet(a)).unwrap_or_default(), None);
                fact(ui, pal, &mut first, "Router", &l.router, None);
                fact(ui, pal, &mut first, if l.v6.len() > 1 { "IPv6 addresses" } else { "IPv6 address" }, &l.v6.join("\n"), None);
                fact(ui, pal, &mut first, "DNS servers", &l.dns.join("\n"), None);
                if let Some(secs) = l.lease_secs {
                    let from = if l.dhcp_server.is_empty() { String::new() } else { format!(" from {}", l.dhcp_server) };
                    let ends = l
                        .lease_ends
                        .filter(|e| *e > now)
                        .map(|e| format!(", renewed by itself (ends in {})", span((e - now) as u64)))
                        .unwrap_or_default();
                    fact(ui, pal, &mut first, "Lease", &format!("{}{from}{ends}", span(secs)), None);
                }
                fact(ui, pal, &mut first, "Domain", &l.domain, None);
                fact(ui, pal, &mut first, "Hardware address", &l.hw, Some("This computer's, as this network sees it"));
                if saved.wifi {
                    fact(ui, pal, &mut first, "Security", netconf::key_mgmt_label(&saved.key_mgmt), Some("As saved"));
                }
            });
        } else if saved.wifi {
            let lease = n.memory.leases.get(uuid);
            ui::group(ui, pal, Some("Last connection"), |ui| {
                let mut first = true;
                fact(ui, pal, &mut first, "Security", netconf::key_mgmt_label(&saved.key_mgmt), Some("As saved"));
                let last = if saved.last_connected > 0 { devices::ago(now, saved.last_connected) } else { "Never".into() };
                fact(ui, pal, &mut first, "Last connected", &last, None);
                if let Some(l) = lease {
                    fact(ui, pal, &mut first, if l.v4.len() > 1 { "IP addresses" } else { "IP address" }, &l.v4.join("\n"), None);
                    fact(ui, pal, &mut first, "Subnet", &l.subnet, None);
                    fact(ui, pal, &mut first, "Router", &l.router, None);
                    fact(ui, pal, &mut first, "IPv6 addresses", &l.v6.join("\n"), None);
                    fact(ui, pal, &mut first, "DNS servers", &l.dns.join("\n"), None);
                    if let Some(secs) = l.lease_secs {
                        let from = if l.dhcp_server.is_empty() { String::new() } else { format!(" from {}", l.dhcp_server) };
                        fact(ui, pal, &mut first, "Lease", &format!("{}{from}", span(secs)), None);
                    }
                    fact(ui, pal, &mut first, "Noted", &devices::ago(now, l.time), Some("When Settings last saw this network connected"));
                }
            });
            if lease.is_none() {
                note(ui, pal, "NetworkManager keeps the last lease (address, router, DNS) where only the system can read it. Settings shows it here once it has seen this network connected.");
            }
        }
        ui.add_space(8.0);
        // This Network: who else is on it, and why it is slow (only on the connection that has the router)
        if live.is_some() {
            if let Some((_, l)) = n.lan.as_ref().filter(|(u, _)| u == uuid) {
                let sub = crate::lan_summary(l, now);
                let mut go = false;
                let mut check = false;
                ui::group(ui, pal, Some("This Network"), |ui| {
                    go = ui::nav_row(ui, pal, true, "Devices on this network", Some(&sub), |_| {});
                    if saved.wifi && wifihealth::available() {
                        check = ui::nav_row(
                            ui,
                            pal,
                            false,
                            "Wi-Fi checks",
                            Some("Why is it slow? Signal, channel, other devices, the internet"),
                            |_| {},
                        );
                    }
                });
                if go {
                    self.lan_open = true;
                    self.lan_dev = None;
                    self.status.clear();
                }
                if check {
                    self.wifi_check_open = true;
                    self.status.clear();
                }
                ui.add_space(8.0);
            } else if saved.wifi && wifihealth::available() {
                let mut check = false;
                ui::group(ui, pal, Some("This Network"), |ui| {
                    check = ui::nav_row(
                        ui,
                        pal,
                        true,
                        "Wi-Fi checks",
                        Some("Why is it slow? Signal, channel, other devices, the internet"),
                        |_| {},
                    );
                });
                if check {
                    self.wifi_check_open = true;
                    self.status.clear();
                }
                ui.add_space(8.0);
            }
        }
        ui::group(ui, pal, None, |ui| {
            let (t, sub) = if saved.wifi {
                ("Auto-Join", "Join this network by itself when it is in range")
            } else {
                ("Connect automatically", "When the cable is plugged in")
            };
            ui::row(ui, pal, true, t, Some(sub), |ui| {
                ui::switch(ui, pal, &mut d.auto_join);
            });
        });
        ui.add_space(8.0);
        if saved.wifi {
            match air {
                Some(a) => air_sections(ui, pal, n, a),
                None => {
                    note(ui, pal, "Not in range now: what it broadcasts shows here when it is.");
                    seen_before(ui, pal, n, &saved, now);
                }
            }
            ui.add_space(8.0);
        }
        ui::group(ui, pal, Some("Private address"), |ui| {
            let (idx, sub) = match &d.mac {
                sys::MacMode::Private => (0, "A random hardware address of its own for this network, the same every time"),
                sys::MacMode::Random => (1, "A new random hardware address every time it joins"),
                sys::MacMode::Real => (
                    2,
                    if saved.wifi {
                        "This computer's real hardware address (used only on networks you trust)"
                    } else {
                        "This computer's real hardware address"
                    },
                ),
                sys::MacMode::Custom(_) => (3, "A hardware address you type in"),
            };
            ui::row(ui, pal, true, "Address", Some(sub), |ui| {
                if let Some(k) = ui::segmented(ui, pal, &["Private", "New each time", "Real", "Custom"], idx) {
                    d.mac = match k {
                        0 => sys::MacMode::Private,
                        1 => sys::MacMode::Random,
                        2 => sys::MacMode::Real,
                        _ => sys::MacMode::Custom(if let sys::MacMode::Custom(a) = &saved.mac { a.clone() } else { sys::random_mac() }),
                    };
                }
            });
            if let sys::MacMode::Custom(a) = &mut d.mac {
                ui::row(ui, pal, false, "Custom address", None, |ui| {
                    if ui::quiet(ui, pal, "Random").on_hover_text("Make up an address, like macchanger -r").clicked() {
                        *a = sys::random_mac();
                    }
                    ui::field(ui, pal, a, "02:1A:2B:3C:4D:5E", 170.0, false);
                });
            }
        });
        ui.add_space(8.0);
        for six in [false, true] {
            let ip = if six { &mut d.v6 } else { &mut d.v4 };
            ui::group(ui, pal, Some(if six { "IPv6" } else { "IPv4" }), |ui| {
                let opts: &[&str] = if six { &["Automatic", "Manual", "Off"] } else { &["Automatic", "Manual"] };
                let idx = match ip.method {
                    netconf::IpMethod::Auto => 0,
                    netconf::IpMethod::Manual => 1,
                    netconf::IpMethod::Off => 2,
                };
                let sub = match ip.method {
                    netconf::IpMethod::Auto => "The router gives the address",
                    netconf::IpMethod::Manual => "An address you type in",
                    netconf::IpMethod::Off => "IPv6 is not used on this network",
                };
                ui::row(ui, pal, true, "Configure", Some(sub), |ui| {
                    if let Some(k) = ui::segmented(ui, pal, opts, idx) {
                        ip.method = [netconf::IpMethod::Auto, netconf::IpMethod::Manual, netconf::IpMethod::Off][k];
                    }
                });
                if ip.method == netconf::IpMethod::Manual {
                    let (ah, rh) = if six { ("2001:db8::20/64", "2001:db8::1") } else { ("192.168.1.20/24", "192.168.1.1") };
                    ui::row(ui, pal, false, "Address", Some("With its prefix length after a slash"), |ui| {
                        ui::field(ui, pal, &mut ip.address, ah, 200.0, false);
                    });
                    ui::row(ui, pal, false, "Router", None, |ui| {
                        ui::field(ui, pal, &mut ip.router, rh, 200.0, false);
                    });
                }
                if ip.method != netconf::IpMethod::Off {
                    ui::row(
                        ui,
                        pal,
                        false,
                        "DNS servers",
                        Some(if six {
                            "Separated by commas, like 2606:4700:4700::1111"
                        } else {
                            "Separated by commas, like 1.1.1.1, 9.9.9.9"
                        }),
                        |ui| {
                            ui::field(ui, pal, &mut ip.dns, "From the router", 200.0, false);
                        },
                    );
                }
            });
            ui.add_space(8.0);
        }
        ui::group(ui, pal, Some("HTTP proxy"), |ui| {
            let idx = match d.proxy {
                netconf::ProxyMode::Off => 0,
                netconf::ProxyMode::Manual => 1,
                netconf::ProxyMode::Auto => 2,
            };
            ui::row(ui, pal, true, "Proxy", Some("For programs that ask the system for its proxy"), |ui| {
                if let Some(k) = ui::segmented(ui, pal, &["Off", "Manual", "Automatic"], idx) {
                    d.proxy = [netconf::ProxyMode::Off, netconf::ProxyMode::Manual, netconf::ProxyMode::Auto][k];
                }
            });
            match d.proxy {
                netconf::ProxyMode::Manual => ui::row(ui, pal, false, "Server", Some("Name or address, a colon, the port"), |ui| {
                    ui::field(ui, pal, &mut d.proxy_server, "proxy.example.com:8080", 200.0, false);
                }),
                netconf::ProxyMode::Auto => ui::row(ui, pal, false, "Configuration", Some("The proxy file's web address (PAC)"), |ui| {
                    ui::field(ui, pal, &mut d.pac_url, "http://wpad.example.com/wpad.dat", 240.0, false);
                }),
                netconf::ProxyMode::Off => {}
            }
        });
        ui.add_space(10.0);
        let dirty = d != saved;
        let (mut save, mut rejoin, mut undo) = (false, false, false);
        if dirty {
            ui.horizontal(|ui| {
                if ui::action(ui, pal, "Save").clicked() {
                    save = true;
                }
                if live.is_some() && ui::quiet(ui, pal, "Save and reconnect").on_hover_text("The connection drops for a moment").clicked() {
                    save = true;
                    rejoin = true;
                }
                if ui::quiet(ui, pal, "Undo changes").clicked() {
                    undo = true;
                }
            });
            ui.label(
                RichText::new(if live.is_some() {
                    "Saved changes are used the next time it connects, or now with Save and reconnect."
                } else {
                    "Saved changes are used the next time it connects."
                })
                .color(pal.dim),
            );
        }
        if undo {
            d = saved.clone();
        }
        if save {
            match netconf::changes(&saved, &d) {
                Ok(pairs) => {
                    let id = uuid.to_string();
                    self.change(ctx, move || {
                        netconf::save(&id, &pairs)?;
                        if rejoin {
                            netconf::rejoin(&id)
                        } else {
                            Ok(())
                        }
                    });
                }
                Err(e) => self.status = e,
            }
        }
        self.net_draft = Some(d);
        if forget {
            let id = uuid.to_string();
            self.change(ctx, move || netconf::forget(&id));
            back = true;
        }
        if back {
            self.net_open = None;
            self.net_draft = None;
            self.net_forget = false;
            // back to the list it was opened from
            self.wifi_history = self.net_from_history;
            self.net_from_history = false;
            self.status.clear();
        }
    }
}

/// A saved network out of range: the access points NetworkManager has seen for it, with their makers and when this computer
/// last heard each.
fn seen_before(ui: &mut egui::Ui, pal: &Palette, n: &sys::Network, saved: &netconf::Profile, now: i64) {
    let aps: Vec<[u8; 6]> = saved.seen_bssids.iter().filter_map(|b| scan::parse_mac(b)).take(16).collect();
    if aps.is_empty() {
        return;
    }
    ui::group(ui, pal, Some(&format!("Access points seen for it ({})", aps.len())), |ui| {
        for (i, m) in aps.iter().enumerate() {
            let text = scan::mac_text(m);
            let mut sub = vec![maker_text(n, m)];
            if let Some((_, last)) = n.memory.heard(&text) {
                sub.push(format!("last heard {}", devices::ago(now, last)));
            }
            ui::row(ui, pal, i == 0, &text, Some(&sub.join(" · ")), |_| {});
        }
    });
}

/// Who made an access point, or why that is not known.
fn maker_text(n: &sys::Network, m: &[u8; 6]) -> String {
    if scan::oui::is_local(m) {
        return "Made-up address (often a second network on the same router)".into();
    }
    match n.maker(m) {
        "" => "Maker not in the list".into(),
        s => s.to_string(),
    }
}

/// What a network broadcasts, from the kernel's last scan: security, generation, WPS and the device details it announces,
/// roaming, country, load, when it was heard; each of its access points; the vendor extras it names.
fn air_sections(ui: &mut egui::Ui, pal: &Palette, n: &sys::Network, a: &scan::Network) {
    let now = sys::unix_now();
    let best = a.best();
    let info = &best.info;
    let sec = best.security();
    ui::group(ui, pal, Some("What it broadcasts"), |ui| {
        let mut first = true;
        let name = if a.hidden() { "Not broadcast".to_string() } else { a.name() };
        fact(ui, pal, &mut first, "Name", &name, best.beacon_hidden.then_some("Hidden: its beacons leave the name out"));
        let mut detail = Vec::new();
        if !sec.akms.is_empty() {
            detail.push(format!("Key management: {}", sec.akms.join(", ")));
        }
        if !sec.ciphers.is_empty() {
            detail.push(format!("Encryption: {}", sec.ciphers.join(", ")));
        }
        if sec.owe_twin {
            detail.push("It names an Enhanced Open twin that encrypts".into());
        }
        let detail = detail.join(" · ");
        fact(ui, pal, &mut first, "Security", sec.kind.short(), Some(if detail.is_empty() { sec.kind.label() } else { &detail }));
        if sec.kind.secured() || sec.kind == scan::SecKind::Owe {
            let pmf = match sec.pmf {
                scan::Pmf::Required => "Required",
                scan::Pmf::Optional => "Optional",
                scan::Pmf::Off => "Off",
            };
            fact(ui, pal, &mut first, "Protected management frames", pmf, Some("Stops anyone nearby from knocking devices off it"));
        }
        let gen_sub = info.multi_link.then_some("Multi-link: one device on several bands at once");
        fact(ui, pal, &mut first, "Wi-Fi generation", a.generation().label(), gen_sub);
        let (wps, wps_sub) = match &info.wps {
            None => ("Off".to_string(), "Not announced"),
            Some(w) if w.selected_registrar => {
                ("On, pairing now".to_string(), "Someone started a WPS pairing on the router (the button or a PIN) and it is open now")
            }
            Some(w) if w.locked => {
                ("On, locked".to_string(), "Joining by button or PIN; the router has locked PIN pairing after failed tries")
            }
            Some(w) => (
                if w.state == Some(1) { "On, not set up".to_string() } else { "On".to_string() },
                "Joining by button or PIN. A PIN is a known weak spot on many routers; it can be turned off in the router's settings",
            ),
        };
        fact(ui, pal, &mut first, "WPS", &wps, Some(wps_sub));
        let mut roam = Vec::new();
        if info.rrm {
            roam.push(if info.neighbor_report { "802.11k neighbour reports" } else { "802.11k" });
        }
        if info.bss_transition {
            roam.push("802.11v transitions");
        }
        if info.mobility_domain.is_some() || sec.akms.iter().any(|k| k.starts_with("FT-")) {
            roam.push("802.11r fast roaming");
        }
        let roam = if roam.is_empty() { "None announced".to_string() } else { roam.join(", ") };
        fact(ui, pal, &mut first, "Roaming help", &roam, Some("How devices move between its access points"));
        if let Some(c) = info.country.as_ref().filter(|c| !c.code.is_empty()) {
            let env = match c.environment {
                scan::ie::Environment::Indoor => " (indoors)",
                scan::ie::Environment::Outdoor => " (outdoors)",
                scan::ie::Environment::Any => " (indoors and outdoors)",
                _ => "",
            };
            fact(ui, pal, &mut first, "Country", &format!("{}{env}", c.code), Some("Whose radio rules it follows"));
        }
        if let Some(l) = info.bss_load {
            let busy = (u32::from(l.utilization) * 100 + 127) / 255;
            fact(
                ui,
                pal,
                &mut first,
                "Load",
                &format!("{} devices · channel busy {busy} %", l.stations),
                Some("On the access point this describes"),
            );
        }
        let first_heard = a.aps.iter().filter_map(|ap| n.memory.heard(&scan::mac_text(&ap.bss.bssid)).map(|h| h.0)).min();
        if let Some(t) = first_heard {
            fact(ui, pal, &mut first, "First seen", &devices::ago(now, t), Some("By this computer (Settings keeps 30 days)"));
        }
        let last = a.aps.iter().filter_map(|ap| ap.bss.seen_ms_ago).min();
        if let Some(ms) = last {
            fact(ui, pal, &mut first, "Last seen", &devices::ago(now, now - i64::from(ms / 1000)), None);
        }
    });
    if a.mixed_security() {
        note(ui, pal, "Not every access point with this name uses the same security. A guest radio can explain it; otherwise one of them could be pretending to be this network.");
    }
    if let Some(w) =
        info.wps.as_ref().filter(|w| [&w.manufacturer, &w.model, &w.model_number, &w.serial, &w.device_name].iter().any(|s| !s.is_empty()))
    {
        ui::group(ui, pal, Some("Device details it broadcasts"), |ui| {
            let mut first = true;
            fact(ui, pal, &mut first, "Manufacturer", &w.manufacturer, None);
            fact(ui, pal, &mut first, "Model", &w.model, None);
            fact(ui, pal, &mut first, "Model number", &w.model_number, None);
            fact(ui, pal, &mut first, "Serial number", &w.serial, None);
            fact(ui, pal, &mut first, "Device name", &w.device_name, None);
            let kind = match w.device_type {
                Some((6, 1)) => "Access point",
                Some((6, 2)) => "Router",
                Some((6, 3)) => "Switch",
                Some((6, 4)) => "Gateway",
                Some((6, 5)) => "Bridge",
                Some((6, _)) => "Network equipment",
                Some((1, _)) => "Computer",
                Some((7, _)) => "Display",
                Some((10, _)) => "Phone",
                _ => "",
            };
            fact(ui, pal, &mut first, "Device type", kind, None);
        });
        ui.add_space(2.0);
        ui.label(
            RichText::new(
                "The router announces these to everyone nearby, through WPS. Turning WPS off in the router's settings usually stops it.",
            )
            .size(12.0)
            .color(pal.dim),
        );
    }
    let mixed = a.mixed_security();
    let count = a.aps.len();
    let heading = if count == 1 { "Access point".to_string() } else { format!("Access points ({count})") };
    ui::group(ui, pal, Some(&heading), |ui| {
        for (i, ap) in a.aps.iter().enumerate() {
            let mac = scan::mac_text(&ap.bss.bssid);
            let mut sub = vec![maker_text(n, &ap.bss.bssid), ap.band().label().to_string()];
            if let Some(c) = ap.channel() {
                sub.push(format!("channel {c}"));
            }
            let (w, split) = ap.width();
            sub.push(if split { "80+80 MHz".into() } else { format!("{w} MHz") });
            if let Some(dbm) = ap.signal_dbm() {
                sub.push(format!("{dbm} dBm"));
            }
            sub.push(ap.generation().short().to_string());
            if mixed {
                sub.push(ap.security().kind.short().to_string());
            }
            let sub = sub.join(" · ");
            ui::row(ui, pal, i == 0, &mac, Some(&sub), |ui| {
                ui::signal_bars(ui, pal, ap.quality());
                if ap.bss.associated {
                    ui.add_space(8.0);
                    ui.label(RichText::new("This computer is on it").color(pal.accent));
                }
            });
        }
    });
    if count > 1 {
        ui.add_space(2.0);
        ui.label(RichText::new("Several access points with one name: a mesh, or a router with extenders.").size(12.0).color(pal.dim));
    }
    // the vendor extras it names: recognised ones by what they are, others by whose they are
    let mut extras: Vec<String> = Vec::new();
    for v in &info.vendors {
        let name = match scan::oui::element_name(v.oui, v.kind) {
            Some(s) => s.to_string(),
            None => match n.makers.get(&v.oui) {
                Some(m) => format!("{m}'s own"),
                None => format!(
                    "{:02X}-{:02X}-{:02X}{}",
                    v.oui[0],
                    v.oui[1],
                    v.oui[2],
                    v.kind.map(|k| format!(" type {k}")).unwrap_or_default()
                ),
            },
        };
        if !extras.contains(&name) {
            extras.push(name);
        }
    }
    if !extras.is_empty() {
        ui::group(ui, pal, Some("Extras it announces"), |ui| {
            for (i, e) in extras.iter().enumerate() {
                ui::row(ui, pal, i == 0, e, None, |_| {});
            }
        });
    }
    if info.truncated {
        ui.add_space(2.0);
        ui.label(RichText::new("Part of what it sent did not read (cut short or malformed) and was left out.").size(12.0).color(pal.amber));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed made-up fixture (tests/fixtures/wifi.json), read through the same parsers Settings uses.
    fn made_up() -> sys::Network {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wifi.json")).unwrap();
        let f: crate::fixture::Fixture = serde_json::from_str(&text).unwrap();
        let out = |k: &str| f.run.get(k).cloned().unwrap_or_default();
        let profiles = out("nmcli -t -f NAME,UUID,TYPE connection show")
            .lines()
            .map(sys::nm_fields)
            .filter(|f| f.len() >= 3)
            .map(|x| netconf::parse(&netconf::settings_map(&out(&format!("nmcli -t connection show {}", x[1])))))
            .collect();
        let live = sys::active_profiles(&out("nmcli -t -f UUID,DEVICE connection show --active"))
            .into_iter()
            .map(|(u, d)| {
                (
                    u,
                    netconf::parse_live(&out(&format!(
                        "nmcli -t -f IP4.ADDRESS,IP4.GATEWAY,IP4.DNS,IP6.ADDRESS,IP6.DNS,GENERAL.HWADDR,DHCP4 device show {d}"
                    ))),
                )
            })
            .collect();
        sys::Network {
            wifi_on: true,
            devices: sys::net_devices(&out("nmcli -t -f DEVICE,TYPE,STATE,CONNECTION device")),
            networks: sys::wifi_list(&out("nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list")),
            saved: profiles,
            live,
            air: std::sync::Arc::new(scan::group(scan::parse_fixture(&f.scan))),
            ..Default::default()
        }
    }

    #[test]
    fn the_list_in_its_sections() {
        let n = made_up();
        let s = sections(&n);
        assert_eq!(s.connected.map(|w| w.ssid.as_str()), Some("Lantern Hall"));
        assert_eq!(s.known.iter().map(|(w, _)| w.ssid.as_str()).collect::<Vec<_>>(), vec!["Birchwood Library"]);
        let mut other: Vec<&str> = s.other.iter().map(|w| w.ssid.as_str()).collect();
        other.sort();
        assert_eq!(other, vec!["CoffeeCat Guest", "Harbor Lights 7", "Northgate Office"]);
        assert_eq!(s.hidden.len(), 1, "the access point that hides its name");
        // previously connected: every saved Wi-Fi network, in range or not, newest first; wired ones are not listed
        let h: Vec<&str> = history(&n).iter().map(|p| p.ssid.as_str()).collect();
        assert_eq!(h, vec!["Lantern Hall", "Birchwood Library", "Maple Street", "Seaside Inn"]);
        // the connected network's page finds what it broadcasts: a mesh of three
        let air = n.air_for("Lantern Hall").unwrap();
        assert_eq!(air.aps.len(), 3);
        assert!(air.best().bss.associated);
        assert_eq!(air.best().security().kind, scan::SecKind::Wpa2Wpa3Personal);
        // the library's router announces its details over WPS
        let w = n.air_for("Birchwood Library").unwrap().best().info.wps.clone().unwrap();
        assert_eq!((w.manufacturer.as_str(), w.serial.as_str()), ("Example Networks", "EXN0042-7781"));
        assert_eq!(n.air_for("Harbor Lights 7").unwrap().generation(), scan::Generation::Wifi7);
        assert_eq!(n.live[&n.saved[0].uuid].lease_secs, Some(86_400));
    }

    #[test]
    fn spans() {
        assert_eq!(span(43_200), "12 h");
        assert_eq!(span(1800), "30 min");
        assert_eq!(span(30), "1 min");
        assert_eq!(span(3 * 86_400), "3 days");
    }

    /// The password window driven headless (an egui context with made-up keys, nothing drawn): Enter connects once a password
    /// is typed, Escape gives up.
    #[test]
    fn enter_connects_and_escape_cancels() {
        let ctx = egui::Context::default();
        let pal = yggdrasil_ui::theme::palette(true);
        let key = |k| egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let frame = |pw: &mut String, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let mut done = None;
            ctx.run_ui(input, |ui| done = password_window(ui.ctx(), &pal, "Home", pw)).drop_without_applying_deltas();
            done
        };
        let mut pw = String::new();
        assert_eq!(frame(&mut pw, vec![]), None, "open, waiting");
        assert_eq!(frame(&mut pw, vec![key(egui::Key::Enter)]), None, "Enter with no password does nothing");
        assert_eq!(frame(&mut pw, vec![egui::Event::Text("hunter22".into())]), None);
        assert_eq!(pw, "hunter22", "the box has the keyboard from the start");
        assert_eq!(frame(&mut pw, vec![key(egui::Key::Enter)]), Some(true), "Enter connects");
        assert_eq!(frame(&mut pw, vec![]), None, "and only on the frame it was pressed");
        assert_eq!(frame(&mut pw, vec![key(egui::Key::Escape)]), Some(false), "Escape cancels");
    }
}
