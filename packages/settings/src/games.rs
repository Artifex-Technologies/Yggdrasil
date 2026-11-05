//! Settings > Games: the graphics cards a game can draw on, and frame generation (Lossless Scaling through lsfg-vk).
//!
//! - Graphics cards: read by `yggdrasil-gpu-run list --json` (helpers/gpu-run), the same code that later starts the game, so
//!   the numbers here are the numbers it takes. With two or more cards each one shows the Steam launch option that runs a game
//!   on it (`yggdrasil-gpu-run 2 %command%`), with a Copy button.
//! - Frame generation (Labyrinth's own, labyrinth-framegen): every installed Steam game with Off / 2x / 3x / 4x, written to
//!   ~/.config/labyrinth-framegen/games.conf, which its launcher (`labyrinth-framegen %command%` in the game's Steam launch
//!   options, once) reads when the game starts.
//! - Lossless Scaling frame generation: lsfg-vk (a Vulkan layer, from Agora: its licence keeps it out of the system itself) and Lossless
//!   Scaling (Steam, paid), whose files it needs. Its own window (lsfg-vk-ui) sets the games and multipliers; this page says
//!   what is there and opens the rest.
//! - On a second card (experimental): lsfg-vk has no dual-card mode and its licence forbids a changed copy, so
//!   `yggdrasil-gpu-run N --frames-on M` runs the game on card N inside gamescope on card M, with lsfg-vk on gamescope's own
//!   output only (see helpers/gpu-run). The page suggests the game on a card without the monitor and the frames on the card
//!   the monitor is plugged into, as Lossless Scaling's dual-card advice has it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;

use crate::ui;

const GPU_RUN: &str = "/usr/bin/yggdrasil-gpu-run";
/// The per-game fixes (helpers/game-fix and data/game-fixes.json).
const GAME_FIX: &str = "/usr/lib/yggdrasil/game-fix";
/// lsfg-vk's Vulkan layer manifest: present when it is installed (package or ~/.local).
const LSFG_LAYERS: [&str; 2] = ["/usr/share/vulkan/implicit_layer.d/VkLayer_LSFGVK_frame_generation.json", ".local/share/vulkan/implicit_layer.d/VkLayer_LSFGVK_frame_generation.json"];
/// Lossless Scaling's Steam app id.
const LOSSLESS_SCALING: &str = "993090";
/// Agora's page for the lsfg-vk component (Labyrinth's catalog).
const AGORA_APP: &str = "frame-generation";
/// Labyrinth's own frame generation: its layer manifest (present when installed) and Agora page.
const FRAMEGEN_LAYER: &str = "/usr/share/vulkan/implicit_layer.d/labyrinth_framegen.x86_64.json";
const FRAMEGEN_AGORA: &str = "labyrinth-framegen";
pub const FRAMEGEN_OPTION: &str = "labyrinth-framegen %command%";
/// The choices per game, as Off, 2x, 3x, 4x.
pub const FRAMEGEN_CHOICES: [&str; 4] = ["Off", "2x", "3x", "4x"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub number: u32,
    pub name: String,
    pub driver: String,
    pub boot: bool,
    pub screens: u32,
}

/// One game's fix, as `game-fix list --json` gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fix {
    pub appid: String,
    pub name: String,
    pub title: String,
    pub about: String,
    pub tip: String,
    /// applied, not applied, start once, no screen
    pub state: String,
    pub screen: String,
}

pub fn parse_fixes(json: &str) -> Option<Vec<Fix>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let s = |x: &serde_json::Value, k: &str| x[k].as_str().unwrap_or("").to_string();
    Some(v.as_array()?.iter().map(|x| Fix { appid: s(x, "appid"), name: s(x, "name"), title: s(x, "title"), about: s(x, "about"), tip: s(x, "tip"), state: s(x, "state"), screen: s(x, "screen") }).collect())
}

