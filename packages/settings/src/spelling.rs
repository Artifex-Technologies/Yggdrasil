//! Settings > Keyboard > Spelling and languages: spell checking on or off, the languages (several at once: a word right in any of
//! them is not marked), downloading and removing a language (its dictionary package, through Agora's package helper and the
//! administrator question), and the user's word list with the app each word came from. The library is yggdrasil-spell.

use eframe::egui::{self, RichText};
use yggdrasil_spell::{apps, conf::SpellConf, dicts, words::WordList};
use yggdrasil_ui::theme::Palette;

use crate::{ui, App, Msg};

#[derive(Default)]
pub struct State {
    conf: Option<SpellConf>,
    installed: Vec<dicts::Installed>,
    packages: Vec<(&'static str, &'static str, bool)>,
    words: Option<WordList>,
    packs: Vec<yggdrasil_spell::packs::Pack>,
    filter: String,
    new_word: String,
    /// the computer's shared list: how many words, and (for an administrator) the words one account offered
    shared: usize,
    pending: Option<Vec<String>>,
    /// the package being downloaded or removed
    pub busy: Option<String>,
    pub note: String,
}

impl State {
    pub fn reload(&mut self) {
        self.conf = Some(SpellConf::load());
        self.installed = dicts::installed();
        self.packages = dicts::PACKAGES.iter().map(|p| (p.package, p.name, dicts::package_installed(p.package))).collect();
        self.words = Some(WordList::load());
        self.packs = yggdrasil_spell::packs::installed();
        self.shared = yggdrasil_spell::words::global().len();
        self.pending = yggdrasil_spell::global::pending(std::path::Path::new(yggdrasil_spell::global::DIR));
    }
}

/// An administrator's choice for the shared list (`pkexec /usr/lib/yggdrasil/spell-global approve|deny WORD`), then tells the window.
fn global_job(tx: std::sync::mpsc::Sender<Msg>, ctx: egui::Context, word: String, approve: bool) {
    std::thread::spawn(move || {
        let out = std::process::Command::new("pkexec").arg("/usr/lib/yggdrasil/spell-global").arg(if approve { "approve" } else { "deny" }).arg(&word).output();
        let r = match out {
            Ok(o) if o.status.success() => Ok(format!("\"{word}\" {}", if approve { "is shared with every account" } else { "will not be shared" })),
            Ok(o) if o.status.code() == Some(126) || o.status.code() == Some(127) => Err("Not done: the password was not given".to_string()),
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("not done").to_string()),
            Err(_) => Err("pkexec is not available".to_string()),
        };
        // the shared words reach this account's apps too
        let _ = apps::sync_all();
        let _ = tx.send(Msg::Spell(r));
        ctx.request_repaint();
    });
}

/// Runs the package helper (`pkexec /usr/lib/agora/pkg-root`, the desktop's password box) to install or remove one dictionary
/// package, then tells the window.
fn package_job(tx: std::sync::mpsc::Sender<Msg>, ctx: egui::Context, package: &'static str, install: bool) {
    std::thread::spawn(move || {
        let run = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| "/tmp".into()).join("agora");
        let _ = std::fs::create_dir_all(&run);
        let cancel = run.join(format!("spell-{}.cancel", std::process::id()));
        let before: Vec<String> = dicts::installed().into_iter().map(|d| d.code).collect();
        let mut args = vec![cancel.to_string_lossy().to_string(), "pacman".into()];
        args.extend(if install { ["-S", "--needed", "--noconfirm"] } else { ["-R", "--noconfirm", "--"] }.iter().map(|s| s.to_string()));
        args.push(package.to_string());
        let out = std::process::Command::new("pkexec").arg("/usr/lib/agora/pkg-root").args(&args).output();
        let r = match out {
            Ok(o) if o.status.success() => Ok(format!("{} {}", package, if install { "installed" } else { "removed" })),
            Ok(o) if o.status.code() == Some(126) || o.status.code() == Some(127) => Err("Not done: the password was not given".to_string()),
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("pacman did not finish").to_string()),
            Err(_) => Err("pkexec is not available".to_string()),
        };
        // like a phone: a downloaded language is switched on (its main dictionary, not the "more words" variants)
        if install && r.is_ok() {
            let mut conf = SpellConf::load();
            if conf.languages.is_empty() {
                conf.languages = conf.active();
            }
            for d in dicts::installed() {
                if !before.contains(&d.code) && !d.code.contains('-') && !conf.languages.contains(&d.code) {
                    conf.languages.push(d.code);
                }
            }
            let _ = conf.save();
        }
        // new languages get the user's words in their GTK list
        let _ = apps::sync_all();
        let _ = tx.send(Msg::Spell(r));
        ctx.request_repaint();
    });
}

