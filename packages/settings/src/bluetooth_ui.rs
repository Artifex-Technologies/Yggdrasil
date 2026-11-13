//! Settings > Bluetooth, as on a phone: the switch, My Devices (paired, each Connected or Not Connected, an (i) for its own
//! page with Forget), Other Devices (found nearby while the page is open, a tap pairs) and the game controllers.
//!
//! Devices are looked for only while this page is open: a BlueZ discovery session on a D-Bus connection of its own, which the
//! page drops when it is left (BlueZ ends a client's discovery when its connection goes, so it stops even if Settings is
//! killed). A code to compare or type while pairing comes up in Yggdrasil's own agent (yggdrasil-polkit), not here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, vec2, RichText, Sense};
use yggdrasil_ui::theme::Palette;

use crate::{loading, sys, ui, App};

/// A device found nearby that is not paired.
#[derive(Clone, Debug, PartialEq)]
pub struct Nearby {
    pub mac: String,
    pub name: String,
    pub rssi: Option<i16>,
    pub paired: bool,
}

/// What the list shows of what BlueZ knows: not paired, with a name (a nameless one shows only its address: nothing to
/// recognise it by), the strongest signal first, then by name.
pub fn other_devices(mut all: Vec<Nearby>) -> Vec<Nearby> {
    all.retain(|d| !d.paired && !d.name.trim().is_empty() && d.name.replace('-', ":") != d.mac);
    all.sort_by(|a, b| b.rssi.unwrap_or(i16::MIN).cmp(&a.rssi.unwrap_or(i16::MIN)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    all
}

/// Looks for devices while it lives (the Bluetooth page is open): a thread with its own system-bus connection asks BlueZ to
/// discover, reads what it found every two seconds, and stops discovery when the scanner is dropped.
pub struct Scanner {
    alive: Arc<AtomicBool>,
    found: Arc<Mutex<Vec<Nearby>>>,
}

impl Scanner {
    pub fn start(ctx: &egui::Context) -> Scanner {
        let alive = Arc::new(AtomicBool::new(true));
        let found: Arc<Mutex<Vec<Nearby>>> = Default::default();
        let (a, f, ctx) = (alive.clone(), found.clone(), ctx.clone());
        std::thread::spawn(move || {
            if crate::fixture::get().is_some() {
                // made up: what `bluetoothctl devices` lists that is not paired
                let list = sys::bt_nearby().into_iter().map(|(mac, name)| Nearby { mac, name, rssi: None, paired: false }).collect();
                *f.lock().unwrap_or_else(|e| e.into_inner()) = other_devices(list);
                ctx.request_repaint();
                return;
            }
            if let Err(e) = discover(&a, &f, &ctx) {
                eprintln!("yggdrasil-settings: looking for Bluetooth devices: {e}");
            }
        });
        Scanner { alive, found }
    }

    pub fn found(&self) -> Vec<Nearby> {
        self.found.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

type Objects = std::collections::HashMap<
    zbus::zvariant::OwnedObjectPath,
    std::collections::HashMap<String, std::collections::HashMap<String, zbus::zvariant::OwnedValue>>,
>;

fn prop<T: TryFrom<zbus::zvariant::OwnedValue>>(p: &std::collections::HashMap<String, zbus::zvariant::OwnedValue>, k: &str) -> Option<T> {
    T::try_from(p.get(k)?.try_clone().ok()?).ok()
}

/// The discovery session: started, read every two seconds while `alive`, stopped (and the connection closed) after.
fn discover(alive: &AtomicBool, found: &Mutex<Vec<Nearby>>, ctx: &egui::Context) -> zbus::Result<()> {
    use zbus::blocking::{Connection, Proxy};
    let conn = Connection::system()?;
    let om = Proxy::new(&conn, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager")?;
    let objects: Objects = om.call("GetManagedObjects", &())?;
    // the first adapter (hci0 before hci1)
    let Some(adapter) = objects.iter().filter(|(_, i)| i.contains_key("org.bluez.Adapter1")).map(|(p, _)| p.to_string()).min() else {
        return Ok(());
    };
    let ad = Proxy::new(&conn, "org.bluez", adapter, "org.bluez.Adapter1")?;
    ad.call::<_, _, ()>("StartDiscovery", &())?;
    while alive.load(Ordering::Acquire) {
        if let Ok(objects) = om.call::<_, _, Objects>("GetManagedObjects", &()) {
            let list = objects
                .values()
                .filter_map(|i| i.get("org.bluez.Device1"))
                .filter_map(|d| {
                    Some(Nearby {
                        mac: prop::<String>(d, "Address")?,
                        name: prop::<String>(d, "Name").or_else(|| prop::<String>(d, "Alias")).unwrap_or_default(),
                        rssi: prop::<i16>(d, "RSSI"),
                        paired: prop::<bool>(d, "Paired").unwrap_or(false),
                    })
                })
                .collect();
            let list = other_devices(list);
            let mut f = found.lock().unwrap_or_else(|e| e.into_inner());
            if *f != list {
                *f = list;
                ctx.request_repaint();
            }
        }
        for _ in 0..8 {
            if !alive.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let _ = ad.call::<_, _, ()>("StopDiscovery", &());
    Ok(())
}

/// A device's kind as BlueZ's icon name says it.
pub fn kind_name(icon: &str) -> &'static str {
    match icon {
        "audio-headset" | "audio-headphones" => "Headphones",
        "audio-card" => "Speaker",
        "input-keyboard" => "Keyboard",
        "input-mouse" => "Mouse",
        "input-tablet" => "Drawing tablet",
        "input-gaming" => "Game controller",
        "phone" => "Phone",
        "computer" => "Computer",
        _ => "Device",
    }
}

/// A still "searching" mark beside "Other Devices" while looking: an open circle and the word, drawn once. A turning spinner
/// would redraw the whole window four or more times a second for as long as the page is open (15 % of a core with software
/// drawing), so it stands still.
fn searching(ui: &mut egui::Ui, pal: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    let pts: Vec<egui::Pos2> = (0..=12)
        .map(|k| {
            let a = -1.2 + k as f32 * 0.4;
            r.center() + vec2(a.cos(), a.sin()) * 5.5
        })
        .collect();
    ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.6, pal.dim)));
    ui.label(RichText::new("Searching").size(12.0).color(pal.dim));
}

impl App {
    pub(crate) fn page_bluetooth(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(b) = self.data.bluetooth.clone() else { return loading(ui) };
        if !b.present {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "No Bluetooth adapter found", None, |_| {}));
            return;
        }
        if self.bt_pairing_done.load(Ordering::Acquire) {
            self.bt_pairing = None;
        }
        if let Some(mac) = self.bt_open.clone() {
            return self.bt_device_page(ui, pal, ctx, &b, &mac);
        }
        let mut on = b.powered;
        let mut toggled = false;
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Bluetooth", None, |ui| {
                toggled = ui::switch(ui, pal, &mut on).changed();
            });
        });
        ui::footnote(
            ui,
            pal,
            "Keyboards, mice, headphones, phones and game controllers. Devices nearby are looked for only while this page is open.",
        );
        if toggled {
            let arg = if on { "on" } else { "off" };
            self.change(ctx, move || sys::act("bluetoothctl", &["power", arg]));
        }
        if !b.powered {
            return;
        }
        // My Devices: a tap connects one that is not connected (or opens one that is); the (i) opens its page
        let mut act: Option<(String, &'static str)> = None;
        let mut open: Option<String> = None;
        ui::group(ui, pal, Some("My Devices"), |ui| {
            if b.devices.is_empty() {
                let none = if b.paired_pads.is_empty() { "No devices yet" } else { "No devices other than controllers" };
                ui::row(ui, pal, true, none, Some("Put one in pairing mode: it shows up under Other Devices"), |_| {});
            }
            for (i, d) in b.devices.iter().enumerate() {
                let status = if self.bt_pairing.as_deref() == Some(d.mac.as_str()) {
                    "Connecting…"
                } else if d.connected {
                    "Connected"
                } else {
                    "Not Connected"
                };
                let sub = d.battery.map(|p| format!("{} · Battery {p}%", kind_name(&d.kind)));
                match ui::status_row(ui, pal, i == 0, &d.name, sub.as_deref(), status, true) {
                    ui::NetClick::Info => open = Some(d.mac.clone()),
                    ui::NetClick::Row if d.connected => open = Some(d.mac.clone()),
                    ui::NetClick::Row => act = Some((d.mac.clone(), "connect")),
                    ui::NetClick::None => {}
                }
            }
        });
        if let Some(m) = open {
            self.bt_open = Some(m);
            self.bt_forget = false;
        }
        if let Some((mac, verb)) = act {
            self.bt_pairing = Some(mac.clone());
            let busy = self.bt_pairing_done.clone();
            busy.store(false, Ordering::Release);
            self.change(ctx, move || {
                let r = sys::bt_act(verb, &mac);
                busy.store(true, Ordering::Release);
                r
            });
        }
        // Other Devices: found while the page is open; a tap pairs, trusts and connects
        let found = self.bt_scan.as_ref().map(Scanner::found).unwrap_or_default();
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Other Devices").strong().color(pal.dim));
            if self.bt_scan.is_some() {
                searching(ui, pal);
            }
        });
        ui.add_space(4.0);
        let mut pair: Option<String> = None;
        egui::Frame::new()
            .fill(pal.field)
            .stroke(egui::Stroke::new(1.0, pal.border))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::symmetric(14, 6))
            .show(ui, |ui| {
                ui.set_width((ui.available_width() - 2.0).max(0.0));
                if found.is_empty() {
                    ui::row(ui, pal, true, "Looking for devices…", Some("Put the device in pairing mode"), |_| {});
                }
                for (i, d) in found.iter().enumerate() {
                    let pairing = self.bt_pairing.as_deref() == Some(d.mac.as_str());
                    if ui::status_row(ui, pal, i == 0, &d.name, None, if pairing { "Pairing…" } else { "" }, false) != ui::NetClick::None
                        && !pairing
                    {
                        pair = Some(d.mac.clone());
                    }
                }
            });
        ui::footnote(ui, pal, "A tap pairs a device. If it shows a code, a box asks you to compare it or type it.");
        if let Some(mac) = pair {
            self.bt_pairing = Some(mac.clone());
            let busy = self.bt_pairing_done.clone();
            busy.store(false, Ordering::Release);
            self.change(ctx, move || {
                let r = sys::bt_pair(&mac);
                busy.store(true, Ordering::Release);
                r
            });
        }
        ui.add_space(8.0);
        self.bt_controllers(ui, pal, ctx, &b);
    }

    /// A paired device's own page: what it is, connect or disconnect, and Forget This Device (asked twice).
    fn bt_device_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, b: &sys::Bluetooth, mac: &str) {
        let Some(d) = b.devices.iter().find(|d| d.mac == mac).cloned() else {
            // forgotten (or gone): back to the list
            self.bt_open = None;
            return;
        };
        let mut act: Option<&'static str> = None;
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Name", None, |ui| {
                ui.label(RichText::new(&d.name).color(pal.dim));
            });
            ui::row(ui, pal, false, "Kind", None, |ui| {
                ui.label(RichText::new(kind_name(&d.kind)).color(pal.dim));
            });
            if let Some(p) = d.battery {
                ui::row(ui, pal, false, "Battery", None, |ui| {
                    ui.label(RichText::new(format!("{p}%")).color(pal.dim));
                });
            }
            ui::row(ui, pal, false, "Address", None, |ui| {
                ui.add(egui::Label::new(RichText::new(&d.mac).color(pal.dim)).selectable(true));
            });
        });
        ui::group(ui, pal, None, |ui| {
            let (title, verb) = if d.connected { ("Disconnect", "disconnect") } else { ("Connect", "connect") };
            ui::row(ui, pal, true, if d.connected { "Connected" } else { "Not Connected" }, None, |ui| {
                if ui::quiet(ui, pal, title).clicked() {
                    act = Some(verb);
                }
            });
        });
        ui::group(ui, pal, None, |ui| {
            if !self.bt_forget {
                ui::row(ui, pal, true, "Forget This Device", Some("It has to be paired again to connect"), |ui| {
                    if ui::quiet(ui, pal, "Forget").clicked() {
                        self.bt_forget = true;
                    }
                });
            } else {
                ui::row(ui, pal, true, &format!("Forget “{}”?", d.name), Some("It has to be paired again to connect"), |ui| {
                    if ui::quiet(ui, pal, "Cancel").clicked() {
                        self.bt_forget = false;
                    }
                    if ui::action(ui, pal, "Forget").clicked() {
                        act = Some("remove");
                    }
                });
            }
        });
        if let Some(verb) = act {
            if verb == "remove" {
                self.bt_open = None;
                self.bt_forget = false;
            }
            let mac = mac.to_string();
            self.change(ctx, move || sys::bt_act(verb, &mac));
        }
    }

    /// Bluetooth > Controllers: game controllers (connected over USB or Bluetooth, or paired and off) with their battery; one's
    /// options open below it: light bar colour, player light, a rumble test, disconnect and forget. The lights chosen are kept in
    /// ~/.config/yggdrasil/controllers.json, which the Controllers plugin reads to put them back on every connection.
    fn bt_controllers(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, b: &sys::Bluetooth) {
        const COLOURS: [(&str, [u8; 3]); 6] = [
            ("Purple", [124, 84, 214]),
            ("Blue", [40, 110, 255]),
            ("Red", [255, 40, 40]),
            ("Green", [34, 197, 94]),
            ("White", [255, 255, 255]),
            ("Off", [0, 0, 0]),
        ];
        enum Do {
            Select(String),
            Light(gamepads::Pad, [u8; 3]),
            Player(gamepads::Pad, u8),
            Rumble(gamepads::Pad),
            Bt(String, &'static str),
        }
        let mut todo: Option<Do> = None;
        // the paired Bluetooth controllers that are off (the connected ones are listed from the kernel, with their battery)
        let off: Vec<&sys::BtDevice> =
            b.paired_pads.iter().filter(|d| !b.pads.iter().any(|p| p.mac() == Some(d.mac.to_lowercase().as_str()))).collect();
        ui::group(ui, pal, Some("Controllers"), |ui| {
            if b.pads.is_empty() && off.is_empty() {
                ui::row(
                    ui,
                    pal,
                    true,
                    "No controllers yet",
                    Some("Use “Add a controller” to pair one, or plug one in with a cable"),
                    |_| {},
                );
            }
            for (i, p) in b.pads.iter().enumerate() {
                let key = p.uniq.clone();
                let open = self.pad_sel.as_deref() == Some(key.as_str());
                let mut sub = p.connection.label().to_string();
                if let Some(bat) = &p.battery {
                    sub.push_str(&format!(" · {}", bat.describe()));
                }
                ui::row(ui, pal, i == 0, &p.title(), Some(&sub), |ui| {
                    if ui::quiet(ui, pal, if open { "Close" } else { "Options" }).clicked() {
                        todo = Some(Do::Select(if open { String::new() } else { key.clone() }));
                    }
                    if let Some(pct) = p.battery.as_ref().and_then(|bat| bat.estimate()) {
                        let charging = p.battery.as_ref().is_some_and(|bat| bat.charging());
                        let (r, _) = ui.allocate_exact_size(egui::vec2(90.0, 8.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 4.0, pal.field);
                        let col = if charging || pct > 40 {
                            pal.green
                        } else if pct > 20 {
                            pal.amber
                        } else {
                            pal.red
                        };
                        let mut fill = r;
                        fill.set_width((r.width() * pct as f32 / 100.0).max(8.0));
                        ui.painter().rect_filled(fill, 4.0, col);
                    }
                });
                if !open {
                    continue;
                }
                if p.leds.has_lightbar() {
                    let current = b.pad_settings.lightbar_for(p);
                    ui::row(
                        ui,
                        pal,
                        false,
                        "Light bar",
                        if p.leds.writable { None } else { Some("Install Controller support from Agora to change the lights") },
                        |ui| {
                            ui.add_enabled_ui(p.leds.writable, |ui| {
                                for (name, rgb) in COLOURS.iter().rev() {
                                    let (r, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                                    let c = if *rgb == [0, 0, 0] { pal.field } else { egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]) };
                                    ui.painter().circle_filled(r.center(), 10.0, c);
                                    if current == Some(*rgb) || resp.hovered() {
                                        ui.painter().circle_stroke(r.center(), 11.5, egui::Stroke::new(2.0, pal.text));
                                    }
                                    if resp.on_hover_text(*name).clicked() {
                                        todo = Some(Do::Light(p.clone(), *rgb));
                                    }
                                }
                            });
                        },
                    );
                }
                if !p.leds.players.is_empty() {
                    let now = b.pad_settings.player_for(p).unwrap_or(0) as usize;
                    ui::row(ui, pal, false, "Player light", None, |ui| {
                        ui.add_enabled_ui(p.leds.writable, |ui| {
                            if let Some(n) = ui::segmented(ui, pal, &["Off", "1", "2", "3", "4", "5"], now) {
                                todo = Some(Do::Player(p.clone(), n as u8));
                            }
                        });
                    });
                }
                if let Some(mode) = p.mode() {
                    // an 8BitDo: the mode it was switched into decides which driver takes it
                    let note = match mode {
                        "Xbox mode" => "Recommended. Over Bluetooth it works best with Xbox controller support from Agora",
                        "Switch mode" => "Driven like a Switch Pro Controller",
                        _ => "Basic buttons only; Xbox mode (Start + X when pairing) works best",
                    };
                    ui::row(ui, pal, false, &format!("Mode: {mode}"), Some(note), |_| {});
                    let fw_note = if p.model == gamepads::Model::XboxCompatible {
                        "If it is an 8BitDo: use a Chromium-based browser such as Vivaldi; connect the controller by USB"
                    } else {
                        "Use a Chromium-based browser such as Vivaldi; connect the controller by USB"
                    };
                    ui::row(ui, pal, false, "Firmware updates", Some(fw_note), |ui| {
                        if ui::quiet(ui, pal, "Open 8BitDo's updater").clicked() {
                            let _ = std::process::Command::new("xdg-open")
                                .arg("https://web.8bitdo.com")
                                .stdin(std::process::Stdio::null())
                                .spawn();
                        }
                    });
                }
                ui::row(ui, pal, false, "Rumble", Some("A short buzz, to check it works"), |ui| {
                    ui.add_enabled_ui(p.event.is_some(), |ui| {
                        if ui::quiet(ui, pal, "Test").clicked() {
                            todo = Some(Do::Rumble(p.clone()));
                        }
                    });
                });
                if let Some(mac) = p.mac() {
                    let mac = mac.to_uppercase();
                    ui::row(ui, pal, false, &mac, Some("Bluetooth address"), |ui| {
                        if ui::quiet(ui, pal, "Forget").clicked() {
                            todo = Some(Do::Bt(mac.clone(), "remove"));
                        }
                        if ui::quiet(ui, pal, "Disconnect").clicked() {
                            todo = Some(Do::Bt(mac.clone(), "disconnect"));
                        }
                    });
                }
            }
            for (i, d) in off.iter().enumerate() {
                ui::row(ui, pal, i == 0 && b.pads.is_empty(), &d.name, Some("Not connected · press its PS, Xbox or Home button"), |ui| {
                    if ui::quiet(ui, pal, "Forget").clicked() {
                        todo = Some(Do::Bt(d.mac.clone(), "remove"));
                    }
                    if ui::action(ui, pal, "Connect").clicked() {
                        todo = Some(Do::Bt(d.mac.clone(), "connect"));
                    }
                });
            }
            if !std::path::Path::new("/usr/bin/yggdrasil-controllers").exists() {
                ui::row(
                    ui,
                    pal,
                    false,
                    "Controller support",
                    Some("Battery in the top bar, and your lights kept between connections"),
                    |ui| {
                        if ui::quiet(ui, pal, "Get it in Agora").clicked() {
                            sys::spawn("agora", &[]);
                        }
                    },
                );
            }
        });
        match todo {
            Some(Do::Select(k)) => self.pad_sel = (!k.is_empty()).then_some(k),
            Some(Do::Light(p, rgb)) => {
                let mut st = b.pad_settings.clone();
                st.per_pad.entry(p.uniq.clone()).or_default().lightbar = Some(gamepads::color_hex(rgb));
                self.change(ctx, move || {
                    st.save().map_err(|e| e.to_string())?;
                    gamepads::set_lightbar(&p, rgb)
                });
            }
            Some(Do::Player(p, n)) => {
                let mut st = b.pad_settings.clone();
                st.per_pad.entry(p.uniq.clone()).or_default().player = Some(n);
                self.change(ctx, move || {
                    st.save().map_err(|e| e.to_string())?;
                    gamepads::set_player(&p, n)
                });
            }
            Some(Do::Rumble(p)) => {
                // it blocks for the length of the buzz: on a thread, not reloading the page
                std::thread::spawn(move || {
                    let _ = gamepads::rumble(&p, 400);
                });
            }
            Some(Do::Bt(mac, verb)) => {
                if verb == "remove" {
                    self.pad_sel = None;
                }
                self.change(ctx, move || sys::bt_act(verb, &mac));
            }
            None => {}
        }
        ui.add_space(8.0);
        self.pad_add_flow(ui, pal);
    }

    /// Bluetooth > Controllers > Add a controller: which kind, and how to put it in pairing mode; it then turns up under Other
    /// Devices, where a tap pairs it.
    fn pad_add_flow(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        use gamepads::Model;
        const KINDS: [(&str, Model); 6] = [
            ("PlayStation 4", Model::DualShock4),
            ("PlayStation 5", Model::DualSense),
            ("Xbox", Model::Xbox),
            ("Switch Pro", Model::SwitchPro),
            ("8BitDo", Model::EightBitDo),
            ("Other", Model::Other),
        ];
        match self.pad_add {
            None => {
                if ui::action(ui, pal, "Add a controller…").clicked() {
                    self.pad_add = Some(None);
                }
            }
            Some(None) => {
                let mut pick = None;
                ui::group(ui, pal, Some("Which controller?"), |ui| {
                    for (i, (name, m)) in KINDS.iter().enumerate() {
                        ui::row(ui, pal, i == 0, name, None, |ui| {
                            if ui::action(ui, pal, "Choose").clicked() {
                                pick = Some(*m);
                            }
                        });
                    }
                });
                if ui::quiet(ui, pal, "Cancel").clicked() {
                    self.pad_add = None;
                }
                if let Some(m) = pick {
                    self.pad_add = Some(Some(m));
                }
            }
            Some(Some(m)) => {
                ui::group(ui, pal, Some("Pairing mode"), |ui| {
                    ui::row(ui, pal, true, m.pairing_hint(), Some("Then tap it under Other Devices when it turns up"), |_| {});
                    if m == Model::Xbox || m == Model::EightBitDo {
                        ui::row(
                            ui,
                            pal,
                            false,
                            "Xbox controllers (and 8BitDo in Xbox mode) work best with Xbox controller support from Agora (xpadneo)",
                            None,
                            |_| {},
                        );
                    }
                });
                // it is looked for with everything else while the page is open: it turns up under Other Devices
                if ui::quiet(ui, pal, "Done").clicked() {
                    self.pad_add = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(mac: &str, name: &str, rssi: Option<i16>, paired: bool) -> Nearby {
        Nearby { mac: mac.into(), name: name.into(), rssi, paired }
    }

    #[test]
    fn other_devices_are_the_unpaired_ones_with_a_name_strongest_first() {
        let list = other_devices(vec![
            d("5A:00:00:B7:20:01", "Example Speaker", Some(-70), false),
            d("5A:00:00:B7:20:02", "Example Phone", Some(-50), false),
            d("5A:00:00:B7:10:01", "Example Headphones", Some(-40), true),
            d("5A:00:00:B7:20:03", "5A-00-00-B7-20-03", Some(-30), false),
            d("5A:00:00:B7:20:04", "", Some(-30), false),
            d("5A:00:00:B7:20:05", "Another Example", None, false),
        ]);
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Example Phone", "Example Speaker", "Another Example"]);
    }

    #[test]
    fn kinds_have_names() {
        assert_eq!(kind_name("audio-headset"), "Headphones");
        assert_eq!(kind_name("something-new"), "Device");
    }
}
