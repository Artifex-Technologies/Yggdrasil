//! The system font: which family the desktop and every program use for ordinary text (`sans-serif`) and for code and terminals
//! (`monospace`). Chosen in the installer and in Settings > Appearance > Font; Noto Sans and Noto Sans Mono when nothing is
//! chosen. It is a fontconfig file, so every program that asks fontconfig for "sans-serif" follows it (GTK, Qt, browsers,
//! Yggdrasil's own panel, lock screen and apps):
//!
//! ```text
//! ~/.config/fontconfig/conf.d/56-yggdrasil-font.conf    yours (Settings writes it)
//! /etc/fonts/conf.d/56-yggdrasil-font.conf              the computer's: the login screen and anyone without their own
//!                                                       (the installer writes it; Settings through pkexec and `fontconf`)
//! ```
//!
//! Each holds one `<alias>` per generic family it sets, preferring the chosen family. Fontconfig reads the user's files at
//! 50-user.conf, before the system file at 56 and the distribution's defaults at 60-latin.conf, and the first preference
//! read wins: so your own choice beats the computer's, and either beats Noto Sans. Only the families listed here are ever
//! written, so the file never carries text from anywhere else.

use std::path::{Path, PathBuf};

/// One font to choose from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    /// What the plan, the helper and the settings use (stable).
    pub id: &'static str,
    /// The family name fontconfig knows it by.
    pub family: &'static str,
    /// What a person reads.
    pub name: &'static str,
    /// A short, plain description (modest: what it was made for, not what it does for anyone).
    pub note: &'static str,
}

/// Fonts for ordinary text, the default first.
pub const SANS: &[Choice] = &[
    Choice {
        id: "noto-sans",
        family: "Noto Sans",
        name: "Noto Sans",
        note: "The default: clear and plain, with letters for almost every language",
    },
    Choice {
        id: "atkinson-hyperlegible",
        family: "Atkinson Hyperlegible",
        name: "Atkinson Hyperlegible",
        note: "Designed by the Braille Institute for low vision readers: letters that are hard to mix up, like I, l and 1",
    },
    Choice {
        id: "atkinson-hyperlegible-next",
        family: "Atkinson Hyperlegible Next",
        name: "Atkinson Hyperlegible Next",
        note: "The newer Atkinson Hyperlegible (2024), with more weights and languages, for low vision readers",
    },
    Choice { id: "lexend", family: "Lexend", name: "Lexend", note: "Wide letters with even spacing, designed to make reading less tiring" },
    Choice {
        id: "opendyslexic",
        family: "OpenDyslexic",
        name: "OpenDyslexic",
        note: "Designed to make reading easier with dyslexia: heavier letter bottoms. Some find it helps, some do not",
    },
];

/// Fonts for code and terminals, the default first.
pub const MONO: &[Choice] = &[
    Choice { id: "noto-sans-mono", family: "Noto Sans Mono", name: "Noto Sans Mono", note: "The default, matching Noto Sans" },
    Choice {
        id: "atkinson-hyperlegible-mono",
        family: "Atkinson Hyperlegible Mono",
        name: "Atkinson Hyperlegible Mono",
        note: "The Braille Institute's monospaced Atkinson Hyperlegible, for low vision readers",
    },
    Choice {
        id: "opendyslexic-mono",
        family: "OpenDyslexicMono",
        name: "OpenDyslexic Mono",
        note: "The monospaced OpenDyslexic (an older design, from 2016)",
    },
];

/// The file's name, in both folders.
pub const FILE: &str = "56-yggdrasil-font.conf";
/// The computer's file.
pub const SYSTEM_PATH: &str = "/etc/fonts/conf.d/56-yggdrasil-font.conf";

pub fn sans(id: &str) -> Option<&'static Choice> {
    SANS.iter().find(|c| c.id == id)
}

pub fn mono(id: &str) -> Option<&'static Choice> {
    MONO.iter().find(|c| c.id == id)
}

/// What one file sets: a choice's id per generic family, None where it sets nothing (the other file, or the default, decides).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prefs {
    pub sans: Option<&'static str>,
    pub mono: Option<&'static str>,
}

impl Prefs {
    /// From the ids the plan or the helper's arguments give ("" or "-": nothing); None when an id is not one of ours.
    pub fn from_ids(sans_id: &str, mono_id: &str) -> Option<Prefs> {
        let pick = |id: &str, list: &'static [Choice]| -> Option<Option<&'static str>> {
            if id.is_empty() || id == "-" {
                return Some(None);
            }
            list.iter().find(|c| c.id == id).map(|c| Some(c.id))
        };
        Some(Prefs { sans: pick(sans_id, SANS)?, mono: pick(mono_id, MONO)? })
    }

