//! General, as on a tablet: the page Settings opens on. Rows to About, Software Update (the system's updates, sysupdate.rs, and
//! a row to Agora's app updates), Storage, Date & Time, Keyboard and Language & Region. About and Date & Time are pages of their
//! own that moved in here from the sidebar; Storage and Language & Region are new and small; Keyboard is Peripherals' own page.

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;

use crate::icons::Icon;
use crate::{loading, peripherals, sys, sysupdate, ui, App, Page};

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

/// Agora on its Updates page, which installs updates (the system's and the apps') with its own progress and one password.
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

/// A row of More options: an icon, the title (and a grey line), the count if any, and a chevron. True when clicked.
fn option_row(ui: &mut egui::Ui, pal: &Palette, first: bool, icon: Icon, title: &str, sub: Option<&str>, count: &str) -> bool {
    let bg = ui.painter().add(egui::Shape::Noop);
    let id = ui.next_auto_id();
    let r = ui.scope(|ui| {
        ui::row_with(
            ui,
            pal,
            first,
            |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                    crate::icons::paint(ui.painter(), rect, icon, pal.text);
                    ui.add_space(6.0);
                    ui.label(RichText::new(title).size(14.0));
                });
            },
            sub,
            |ui| {
                ui.label(RichText::new("\u{203a}").size(20.0).color(pal.dim));
                if !count.is_empty() {
                    ui.label(RichText::new(count).color(pal.dim));
                }
            },
        );
    });
    let resp = ui.interact(r.response.rect, id.with("opt"), egui::Sense::click());
    if resp.hovered() {
        let mut hl = r.response.rect.expand2(egui::vec2(6.0, 0.0));
        hl.min.y += if first { 3.0 } else { 4.0 };
        hl.max.y -= 3.0;
        ui.painter().set(bg, egui::Shape::rect_filled(hl, egui::CornerRadius::same(8), pal.hover));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// Two upright bars, the pause sign, in the size of the other rows' icons.
fn pause_icon(ui: &mut egui::Ui, pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
    for x in [6.0, 12.0] {
        let bar = egui::Rect::from_min_size(rect.min + egui::vec2(x, 3.0), egui::vec2(3.0, 14.0));
        ui.painter().rect_stroke(bar, egui::CornerRadius::same(1), egui::Stroke::new(1.5, pal.text), egui::StrokeKind::Middle);
    }
    ui.add_space(6.0);
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
                open = Some(Page::SoftwareUpdate);
            }
        });
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

    /// Ask the repositories on a thread of its own; the answer comes back as Msg::SysUpdates.
    pub(crate) fn check_sys_updates(&mut self, ctx: &egui::Context) {
        if self.sys_checking {
            return;
        }
        self.sys_checking = true;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(crate::Msg::SysUpdates(sysupdate::check(), false));
            ctx.request_repaint();
        });
    }

    /// General > Software Update: the big update icon with its badge, the state and when it was checked, Check for updates
    /// (Install updates once the assistant approved exactly these updates); then More options (Available Updates, Pause
    /// updates, Update history, App Updates) and Help with updates.
    pub(crate) fn page_software_update(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        match self.upd_sub {
            sysupdate::Sub::Available => return self.page_available_updates(ui, pal),
            sysupdate::Sub::History => return self.page_update_history(ui, pal),
            sysupdate::Sub::Main => {}
        }
        let Some(s) = self.data.sys_updates.clone() else { return loading(ui) };
        let now = yggdrasil_updates::pause::now();
        let approved = matches!(s.validation, sysupdate::Validation::Approved(_)) && !s.updates.is_empty();
        let mut check = false;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(72.0, 72.0), egui::Sense::hover());
            crate::icons::paint(ui.painter(), rect.shrink(8.0), Icon::Restart, pal.accent);
            // the badge: up to date, updates waiting, or approved by the assistant
            let badge = egui::Rect::from_center_size(rect.right_bottom() - egui::vec2(14.0, 14.0), egui::vec2(26.0, 26.0));
            let (fill, mark) = if approved {
                (pal.accent, Some(Icon::Shield))
            } else if s.updates.is_empty() && s.checked > 0 {
                (pal.green, Some(Icon::Check))
            } else if !s.updates.is_empty() {
                (pal.accent, None)
            } else {
                (egui::Color32::TRANSPARENT, None)
            };
            if fill != egui::Color32::TRANSPARENT {
                let r = if mark.is_some() { 12.0 } else { 7.0 };
                ui.painter().circle_filled(badge.center(), r, fill);
                if let Some(m) = mark {
                    crate::icons::paint(ui.painter(), badge.shrink(7.0), m, egui::Color32::WHITE);
                }
            }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(12.0);
                ui.label(RichText::new(sysupdate::headline(&s, self.sys_checking)).size(20.0).strong());
                ui.label(RichText::new(sysupdate::last_checked(&s, now, self.sys_clock24)).color(pal.dim));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.sys_checking {
                    ui.spinner();
                } else if approved {
                    if ui::action(ui, pal, "Install updates").clicked() {
                        open_updates();
                    }
                } else if ui::action(ui, pal, "Check for updates").clicked() {
                    check = true;
                }
            });
        });
        if let Some(note) = s.validation.note() {
            ui::footnote(ui, pal, &note);
        }
        ui.add_space(12.0);
        let mut go: Option<sysupdate::Sub> = None;
        let mut pause: Option<bool> = None;
        let paused = self.sys_paused;
        ui::group(ui, pal, Some("More options"), |ui| {
            let n = s.updates.len();
            let sub = match n {
                0 => "None right now".to_string(),
                _ if s.download == 0 => format!("{n} to install, already downloaded"),
                _ => format!("{n} to install, {} to download", sysupdate::size(s.download)),
            };
            if option_row(ui, pal, true, Icon::Info, "Available Updates", Some(&sub), &n.to_string()) {
                go = Some(sysupdate::Sub::Available);
            }
            let (title, sub) = match paused {
                Some(t) => (format!("Paused until {}", sysupdate::day(t)), "Nothing looks for updates by itself until then"),
                None => (
                    "Pause updates".to_string(),
                    "No automatic checks for a week; pacman in a terminal is never held. Game time already holds them during games",
                ),
            };
            ui::row_with(
                ui,
                pal,
                false,
                |ui| {
                    ui.horizontal(|ui| {
                        pause_icon(ui, pal);
                        ui.label(RichText::new(&title).size(14.0));
                    });
                },
                Some(sub),
                |ui| {
                    let label = if paused.is_some() { "Resume" } else { "Pause for 1 week" };
                    if ui::quiet(ui, pal, label).clicked() {
                        pause = Some(paused.is_none());
                    }
                },
            );
            if option_row(ui, pal, false, Icon::Clock, "Update history", Some("Every update and install, newest first"), "") {
                go = Some(sysupdate::Sub::History);
            }
            if option_row(ui, pal, false, Icon::Apps, "App Updates", Some("Apps, Flatpaks and the system's own apps, in Agora"), "") {
                open_updates();
            }
        });
        ui.add_space(12.0);
        ui::group(ui, pal, Some("Related support"), |ui| {
            egui::CollapsingHeader::new(RichText::new("Help with updates").size(14.0)).id_salt("upd-help").show(ui, |ui| {
                for line in [
                    "Check for updates asks the update servers what is new, without changing anything; it never needs a password.",
                    "Installing is Agora's: its Updates page installs the system's and the apps' updates with one administrator password, and shows its progress.",
                    "Update history lists every update and install, including those made with pacman in a terminal.",
                    "More in the Yggdrasil documentation: docs/APPS_SETTINGS.md, Software Update.",
                ] {
                    ui.add(egui::Label::new(RichText::new(line).color(pal.dim)).wrap());
                }
            });
        });
        if check {
            self.check_sys_updates(ctx);
        }
        if let Some(on) = pause {
            let r =
                if on { yggdrasil_updates::pause::pause(now, yggdrasil_updates::pause::WEEK) } else { yggdrasil_updates::pause::resume() };
            match r {
                Ok(()) => self.sys_paused = yggdrasil_updates::pause::until(now),
                Err(e) => self.status = format!("Not changed: {e}"),
            }
        }
        if let Some(g) = go {
            self.upd_sub = g;
            if g == sysupdate::Sub::History {
                self.sys_history = None;
                let (tx, ctx) = (self.tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(crate::Msg::UpdHistory(sysupdate::history()));
                    ctx.request_repaint();
                });
            }
        }
    }

    /// Software Update > Available Updates: each update with its versions, size and repository, the kernel, drivers and the
    /// base system first; installing them is Agora's.
    fn page_available_updates(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(s) = self.data.sys_updates.clone() else { return loading(ui) };
        if s.updates.is_empty() {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "No updates right now", None, |_| {}));
            return;
        }
        ui::group(ui, pal, None, |ui| {
            let sub = format!("{} to download · Agora installs them with one administrator password", sysupdate::size(s.download));
            ui::row(ui, pal, true, &format!("{} updates", s.updates.len()), Some(&sub), |ui| {
                if ui::action(ui, pal, "Update in Agora").clicked() {
                    open_updates();
                }
            });
        });
        for g in sysupdate::GROUPS {
            let list: Vec<&yggdrasil_updates::Update> = s.updates.iter().filter(|u| sysupdate::group(u) == g).collect();
            if list.is_empty() {
                continue;
            }
            ui.add_space(8.0);
            ui::group(ui, pal, Some(&format!("{g} ({})", list.len())), |ui| {
                for (i, u) in list.iter().enumerate() {
                    let mut sub = vec![format!("{} -> {}", u.old, u.new)];
                    if !u.repo.is_empty() {
                        sub.push(u.repo.clone());
                    }
                    ui::row(ui, pal, i == 0, &u.name, Some(&sub.join(" · ")), |ui| {
                        let size = if u.size == 0 { "downloaded".to_string() } else { sysupdate::size(u.size) };
                        ui.label(RichText::new(size).color(pal.dim));
                    });
                }
            });
        }
    }

    /// Software Update > Update History: every package transaction in pacman's log, newest first, each opening to its packages.
    fn page_update_history(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(h) = self.sys_history.clone() else { return loading(ui) };
        if h.is_empty() {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "No updates or installs recorded", None, |_| {}));
            return;
        }
        ui::group(ui, pal, None, |ui| {
            for (i, t) in h.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let counts: Vec<String> = t.counts().iter().map(|(c, n)| format!("{n} {}", c.word())).collect();
                let mut head = format!("{}  ·  {}", t.started.get(..16).unwrap_or(&t.started).replace('T', " "), counts.join(", "));
                match t.outcome {
                    yggdrasil_updates::history::Outcome::Completed => {}
                    yggdrasil_updates::history::Outcome::Failed => head.push_str("  ·  failed"),
                    yggdrasil_updates::history::Outcome::Unfinished => head.push_str("  ·  did not finish"),
                }
                egui::CollapsingHeader::new(RichText::new(head).strong()).id_salt(("upd-hist", i)).show(ui, |ui| {
                    if !t.command.is_empty() {
                        ui.label(RichText::new(&t.command).monospace().size(12.0).color(pal.dim));
                    }
                    for p in &t.packages {
                        let v = match p.change {
                            yggdrasil_updates::history::Change::Upgraded | yggdrasil_updates::history::Change::Downgraded => {
                                format!("{} -> {}", p.old, p.new)
                            }
                            yggdrasil_updates::history::Change::Removed => p.old.clone(),
                            _ => p.new.clone(),
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(p.change.word()).size(12.0).color(pal.dim));
                            ui.label(&p.name);
                            ui.label(RichText::new(v).color(pal.dim));
                        });
                    }
                });
            }
        });
        ui::footnote(ui, pal, "From pacman's log: updates and installs made here, in Agora or in a terminal (the newest 300).");
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
