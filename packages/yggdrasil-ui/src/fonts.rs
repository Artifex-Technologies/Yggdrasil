//! The system font in Yggdrasil's egui apps: the files fontconfig names for `sans-serif` (regular and bold) and `monospace`,
//! the same that the panel, the lock screen and every other program use, so a font chosen in Settings > Appearance > Font
//! (yggdrasil_conf::fonts) shows here too. egui's own fonts stay behind them for whatever a system font lacks (emoji, symbols).
//!
//! - `install(ctx)` at the app's start (in eframe's creation callback, so the first frame already has them);
//! - `theme::apply_accessibility` calls `refresh` once a second: when the font settings' files changed, the fonts are loaded
//!   anew, so a new choice shows in open windows within a second;
//! - `FontFamily::Proportional` is the regular face, `FontFamily::Monospace` the monospace one, and `bold()` a bold family that
//!   always exists: the regular face until an app asks for the real bold one with `want_bold` (no app does yet; loading a bold
//!   face it never draws would cost every app about 1.4 MB);
//! - an app with families of its own (Saga MD's page, Settings' previews) adds them with `set_extra`, kept through every reload.
//!
//! A variable font (one file for every weight, like Atkinson Hyperlegible Next or Lexend) is set to weight 400, and 700 for
//! bold: its file's default weight need not be the regular one (Atkinson Hyperlegible Mono's is ExtraLight).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontTweak};
use eframe::epaint::text::VariationCoords;
use yggdrasil_conf::fonts::Choice;

/// The bold family (see `want_bold`).
pub fn bold() -> FontFamily {
    FontFamily::Name("yggdrasil-bold".into())
}

fn file_ok(p: PathBuf) -> Option<PathBuf> {
    let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    (p.is_file() && matches!(ext.as_str(), "ttf" | "otf")).then_some(p)
}

/// Several patterns at once (one fc-match each, side by side: starting them one after the other would add their times).
pub fn fc_files(patterns: &[&str]) -> Vec<Option<PathBuf>> {
    let children: Vec<_> = patterns
        .iter()
        .map(|p| {
            std::process::Command::new("fc-match")
                .args(["-f", "%{file}", p])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()
        })
        .collect();
    children
        .into_iter()
        .map(|c| {
            let out = c?.wait_with_output().ok()?;
            file_ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
        })
        .collect()
}

/// The system's three faces: regular, bold and monospace (None where fontconfig names nothing egui can read).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Faces {
    pub regular: Option<PathBuf>,
    pub bold: Option<PathBuf>,
    pub mono: Option<PathBuf>,
}

impl Faces {
    /// Through `resolve` (a pattern to a file; `fc_files` outside tests). `families` names the sans and monospace families to
    /// ask for instead of the system's (the installer's preview of a font before it is chosen).
    /// `bold`: look for the bold face too (`want_bold`).
    pub fn find(families: Option<(&str, &str)>, bold: bool, resolve: impl Fn(&[&str]) -> Vec<Option<PathBuf>>) -> Faces {
        let (sans, mono) = families.unwrap_or(("sans-serif", "monospace"));
        let mut pats = vec![format!("{sans}:weight=regular"), format!("{mono}:weight=regular")];
        if bold {
            pats.push(format!("{sans}:weight=bold"));
        }
        let refs: Vec<&str> = pats.iter().map(String::as_str).collect();
        let mut got = resolve(&refs).into_iter();
        Faces { regular: got.next().flatten(), mono: got.next().flatten(), bold: got.next().flatten() }
    }
}

/// A face for egui: the file's bytes, a variable font set to `weight` (100 to 900).
pub fn face(bytes: Vec<u8>, weight: f32) -> FontData {
    let data = FontData::from_owned(bytes);
    if data.variation_axes().iter().any(|a| a.tag == *b"wght") {
        data.tweak(FontTweak { coords: VariationCoords::new([(b"wght", weight)]), ..Default::default() })
    } else {
        data
    }
}

fn read(p: &Option<PathBuf>) -> Option<Vec<u8>> {
    p.as_deref().and_then(|p| std::fs::read(p).ok())
}