    /// `self`, with the other file's settings where this one sets nothing.
    pub fn over(self, below: Prefs) -> Prefs {
        Prefs { sans: self.sans.or(below.sans), mono: self.mono.or(below.mono) }
    }

    /// The choices in effect (the defaults where nothing is set).
    pub fn sans_choice(&self) -> &'static Choice {
        self.sans.and_then(sans).unwrap_or(&SANS[0])
    }

    pub fn mono_choice(&self) -> &'static Choice {
        self.mono.and_then(mono).unwrap_or(&MONO[0])
    }
}

/// The `<family>` names in `block`, in order.
fn families(block: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut rest = block;
    while let Some(i) = rest.find("<family>") {
        rest = &rest[i + "<family>".len()..];
        let Some(j) = rest.find("</family>") else { break };
        out.push(rest[..j].trim());
        rest = &rest[j..];
    }
    out
}

/// Reads one of our files: each `<alias>` whose generic family is sans-serif or monospace, and the first family it prefers,
/// when that is one of the choices. Anything else (another file's rules, a family not listed) is not ours and reads as unset.
pub fn parse(text: &str) -> Prefs {
    let mut p = Prefs::default();
    let mut rest = text;
    while let Some(i) = rest.find("<alias") {
        rest = &rest[i..];
        let end = rest.find("</alias>").map(|j| j + "</alias>".len()).unwrap_or(rest.len());
        let block = &rest[..end];
        rest = &rest[end..];
        let Some(generic) = families(block.split("<prefer>").next().unwrap_or("")).first().copied() else { continue };
        let Some(chosen) = block.split_once("<prefer>").and_then(|(_, b)| families(b).first().copied()) else { continue };
        match generic {
            "sans-serif" => p.sans = SANS.iter().find(|c| c.family == chosen).map(|c| c.id).or(p.sans),
            "monospace" => p.mono = MONO.iter().find(|c| c.family == chosen).map(|c| c.id).or(p.mono),
            _ => {}
        }
    }
    p
}

/// The file for these settings; None when it sets nothing (then there is no file).
pub fn render(p: &Prefs) -> Option<String> {
    let sans = p.sans.and_then(sans);
    let mono = p.mono.and_then(mono);
    if sans.is_none() && mono.is_none() {
        return None;
    }
    let mut t = String::from(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n\
         <!-- Written by Yggdrasil (the installer, Settings > Appearance > Font): the system font. Change it there. -->\n\
         <fontconfig>\n  <description>Yggdrasil: the chosen system font</description>\n",
    );
    // "sans" and "system-ui" are what some programs ask for instead of "sans-serif"
    if let Some(c) = sans {
        for generic in ["sans-serif", "sans", "system-ui"] {
            t.push_str(&format!(
                "  <alias binding=\"same\">\n    <family>{generic}</family>\n    <prefer><family>{}</family></prefer>\n  </alias>\n",
                c.family
            ));
        }
    }
    if let Some(c) = mono {
        t.push_str(&format!(
            "  <alias binding=\"same\">\n    <family>monospace</family>\n    <prefer><family>{}</family></prefer>\n  </alias>\n",
            c.family
        ));
    }
    t.push_str("</fontconfig>\n");
    Some(t)
}

/// Your own file: `$XDG_CONFIG_HOME/fontconfig/conf.d/56-yggdrasil-font.conf`.
pub fn user_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or(home.join(".config"));
    base.join("fontconfig").join("conf.d").join(FILE)
}

pub fn load_from(path: &Path) -> Prefs {
    std::fs::read_to_string(path).map(|t| parse(&t)).unwrap_or_default()
}

/// What you see: your own file over the computer's.
pub fn load() -> Prefs {
    load_from(&user_path()).over(load_from(Path::new(SYSTEM_PATH)))
}

/// Writes `p` to `path` whole (a new file renamed over the old one; the folder is made when missing), or removes the file when
/// `p` sets nothing. Never writes through a link: the rename replaces the name itself.
pub fn save_to(path: &Path, p: &Prefs) -> std::io::Result<()> {
    match render(p) {
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
        Some(text) => {
            let dir = path.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no folder"))?;
            std::fs::create_dir_all(dir)?;
            let tmp = dir.join(format!(".{FILE}.{}.tmp", std::process::id()));
            let _ = std::fs::remove_file(&tmp);
            {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(&tmp)?;
                f.write_all(text.as_bytes())?;
                f.sync_all()?;
            }
            std::fs::rename(&tmp, path).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
        }
    }
}

/// Your own choice (Settings > Appearance > Font).
pub fn save_user(p: &Prefs) -> std::io::Result<()> {
    save_to(&user_path(), p)
}

/// Something that changes whenever either file is written, replaced or removed (their sizes and times), so a program can notice
/// a new choice by looking at two files' metadata.
pub fn signature() -> u64 {
    signature_of(&[user_path(), PathBuf::from(SYSTEM_PATH)])
}

