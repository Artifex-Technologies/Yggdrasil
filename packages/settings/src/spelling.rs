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
    filter: String,
    new_word: String,
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
    }
}

/// Runs the package helper (`pkexec /usr/lib/agora/pkg-root`, the desktop's password box) to install or remove one dictionary
/// package, then tells the window.
fn package_job(tx: std::sync::mpsc::Sender<Msg>, ctx: egui::Context, package: &'static str, install: bool) {
    std::thread::spawn(move || {
        let run = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| "/tmp".into()).join("agora");
        let _ = std::fs::create_dir_all(&run);
        let cancel = run.join(format!("spell-{}.cancel", std::process::id()));
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

        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Check spelling", Some("Mark misspelled words in apps that check spelling (GTK apps, browsers, Electron apps)"), |ui| {
                if ui::switch(ui, pal, &mut conf.enabled).changed() {
                    conf_changed = true;
                    resync = true;
                }
            });
        });

        ui.add_space(8.0);
        ui::group(ui, pal, Some("Languages"), |ui| {
            if self.spell.installed.is_empty() {
                ui::row(ui, pal, true, "No language is installed", Some("Download one below"), |_| {});
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
        });
        ui.label(RichText::new("Several languages can be on at once: a word that is right in any of them is not marked.").color(pal.dim));

        ui.add_space(8.0);
        let mut add: Option<String> = None;
        let mut remove: Option<String> = None;
        let words = self.spell.words.clone().unwrap_or_default();
        ui::group(ui, pal, Some(&format!("Your words ({})", words.len())), |ui| {
            ui::row(ui, pal, true, "Add a word", Some("Known to every app from then on"), |ui| {
                let enter = ui.add(egui::TextEdit::singleline(&mut self.spell.new_word).hint_text("Word").desired_width(180.0)).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui::action(ui, pal, "Add").clicked() || enter) && !self.spell.new_word.trim().is_empty() {
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

        ui.add_space(8.0);
        let busy = self.spell.busy.clone();
        let mut job: Option<(&'static str, bool)> = None;
        ui::group(ui, pal, Some("Download or remove languages"), |ui| {
            for (i, (pkg, name, installed)) in self.spell.packages.iter().enumerate() {
                ui::row(ui, pal, i == 0, name, None, |ui| {
                    if busy.as_deref() == Some(*pkg) {
                        ui.spinner();
                    } else if *installed {
                        if ui.add_enabled(busy.is_none(), egui::Button::new("Remove")).clicked() {
                            job = Some((pkg, false));
                        }
                    } else if ui.add_enabled(busy.is_none(), egui::Button::new("Download")).clicked() {
                        job = Some((pkg, true));
                    }
                });
            }
        });
        ui.label(RichText::new("Each language is a signed package from the system's repositories; downloading or removing one asks for your password.").color(pal.dim));
        if let Some((pkg, install)) = job {
            self.spell.busy = Some(pkg.to_string());
            self.spell.note.clear();
            package_job(self.tx.clone(), ctx.clone(), pkg, install);
        }
        if !self.spell.note.is_empty() {
            ui.label(RichText::new(&self.spell.note).color(pal.dim));
        }

        if conf_changed {
            let _ = conf.save();
            self.spell.conf = Some(conf);
        }
        if resync {
            std::thread::spawn(|| {
                let _ = apps::sync_all();
            });
        }
    }
}
