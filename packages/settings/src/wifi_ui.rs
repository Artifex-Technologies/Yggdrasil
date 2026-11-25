//! Wi-Fi & Network, laid out as a phone's Wi-Fi settings: the Wi-Fi switch with the network this computer is on under it,
//! then Known Networks (saved ones in range) and Other Networks (the rest in range), each with a padlock, signal bars and an
//! (i). The (i) opens the network's own page: for any network, what it broadcasts (read from the kernel's last scan without
//! sending anything: yggdrasil-wifiscan); for a saved one also what NetworkManager keeps and its settings; for the connected
//! one also what it gave this computer and This Network (its devices, Wi-Fi checks). Below the lists, Previously connected
//! networks: every saved Wi-Fi network, the most recent first. While the list is on screen Settings keeps looking for
//! networks (wifi_live.rs): no Scan button, the list stays in place as results come in.

use crate::{devices, loading, netconf, sys, ui, wifi_live, wifihealth, wifitrust, App};
use eframe::egui::{self, RichText};
use yggdrasil_netjoin::join::Request;
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
/// The last three in the order they are shown (wifi_live::Shown), each with whether it is faded (not heard in the last scan).
pub(crate) struct Sections<'a> {
    pub connected: Option<&'a sys::Wifi>,
    pub known: Vec<(&'a sys::Wifi, &'a netconf::Profile, bool)>,
    pub other: Vec<(&'a sys::Wifi, bool)>,
    pub hidden: Vec<(&'a scan::Network, bool)>,
}

pub(crate) fn sections<'a>(n: &'a sys::Network, shown: &'a wifi_live::Shown) -> Sections<'a> {
    let saved_for = |ssid: &str| n.saved.iter().find(|p| p.wifi && p.ssid == ssid);
    let connected = n.networks.iter().find(|w| w.in_use);
    let listed = || shown.named.rows().filter(|(w, _)| connected.is_none_or(|c| c.ssid != w.ssid));
    Sections {
        connected,
        known: listed().filter_map(|(w, faded)| saved_for(&w.ssid).map(|p| (w, p, faded))).collect(),
        other: listed().filter(|(w, _)| saved_for(&w.ssid).is_none()).collect(),
        hidden: shown.hidden.rows().collect(),
    }
}

/// The access points that hide their name, as the radio heard them; not the hidden, encrypted twin of an open guest network in
/// range (Enhanced Open in transition mode): a phone shows that network once, and so does this list.
pub(crate) fn hidden_heard(n: &sys::Network) -> impl Iterator<Item = &scan::Network> {
    n.air
        .iter()
        .filter(|a| a.hidden())
        .filter(|a| !(a.best().security().kind == scan::SecKind::Owe && n.air.iter().any(|o| o.best().security().owe_twin)))
}

/// A row of the list, faded when its network was not heard in the last scan (it stays in its place until it is forgotten).
fn faded_row<R>(ui: &mut egui::Ui, faded: bool, row: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        if faded {
            ui.multiply_opacity(0.45);
        }
        row(ui)
    })
    .inner
}

/// Every saved Wi-Fi network, the most recently connected first.
pub(crate) fn history(n: &sys::Network) -> Vec<&netconf::Profile> {
    let mut list: Vec<&netconf::Profile> = n.saved.iter().filter(|p| p.wifi).collect();
    list.sort_by_key(|p| std::cmp::Reverse(p.last_connected));
    list
}

/// Under a network not heard in the last scan (faded in the list until it is forgotten, two minutes on).
const NOT_HEARD: &str = "Not heard just now";

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

