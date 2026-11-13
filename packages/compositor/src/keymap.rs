//! The keyboard layout the system was set up with, read the way the session's Hyprland config reads it
//! (`packages/session/hyprland/yggdrasil.lua`, `system_keyboard`): `/etc/X11/xorg.conf.d/00-keyboard.conf`, which the installer
//! and `localectl set-x11-keymap` write, its `Option "XkbLayout"`, `"XkbVariant"` and `"XkbOptions"` lines; US when there is none.
//! One addition: when that file is missing, the `XKBLAYOUT`, `XKBVARIANT` and `XKBOPTIONS` lines of `/etc/vconsole.conf` (the
//! installer writes the same layout there, and systemd-localed keeps both).
//!
//! Read at start and again when either file changes (the 2-second look at the config files); a layout set from outside with the
//! control socket's `keyboard` (the installer's try-out) stays until the system's file changes, as on Hyprland until a reload.
//! `YGGDRASIL_COMPOSITOR_ETC` names another folder in place of /etc (the tests' own).

use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub layout: String,
    pub variant: String,
    pub options: String,
}

fn etc() -> PathBuf {
    std::env::var_os("YGGDRASIL_COMPOSITOR_ETC").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/etc"))
}

/// The two files, X11's first.
pub fn files() -> [PathBuf; 2] {
    let e = etc();
    [e.join("X11/xorg.conf.d/00-keyboard.conf"), e.join("vconsole.conf")]
}

/// `Option "XkbLayout" "de"` lines (as the Lua pattern `^%s*Option%s+"(Xkb%a+)"%s+"([^"]*)"`): an empty layout is no layout.
pub fn parse_x11(text: &str) -> Layout {
    let mut l = Layout::default();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("Option") else { continue };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let mut quoted = rest.split('"').skip(1).step_by(2);
        let (Some(key), Some(value)) = (quoted.next(), quoted.next()) else { continue };
        match key {
            "XkbLayout" if !value.is_empty() => l.layout = value.to_string(),
            "XkbVariant" => l.variant = value.to_string(),
            "XkbOptions" => l.options = value.to_string(),
            _ => {}
        }
    }
    l
}

/// `XKBLAYOUT=de` lines of vconsole.conf (shell-style, quotes allowed).
pub fn parse_vconsole(text: &str) -> Layout {
    let mut l = Layout::default();
    for line in text.lines() {
        let Some((k, v)) = line.trim().split_once('=') else { continue };
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
        match k.trim() {
            "XKBLAYOUT" => l.layout = v,
            "XKBVARIANT" => l.variant = v,
            "XKBOPTIONS" => l.options = v,
            _ => {}
        }
    }
    l
}

/// Letters, digits and `_-,:+()` only, and not too long: what xkb names are made of (an option such as `grp:alt_shift_toggle`).
fn plain(s: &str) -> bool {
    s.len() <= 256 && s.chars().all(|c| c.is_ascii_alphanumeric() || "_-,:+()".contains(c))
}

/// The system's layout: X11's file, else vconsole.conf's lines, else US. Anything that is not an xkb name is left out.
pub fn system() -> Layout {
    let [x11, vconsole] = files();
    let mut l = match std::fs::read_to_string(&x11) {
        Ok(t) => parse_x11(&t),
        Err(_) => std::fs::read_to_string(&vconsole).map(|t| parse_vconsole(&t)).unwrap_or_default(),
    };
    if l.layout.is_empty() || !plain(&l.layout) {
        l.layout = "us".into();
    }
    if !plain(&l.variant) {
        l.variant.clear();
    }
    if !plain(&l.options) {
        l.options.clear();
    }
    l
}

impl Layout {
    pub fn xkb(&self) -> smithay::input::keyboard::XkbConfig<'_> {
        smithay::input::keyboard::XkbConfig {
            layout: &self.layout,
            variant: &self.variant,
            options: (!self.options.is_empty()).then(|| self.options.clone()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installers_x11_file_is_read() {
        // as packages/install-system writes it
        let t = "Section \"InputClass\"\n    Identifier \"system-keyboard\"\n    MatchIsKeyboard \"on\"\n    Option \"XkbLayout\" \"de\"\n    Option \"XkbVariant\" \"nodeadkeys\"\nEndSection\n";
        assert_eq!(parse_x11(t), Layout { layout: "de".into(), variant: "nodeadkeys".into(), options: String::new() });
        // as localectl writes it, with options
        let t = "Section \"InputClass\"\n        Identifier \"system-keyboard\"\n        Option \"XkbLayout\" \"us,ru\"\n        Option \"XkbModel\" \"pc105\"\n        Option \"XkbOptions\" \"grp:alt_shift_toggle\"\nEndSection\n";
        assert_eq!(parse_x11(t), Layout { layout: "us,ru".into(), variant: String::new(), options: "grp:alt_shift_toggle".into() });
        assert_eq!(parse_x11("Option \"XkbLayout\" \"\"\n").layout, "", "an empty layout is none (US then)");
        assert_eq!(parse_x11("Options \"XkbLayout\" \"fr\"\n").layout, "", "only Option lines");
    }

    #[test]
    fn vconsole_conf_is_the_second_choice() {
        let t = "KEYMAP=de-latin1-nodeadkeys\nXKBLAYOUT=de\nXKBVARIANT=nodeadkeys\n";
        assert_eq!(parse_vconsole(t), Layout { layout: "de".into(), variant: "nodeadkeys".into(), options: String::new() });
        assert_eq!(parse_vconsole("XKBLAYOUT=\"fr\"\nXKBOPTIONS='compose:ralt'\n").options, "compose:ralt");
        assert!(plain("grp:alt_shift_toggle,compose:ralt") && !plain("de; rm") && !plain("a\"b"));
    }
}
