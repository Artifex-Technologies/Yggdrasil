//! General, as on a tablet: the page Settings opens on. Rows to About, Software Update (Agora's Updates), Storage, Date & Time,
//! Keyboard and Language & Region. About and Date & Time are pages of their own that moved in here from the sidebar; Storage
//! and Language & Region are new and small; Keyboard is Peripherals' own page.

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;

use crate::{loading, peripherals, sys, ui, App, Page};

/// One mounted disk as `df` sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Disk {
    pub source: String,
    pub mount: String,
    pub size: u64,
    pub used: u64,
    pub avail: u64,
}

/// What Storage reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Storage {
    pub disks: Vec<Disk>,
}

pub const DF_ARGS: [&str; 13] = [
    "-B1",
    "--output=source,size,used,avail,target",
    "-x",
    "tmpfs",
    "-x",
    "devtmpfs",
    "-x",
    "efivarfs",
    "-x",
    "squashfs",
    "-x",
    "overlay",
    "-l",
];

/// `df -B1 --output=source,size,used,avail,target`: one disk per source (a btrfs disk shows once, at its shortest mount
/// point), the system's own disk first. Pseudo filesystems and anything smaller than 64 MiB (a boot partition's firmware
/// part) are left out.
pub fn parse_df(text: &str) -> Vec<Disk> {
    let mut disks: Vec<Disk> = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 || !f[0].starts_with('/') {
            continue;
        }
        let num = |s: &str| s.parse::<u64>().unwrap_or(0);
        let d = Disk { source: f[0].to_string(), size: num(f[1]), used: num(f[2]), avail: num(f[3]), mount: f[4..].join(" ") };
        if d.size < 64 << 20 {
            continue;
        }
        match disks.iter_mut().find(|x| x.source == d.source) {
            Some(x) if d.mount.len() < x.mount.len() => *x = d,
            Some(_) => {}
            None => disks.push(d),
        }
    }
    disks.sort_by_key(|d| (d.mount != "/", d.mount.clone()));
    disks
}

/// A size for Storage: megabytes under a gigabyte (a boot partition), else as everywhere else in Settings.
fn size(bytes: u64) -> String {
    if bytes < 1_000_000_000 {
        format!("{} MB", bytes / 1_000_000)
    } else {
        ui::gib(bytes)
    }
}

pub fn storage() -> Storage {
    Storage { disks: parse_df(&sys::run("df", &DF_ARGS).unwrap_or_default()) }
}

/// What Language & Region reads: the system's language (LANG) and the ones that can be chosen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Language {
    pub lang: String,
    /// LC_TIME, when it differs from LANG (dates and times in another region's style).
    pub formats: Option<String>,
    pub available: Vec<String>,
}

/// `localectl status`'s "System Locale: LANG=en_US.UTF-8" (and the LC_ lines under it).
pub fn parse_locale(status: &str) -> (String, Option<String>) {
    let mut lang = String::new();
    let mut time = None;
    for line in status.lines() {
        let l = line.trim().trim_start_matches("System Locale:").trim();
        if let Some(v) = l.strip_prefix("LANG=") {
            lang = v.to_string();
        } else if let Some(v) = l.strip_prefix("LC_TIME=") {
            time = Some(v.to_string());
        }
    }
    let time = time.filter(|t| *t != lang);
    (lang, time)
}

pub fn language() -> Language {
    let (lang, formats) = parse_locale(&sys::run("localectl", &["status"]).unwrap_or_default());
    let mut available: Vec<String> = sys::run("localectl", &["list-locales"])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    if !lang.is_empty() && !available.contains(&lang) {
        available.insert(0, lang.clone());
    }
    Language { lang, formats, available }
}

