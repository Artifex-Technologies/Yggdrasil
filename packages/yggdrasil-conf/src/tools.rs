//! ~/.config/yggdrasil/tools.conf: the small built-in helpers in the spirit of PowerToys (Find my mouse, ...), written by Settings and
//! read by the panel. Plain `key = value` lines; keys this version does not know are kept when the file is written again, so tools
//! added later (or by another program) keep their settings.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolsConf {
    /// Find my mouse: pressing Ctrl twice puts a spotlight on the pointer.
    pub find_mouse: bool,
    /// Lines this version does not read, kept as they were.
    other: Vec<String>,
}

impl Default for ToolsConf {
    fn default() -> Self {
        ToolsConf { find_mouse: true, other: vec![] }
    }
}

fn yes(v: &str) -> bool {
    !matches!(v, "no" | "false" | "off" | "0")
}

impl ToolsConf {
    pub fn path() -> PathBuf {
        crate::dirs::config().join("tools.conf")
    }

    pub fn parse(text: &str) -> ToolsConf {
        let mut c = ToolsConf::default();
        for line in text.lines() {
            let t = line.split('#').next().unwrap_or("").trim();
            match t.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("find_mouse", v)) => c.find_mouse = yes(v),
                _ if !t.is_empty() => c.other.push(line.to_string()),
                _ => {}
            }
        }
        c
    }

    pub fn load() -> ToolsConf {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn render(&self) -> String {
        let mut s = format!("# Yggdrasil's tools (Settings > Keyboard Shortcuts)\nfind_mouse = {}\n", if self.find_mouse { "yes" } else { "no" });
        for l in &self.other {
            s.push_str(l);
            s.push('\n');
        }
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("conf.tmp");
        std::fs::write(&tmp, self.render())?;
        std::fs::rename(tmp, p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_by_default_and_other_keys_are_kept() {
        assert!(ToolsConf::parse("").find_mouse);
        let c = ToolsConf::parse("find_mouse = no\nzones = 3\n# a note\n");
        assert!(!c.find_mouse);
        assert_eq!(ToolsConf::parse(&c.render()), c);
        assert!(c.render().contains("zones = 3"));
    }
}
