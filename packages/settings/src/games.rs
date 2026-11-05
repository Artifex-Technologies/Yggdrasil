//! Settings > Games: the graphics cards a game can draw on, and frame generation (Lossless Scaling through lsfg-vk).
//!
//! - Graphics cards: read by `yggdrasil-gpu-run list --json` (helpers/gpu-run), the same code that later starts the game, so
//!   the numbers here are the numbers it takes. With two or more cards each one shows the Steam launch option that runs a game
//!   on it (`yggdrasil-gpu-run 2 %command%`), with a Copy button.
//! - Frame generation: lsfg-vk (a Vulkan layer, from Agora: its licence keeps it out of the system itself) and Lossless
//!   Scaling (Steam, paid), whose files it needs. Its own window (lsfg-vk-ui) sets the games and multipliers; this page says
//!   what is there and opens the rest. Frame generation on a second card waits on lsfg-vk (not supported by it yet).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;

use crate::ui;

const GPU_RUN: &str = "/usr/bin/yggdrasil-gpu-run";
/// lsfg-vk's Vulkan layer manifest: present when it is installed (package or ~/.local).
const LSFG_LAYERS: [&str; 2] = ["/usr/share/vulkan/implicit_layer.d/VkLayer_LSFGVK_frame_generation.json", ".local/share/vulkan/implicit_layer.d/VkLayer_LSFGVK_frame_generation.json"];
/// Lossless Scaling's Steam app id.
const LOSSLESS_SCALING: &str = "993090";
/// Agora's page for the frame generation component (Labyrinth's catalog).
const AGORA_APP: &str = "frame-generation";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub number: u32,
    pub name: String,
    pub driver: String,
    pub boot: bool,
    pub screens: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub cards: Vec<Card>,
    /// yggdrasil-gpu-run is missing or failed (the cards could not be read)
    pub no_reader: bool,
    pub lsfg: bool,
    pub lossless: bool,
    pub steam: bool,
}

/// `yggdrasil-gpu-run list --json`.
pub fn parse_cards(json: &str) -> Option<Vec<Card>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(v.as_array()?.iter().map(|c| Card {
        number: c["number"].as_u64().unwrap_or(0) as u32,
        name: c["name"].as_str().unwrap_or("").to_string(),
        driver: c["driver"].as_str().unwrap_or("").to_string(),
        boot: c["boot"].as_bool().unwrap_or(false),
        screens: c["screens"].as_u64().unwrap_or(0) as u32,
    }).collect())
}

/// What a card's row says under its name.
pub fn describe(c: &Card) -> String {
    let mut parts = vec![];
    match c.screens {
        0 => {}
        1 => parts.push("shows your screen".to_string()),
        n => parts.push(format!("shows {n} screens")),
    }
    if c.boot {
        parts.push("the computer starts on it".into());
    }
    parts.push(if c.driver.is_empty() { "no driver loaded".into() } else { format!("driver: {}", c.driver) });
    let s = parts.join(" · ");
    let mut it = s.chars();
    it.next().map(|f| f.to_uppercase().collect::<String>() + it.as_str()).unwrap_or_default()
}

/// The Steam launch option that runs a game on card `n`.
pub fn launch_option(n: u32) -> String {
    format!("yggdrasil-gpu-run {n} %command%")
}

/// Steam's library folders: its own folder and every `"path"` in libraryfolders.vdf.
pub fn libraries(steam_root: &Path, vdf: &str) -> Vec<PathBuf> {
    let mut out = vec![steam_root.to_path_buf()];
    for line in vdf.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("\"path\"") {
            let p = rest.trim().trim_matches('"').replace("\\\\", "\\");
            if !p.is_empty() && !out.iter().any(|x| x == Path::new(&p)) {
                out.push(PathBuf::from(p));
            }
        }
    }
    out
}

fn steam_roots(home: &Path) -> Vec<PathBuf> {
    [".local/share/Steam", ".steam/steam"].iter().map(|r| home.join(r)).filter(|p| p.join("steamapps").is_dir()).collect()
}

/// Whether Steam has Lossless Scaling installed in any of its libraries.
pub fn lossless_installed(home: &Path) -> bool {
    steam_roots(home).iter().any(|root| {
        let vdf = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
        libraries(root, &vdf).iter().any(|lib| lib.join(format!("steamapps/appmanifest_{LOSSLESS_SCALING}.acf")).is_file())
    })
}

pub fn load() -> Info {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let listed = std::process::Command::new(GPU_RUN).args(["list", "--json"]).output().ok().filter(|o| o.status.success());
    let cards = listed.as_ref().and_then(|o| parse_cards(&String::from_utf8_lossy(&o.stdout)));
    Info {
        no_reader: cards.is_none(),
        cards: cards.unwrap_or_default(),
        lsfg: Path::new(LSFG_LAYERS[0]).is_file() || home.join(LSFG_LAYERS[1]).is_file(),
        lossless: lossless_installed(&home),
        steam: !steam_roots(&home).is_empty(),
    }
}

#[derive(Default)]
pub struct State {
    info: Option<Info>,
    rx: Option<Receiver<Info>>,
    copied: Option<u32>,
}

impl State {
    /// Read again the next time the page is shown (after an install from Agora, say).
    pub fn reload(&mut self) {
        self.info = None;
        self.rx = None;
    }