/// "en_US.UTF-8" as people say it: "English (United States)". Codes not in the short list below are shown as they are.
pub fn locale_name(code: &str) -> String {
    let base = code.split(['.', '@']).next().unwrap_or(code);
    let (l, r) = base.split_once('_').unwrap_or((base, ""));
    let lang = match l {
        "en" => "English",
        "de" => "Deutsch",
        "nl" => "Nederlands",
        "fr" => "Français",
        "es" => "Español",
        "it" => "Italiano",
        "pt" => "Português",
        "sv" => "Svenska",
        "da" => "Dansk",
        "nb" | "no" => "Norsk",
        "fi" => "Suomi",
        "pl" => "Polski",
        "cs" => "Čeština",
        "tr" => "Türkçe",
        "ru" => "Русский",
        "uk" => "Українська",
        "el" => "Ελληνικά",
        "ja" => "日本語",
        "zh" => "中文",
        "ko" => "한국어",
        "C" | "POSIX" => return "Plain (C)".into(),
        _ => return code.to_string(),
    };
    let region = match r {
        "" => return lang.to_string(),
        "US" => "United States",
        "GB" => "United Kingdom",
        "CA" => "Canada",
        "AU" => "Australia",
        "IE" => "Ireland",
        "NZ" => "New Zealand",
        "DE" => "Deutschland",
        "AT" => "Österreich",
        "CH" => "Schweiz",
        "NL" => "Nederland",
        "BE" => "België",
        "FR" => "France",
        "ES" => "España",
        "MX" => "México",
        "IT" => "Italia",
        "PT" => "Portugal",
        "BR" => "Brasil",
        "SE" => "Sverige",
        "DK" => "Danmark",
        "NO" => "Norge",
        "FI" => "Suomi",
        "PL" => "Polska",
        "JP" => "日本",
        "CN" => "中国",
        "TW" => "台灣",
        "KR" => "대한민국",
        other => other,
    };
    format!("{lang} ({region})")
}