pub fn signature_of(paths: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in paths {
        match std::fs::metadata(p) {
            Ok(m) => (m.len(), m.modified().ok(), std::os::unix::fs::MetadataExt::ino(&m)).hash(&mut h),
            Err(_) => 0u8.hash(&mut h),
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_chosen_is_no_file_and_the_defaults() {
        assert_eq!(render(&Prefs::default()), None);
        let p = parse("");
        assert_eq!(p, Prefs::default());
        assert_eq!(p.sans_choice().family, "Noto Sans");
        assert_eq!(p.mono_choice().family, "Noto Sans Mono");
    }

    #[test]
    fn what_is_written_reads_back() {
        for s in SANS.iter().map(|c| Some(c.id)).chain([None]) {
            for m in MONO.iter().map(|c| Some(c.id)).chain([None]) {
                let p = Prefs { sans: s, mono: m };
                assert_eq!(render(&p).map(|t| parse(&t)).unwrap_or_default(), p, "{p:?}");
            }
        }
    }

    #[test]
    fn the_file_prefers_the_family_for_every_generic_name() {
        let t = render(&Prefs { sans: Some("atkinson-hyperlegible-next"), mono: Some("opendyslexic-mono") }).unwrap();
        assert!(t.starts_with("<?xml"));
        for g in ["sans-serif", "sans", "system-ui"] {
            assert!(t.contains(&format!("<family>{g}</family>\n    <prefer><family>Atkinson Hyperlegible Next</family></prefer>")), "{g}");
        }
        assert!(t.contains("<family>monospace</family>\n    <prefer><family>OpenDyslexicMono</family></prefer>"));
        // only the sans block when only sans is chosen
        let t = render(&Prefs { sans: Some("lexend"), mono: None }).unwrap();
        assert!(!t.contains("monospace"));
    }

    #[test]
    fn a_family_that_is_not_ours_reads_as_unset() {
        let t = "<fontconfig><alias><family>sans-serif</family><prefer><family>Comic Sans MS</family></prefer></alias>\
                 <alias><family>monospace</family><prefer><family>Atkinson Hyperlegible Mono</family><family>X</family></prefer></alias>\
                 <alias><family>serif</family><prefer><family>Lexend</family></prefer></alias></fontconfig>";
        assert_eq!(parse(t), Prefs { sans: None, mono: Some("atkinson-hyperlegible-mono") });
        // half a file
        assert_eq!(parse("<alias><family>sans-serif</family><prefer><family>Lexend"), Prefs::default());
    }

    #[test]
    fn ids_from_outside_are_checked() {
        assert_eq!(Prefs::from_ids("lexend", "-"), Some(Prefs { sans: Some("lexend"), mono: None }));
        assert_eq!(Prefs::from_ids("", "opendyslexic-mono"), Some(Prefs { sans: None, mono: Some("opendyslexic-mono") }));
        assert_eq!(Prefs::from_ids("opendyslexic-mono", "-"), None, "a monospace font is not a choice for text");
        assert_eq!(Prefs::from_ids("lexend", "lexend"), None);
        assert_eq!(Prefs::from_ids("../../etc/passwd", "-"), None);
        assert_eq!(Prefs::from_ids("Lexend", "-"), None, "ids, not family names");
    }

    #[test]
    fn your_own_choice_wins_field_by_field() {
        let mine = Prefs { sans: Some("noto-sans"), mono: None };
        let computer = Prefs { sans: Some("opendyslexic"), mono: Some("atkinson-hyperlegible-mono") };
        assert_eq!(mine.over(computer), Prefs { sans: Some("noto-sans"), mono: Some("atkinson-hyperlegible-mono") });
    }

    #[test]
    fn saving_writes_whole_files_and_removes_an_empty_one() {
        let dir = std::env::temp_dir().join(format!("ygg-fonts-{}", std::process::id()));
        let path = dir.join("conf.d").join(FILE);
        let before = signature_of(std::slice::from_ref(&path));
        save_to(&path, &Prefs { sans: Some("lexend"), mono: None }).unwrap();
        assert_eq!(load_from(&path).sans, Some("lexend"));
        let after = signature_of(std::slice::from_ref(&path));
        assert_ne!(before, after);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o644);
        // a link where the file goes is replaced, not written through
        let target = dir.join("elsewhere");
        std::fs::write(&target, "keep").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        save_to(&path, &Prefs { sans: Some("opendyslexic"), mono: None }).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        assert!(!std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
        save_to(&path, &Prefs::default()).unwrap();
        assert!(!path.exists());
        save_to(&path, &Prefs::default()).unwrap();
        assert_eq!(std::fs::read_dir(dir.join("conf.d")).unwrap().count(), 0, "no temporary file left");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