/// What a fix's row says about its state.
pub fn fix_state(f: &Fix) -> &'static str {
    match f.state.as_str() {
        "applied" => "Applied",
        "start once" => "Start the game once first: Proton makes its folders then",
        "no screen" => "The screen size could not be read",
        _ => "Not applied",
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub fixes: Vec<Fix>,
    pub cards: Vec<Card>,
    /// yggdrasil-gpu-run is missing or failed (the cards could not be read)
    pub no_reader: bool,
    pub lsfg: bool,
    pub lossless: bool,
    pub steam: bool,
    pub framegen: bool,
    /// installed Steam games: (app id, name), by name
    pub games: Vec<(String, String)>,
    /// labyrinth-framegen's games.conf as it is
    pub framegen_conf: String,
}

/// ~/.config/labyrinth-framegen/games.conf
pub fn framegen_conf_path() -> PathBuf {
    // the launcher's own folder (XDG_CONFIG_HOME, else ~/.config), not Yggdrasil's
    let base = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".config"));
    base.join("labyrinth-framegen/games.conf")
}

/// A game's choice in games.conf: 0 (off or not listed), 2, 3 or 4.
pub fn framegen_choice(conf: &str, id: &str) -> u32 {
    let mut v = 0;
    for l in conf.lines() {
        let mut w = l.split_whitespace();
        if w.next() == Some(id) {
            v = match w.next() {
                Some("2") => 2,
                Some("3") => 3,
                Some("4") => 4,
                _ => 0,
            };
        }
    }
    v
}

/// games.conf with this game's choice set (0 = off); every other line is kept as it is.
pub fn framegen_set(conf: &str, id: &str, choice: u32) -> String {
    let value = if (2..=4).contains(&choice) { choice.to_string() } else { "off".to_string() };
    let mut out: Vec<String> = vec![];
    let mut done = false;
    for l in conf.lines() {
        if l.split_whitespace().next() == Some(id) {
            if !done {
                out.push(format!("{id} {value}"));
                done = true;
            }
        } else {
            out.push(l.to_string());
        }
    }
    if !done {
        if out.is_empty() {
            out.push("# Frame generation per Steam game: <app id> <off|2|3|4>, written by Settings > Games".into());
        }
        out.push(format!("{id} {value}"));
    }
    out.join("\n") + "\n"
}

/// An appmanifest's id and name.
pub fn manifest_game(acf: &str) -> Option<(String, String)> {
    let field = |key: &str| {
        acf.lines().find_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix(&format!("\"{key}\""))?;
            Some(rest.trim().trim_matches('"').to_string())
        })
    };
    let (id, name) = (field("appid")?, field("name")?);
    // Steam's own tools are not games
    let tool = ["Proton", "Steam Linux Runtime", "Steamworks Common", "SteamVR"].iter().any(|t| name.starts_with(t)) || name.contains("Redistributable");
    (!tool && !id.is_empty() && !name.is_empty()).then_some((id, name))
}

/// The installed Steam games, by name.
pub fn steam_games(home: &Path) -> Vec<(String, String)> {
    let mut games: Vec<(String, String)> = vec![];
    for root in steam_roots(home) {
        let vdf = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
        for lib in libraries(&root, &vdf) {
            let Ok(rd) = std::fs::read_dir(lib.join("steamapps")) else { continue };
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("appmanifest_") && n.ends_with(".acf") {
                    if let Some(g) = std::fs::read_to_string(e.path()).ok().as_deref().and_then(manifest_game) {
                        if !games.iter().any(|x| x.0 == g.0) {
                            games.push(g);
                        }
                    }
                }
            }
        }
    }
    games.sort_by_key(|g| g.1.to_lowercase());
    games
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

/// For two or more cards: (the card the game draws on, the card that makes the extra frames). The frames go on the card the
/// monitor is plugged into (else the one the computer started on), the game on the first other card.
pub fn dual_pick(cards: &[Card]) -> Option<(u32, u32)> {
    if cards.len() < 2 {
        return None;
    }
    let frames = cards.iter().find(|c| c.screens > 0).or_else(|| cards.iter().find(|c| c.boot)).unwrap_or(&cards[0]);
    let game = cards.iter().find(|c| c.number != frames.number)?;
    Some((game.number, frames.number))
}