/// Agora on its Updates page: the software updates of the whole system (packages and apps) are Agora's.
fn open_updates() {
    if crate::fixture::get().is_some() {
        return;
    }
    let _ = std::process::Command::new("agora")
        .env("AGORA_PAGE", "updates")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// The operating system's version (VERSION_ID, else BUILD_ID, in os-release), for the Software Update row.
fn os_version() -> String {
    let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .map(|v| v.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    };
    match (get("NAME"), get("VERSION_ID").or_else(|| get("BUILD_ID"))) {
        (Some(n), Some(v)) => format!("{n} {v}"),
        (Some(n), None) => n,
        _ => String::new(),
    }
}

impl App {
    /// General: rows to the pages inside it.
    pub(crate) fn page_general(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let mut open: Option<Page> = None;
        ui::group(ui, pal, None, |ui| {
            if ui::nav_row(ui, pal, true, "About", None, |_| {}) {
                open = Some(Page::About);
            }
            let version = os_version();
            if ui::nav_row(ui, pal, false, "Software Update", None, |ui| {
                if !version.is_empty() {
                    ui.label(RichText::new(&version).color(pal.dim));
                }
            }) {
                open_updates();
            }
        });
        ui::footnote(ui, pal, "Updates for the system and your apps are in Agora; Software Update opens them there.");
        ui::group(ui, pal, None, |ui| {
            if ui::nav_row(ui, pal, true, "Storage", None, |_| {}) {
                open = Some(Page::Storage);
            }
        });
        ui::group(ui, pal, None, |ui| {
            if ui::nav_row(ui, pal, true, "Date & Time", None, |_| {}) {
                open = Some(Page::DateTime);
            }
            if ui::nav_row(ui, pal, false, "Keyboard", None, |_| {}) {
                open = Some(Page::Peripherals);
                self.peri.sub = peripherals::Sub::Keyboard;
                self.media = None;
                self.key_sub = crate::KeySub::Main;
            }
            if ui::nav_row(ui, pal, false, "Language & Region", None, |_| {}) {
                open = Some(Page::Language);
            }
        });
        if let Some(p) = open {
            self.open_page(p, ui.ctx());
        }
    }

    /// General > Storage: each disk's use, and where the space goes.
    pub(crate) fn page_storage(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(s) = self.data.storage.clone() else { return loading(ui) };
        if s.disks.is_empty() {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "No disks could be read", None, |_| {}));
        }
        for d in &s.disks {
            let title = if d.mount == "/" { "This computer".to_string() } else { d.mount.clone() };
            ui::group(ui, pal, Some(&title), |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("{} of {} used", size(d.used), size(d.size))).size(15.0).strong());
                ui.add_space(6.0);
                let frac = if d.size > 0 { d.used as f32 / d.size as f32 } else { 0.0 };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 12.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 6.0, pal.hover);
                let mut fill = rect;
                fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
                ui.painter().rect_filled(fill, 6.0, yggdrasil_ui::theme::usage_color(pal, frac));
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{} free", size(d.avail))).size(12.0).color(pal.dim));
                ui.add_space(8.0);
            });
        }
        let mut open: Option<Page> = None;
        ui::group(ui, pal, None, |ui| {
            if ui::nav_row(ui, pal, true, "Apps", Some("How much space each app and its data take"), |_| {}) {
                open = Some(Page::Apps);
            }
            if ui::nav_row(ui, pal, false, "Restore points", Some("Snapshots of the system, kept on the same disk"), |_| {}) {
                open = Some(Page::Restore);
            }
        });
        ui::footnote(ui, pal, "Space is read when this page opens; nothing is measured in the background.");
        if let Some(p) = open {
            self.open_page(p, ui.ctx());
        }
    }

    /// General > Language & Region: the system's language (and region) for every account.
    pub(crate) fn page_language(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(l) = self.data.language.clone() else { return loading(ui) };
        let mut pick: Option<String> = None;
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Language and region", Some(&l.lang), |ui| {
                egui::ComboBox::from_id_salt("lang").selected_text(locale_name(&l.lang)).width(260.0).height(360.0).show_ui(ui, |ui| {
                    for c in &l.available {
                        if ui.selectable_label(*c == l.lang, format!("{}  ·  {c}", locale_name(c))).clicked() && *c != l.lang {
                            pick = Some(c.clone());
                        }
                    }
                });
            });
            if let Some(f) = &l.formats {
                ui::row(ui, pal, false, "Dates and times", Some(f), |ui| {
                    ui.label(RichText::new(locale_name(f)).color(pal.dim));
                });
            }
        });
        ui::footnote(
            ui,
            pal,
            "The language of menus and messages, and how dates, times and numbers are written, for every account on this \
             computer. It applies the next time you sign in. Keyboard layouts are under Keyboard.",
        );
        if let Some(c) = pick {
            let arg = format!("LANG={c}");
            self.change(ctx, move || sys::act("localectl", &["set-locale", &arg]));
        }
    }

    /// Opens a page as its sidebar row would (from a row on another page).
    pub(crate) fn open_page(&mut self, page: Page, ctx: &egui::Context) {
        if page != self.page {
            self.page = page;
            self.status.clear();
            self.load(page, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn df_lists_each_disk_once_system_first() {
        let text = "Filesystem 1B-blocks Used Avail Mounted on\n\
            /dev/nvme0n1p2 500000000000 200000000000 300000000000 /home\n\
            /dev/nvme0n1p2 500000000000 200000000000 300000000000 /\n\
            /dev/nvme0n1p1 1073741824 104857600 968884224 /boot\n\
            /dev/nvme0n1p9 33554432 1048576 32505856 /boot/efi\n\
            /dev/sda1 2000000000000 1500000000000 500000000000 /run/media/alex/Big Disk\n\
            tmpfs 8000000000 0 8000000000 /tmp\n";
        let d = parse_df(text);
        assert_eq!(d.iter().map(|d| d.mount.as_str()).collect::<Vec<_>>(), ["/", "/boot", "/run/media/alex/Big Disk"]);
        assert_eq!(d[0].used, 200_000_000_000);
    }

    #[test]
    fn localectl_gives_the_language_and_a_separate_formats_region() {
        let s = "   System Locale: LANG=en_US.UTF-8\n                  LC_TIME=en_GB.UTF-8\n       VC Keymap: us\n";
        assert_eq!(parse_locale(s), ("en_US.UTF-8".into(), Some("en_GB.UTF-8".into())));
        assert_eq!(parse_locale("System Locale: LANG=de_DE.UTF-8\n LC_TIME=de_DE.UTF-8"), ("de_DE.UTF-8".into(), None));
        assert_eq!(locale_name("en_US.UTF-8"), "English (United States)");
        assert_eq!(locale_name("nl_BE.UTF-8"), "Nederlands (België)");
        assert_eq!(locale_name("xx_YY.UTF-8"), "xx_YY.UTF-8");
    }
}