/// The window for joining a secured Wi-Fi network (its password), or a hidden one (its name, its security, the password):
/// Some(true) to connect (the Connect button, or Enter, with what it needs typed), Some(false) to give up (Cancel, or Escape),
/// None while it stays open. The password box keeps the keyboard while the window is open (the name box takes it while a
/// hidden network's name is empty), so Enter is read from the frame's keys: it never "loses focus" (it takes it back every
/// frame), which is why Enter used to do nothing here. Below: the hardware address the network will see, with the choice of
/// this computer's real one for networks that reserve or allow addresses (`address_choice`).
pub(crate) fn password_window(ctx: &egui::Context, pal: &Palette, r: &mut Request, n: &sys::Network) -> Option<bool> {
    let mut done = None;
    let title = if r.hidden { "Join a hidden network".to_string() } else { format!("Connect to “{}”", r.ssid) };
    egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(
        ctx,
        |ui| {
            if r.hidden {
                ui.label("Network name");
                let name = ui.add(egui::TextEdit::singleline(&mut r.ssid).desired_width(280.0));
                if r.ssid.is_empty() {
                    name.request_focus();
                }
                ui.add_space(4.0);
                ui.label("Security");
                if let Some(k) = ui::segmented(ui, pal, &["None", "WPA/WPA2/WPA3 Personal"], usize::from(r.secured)) {
                    r.secured = k == 1;
                }
                ui.add_space(4.0);
            }
            let asks = !r.hidden || r.secured;
            if asks {
                ui.label("Password");
                let pw = ui.add(egui::TextEdit::singleline(&mut r.password).password(true).desired_width(280.0));
                if !r.hidden || !r.ssid.is_empty() {
                    pw.request_focus();
                }
            }
            ui.add_space(6.0);
            address_choice(ui, pal, r, n);
            let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            ui.add_space(6.0);
            let ready = !r.ssid.trim().is_empty() && (!asks || !r.password.is_empty());
            ui.horizontal(|ui| {
                if (ui::action(ui, pal, "Connect").clicked() || enter) && ready {
                    done = Some(true);
                }
                if ui::quiet(ui, pal, "Cancel").clicked() || escape {
                    done = Some(false);
                }
            });
        },
    );
    done
}

/// The address the network will see (yggdrasil-netjoin's `seen_by_network`), each with a Copy button; while the system gives
/// each network a private address, the choice of this computer's real one instead.
pub(crate) fn address_choice(ui: &mut egui::Ui, pal: &Palette, r: &mut Request, n: &sys::Network) {
    if n.private_wifi {
        ui.checkbox(&mut r.real_address, "Use this computer's real address (for networks that reserve or allow addresses)");
    }
    let seen = yggdrasil_netjoin::mac::seen_by_network(&n.addresses, n.private_wifi, r.real_address);
    let many = seen.len() > 1;
    for (label, mac) in seen {
        ui.horizontal(|ui| match mac {
            Some(m) => {
                let what = if many { format!("{label}: address (MAC) {m}") } else { format!("This computer's Wi-Fi address (MAC): {m}") };
                ui.label(RichText::new(what).color(pal.dim));
                if ui::quiet(ui, pal, "Copy").clicked() {
                    ui.ctx().copy_text(m);
                }
            }
            None => {
                let what = if many {
                    format!("{label}: a private address of its own for this network")
                } else {
                    "A private address of its own for this network (made as it joins)".to_string()
                };
                ui.label(RichText::new(what).color(pal.dim));
            }
        });
    }
}