/// egui's own fonts with the system's in front: regular for Proportional, monospace for Monospace, and the bold family
/// (the regular face, then egui's, when there is no bold one). A face that cannot be read leaves egui's own in place.
pub fn definitions(faces: &Faces) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let regular = read(&faces.regular);
    let bold_bytes = if faces.bold.is_some() && faces.bold == faces.regular { regular.clone() } else { read(&faces.bold) };
    let mut bold_list = prop.clone();
    if let Some(b) = regular.clone() {
        defs.font_data.insert("system-regular".into(), Arc::new(face(b, 400.0)));
        defs.families.insert(FontFamily::Proportional, [vec!["system-regular".to_string()], prop.clone()].concat());
        bold_list.insert(0, "system-regular".into());
    }
    if let Some(b) = bold_bytes {
        defs.font_data.insert("system-bold".into(), Arc::new(face(b, 700.0)));
        bold_list.insert(0, "system-bold".into());
    }
    defs.families.insert(bold(), bold_list);
    if let Some(b) = read(&faces.mono) {
        defs.font_data.insert("system-mono".into(), Arc::new(face(b, 400.0)));
        defs.families.insert(FontFamily::Monospace, [vec!["system-mono".to_string()], mono].concat());
    }
    defs
}

/// Adds a named family to `defs`: the face, then egui's Proportional fallbacks (emoji and symbols).
pub fn add_family(defs: &mut FontDefinitions, name: &str, data: FontData) {
    let fallbacks = FontDefinitions::default().families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    defs.font_data.insert(name.to_string(), Arc::new(data));
    defs.families.insert(FontFamily::Name(name.into()), [vec![name.to_string()], fallbacks].concat());
}

struct State {
    /// The font settings' files when the fonts were last loaded (None: not loaded yet).
    seen: Option<u64>,
    /// The installer's preview: these families instead of the system's.
    families: Option<(String, String)>,
    extra: Option<fn(&mut FontDefinitions)>,
    bold: bool,
}

static STATE: Mutex<State> = Mutex::new(State { seen: None, families: None, extra: None, bold: false });

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn load(ctx: &egui::Context, st: &mut State) {
    st.seen = Some(yggdrasil_conf::fonts::signature());
    let faces = Faces::find(st.families.as_ref().map(|(a, b)| (a.as_str(), b.as_str())), st.bold, fc_files);
    let mut defs = definitions(&faces);
    if let Some(f) = st.extra {
        f(&mut defs);
    }
    ctx.set_fonts(defs);
}

/// The system's fonts, now (at the app's start).
pub fn install(ctx: &egui::Context) {
    let mut st = state();
    load(ctx, &mut st);
}

/// Loads the fonts again when the font settings changed since they were loaded (or were never loaded); cheap otherwise (two
/// files' metadata). `theme::apply_accessibility` calls it once a second.
pub fn refresh(ctx: &egui::Context) {
    let mut st = state();
    if st.seen != Some(yggdrasil_conf::fonts::signature()) {
        load(ctx, &mut st);
    }
}

/// The real bold face in the `bold()` family (otherwise it is the regular one), now and at every reload.
pub fn want_bold(ctx: &egui::Context) {
    let mut st = state();
    if !st.bold {
        st.bold = true;
        load(ctx, &mut st);
    }
}

/// An app's own families, added now and at every reload (one function per app; a second call replaces the first).
pub fn set_extra(ctx: &egui::Context, f: fn(&mut FontDefinitions)) {
    let mut st = state();
    st.extra = Some(f);
    load(ctx, &mut st);
}

/// The installer: show these families (sans, monospace) instead of the system's, before anything is chosen for the system;
/// None goes back to the system's.
pub fn preview_families(ctx: &egui::Context, families: Option<(&str, &str)>) {
    let mut st = state();
    let want = families.map(|(a, b)| (a.to_string(), b.to_string()));
    if st.families != want || st.seen.is_none() {
        st.families = want;
        load(ctx, &mut st);
    }
}

/// Whether `name` is a family egui has now (a family added with `set_extra` is there from the next frame on; drawing with
/// one egui does not have stops the program).
pub fn has_family(ctx: &egui::Context, name: &str) -> bool {
    ctx.fonts(|f| f.definitions().families.contains_key(&FontFamily::Name(name.into())))
}

