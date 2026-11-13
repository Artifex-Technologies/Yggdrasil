//! Where you are inside a page, and the way back: one header over every page, as on a tablet.
//!
//! A page's sub-pages are kept in its own fields (`app_sub`, `key_sub`, `net_open`, ...). This module reads them as one stack:
//! `path()` is the page and every level opened under it, `back()` takes the top level off (one at a time), and
//! `rewind()` takes them all off (a click on the sidebar row of the page). The header above the page draws the path
//! ("Appearance › Wallpaper") with a Back button, which stays put while the page scrolls; the mouse's back button and Alt+Left
//! go back too.

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;

use crate::{icons::Icon, peripherals, ui, App, AppSub, KeySub, Page, SecSub};

impl App {
    /// The page's name as the sidebar shows it.
    pub(crate) fn page_title(&self, page: Page) -> String {
        if page == Page::Assistant {
            if let Some(n) = yggdrasil_conf::integrations::first("assistant").and_then(|i| i.get("name").map(str::to_string)) {
                return n;
            }
        }
        crate::PAGES.iter().find(|(p, ..)| *p == page).map(|(_, n, ..)| n.to_string()).unwrap_or_default()
    }

    /// The open page, then each level opened under it, outermost first.
    pub(crate) fn path(&self) -> Vec<String> {
        let mut path: Vec<String> = crate::parent(self.page).map(|p| self.page_title(p)).into_iter().collect();
        path.push(self.page_title(self.page));
        path.extend(self.levels());
        path
    }

    /// The levels opened under the page, outermost first (empty on the page's first level).
    fn levels(&self) -> Vec<String> {
        let mut l: Vec<String> = Vec::new();
        match self.page {
            Page::Network => {
                let net = self.data.network.as_ref();
                if self.wifi_history || self.net_open.is_some() && self.net_from_history {
                    l.push("Previously connected networks".into());
                }
                if let Some(uuid) = &self.net_open {
                    let p = net.and_then(|n| n.saved.iter().find(|p| &p.uuid == uuid));
                    l.push(match p {
                        Some(p) if p.wifi && !p.ssid.is_empty() => p.ssid.clone(),
                        Some(p) => p.name.clone(),
                        None => "Network".into(),
                    });
                } else if let Some(k) = &self.wifi_air {
                    l.push(if k.ssid.is_empty() { "Hidden network".into() } else { k.ssid.clone() });
                }
                if self.lan_open {
                    l.push("Devices".into());
                    if let Some(mac) = &self.lan_dev {
                        let d = net.and_then(|n| n.lan.as_ref()).and_then(|(_, l)| l.devices.iter().find(|d| &d.mac == mac));
                        l.push(d.map(|d| crate::devices::shown_name(d, &self.lan_names)).unwrap_or_else(|| "Device".into()));
                    }
                } else if self.wifi_check_open {
                    l.push("Wi-Fi checks".into());
                }
            }
            Page::Appearance => {
                let t = match self.app_sub {
                    AppSub::Main => None,
                    AppSub::Icons => Some("Icons"),
                    AppSub::Bar => Some("Top bar and dock"),
                    AppSub::Notifs => Some("Notifications"),
                    AppSub::Wallpaper => Some("Wallpaper"),
                    AppSub::Minos => Some("Minos"),
                    AppSub::Lock => Some("Lock screen"),
                    AppSub::Workspaces => Some("Workspaces"),
                    AppSub::Boot => Some("Boot screen"),
                };
                l.extend(t.map(String::from));
            }
            Page::Power => {
                if self.power_fans {
                    l.push("Cooling".into());
                } else if self.power_gpu {
                    l.push("Graphics card".into());
                } else if self.power_cpu {
                    l.push("Processor".into());
                }
            }
            Page::Security => {
                let t = match self.sec_sub {
                    SecSub::Main => None,
                    SecSub::Monitor => Some(
                        yggdrasil_conf::integrations::security().map(crate::security::monitor_name).unwrap_or_else(|| "Monitor".into()),
                    ),
                    SecSub::Logins => Some("Login history".into()),
                    SecSub::Sudo => Some("Sudo history".into()),
                    SecSub::Distress => Some("Distress password".into()),
                    SecSub::Limbo => Some("Limbo".into()),
                };
                l.extend(t);
            }
            Page::Apps => {
                match self.app_tab {
                    1 => l.push("Optional software".into()),
                    2 => l.push("Minos".into()),
                    _ => {}
                }
                if let Some(id) = &self.app_sel {
                    let name = self.data.installed.as_ref().and_then(|a| a.iter().find(|a| &a.id == id)).map(|a| a.name.clone());
                    l.push(name.unwrap_or_else(|| id.clone()));
                }
            }
            Page::Peripherals => match self.peri.sub {
                peripherals::Sub::Main => {}
                peripherals::Sub::Mouse => l.push("Mouse".into()),
                peripherals::Sub::Keyboard => {
                    l.push("Keyboard".into());
                    match self.key_sub {
                        KeySub::Main => {}
                        KeySub::Shortcuts => l.push("Shortcuts".into()),
                        KeySub::Spelling => l.push("Spelling and languages".into()),
                        KeySub::SpellDownload => {
                            l.push("Spelling and languages".into());
                            l.push("Download more languages".into());
                        }
                    }
                }
            },
            _ => {}
        }
        l
    }