pub fn dual_option(game: u32, frames: u32) -> String {
    format!("yggdrasil-gpu-run {game} --frames-on {frames} %command%")
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
        fixes: std::process::Command::new(GAME_FIX).args(["list", "--json"]).output().ok().filter(|o| o.status.success()).and_then(|o| parse_fixes(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default(),
        framegen: Path::new(FRAMEGEN_LAYER).is_file(),
        games: steam_games(&home),
        framegen_conf: std::fs::read_to_string(framegen_conf_path()).unwrap_or_default(),
    }
}

#[derive(Default)]
pub struct State {
    info: Option<Info>,
    fix_message: Option<String>,
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

        if !info.fixes.is_empty() {
            let mut run: Option<(&'static str, String)> = None;
            ui::group(ui, pal, Some("Game fixes"), |ui| {
                for (i, f) in info.fixes.iter().enumerate() {
                    let sub = format!("{}. {}\n{}{}{}", f.title, fix_state(f), f.about, if f.tip.is_empty() { "" } else { "\n" }, f.tip);
                    ui::row(ui, pal, i == 0, &f.name, Some(&sub), |ui| {
                        match f.state.as_str() {
                            "applied" => {
                                if ui::quiet(ui, pal, "Undo").on_hover_text("Put the game's files back as they were").clicked() {
                                    run = Some(("undo", f.appid.clone()));
                                }
                            }
                            "not applied" => {
                                let tip = if f.screen.is_empty() { String::new() } else { format!("For your {} screen; the game's files are backed up first", f.screen.replace('x', "\u{d7}")) };
                                if ui::action(ui, pal, "Apply").on_hover_text(tip).clicked() {
                                    run = Some(("apply", f.appid.clone()));
                                }
                            }
                            _ => {}
                        }
                    });
                }
            });
            if let Some((what, id)) = run {
                let out = std::process::Command::new(GAME_FIX).args([what, &id]).output();
                self.fix_message = match out {
                    Ok(o) if o.status.success() => None,
                    Ok(o) => Some(String::from_utf8_lossy(&o.stderr).trim().trim_start_matches("yggdrasil-game-fix: ").to_string()),
                    Err(e) => Some(e.to_string()),
                };
                self.reload();
            }
            if let Some(m) = &self.fix_message {
                ui.colored_label(egui::Color32::from_rgb(200, 90, 60), m);
            }
            ui.add_space(4.0);
            ui.label(RichText::new("Fixes are made per game. A game's own menu may undo one if you change the same setting there.").color(pal.dim).size(12.5));
        }

        ui::group(ui, pal, Some("Frame generation (preview)"), |ui| {
            if !info.framegen {
                ui::row(ui, pal, true, "Frame generation", Some("Labyrinth's own: more frames on the screen than the game draws, for Vulkan and Proton games. From Agora."), |ui| {
                    if ui::action(ui, pal, "Get it in Agora").clicked() {
                        let _ = std::process::Command::new("agora").env("AGORA_PAGE", format!("detail:{FRAMEGEN_AGORA}")).spawn();
                        self.info = None;
                    }
                });
                return;
            }
            let label = if self.copied == Some(u32::MAX) { "Copied" } else { "Copy launch option" };
            ui::row(ui, pal, true, "In Steam, once per game", Some("Properties > Launch options: labyrinth-framegen %command%. Then choose below; no further Steam changes."), |ui| {
                if ui::action(ui, pal, label).on_hover_text(FRAMEGEN_OPTION).clicked() {
                    ui.ctx().copy_text(FRAMEGEN_OPTION.to_string());
                    self.copied = Some(u32::MAX);
                }
            });
            if info.games.is_empty() {
                ui::row(ui, pal, false, "No Steam games found", None, |_| {});
            }
            for (id, name) in &info.games {
                let now = framegen_choice(&info.framegen_conf, id);
                let selected = if now >= 2 { (now - 1) as usize } else { 0 };
                let mut pick = None;
                ui::row(ui, pal, false, name, None, |ui| {
                    pick = ui::segmented(ui, pal, &FRAMEGEN_CHOICES, selected);
                });
                if let Some(i) = pick {
                    let choice = if i == 0 { 0 } else { i as u32 + 1 };
                    let text = framegen_set(&info.framegen_conf, id, choice);
                    let path = framegen_conf_path();
                    let saved = path.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&path, &text));
                    if saved.is_ok() {
                        if let Some(i) = self.info.as_mut() {
                            i.framegen_conf = text;
                        }
                    }
                }
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("The game is held to your refresh rate divided by the multiplier (at 180 Hz and 3x: the game at 60, the screen at 180). It adds about one game frame of delay. A choice takes effect the next time the game starts.").color(pal.dim).size(12.5));

        ui::group(ui, pal, Some("Lossless Scaling frame generation (lsfg-vk)"), |ui| {
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
            match dual_pick(&info.cards) {
                Some((game, frames)) => {
                    let sub = format!("Experimental: the game draws on card {game}, the extra frames are made on card {frames} (the one your monitor is on), through gamescope. Not yet tried on two real cards. To swap them, swap the two numbers.");
                    ui::row(ui, pal, false, "On a second graphics card", Some(&sub), |ui| {
                        let ready = info.lsfg && info.lossless;
                        let label = if self.copied == Some(0) { "Copied" } else { "Copy launch option" };
                        let r = ui.add_enabled_ui(ready, |ui| ui::action(ui, pal, label)).inner;
                        let r = r.on_hover_text(if ready { dual_option(game, frames) } else { "Needs lsfg-vk and Lossless Scaling first".to_string() });
                        if r.clicked() {
                            ui.ctx().copy_text(dual_option(game, frames));
                            self.copied = Some(0);
                        }
                    });
                }
                None => ui::row(ui, pal, false, "On a second graphics card", Some("Lossless Scaling's dual-card mode (the game on one card, the extra frames on another): with a second card installed, a launch option for it shows here (experimental)."), |_| {}),
            }
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
    fn game_fixes_are_read() {
        let j = r#"[{"appid":"47780","name":"Dead Space 2","title":"Full screen","about":"a","tip":"","state":"applied","screen":"3440x1440"},
                    {"appid":"377160","name":"Fallout 4","title":"Size","about":"b","tip":"t","state":"start once","screen":""}]"#;
        let f = parse_fixes(j).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(fix_state(&f[0]), "Applied");
        assert!(fix_state(&f[1]).starts_with("Start the game once"));
        assert!(parse_fixes("[").is_none());
    }

    #[test]
    fn frame_generation_choices_are_read_and_written() {
        let conf = "# mine\n47780 3\ndefault 2\n";
        assert_eq!(framegen_choice(conf, "47780"), 3);
        assert_eq!(framegen_choice(conf, "1332010"), 0, "not listed: off here (the default line is the launcher's business)");
        let c = framegen_set(conf, "47780", 0);
        assert_eq!(c, "# mine\n47780 off\ndefault 2\n", "changed in place, the rest kept");
        let c = framegen_set(&c, "1332010", 4);
        assert!(c.ends_with("1332010 4\n"));
        assert_eq!(framegen_choice(&c, "1332010"), 4);
        assert!(framegen_set("", "1", 2).starts_with("# Frame generation per Steam game"), "a new file says what it is");
    }

    #[test]
    fn steam_manifests_name_games_and_skip_tools() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"47780\"\n\t\"name\"\t\t\"Dead Space 2\"\n}\n";
        assert_eq!(manifest_game(acf), Some(("47780".into(), "Dead Space 2".into())));
        let tool = "\"AppState\"\n{\n\t\"appid\"\t\t\"1493710\"\n\t\"name\"\t\t\"Proton Experimental\"\n}\n";
        assert_eq!(manifest_game(tool), None);
    }

    #[test]
    fn the_frames_go_on_the_card_the_monitor_is_on() {
        let c = |number, screens, boot| Card { number, name: String::new(), driver: String::new(), boot, screens };
        assert_eq!(dual_pick(&[c(1, 0, true)]), None, "one card");
        assert_eq!(dual_pick(&[c(1, 1, true), c(2, 0, false)]), Some((2, 1)));
        assert_eq!(dual_pick(&[c(1, 0, true), c(2, 1, false)]), Some((1, 2)), "the monitor on the second card");
        assert_eq!(dual_pick(&[c(1, 0, false), c(2, 0, true)]), Some((1, 2)), "no screen known: the card it started on");
        assert_eq!(dual_option(1, 2), "yggdrasil-gpu-run 1 --frames-on 2 %command%");
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