/// One of the fonts to choose from (yggdrasil_conf::fonts), as found on this computer: its regular file, None when it is not
/// installed (fontconfig would give another family for its name).
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub choice: &'static Choice,
    pub file: Option<PathBuf>,
}

/// fc-match's `%{family}\t%{file}` for a family asked for by name: the file when the family it found is that one (fontconfig
/// lists a variable font's family with its weight too: "Atkinson Hyperlegible Mono,Atkinson Hyperlegible Mono ExtraLight").
pub fn match_of(out: &str, family: &str) -> Option<PathBuf> {
    let (families, file) = out.trim().split_once('\t')?;
    families.split(',').any(|f| f.trim() == family).then(|| file_ok(PathBuf::from(file.trim()))).flatten()
}

/// Every font to choose from, text ones first, and whether it is here (one fc-match each, side by side).
pub fn find_choices() -> Vec<Found> {
    let all: Vec<&'static Choice> = yggdrasil_conf::fonts::SANS.iter().chain(yggdrasil_conf::fonts::MONO).collect();
    let children: Vec<_> = all
        .iter()
        .map(|c| {
            std::process::Command::new("fc-match")
                .args(["-f", "%{family}\t%{file}", &format!("{}:weight=regular", c.family)])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()
        })
        .collect();
    all.into_iter()
        .zip(children)
        .map(|(choice, child)| {
            let out = child.and_then(|c| c.wait_with_output().ok()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
            Found { choice, file: out.and_then(|o| match_of(&o, choice.family)) }
        })
        .collect()
}

/// The family a choice's preview is drawn in (`show_previews`).
pub fn preview_name(id: &str) -> String {
    format!("preview-{id}")
}

static PREVIEWS: Mutex<Vec<(String, PathBuf)>> = Mutex::new(Vec::new());

fn add_previews(defs: &mut FontDefinitions) {
    for (name, file) in PREVIEWS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if let Ok(bytes) = std::fs::read(file) {
            add_family(defs, name, face(bytes, 400.0));
        }
    }
}