    /// Back one level (from a page inside another, such as General > About, to that page). False when there is nowhere to go.
    pub(crate) fn back(&mut self) -> bool {
        if self.levels().is_empty() {
            let Some(up) = crate::parent(self.page) else { return false };
            self.page = up;
            self.status.clear();
            return true;
        }
        match self.page {
            Page::Network => {
                if self.lan_dev.is_some() {
                    self.lan_dev = None;
                    self.lan_rename = None;
                } else if self.lan_open {
                    self.lan_open = false;
                } else if self.wifi_check_open {
                    self.wifi_check_open = false;
                } else if self.net_open.is_some() {
                    self.net_open = None;
                    self.net_draft = None;
                    self.net_forget = false;
                    // back to the list it was opened from
                    self.wifi_history = self.net_from_history;
                    self.net_from_history = false;
                } else if self.wifi_air.is_some() {
                    self.wifi_air = None;
                } else {
                    self.wifi_history = false;
                }
            }
            Page::Appearance => self.app_sub = AppSub::Main,
            Page::Power => {
                self.power_fans = false;
                self.power_gpu = false;
                self.power_cpu = false;
                self.unlock_confirm = None;
            }
            Page::Security => {
                self.sec_sub = SecSub::Main;
                self.distress.close();
            }
            Page::Apps => {
                if self.app_sel.is_some() {
                    self.app_sel = None;
                    self.app_confirm = None;
                } else {
                    self.app_tab = 0;
                }
            }
            Page::Peripherals => match (self.peri.sub, self.key_sub) {
                (peripherals::Sub::Keyboard, KeySub::SpellDownload) => self.key_sub = KeySub::Spelling,
                (peripherals::Sub::Keyboard, KeySub::Shortcuts | KeySub::Spelling) => self.key_sub = KeySub::Main,
                _ => self.peri.sub = peripherals::Sub::Main,
            },
            _ => {}
        }
        self.status.clear();
        true
    }

    /// Every level of `page` closed: the page as it first opens (its sidebar row was clicked).
    pub(crate) fn rewind(&mut self, page: Page) {
        match page {
            Page::Network => {
                self.net_open = None;
                self.net_draft = None;
                self.net_forget = false;
                self.net_from_history = false;
                self.wifi_air = None;
                self.wifi_history = false;
                self.lan_open = false;
                self.lan_dev = None;
                self.lan_rename = None;
                self.wifi_check_open = false;
            }
            Page::Appearance => self.app_sub = AppSub::Main,
            Page::Power => {
                self.power_fans = false;
                self.power_gpu = false;
                self.power_cpu = false;
                self.unlock_confirm = None;
            }
            Page::Security => {
                self.sec_sub = SecSub::Main;
                self.sec_open = None;
            }
            Page::Apps => {
                self.app_sel = None;
                self.app_confirm = None;
                self.app_tab = 0;
            }
            Page::Peripherals => {
                self.peri.sub = peripherals::Sub::Main;
                self.key_sub = KeySub::Main;
            }
            Page::Games => self.games.reload(),
            _ => {}
        }
        // nothing secret (the distress password's boxes) stays in memory after its page is left
        self.distress.close();
    }