    pub fn page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        if let Some(rx) = &self.rx {
            if let Ok(i) = rx.try_recv() {
                self.info = Some(i);
                self.rx = None;
            }
        }
        if self.info.is_none() && self.rx.is_none() {
            let (tx, rx) = channel();
            let c = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(load());
                c.request_repaint();
            });
            self.rx = Some(rx);
        }
        let Some(info) = self.info.clone() else {
            ui.add_space(12.0);
            ui.label(RichText::new("Looking at the graphics cards…").color(pal.dim));
            return;
        };

        ui::group(ui, pal, Some("Graphics cards"), |ui| {
            if info.no_reader {
                ui::row(ui, pal, true, "The cards could not be read", Some("yggdrasil-gpu-run is missing; reinstall Settings."), |_| {});
                return;
            }
            if info.cards.is_empty() {
                ui::row(ui, pal, true, "No graphics card found", None, |_| {});
            }
            let many = info.cards.len() > 1;
            for (i, c) in info.cards.iter().enumerate() {
                let title = if many { format!("{}. {}", c.number, c.name) } else { c.name.clone() };
                ui::row(ui, pal, i == 0, &title, Some(&describe(c)), |ui| {
                    if many {
                        let label = if self.copied == Some(c.number) { "Copied" } else { "Copy launch option" };
                        if ui::action(ui, pal, label).on_hover_text(launch_option(c.number)).clicked() {
                            ui.ctx().copy_text(launch_option(c.number));
                            self.copied = Some(c.number);
                        }
                    }
                });
            }
        });
        if info.cards.len() > 1 {
            ui.add_space(6.0);
            ui.label(RichText::new("To run a game on a card: in Steam, the game's Properties > Launch options, paste its launch option (for example yggdrasil-gpu-run 2 %command%). The picture reaches your screen through the card the monitor is plugged into.").color(pal.dim).size(12.5));
        }

        ui::group(ui, pal, Some("Frame generation (Lossless Scaling)"), |ui| {
            ui::row(ui, pal, true, "lsfg-vk", Some(if info.lsfg { "Installed: Lossless Scaling's frame generation for Linux games" } else { "Brings Lossless Scaling's frame generation to Linux games. From Agora." }), |ui| {
                if info.lsfg {
                    if ui::action(ui, pal, "Choose games").on_hover_text("lsfg-vk's own settings: which games, and how many frames").clicked() {
                        crate::sys::spawn("lsfg-vk-ui", &[]);
                    }
                } else if ui::action(ui, pal, "Get it in Agora").clicked() {
                    let _ = std::process::Command::new("agora").env("AGORA_PAGE", format!("detail:{AGORA_APP}")).spawn();
                    self.info = None;
                }
            });
            let sub = if info.lossless {
                "Installed in Steam: lsfg-vk uses its files"
            } else if info.steam {
                "Not installed: lsfg-vk needs it (a paid Windows app on Steam; install it there, no need to run it)"
            } else {
                "Needs Steam: lsfg-vk uses this paid Windows app's files"
            };
            ui::row(ui, pal, false, "Lossless Scaling", Some(sub), |ui| {
                if !info.lossless && info.steam && ui::action(ui, pal, "Open in Steam").clicked() {
                    crate::sys::spawn("steam", &[&format!("steam://store/{LOSSLESS_SCALING}")]);
                }
            });
            ui::row(ui, pal, false, "On a second graphics card", Some("Lossless Scaling's dual-card mode (the game on one card, the extra frames on another) is not in lsfg-vk yet. When it is, it is set here."), |_| {});
        });
        ui.add_space(6.0);
        if ui::quiet(ui, pal, "Look again").clicked() {
            self.reload();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cards_are_read() {
        let j = r#"[{"number":1,"name":"GeForce RTX 2070","driver":"nvidia","boot":true,"screens":1,"slot":"0000:01:00.0"},
                    {"number":2,"name":"Arc B580","driver":"xe","boot":false,"screens":0}]"#;
        let c = parse_cards(j).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(describe(&c[0]), "Shows your screen · the computer starts on it · driver: nvidia");
        assert_eq!(describe(&c[1]), "Driver: xe");
        assert!(parse_cards("not json").is_none());
        assert_eq!(launch_option(2), "yggdrasil-gpu-run 2 %command%");
    }

    #[test]
    fn steam_libraries_are_found_and_lossless_scaling_in_any_of_them() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/me/.local/share/Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/mnt/games/SteamLibrary\"\n\t}\n}\n";
        let libs = libraries(Path::new("/home/me/.local/share/Steam"), vdf);
        assert_eq!(libs, vec![PathBuf::from("/home/me/.local/share/Steam"), PathBuf::from("/mnt/games/SteamLibrary")]);

        let home = std::env::temp_dir().join(format!("yggdrasil-games-{}", std::process::id()));
        let root = home.join(".local/share/Steam");
        let other = home.join("Games");
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        std::fs::create_dir_all(other.join("steamapps")).unwrap();
        std::fs::write(root.join("steamapps/libraryfolders.vdf"), format!("\"libraryfolders\"\n{{\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n", other.display())).unwrap();
        assert!(!lossless_installed(&home));
        std::fs::write(other.join("steamapps/appmanifest_993090.acf"), "\"AppState\" {}").unwrap();
        assert!(lossless_installed(&home), "found in the second library");
        std::fs::remove_dir_all(&home).unwrap();
    }
}