/// Each installed choice as a family of its own (`preview_name`), for a line of text in it (Settings > Appearance > Font, the
/// installer's Font page). Drawn only once `has_family` says it is there.
pub fn show_previews(ctx: &egui::Context, found: &[Found]) {
    *PREVIEWS.lock().unwrap_or_else(|e| e.into_inner()) =
        found.iter().filter_map(|f| Some((preview_name(f.choice.id), f.file.clone()?))).collect();
    set_extra(ctx, add_previews);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in system fonts: egui's own files written out (Ubuntu Light as the "regular", Hack as the "monospace").
    fn stand_ins() -> (PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("ygg-ui-fonts-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let defs = FontDefinitions::default();
        let write = |name: &str, key: &str| {
            let p = dir.join(name);
            std::fs::write(&p, &defs.font_data[key].font[..]).unwrap();
            p
        };
        (write("Regular.ttf", "Ubuntu-Light"), write("Mono.ttf", "Hack"), dir)
    }

    #[test]
    fn the_patterns_asked_for() {
        let asked = std::cell::RefCell::new(vec![]);
        let f = Faces::find(None, true, |p| {
            asked.borrow_mut().extend(p.iter().map(|s| s.to_string()));
            vec![None, None, None]
        });
        assert_eq!(f, Faces::default());
        assert_eq!(*asked.borrow(), ["sans-serif:weight=regular", "monospace:weight=regular", "sans-serif:weight=bold"]);
        asked.borrow_mut().clear();
        Faces::find(Some(("Lexend", "OpenDyslexicMono")), false, |p| {
            asked.borrow_mut().extend(p.iter().map(|s| s.to_string()));
            vec![]
        });
        assert_eq!(*asked.borrow(), ["Lexend:weight=regular", "OpenDyslexicMono:weight=regular"], "no bold face unless wanted");
    }

    #[test]
    fn the_system_faces_go_in_front_of_eguis_own() {
        let (regular, mono, dir) = stand_ins();
        let d = definitions(&Faces { regular: Some(regular.clone()), bold: Some(regular.clone()), mono: Some(mono.clone()) });
        let own = FontDefinitions::default();
        let prop = &d.families[&FontFamily::Proportional];
        assert_eq!(prop[0], "system-regular");
        assert_eq!(&prop[1..], &own.families[&FontFamily::Proportional][..], "egui's fonts behind it, for emoji and symbols");
        let m = &d.families[&FontFamily::Monospace];
        assert_eq!(m[0], "system-mono");
        assert_eq!(&m[1..], &own.families[&FontFamily::Monospace][..]);
        assert_eq!(d.families[&bold()][..2], ["system-bold".to_string(), "system-regular".to_string()]);
        assert_eq!(d.font_data["system-regular"].font.len(), std::fs::metadata(&regular).unwrap().len() as usize);
        assert!(own.font_data.keys().all(|k| d.font_data.contains_key(k)), "egui's own fonts are all still there");
        // a static font gets no weight setting
        assert_eq!(d.font_data["system-regular"].tweak.coords, VariationCoords::default());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nothing_found_or_unreadable_keeps_eguis_fonts() {
        let own = FontDefinitions::default();
        let d = definitions(&Faces::default());
        assert_eq!(d.families[&FontFamily::Proportional], own.families[&FontFamily::Proportional]);
        assert_eq!(d.families[&FontFamily::Monospace], own.families[&FontFamily::Monospace]);
        assert_eq!(d.families[&bold()], own.families[&FontFamily::Proportional], "the bold family exists all the same");
        let gone = Faces { regular: Some("/nonexistent/x.ttf".into()), bold: None, mono: Some("/nonexistent/y.otf".into()) };
        let d = definitions(&gone);
        assert_eq!(d.families[&FontFamily::Proportional], own.families[&FontFamily::Proportional]);
    }

    #[test]
    fn only_font_files_are_taken() {
        let (regular, _, dir) = stand_ins();
        assert_eq!(file_ok(regular.clone()), Some(regular));
        let other = dir.join("font.pcf.gz");
        std::fs::write(&other, b"x").unwrap();
        assert_eq!(file_ok(other), None, "bitmap fonts egui cannot read");
        assert_eq!(file_ok(dir.join("missing.ttf")), None);
        assert_eq!(file_ok(PathBuf::new()), None, "fc-match printed nothing");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_choice_is_installed_only_when_fontconfig_finds_that_family() {
        let (regular, _, dir) = stand_ins();
        let f = regular.display().to_string();
        assert_eq!(match_of(&format!("Lexend\t{f}\n"), "Lexend"), Some(regular.clone()));
        let var = format!("Atkinson Hyperlegible Mono,Atkinson Hyperlegible Mono ExtraLight\t{f}");
        assert_eq!(match_of(&var, "Atkinson Hyperlegible Mono"), Some(regular.clone()));
        assert_eq!(match_of(&format!("Noto Sans\t{f}"), "Lexend"), None, "fontconfig's stand-in for a missing family");
        assert_eq!(match_of(&format!("Atkinson Hyperlegible Next\t{f}"), "Atkinson Hyperlegible"), None, "the whole name");
        assert_eq!(match_of("Lexend\t/nonexistent.ttf", "Lexend"), None);
        assert_eq!(match_of("", "Lexend"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_named_family_has_eguis_fallbacks() {
        let (regular, _, dir) = stand_ins();
        let mut d = definitions(&Faces::default());
        add_family(&mut d, "preview-x", face(std::fs::read(&regular).unwrap(), 400.0));
        let l = &d.families[&FontFamily::Name("preview-x".into())];
        assert_eq!(l[0], "preview-x");
        assert!(l.contains(&"NotoEmoji-Regular".to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_fonts_load_into_a_context() {
        let (regular, mono, dir) = stand_ins();
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions(&Faces { regular: Some(regular), bold: None, mono: Some(mono) }));
        let mut out = ctx.run_ui(Default::default(), |ui| {
            ui.label("Text");
            ui.label(egui::RichText::new("Bold").family(bold()));
            ui.label(egui::RichText::new("code ⏻ 🙂").monospace());
        });
        out.textures_delta.clear();
        assert!(ctx.fonts(|f| f.definitions().font_data.contains_key("system-mono")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