    /// The mouse's back button or Alt+Left, taken from this frame's input (not while a text box has the keyboard: there
    /// Alt+Left is the text's own).
    pub(crate) fn back_pressed(ctx: &egui::Context) -> bool {
        let typing = ctx.memory(|m| m.focused().is_some());
        let mouse = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1));
        let keys = !typing && ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft));
        mouse || keys
    }

    /// The header over the page: Back (on a sub-page) and the path, the last part strong. True when Back was clicked.
    pub(crate) fn nav_header(ui: &mut egui::Ui, pal: &Palette, path: &[String]) -> bool {
        let mut back = false;
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            if path.len() > 1 {
                let parent = &path[path.len() - 2];
                if ui::icon_button(ui, pal, Icon::Back, &format!("Back to {parent} (Alt+Left)"), true, false).clicked() {
                    back = true;
                }
                ui.add_space(2.0);
            }
            let mut job = egui::text::LayoutJob::default();
            let n = path.len();
            for (i, p) in path.iter().enumerate() {
                let last = i + 1 == n;
                let (size, color) = if last { (20.0, pal.text) } else { (15.0, pal.dim) };
                RichText::new(p).size(size).color(color).strong().append_to(
                    &mut job,
                    ui.style(),
                    egui::FontSelection::Default,
                    egui::Align::Center,
                );
                if !last {
                    RichText::new("  \u{203a}  ").size(15.0).color(pal.dim).append_to(
                        &mut job,
                        ui.style(),
                        egui::FontSelection::Default,
                        egui::Align::Center,
                    );
                }
            }
            ui.add(egui::Label::new(job).truncate());
        });
        back
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Page;

    fn app() -> App {
        App::bare()
    }

    #[test]
    fn a_page_on_its_first_level_has_nowhere_to_go_back_to() {
        let mut a = app();
        a.page = Page::Sound;
        assert_eq!(a.path(), ["Sound"]);
        assert!(!a.back());
        assert_eq!(a.path(), ["Sound"]);
    }

    #[test]
    fn back_goes_up_one_level_at_a_time() {
        let mut a = app();
        a.page = Page::Peripherals;
        a.peri.sub = peripherals::Sub::Keyboard;
        a.key_sub = KeySub::SpellDownload;
        assert_eq!(a.path(), ["Peripherals", "Keyboard", "Spelling and languages", "Download more languages"]);
        assert!(a.back());
        assert_eq!(a.path(), ["Peripherals", "Keyboard", "Spelling and languages"]);
        assert!(a.back());
        assert_eq!(a.path(), ["Peripherals", "Keyboard"]);
        assert!(a.back());
        assert_eq!(a.path(), ["Peripherals"]);
        assert!(!a.back());
    }

    #[test]
    fn the_network_pages_go_back_the_way_they_came() {
        let mut a = app();
        a.page = Page::Network;
        // Previously connected networks > a network > Devices > one device
        a.wifi_history = true;
        a.net_open = Some("uuid-1".into());
        a.net_from_history = true;
        a.wifi_history = false;
        a.lan_open = true;
        a.lan_dev = Some("02:00:00:00:00:01".into());
        assert_eq!(a.path(), ["Wi-Fi & Network", "Previously connected networks", "Network", "Devices", "Device"]);
        let mut seen = vec![a.path().len()];
        while a.back() {
            seen.push(a.path().len());
        }
        assert_eq!(seen, [5, 4, 3, 2, 1]);
        assert!(a.net_open.is_none() && !a.wifi_history && !a.lan_open);
        // Wi-Fi checks, opened from the connected network's page
        a.net_open = Some("uuid-1".into());
        a.wifi_check_open = true;
        assert_eq!(a.path(), ["Wi-Fi & Network", "Network", "Wi-Fi checks"]);
        a.back();
        assert_eq!(a.path(), ["Wi-Fi & Network", "Network"]);
    }

    #[test]
    fn the_sidebar_row_of_the_open_page_returns_to_its_first_level() {
        let mut a = app();
        a.page = Page::Appearance;
        a.app_sub = AppSub::Wallpaper;
        assert_eq!(a.path(), ["Appearance", "Wallpaper"]);
        a.rewind(Page::Appearance);
        assert_eq!(a.path(), ["Appearance"]);
        a.page = Page::Power;
        a.power_gpu = true;
        assert_eq!(a.path(), ["Power", "Graphics card"]);
        a.rewind(Page::Power);
        assert_eq!(a.path(), ["Power"]);
        a.page = Page::Security;
        a.sec_sub = SecSub::Logins;
        a.back();
        assert_eq!(a.path(), ["Security"]);
    }

    #[test]
    fn a_page_inside_general_goes_back_to_general() {
        let mut a = app();
        a.page = Page::DateTime;
        assert_eq!(a.path(), ["General", "Date & Time"]);
        assert!(a.back());
        assert_eq!(a.page, Page::General);
        assert_eq!(a.path(), ["General"]);
        assert!(!a.back());
    }

    #[test]
    fn every_page_has_a_title() {
        let a = app();
        for (p, ..) in crate::PAGES {
            assert!(!a.page_title(p).is_empty(), "{p:?}");
        }
    }
}
