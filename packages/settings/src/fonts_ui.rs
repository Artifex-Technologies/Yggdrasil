//! Settings > Appearance > Font: the system font for text and for code (yggdrasil_conf::fonts), each choice with a line of
//! text in it, the text size (the same setting as Accessibility's), and the login screen's font.
//!
//! Your own choice is your fontconfig file, written here at once (no password): Yggdrasil's apps change within a second, the
//! panel at once (it is told through appearance.conf), other programs as they start. The login screen reads the computer's
//! file, written through pkexec by /usr/lib/yggdrasil/fontconf.

use eframe::egui::{self, RichText};
use yggdrasil_conf::fonts::{self, Choice, Prefs};
use yggdrasil_ui::fonts::{has_family, preview_name, Found};
use yggdrasil_ui::theme::Palette;

use crate::{ui, App};

pub const SANS_SAMPLE: &str = "Reading is easier when Il1 and O0 look different.";
pub const MONO_SAMPLE: &str = "let total = items.len() + 10; // Il1 O0";

/// What the two files say, and what is in effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Now {
    pub mine: Prefs,
    pub computer: Prefs,
}

impl Now {
    pub fn read() -> Now {
        Now { mine: fonts::load_from(&fonts::user_path()), computer: fonts::load_from(std::path::Path::new(fonts::SYSTEM_PATH)) }
    }

    pub fn effective(&self) -> Prefs {
        self.mine.over(self.computer)
    }
}

/// Your file after choosing `id` (a text font when `mono` is false): the other family as it was.
pub fn choose(mine: Prefs, id: &'static str, mono: bool) -> Prefs {
    if mono {
        Prefs { mono: Some(id), ..mine }
    } else {
        Prefs { sans: Some(id), ..mine }
    }
}

/// The helper's arguments for the login screen to match `eff`: a default is "-" (no file is needed for it).
pub fn login_args(eff: &Prefs) -> [&'static str; 2] {
    let s = eff.sans_choice();
    let m = eff.mono_choice();
    [if *s == fonts::SANS[0] { "-" } else { s.id }, if *m == fonts::MONO[0] { "-" } else { m.id }]
}

/// Whether the login screen already shows `eff`.
pub fn login_matches(computer: &Prefs, eff: &Prefs) -> bool {
    computer.sans_choice() == eff.sans_choice() && computer.mono_choice() == eff.mono_choice()
}

