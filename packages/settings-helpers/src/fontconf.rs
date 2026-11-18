//! fontconf: the computer's system font, which the login screen and every account without its own choice use (Settings >
//! Appearance > Fonts, "Use on the login screen"), run as root through pkexec:
//!
//!   fontconf set SANS MONO     the ids of yggdrasil_conf::fonts' choices, or "-" for none of each; both "-" removes the file
//!
//! It writes /etc/fonts/conf.d/56-yggdrasil-font.conf (yggdrasil_conf::fonts::render: only the families in the list, never
//! text from the caller), whole and never through a link, mode 0644. Nothing else is touched. Exit codes: 0 done, 2 a wrong
//! command or id (nothing written), 1 the file could not be written.

use std::path::Path;

use yggdrasil_conf::fonts::{self, Prefs};

/// The work, with the file's path given (tests write in a scratch folder).
pub fn run(argv: &[String], path: &Path) -> i32 {
    let usage = || {
        eprintln!("usage: fontconf set SANS MONO   (ids from Settings > Appearance > Fonts, or - for none)");
        2
    };
    let [_, cmd, sans, mono] = argv else { return usage() };
    if cmd != "set" {
        return usage();
    }
    let Some(p) = Prefs::from_ids(sans, mono) else {
        eprintln!("fontconf: not one of the fonts: {sans:?} {mono:?}");
        return 2;
    };
    match fonts::save_to(path, &p) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("fontconf: {}: {e}", path.display());
            1
        }
    }
}

pub fn main(argv: &[String]) -> i32 {
    run(argv, Path::new(fonts::SYSTEM_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn writes_and_removes_the_computers_file() {
        let dir = std::env::temp_dir().join(format!("fontconf-{}", std::process::id()));
        let path = dir.join("conf.d").join(fonts::FILE);
        assert_eq!(run(&args(&["fontconf", "set", "atkinson-hyperlegible", "-"]), &path), 0);
        assert_eq!(fonts::load_from(&path), Prefs { sans: Some("atkinson-hyperlegible"), mono: None });
        assert_eq!(run(&args(&["fontconf", "set", "-", "opendyslexic-mono"]), &path), 0);
        assert_eq!(fonts::load_from(&path), Prefs { sans: None, mono: Some("opendyslexic-mono") });
        assert_eq!(run(&args(&["fontconf", "set", "-", "-"]), &path), 0);
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_anything_else_and_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("fontconf-bad-{}", std::process::id()));
        let path = dir.join(fonts::FILE);
        for bad in [
            &["fontconf"][..],
            &["fontconf", "set", "lexend"],
            &["fontconf", "get", "lexend", "-"],
            &["fontconf", "set", "lexend", "-", "extra"],
            &["fontconf", "set", "Lexend", "-"],
            &["fontconf", "set", "lexend</family><dir>/home</dir>", "-"],
            &["fontconf", "set", "-", "noto-sans"],
            &["fontconf", "set", "../../etc/shadow", "-"],
        ] {
            assert_eq!(run(&args(bad), &path), 2, "{bad:?}");
        }
        assert!(!dir.exists());
    }
}