/// With two or more connections up (YGG-107): the one the internet goes through and the others standing by, as the network
/// list's first card says it: (title, subtitle). `links`: each connected link's interface and how the card names it.
pub fn internet_line(through: Option<&str>, links: &[(String, String)]) -> Option<(String, String)> {
    if links.len() < 2 {
        return None;
    }
    let (_, main) = links.iter().find(|(d, _)| Some(d.as_str()) == through)?;
    let others: Vec<&str> = links.iter().filter(|(d, _)| Some(d.as_str()) != through).map(|(_, t)| t.as_str()).collect();
    let verb = if others.len() == 1 { "is" } else { "are" };
    Some((format!("Internet through {main}"), format!("{} {verb} connected too, standing by", others.join(" and "))))
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
        // Several connections at once (multilink_ui.rs), opened from the list
        if let Some(p) = self.multilink.as_mut() {
            return p.show(ui, pal, ctx);
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
        // back on the list: a network's Heimdall card is read again when its page opens next
        if self.wifi_trust_busy.is_none() {
            self.wifi_trust = None;
            self.wired_trust = None;
        }
        if let Some(k) = self.wifi_air.clone() {
            return self.air_page(ui, pal, ctx, &n, &k);
        }
        if self.wifi_history {
            return self.history_page(ui, pal, &n);
        }
        let mut open: Option<String> = None;
        let mut air_open: Option<AirKey> = None;
        // Wired shows only while a cable is plugged in. A phone sharing its connection over USB is not Ethernet: it has a group
        // of its own, named after the phone ("Tether (Rover)")
        let plugged = n.devices.iter().filter(|d| d.kind == "ethernet" && sys::cable_in(&d.state));
        let (phones, wired): (Vec<_>, Vec<_>) = plugged.map(|d| (d, sys::phone_name(&d.device))).partition(|(_, p)| p.is_some());
        // several connections at once: which one the internet goes through, the others standing by (YGG-107)
        let wifi_dev = n.devices.iter().find(|d| d.kind == "wifi" && d.state.starts_with("connected"));
        let mut links: Vec<(String, String)> = Vec::new();
        for (d, phone) in phones.iter().chain(wired.iter()).filter(|(d, _)| d.state.starts_with("connected")) {
            links.push((
                d.device.clone(),
                match phone {
                    Some(name) => format!("Tether ({name}), over USB"),
                    None => format!("Ethernet ({})", d.device),
                },
            ));
        }
        if let Some(w) = wifi_dev {
            let ssid =
                n.saved.iter().find(|p| p.wifi && p.name == w.connection).map(|p| p.ssid.clone()).unwrap_or_else(|| w.connection.clone());
            links.push((w.device.clone(), format!("{ssid} (Wi-Fi)")));
        }
        if let Some((title, sub)) = internet_line(n.through.as_deref(), &links) {
            ui::group(ui, pal, Some("Internet"), |ui| {
                ui::row(ui, pal, true, &title, Some(&sub), |_| {});
            });
        }
        let standing_by = |dev: &str| n.through.as_deref().is_some_and(|t| t != dev) && links.len() >= 2;
        for (group, list) in [("Phone", &phones), ("Wired", &wired)] {
            if list.is_empty() {
                continue;
            }
            ui::group(ui, pal, Some(group), |ui| {
                for (i, (d, phone)) in list.iter().enumerate() {
                    let on = d.state.starts_with("connected");
                    let sub = match (on, phone.is_some()) {
                        (true, _) if standing_by(&d.device) => "Connected, standing by".to_string(),
                        (true, true) => "Connected through the phone".to_string(),
                        (true, false) => "Connected".to_string(),
                        (false, true) => "Plugged in, not connected (is the phone sharing its connection?)".to_string(),
                        (false, false) => "Cable plugged in, not connected".to_string(),
                    };
                    let title = match phone {
                        Some(name) => format!("Tether ({name})"),
                        None => format!("Ethernet ({})", d.device),
                    };
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
        let parts = sections(&n, &self.wifi_shown);
        let mut connect: Option<(String, bool)> = None;
        let mut join: Option<String> = None;
        let mut join_hidden = false;
        // the switch, and the network this computer is on right under it, on one card
        ui::group(ui, pal, Some("Wi-Fi"), |ui| {
            ui::row(ui, pal, true, "Wi-Fi", Some(if has_wifi { "Connect to wireless networks" } else { "No Wi-Fi adapter found" }), |ui| {
                if has_wifi && ui::switch(ui, pal, &mut wifi_on).changed() {
                    toggled = true;
                }
            });
            if let Some(w) = parts.connected.filter(|_| listing) {
                let sub = wifi_dev.filter(|d| standing_by(&d.device)).map(|_| "Connected, standing by");
                if ui::net_row(ui, pal, false, true, &w.ssid, sub, w.secured, Some(w.signal)) != ui::NetClick::None {
                    match saved_for(&w.ssid) {
                        Some(p) => open = Some(p.uuid.clone()),
                        None => air_open = Some(AirKey { ssid: w.ssid.clone(), bssid: None }),
                    }
                }
            }
        });
        if listing {
            // Known Networks: saved, in range, not the one in use. A click joins; the (i) opens its page.
            let Sections { known, other, hidden, .. } = parts;
            if !known.is_empty() {
                ui::group(ui, pal, Some("Known Networks"), |ui| {
                    for (i, (w, p, faded)) in known.iter().enumerate() {
                        let sub = faded.then_some(NOT_HEARD);
                        match faded_row(ui, *faded, |ui| ui::net_row(ui, pal, i == 0, false, &w.ssid, sub, w.secured, Some(w.signal))) {
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
                for (i, (w, faded)) in other.iter().enumerate() {
                    let sub = faded.then_some(NOT_HEARD);
                    match faded_row(ui, *faded, |ui| ui::net_row(ui, pal, i == 0, false, &w.ssid, sub, w.secured, Some(w.signal))) {
                        ui::NetClick::Row => connect = Some((w.ssid.clone(), w.secured)),
                        ui::NetClick::Info => air_open = Some(AirKey { ssid: w.ssid.clone(), bssid: None }),
                        ui::NetClick::None => {}
                    }
                }
                for (i, (a, faded)) in hidden.iter().enumerate() {
                    let ap = a.best();
                    let maker = sys::maker(&ap.bss.bssid);
                    let heard = if *faded { NOT_HEARD } else { "" };
                    let sub = [maker, ap.band().label(), heard].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
                    // nothing to join without its name: the row opens its page too
                    let first = other.is_empty() && i == 0;
                    let (secured, quality) = (ap.security().kind.secured(), Some(ap.quality()));
                    let click =
                        faded_row(ui, *faded, |ui| ui::net_row(ui, pal, first, false, "Hidden network", Some(&sub), secured, quality));
                    if click != ui::NetClick::None {
                        air_open = Some(AirKey { ssid: String::new(), bssid: Some(ap.bss.bssid) });
                    }
                }
                // one that hides its name is joined by its name: typed here, with its security and password
                if ui::nav_row(ui, pal, false, "Hidden network…", Some("Join a network that does not broadcast its name"), |_| {}) {
                    join_hidden = true;
                }
            });
            if join_hidden {
                self.wifi_join = Some(Request { hidden: true, secured: true, ..Request::default() });
            }
            // looking all the while the list is on screen (wifi_live.rs): a quiet line instead of a Scan button
            if self.wifi_scan.as_ref().is_some_and(wifi_live::Scanner::on) {
                ui.add_space(4.0);
                ui.label(RichText::new("Searching…").size(12.0).color(pal.dim));
            }
        }
        if toggled {
            let arg = if wifi_on { "on" } else { "off" };
            self.change(ctx, move || sys::act("nmcli", &["radio", "wifi", arg]));
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
        // Heimdall's heimdall-multilink, when it is installed (multilink_ui.rs)
        let mut multilink = false;
        if crate::multilink_ui::installed() {
            ui::group(ui, pal, None, |ui| {
                let sub = "Wi-Fi, Ethernet and a phone together, for more speed";
                multilink = ui::nav_row(ui, pal, true, "Several connections at once", Some(sub), |_| {});
            });
        }
        if multilink {
            self.multilink = Some(crate::multilink_ui::Panel::new(ctx));
            self.status.clear();
        }
        if let Some((ssid, ask)) = connect {
            if ask {
                self.wifi_join = Some(Request::new(&ssid, ""));
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
                    self.wifi_join = Some(Request::new(&w.ssid, ""));
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
            None => note(
                ui,
                pal,
                "Not heard in the last scan. Settings keeps looking while this page is open: move closer, and it shows up here.",
            ),
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
            // a network that is not home is looked at when its page opens, and only then (HEI-74)
            if live.is_some_and(|l| !l.router.is_empty()) {
                self.lan_look_start(ctx, n, uuid, false);
            }
            if let Some(l) = self.lan_shown(n, uuid) {
                let sub = crate::lan_summary(&l, now);
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
            self.trust_group(ui, pal, ctx, uuid);
        } else {
            self.wired_trust_group(ui, pal, ctx, &saved.name, uuid, live.is_some());
        }
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

impl App {
    /// A saved Wi-Fi network's Heimdall card (wifitrust.rs): its trust level, the switch that lets others on the network
    /// discover this computer, and what kind of network it is. Read when the page opens; flipping the switch starts Heimdall's
    /// unit, choosing a kind runs Heimdall's guard through pkexec (polkit asks for the password either way), and it is read
    /// again.
    fn trust_group(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, uuid: &str) {
        let fresh = self.wifi_trust.as_ref().filter(|t| t.uuid == uuid).cloned();
        if fresh.is_none() && self.wifi_trust_busy.is_none() {
            let id = uuid.to_string();
            self.trust_thread(ctx, uuid, move || (wifitrust::load(&id), None));
        }
        let busy = self.wifi_trust_busy.is_some();
        let mut want: Option<bool> = None;
        let mut want_kind: Option<wifitrust::NetKind> = None;
        let mut foot: Option<&str> = None;
        ui::group(ui, pal, Some("Heimdall"), |ui| {
            let Some(t) = &fresh else {
                ui::row(ui, pal, true, wifitrust::DISC_TITLE, Some(wifitrust::DISC_ABOUT), |ui| {
                    ui.label(RichText::new("Reading...").color(pal.dim));
                });
                return;
            };
            let mut first = true;
            if let Some(l) = t.level {
                ui::row(ui, pal, true, "Trust level", Some(l.about()), |ui| {
                    ui.label(RichText::new(l.label()).color(pal.dim));
                });
                first = false;
            }
            ui::row(ui, pal, first, wifitrust::DISC_TITLE, Some(wifitrust::DISC_ABOUT), |ui| match t.disc {
                Ok(d) => {
                    let mut on = d != wifitrust::Disc::Off;
                    let free = !busy && d != wifitrust::Disc::Home;
                    if ui.add_enabled_ui(free, |ui| ui::switch(ui, pal, &mut on)).inner.changed() {
                        want = Some(on);
                    }
                    if busy {
                        ui.label(RichText::new("Waiting...").color(pal.dim));
                    }
                }
                Err(_) => {
                    let mut off = false;
                    ui.add_enabled_ui(false, |ui| ui::switch(ui, pal, &mut off));
                }
            });
            let about = match t.kind {
                wifitrust::KindState::Set(k) => k.about(),
                wifitrust::KindState::NotTrusted => wifitrust::KIND_NOT_TRUSTED,
                wifitrust::KindState::Missing(m) => m.note(),
            };
            ui::row(ui, pal, false, wifitrust::KIND_TITLE, Some(about), |ui| {
                let cur = match t.kind {
                    wifitrust::KindState::Set(k) => Some(k),
                    _ => None,
                };
                if busy {
                    ui.label(RichText::new("Waiting...").color(pal.dim));
                }
                ui.add_enabled_ui(cur.is_some() && !busy, |ui| {
                    egui::ComboBox::from_id_salt(("net-kind", &t.uuid))
                        .selected_text(cur.map_or("Not set", |k| k.label()))
                        .width(160.0)
                        .show_ui(ui, |ui| {
                            for k in wifitrust::NetKind::ALL {
                                if ui.selectable_label(cur == Some(k), k.label()).clicked() && cur != Some(k) {
                                    want_kind = Some(k);
                                }
                            }
                        });
                });
            });
            foot = match t.disc {
                Ok(wifitrust::Disc::Home) => Some("Always on for home networks."),
                Err(m) => Some(m.note()),
                Ok(_) => None,
            };
        });
        if let Some(f) = foot {
            ui::footnote(ui, pal, f);
        }
        if let Some(on) = want {
            self.status.clear();
            let id = uuid.to_string();
            self.trust_thread(ctx, uuid, move || {
                let asked = wifitrust::set(&id, on);
                let t = wifitrust::load(&id);
                let err = asked.err().or_else(|| wifitrust::check(&t, on).err());
                (t, err)
            });
        }
        if let Some(k) = want_kind {
            self.status.clear();
            let id = uuid.to_string();
            self.trust_thread(ctx, uuid, move || {
                let asked = wifitrust::set_kind(&id, k);
                let t = wifitrust::load(&id);
                let err = asked.err().or_else(|| wifitrust::check_kind(&t, k).err());
                (t, err)
            });
        }
    }

    /// A saved wired network's Heimdall card (wifitrust.rs, Heimdall's docs/WIFI_TRUST.md "Wired networks"): untrusted (a
    /// hardware address of its own, no name sent) or home, bound to its router. Read from the guard's status when the page opens;
    /// "Make it home" (with the real hardware address there, if chosen) and "Forget" run the guard through pkexec, then it is
    /// read again. Only the wired network connected now can be made home: the guard checks its router.
    fn wired_trust_group(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, name: &str, uuid: &str, connected: bool) {
        let fresh = self.wired_trust.as_ref().filter(|w| w.uuid == uuid).cloned();
        if fresh.is_none() && self.wifi_trust_busy.is_none() {
            let (id, nm) = (uuid.to_string(), name.to_string());
            self.wired_thread(ctx, uuid, move || (wifitrust::load_wired(&id, &nm, connected), None));
        }
        let busy = self.wifi_trust_busy.is_some();
        let mut want: Option<(bool, bool)> = None;
        let mut foot: Option<&str> = None;
        ui::group(ui, pal, Some("Heimdall"), |ui| {
            let Some(w) = &fresh else {
                ui::row(ui, pal, true, wifitrust::WIRED_HOME_TITLE, None, |ui| {
                    ui.label(RichText::new("Reading...").color(pal.dim));
                });
                return;
            };
            let st = match w.state {
                Ok(st) => st,
                Err(m) => {
                    ui::row(ui, pal, true, wifitrust::WIRED_HOME_TITLE, Some(wifitrust::WIRED_UNTRUSTED_ABOUT), |_| {});
                    foot = Some(if m == wifitrust::Missing::Newer { wifitrust::WIRED_NEWER } else { m.note() });
                    return;
                }
            };
            if st.home {
                ui::row(ui, pal, true, "Home network", Some(wifitrust::WIRED_HOME_ABOUT), |ui| {
                    if busy {
                        ui.label(RichText::new("Waiting...").color(pal.dim));
                    } else if ui::quiet(ui, pal, "Forget").on_hover_text("Untrusted again: an address of its own, no name sent").clicked() {
                        want = Some((false, false));
                    }
                });
                if let Some(own) = st.own_address {
                    let (t, sub) = if own {
                        ("One of its own", "From the next connection on, if you chose the real one")
                    } else {
                        ("The real one", "The router sees this computer as itself")
                    };
                    ui::row(ui, pal, false, "Hardware address here", Some(sub), |ui| {
                        ui.label(RichText::new(t).color(pal.dim));
                    });
                }
                return;
            }
            ui::row(ui, pal, true, wifitrust::WIRED_HOME_TITLE, Some(wifitrust::WIRED_UNTRUSTED_ABOUT), |ui| {
                if busy {
                    ui.label(RichText::new("Waiting...").color(pal.dim));
                } else if ui.add_enabled_ui(connected, |ui| ui::action(ui, pal, "Make it home")).inner.clicked() {
                    want = Some((true, self.wired_real));
                }
            });
            ui::row(ui, pal, false, wifitrust::WIRED_REAL_TITLE, Some(wifitrust::WIRED_REAL_ABOUT), |ui| {
                ui.add_enabled_ui(connected && !busy, |ui| ui::switch(ui, pal, &mut self.wired_real));
            });
            if !connected {
                foot = Some(wifitrust::WIRED_NEEDS_LINK);
            }
        });
        if let Some(f) = foot {
            ui::footnote(ui, pal, f);
        }
        if let Some((home, real)) = want {
            self.status.clear();
            let (id, nm) = (uuid.to_string(), name.to_string());
            self.wired_thread(ctx, uuid, move || {
                let asked = wifitrust::set_wired(&id, home, real);
                let w = wifitrust::load_wired(&id, &nm, connected);
                let err = asked.err().or_else(|| wifitrust::check_wired(&w, home).err());
                (w, err)
            });
        }
    }

    /// The wired card's reading (or change, then reading) on a thread of its own; the card waits for it meanwhile.
    fn wired_thread(&mut self, ctx: &egui::Context, uuid: &str, f: impl FnOnce() -> (wifitrust::Wired, Option<String>) + Send + 'static) {
        self.wifi_trust_busy = Some(uuid.to_string());
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (w, err) = f();
            let _ = tx.send(crate::Msg::WiredTrust(w, err));
            ctx.request_repaint();
        });
    }

    /// Reads (or changes, then reads) a network's Heimdall card on a thread of its own; the page waits for it meanwhile.
    fn trust_thread(&mut self, ctx: &egui::Context, uuid: &str, f: impl FnOnce() -> (wifitrust::Trust, Option<String>) + Send + 'static) {
        self.wifi_trust_busy = Some(uuid.to_string());
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (t, err) = f();
            let _ = tx.send(crate::Msg::WifiTrust(t, err));
            ctx.request_repaint();
        });
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
            let mut sub = vec![maker_text(m)];
            if let Some((_, last)) = n.memory.heard(&text) {
                sub.push(format!("last heard {}", devices::ago(now, last)));
            }
            ui::row(ui, pal, i == 0, &text, Some(&sub.join(" · ")), |_| {});
        }
    });
}

/// Who made an access point, or why that is not known.
fn maker_text(m: &[u8; 6]) -> String {
    if heimdall_macvendor::is_local(m) {
        return "Made-up address (often a second network on the same router)".into();
    }
    match sys::maker(m) {
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
            let mut sub = vec![maker_text(&ap.bss.bssid), ap.band().label().to_string()];
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
            None => match sys::element_maker(v.oui) {
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
    fn which_connection_the_internet_goes_through() {
        let links = vec![("usb0".to_string(), "Tether (Rover), over USB".to_string()), ("wlan0".to_string(), "Cafe (Wi-Fi)".to_string())];
        assert_eq!(
            super::internet_line(Some("usb0"), &links),
            Some(("Internet through Tether (Rover), over USB".to_string(), "Cafe (Wi-Fi) is connected too, standing by".to_string()))
        );
        assert_eq!(
            super::internet_line(Some("wlan0"), &links).unwrap().0,
            "Internet through Cafe (Wi-Fi)",
            "the Wi-Fi first when its route is"
        );
        let mut three = links.clone();
        three.push(("eth0".to_string(), "Ethernet (eth0)".to_string()));
        assert_eq!(
            super::internet_line(Some("eth0"), &three).unwrap().1,
            "Tether (Rover), over USB and Cafe (Wi-Fi) are connected too, standing by"
        );
        // one connection says enough by itself; a route through none of them (a VPN only) is not guessed
        assert_eq!(super::internet_line(Some("usb0"), &links[..1]), None);
        assert_eq!(super::internet_line(None, &links), None);
        assert_eq!(super::internet_line(Some("wg0"), &links), None);
    }

    #[test]
    fn the_list_in_its_sections() {
        let n = made_up();
        let mut shown = wifi_live::Shown::default();
        shown.update(std::time::Instant::now(), &n);
        let s = sections(&n, &shown);
        assert_eq!(s.connected.map(|w| w.ssid.as_str()), Some("Lantern Hall"));
        assert_eq!(s.known.iter().map(|(w, ..)| w.ssid.as_str()).collect::<Vec<_>>(), vec!["Birchwood Library"]);
        let mut other: Vec<&str> = s.other.iter().map(|(w, _)| w.ssid.as_str()).collect();
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

    /// The made-up fixture: what the radio heard counts as heard, the hidden access point is listed by its address.
    #[test]
    fn the_page_read_fills_both_lists() {
        let n = made_up();
        let mut sh = wifi_live::Shown::default();
        sh.update(std::time::Instant::now(), &n);
        assert_eq!(sh.named.rows().count(), n.networks.len());
        assert_eq!(sh.hidden.rows().count(), 1);
        assert!(sh.hidden.rows().all(|(_, faded)| !faded));
        // the radio's list could not be read: NetworkManager's list is all there is, nothing faded
        let mut blind = n.clone();
        blind.air = Default::default();
        blind.air_error = "no adapter".into();
        let mut sh = wifi_live::Shown::default();
        sh.update(std::time::Instant::now(), &blind);
        assert!(sh.named.rows().all(|(_, faded)| !faded));
    }

    #[test]
    fn spans() {
        assert_eq!(span(43_200), "12 h");
        assert_eq!(span(1800), "30 min");
        assert_eq!(span(30), "1 min");
        assert_eq!(span(3 * 86_400), "3 days");
    }

    /// The join window driven headless (an egui context with made-up keys, nothing drawn): Enter connects once a password
    /// is typed, Escape gives up; a hidden network is asked for by its name first.
    #[test]
    fn enter_connects_and_escape_cancels() {
        let ctx = egui::Context::default();
        let pal = yggdrasil_ui::theme::palette(true);
        let n = sys::Network::default();
        let key = |k| egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let frame = |r: &mut Request, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let mut done = None;
            ctx.run_ui(input, |ui| done = password_window(ui.ctx(), &pal, r, &n)).drop_without_applying_deltas();
            done
        };
        let mut r = Request::new("Home", "");
        assert_eq!(frame(&mut r, vec![]), None, "open, waiting");
        assert_eq!(frame(&mut r, vec![key(egui::Key::Enter)]), None, "Enter with no password does nothing");
        assert_eq!(frame(&mut r, vec![egui::Event::Text("hunter22".into())]), None);
        assert_eq!(r.password, "hunter22", "the box has the keyboard from the start");
        assert_eq!(frame(&mut r, vec![key(egui::Key::Enter)]), Some(true), "Enter connects");
        assert_eq!(frame(&mut r, vec![]), None, "and only on the frame it was pressed");
        assert_eq!(frame(&mut r, vec![key(egui::Key::Escape)]), Some(false), "Escape cancels");
        assert!(!r.real_address, "the system's own address unless asked");
        // a hidden network: its name first, then (secured) its password
        let mut h = Request { hidden: true, secured: true, ..Request::default() };
        assert_eq!(frame(&mut h, vec![]), None, "open, waiting");
        assert_eq!(frame(&mut h, vec![egui::Event::Text("Quiet".into())]), None);
        assert_eq!(h.ssid, "Quiet", "the name box has the keyboard while the name is empty");
        assert_eq!(frame(&mut h, vec![key(egui::Key::Enter)]), None, "no password yet");
        frame(&mut h, vec![]);
        assert_eq!(frame(&mut h, vec![egui::Event::Text("hunter22".into())]), None);
        assert_eq!(h.password, "hunter22");
        assert_eq!(frame(&mut h, vec![key(egui::Key::Enter)]), Some(true));
        let mut open = Request { hidden: true, secured: false, ssid: "Quiet".into(), ..Request::default() };
        assert_eq!(frame(&mut open, vec![key(egui::Key::Enter)]), Some(true), "an open hidden network needs no password");
    }
}