/// What was clicked on the page.
enum Click {
    Use(&'static str, bool),
    Login,
}

fn list(ui: &mut egui::Ui, pal: &Palette, found: &[Found], choices: &[Choice], current: &Choice, mono: bool) -> Option<Click> {
    let mut click = None;
    for (i, c) in choices.iter().enumerate() {
        let here = found.iter().find(|f| f.choice == c).is_some_and(|f| f.file.is_some());
        let note = if here { c.note.to_string() } else { format!("{} Not installed.", c.note) };
        ui::row_with(
            ui,
            pal,
            i == 0,
            |ui| {
                ui.label(RichText::new(c.name).size(14.0));
                let family = preview_name(c.id);
                if here && has_family(ui.ctx(), &family) {
                    let sample = if mono { MONO_SAMPLE } else { SANS_SAMPLE };
                    ui.add(egui::Label::new(RichText::new(sample).size(17.0).family(egui::FontFamily::Name(family.into()))).wrap());
                }
            },
            Some(&note),
            |ui| {
                if c == current {
                    ui.label(RichText::new("In use").color(pal.accent));
                } else if here && ui::action(ui, pal, "Use").clicked() {
                    click = Some(Click::Use(c.id, mono));
                }
            },
        );
    }
    click
}

impl App {
    pub(crate) fn font_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        // which fonts are here, and a family for each one's line of text: once per visit (one fc-match each, side by side)
        if self.font_found.is_none() {
            let found = yggdrasil_ui::fonts::find_choices();
            yggdrasil_ui::fonts::show_previews(ctx, &found);
            self.font_found = Some(found);
        }
        let found = self.font_found.clone().unwrap_or_default();
        let now = Now::read();
        let eff = now.effective();
        let mut click = None;
        ui::group(ui, pal, Some("Text"), |ui| {
            click = list(ui, pal, &found, fonts::SANS, eff.sans_choice(), false).or(click.take());
        });
        ui::group(ui, pal, Some("Code and terminals"), |ui| {
            click = list(ui, pal, &found, fonts::MONO, eff.mono_choice(), true).or(click.take());
        });
        let mut conf = yggdrasil_conf::PanelConf::load();
        ui::group(ui, pal, Some("Size"), |ui| {
            let sizes = ["100%", "125%", "150%", "200%"];
            let cur = match conf.text_scale {
                0..=112 => 0,
                113..=137 => 1,
                138..=174 => 2,
                _ => 3,
            };
            ui::row(
                ui,
                pal,
                true,
                "Text and control size",
                Some("Yggdrasil's own apps. The same setting as Accessibility > Text and control size"),
                |ui| {
                    if let Some(i) = ui::segmented(ui, pal, &sizes, cur) {
                        conf.text_scale = [100, 125, 150, 200][i];
                        let _ = conf.save();
                    }
                },
            );
        });
        let same = login_matches(&now.computer, &eff);
        ui::group(ui, pal, Some("Login screen"), |ui| {
            let shows = format!(
                "It shows {} and {}.{}",
                now.computer.sans_choice().name,
                now.computer.mono_choice().name,
                if same { "" } else { " Use asks for an administrator's password" }
            );
            ui::row(ui, pal, true, "Use these fonts on the login screen", Some(&shows), |ui| {
                if same {
                    ui.label(RichText::new("In use").color(pal.accent));
                } else if ui::action(ui, pal, "Use").clicked() {
                    click = Some(Click::Login);
                }
            });
        });
        ui.label(
            RichText::new("Yggdrasil's apps and the top bar change at once; other programs when they start again.")
                .size(12.5)
                .color(pal.dim),
        );
        match click {
            Some(Click::Use(id, mono)) => {
                let mine = choose(now.mine, id, mono);
                let after = mine.over(now.computer);
                // a tiny file, written at once; the note in appearance.conf tells the panel
                self.status = match fonts::save_user(&mine)
                    .and_then(|_| yggdrasil_conf::appearance::note_fonts(after.sans_choice().id, after.mono_choice().id))
                {
                    Ok(()) => String::new(),
                    Err(e) => format!("The font could not be saved: {e}"),
                };
                ctx.request_repaint();
            }
            Some(Click::Login) => {
                let [s, m] = login_args(&eff);
                self.change(ctx, move || crate::sys::act("pkexec", &["/usr/lib/yggdrasil/fontconf", "set", s, m]));
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choosing_keeps_the_other_family() {
        let mine = Prefs { sans: None, mono: Some("opendyslexic-mono") };
        assert_eq!(choose(mine, "lexend", false), Prefs { sans: Some("lexend"), mono: Some("opendyslexic-mono") });
        assert_eq!(choose(mine, "noto-sans-mono", true), Prefs { sans: None, mono: Some("noto-sans-mono") });
    }

    #[test]
    fn the_login_screen_gets_what_is_in_effect() {
        let now = Now { mine: Prefs { sans: Some("atkinson-hyperlegible"), mono: None }, computer: Prefs::default() };
        let eff = now.effective();
        assert_eq!(login_args(&eff), ["atkinson-hyperlegible", "-"]);
        assert!(!login_matches(&now.computer, &eff));
        // your own Noto Sans over the computer's OpenDyslexic: the login screen goes back to the default (no file)
        let now = Now { mine: Prefs { sans: Some("noto-sans"), mono: None }, computer: Prefs { sans: Some("opendyslexic"), mono: None } };
        assert_eq!(login_args(&now.effective()), ["-", "-"]);
        assert!(!login_matches(&now.computer, &now.effective()));
        // the computer's choice with nothing of your own: already the same
        let now = Now { mine: Prefs::default(), computer: Prefs { sans: Some("lexend"), mono: Some("atkinson-hyperlegible-mono") } };
        assert!(login_matches(&now.computer, &now.effective()));
        assert_eq!(login_args(&now.effective()), ["lexend", "atkinson-hyperlegible-mono"]);
    }

    #[test]
    fn the_helper_takes_what_the_page_sends() {
        for s in fonts::SANS {
            for m in fonts::MONO {
                let [a, b] = login_args(&Prefs { sans: Some(s.id), mono: Some(m.id) });
                assert!(Prefs::from_ids(a, b).is_some(), "{a} {b}");
            }
        }
    }
}