impl App {
    pub(crate) fn page_spelling(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        if self.spell.conf.is_none() {
            self.spell.reload();
        }
        let mut conf = self.spell.conf.clone().unwrap_or_default();
        let active = conf.active();
        let mut conf_changed = false;
        let mut resync = false;
        let mut autocorrect_changed = false;

        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Check spelling", Some("Mark misspelled words in apps that check spelling (GTK apps, browsers, Electron apps)"), |ui| {
                if ui::switch(ui, pal, &mut conf.enabled).changed() {
                    conf_changed = true;
                    resync = true;
                    autocorrect_changed = true;
                }
            });
            ui::row(ui, pal, false, "Correct as I type", Some("Fixes a word when you finish it, only when the fix is all but certain (teh, adn, thier). Backspace straight after puts your word back. Never in passwords, numbers, addresses or terminals"), |ui| {
                ui.add_enabled_ui(conf.enabled, |ui| {
                    if ui::switch(ui, pal, &mut conf.autocorrect).changed() {
                        conf_changed = true;
                        autocorrect_changed = true;
                    }
                });
            });
            if conf.autocorrect {
                let mut electron = yggdrasil_spell::electron::is_on();
                ui::row(ui, pal, false, "Also in Electron apps", Some("VS Code, Obsidian and other Electron apps, after they are restarted. With some versions this flag has caused doubled or lost keys there: switch it off again if typing goes wrong"), |ui| {
                    if ui::switch(ui, pal, &mut electron).changed() {
                        if let Err(e) = yggdrasil_spell::electron::set(electron) {
                            self.spell.note = format!("Electron apps: {e}");
                        }
                    }
                });
            }
        });

        ui.add_space(8.0);
        let mut more = false;
        ui::group(ui, pal, Some("Languages"), |ui| {
            if self.spell.installed.is_empty() {
                ui::row(ui, pal, true, "No language is installed", Some("Download one: \"Download more languages\" below"), |_| {});
            }
            for (i, d) in self.spell.installed.iter().enumerate() {
                let mut on = active.contains(&d.code);
                let sub = if d.code.contains("-large") { Some("Also knows rarer words") } else { None };
                ui::row(ui, pal, i == 0, &d.name(), sub, |ui| {
                    ui.add_enabled_ui(conf.enabled, |ui| {
                        if ui::switch(ui, pal, &mut on).changed() {
                            // the first change turns "the system's language" into a list of its own
                            if conf.languages.is_empty() {
                                conf.languages = active.clone();
                            }
                            conf.languages.retain(|c| c != &d.code);
                            if on {
                                conf.languages.push(d.code.clone());
                            }
                            conf_changed = true;
                            resync = true;
                        }
                    });
                });
            }
            more = ui::nav_row(ui, pal, false, "Download more languages…", Some("Add or remove languages"), |_| {});
        });
        ui.label(RichText::new("Several languages can be on at once: a word that is right in any of them is not marked.").color(pal.dim));
        if more {
            self.key_sub = crate::KeySub::SpellDownload;
        }

        // word packs: words the language dictionaries do not have (yggdrasil-spell-packs)
        if !self.spell.packs.is_empty() {
            ui.add_space(8.0);
            let on_now = conf.active_packs();
            ui::group(ui, pal, Some("Word packs"), |ui| {
                for (i, p) in self.spell.packs.iter().enumerate() {
                    let mut on = on_now.contains(&p.id);
                    let sub = format!("{} · {} words", p.description, p.count);
                    ui::row(ui, pal, i == 0, &p.name, Some(&sub), |ui| {
                        ui.add_enabled_ui(conf.enabled, |ui| {
                            if ui::switch(ui, pal, &mut on).changed() {
                                let mut list = on_now.clone();
                                list.retain(|x| x != &p.id);
                                if on {
                                    list.push(p.id.clone());
                                }
                                conf.packs = Some(list);
                                conf_changed = true;
                                resync = true;
                            }
                        });
                    });
                }
            });
            ui.label(RichText::new("Pack words are known everywhere while the pack is on, and leave the apps' lists again when it is switched off. They are not added to your words.").color(pal.dim));
        }

        ui.add_space(8.0);
        let mut add: Option<String> = None;
        let mut remove: Option<String> = None;
        let words = self.spell.words.clone().unwrap_or_default();
        ui::group(ui, pal, Some(&format!("Your words ({})", words.len())), |ui| {
            ui::row(ui, pal, true, "Add a word", Some("Known to every app from then on"), |ui| {
                // the row lays its controls out from the right: the button first, so it ends up right of the box
                let clicked = ui::action(ui, pal, "Add").clicked();
                let enter = ui.add(egui::TextEdit::singleline(&mut self.spell.new_word).hint_text("Word").desired_width(180.0)).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (clicked || enter) && !self.spell.new_word.trim().is_empty() {
                    add = Some(self.spell.new_word.trim().to_string());
                }
            });
            if words.len() > 8 {
                ui::row(ui, pal, false, "Find", None, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.spell.filter).hint_text("Search your words").desired_width(220.0));
                });
            }
            let q = self.spell.filter.to_lowercase();
            for (w, from) in words.entries().filter(|(w, _)| q.is_empty() || w.to_lowercase().contains(&q)).take(200) {
                let sub = if from.is_empty() { "Added here".to_string() } else { format!("From {}", from.join(", ")) };
                ui::row(ui, pal, false, w, Some(&sub), |ui| {
                    if ui::quiet(ui, pal, "Remove").on_hover_text("Takes the word out of every app's list too").clicked() {
                        remove = Some(w.to_string());
                    }
                });
            }
        });
        ui.label(RichText::new("Words you add in any app (\"Add to dictionary\") come here, and every app gets them. A browser reads its list when it starts.").color(pal.dim));
        ui.add_space(8.0);
        let mut choice: Option<(String, bool)> = None;
        ui::group(ui, pal, Some("Shared with this computer"), |ui| {
            ui::row(ui, pal, true, "Offer my words", Some("A word you add goes to every account on this computer once a second account adds it too, or an administrator approves it. Only plain words: never anything with digits, links or addresses"), |ui| {
                if ui::switch(ui, pal, &mut conf.share).changed() {
                    conf_changed = true;
                    resync = true;
                }
            });
            ui::row(ui, pal, false, "Shared words", Some("Every account on this computer gets these"), |ui| {
                ui.label(self.spell.shared.to_string());
            });
            // administrators (the wheel group) can read the words waiting; nobody else sees which words another account offered
            if let Some(waiting) = self.spell.pending.clone().filter(|p| !p.is_empty()) {
                ui::row(ui, pal, false, &format!("Waiting for you ({})", waiting.len()), Some("Offered by one account. Share gives it to every account; Don't share keeps it out for good. Both ask for your password"), |_| {});
                for w in waiting.iter().take(100) {
                    ui::row(ui, pal, false, w, None, |ui| {
                        let idle = self.spell.busy.is_none();
                        if ui.add_enabled(idle, egui::Button::new("Don't share")).clicked() {
                            choice = Some((w.clone(), false));
                        }
                        if ui.add_enabled(idle, egui::Button::new("Share")).clicked() {
                            choice = Some((w.clone(), true));
                        }
                    });
                }
            }
        });
        if let Some((w, approve)) = choice {
            self.spell.busy = Some(format!("global:{w}"));
            self.spell.note.clear();
            global_job(self.tx.clone(), ctx.clone(), w, approve);
        }
        if let Some(w) = add.as_deref() {
            if !yggdrasil_spell::words::valid(w) {
                self.spell.note = format!("\"{w}\" is not kept: a word has no spaces and is not a link, an address or a code");
            } else {
                let _ = apps::edit_words(|l| {
                    l.add(w, "");
                });
                self.spell.new_word.clear();
                self.spell.words = Some(WordList::load());
            }
        }
        if let Some(w) = remove.as_deref() {
            let _ = apps::edit_words(|l| {
                l.remove(w);
            });
            self.spell.words = Some(WordList::load());
        }

        if !self.spell.note.is_empty() {
            ui.label(RichText::new(&self.spell.note).color(pal.dim));
        }

        if conf_changed {
            let _ = conf.save();
            // the input method runs only while it is wanted (no cost otherwise): its user service follows the switch
            if autocorrect_changed {
                let on = conf.enabled && conf.autocorrect;
                std::thread::spawn(move || {
                    let verb = if on { "enable" } else { "disable" };
                    let _ = std::process::Command::new("systemctl").args(["--user", verb, "--now", "yggdrasil-autocorrect.service"]).output();
                });
            }
            self.spell.conf = Some(conf);
        }
        if resync {
            std::thread::spawn(|| {
                let _ = apps::sync_all();
            });
        }
    }

    /// Keyboard > Spelling and languages > Download more languages: the installed language packages (Remove) and the others
    /// (Download), like a phone's keyboard languages.
    pub(crate) fn page_spell_download(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        if self.spell.conf.is_none() {
            self.spell.reload();
        }
        let busy = self.spell.busy.clone();
        let mut job: Option<(&'static str, bool)> = None;
        for (title, installed) in [("Installed", true), ("Available to download", false)] {
            let rows: Vec<_> = self.spell.packages.iter().filter(|p| p.2 == installed).collect();
            if rows.is_empty() {
                continue;
            }
            ui::group(ui, pal, Some(title), |ui| {
                for (i, (pkg, name, _)) in rows.into_iter().enumerate() {
                    ui::row(ui, pal, i == 0, name, None, |ui| {
                        if busy.as_deref() == Some(*pkg) {
                            ui.spinner();
                        } else if ui.add_enabled(busy.is_none(), egui::Button::new(if installed { "Remove" } else { "Download" })).clicked() {
                            job = Some((pkg, !installed));
                        }
                    });
                }
            });
            ui.add_space(8.0);
        }
        ui.label(RichText::new("Each language is a signed package from the system's repositories; downloading or removing one asks for your password. A downloaded language is switched on in Spelling and languages.").color(pal.dim));
        if let Some((pkg, install)) = job {
            self.spell.busy = Some(pkg.to_string());
            self.spell.note.clear();
            package_job(self.tx.clone(), ctx.clone(), pkg, install);
        }
        if !self.spell.note.is_empty() {
            ui.label(RichText::new(&self.spell.note).color(pal.dim));
        }
    }
}
