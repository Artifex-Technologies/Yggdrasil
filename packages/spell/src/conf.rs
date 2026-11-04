//! ~/.config/yggdrasil/spell.conf: whether spell checking is on and which languages are switched on, written by Settings > Keyboard >
//! Spelling and languages. Plain `key = value` lines like the other Yggdrasil settings files; lines this version does not know are kept.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpellConf {
    pub enabled: bool,
    /// Dictionary codes (`en_US`, `de_DE`); empty means "the language the system is set to".
    pub languages: Vec<String>,
    /// Word packs that are on (ids); `None` means "the packs that are on by default".
    pub packs: Option<Vec<String>>,
    other: Vec<String>,
}

impl Default for SpellConf {
    fn default() -> Self {
        SpellConf { enabled: true, languages: vec![], packs: None, other: vec![] }
    }
}

impl SpellConf {
    pub fn path() -> PathBuf {
        yggdrasil_conf::dirs::config().join("spell.conf")
    }

    pub fn parse(text: &str) -> SpellConf {
        let mut c = SpellConf::default();
        for line in text.lines() {
            let t = line.split('#').next().unwrap_or("").trim();
            match t.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("enabled", v)) => c.enabled = !matches!(v, "no" | "false" | "off" | "0"),
                Some(("languages", v)) => c.languages = ids(v),
                Some(("packs", v)) => c.packs = Some(ids(v)),
                _ if !t.is_empty() => c.other.push(line.to_string()),
                _ => {}
            }
        }
        c
    }

    pub fn load() -> SpellConf {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn render(&self) -> String {
        let mut s = format!("# Spell checking (Settings > Keyboard > Spelling and languages)\nenabled = {}\nlanguages = {}\n", if self.enabled { "yes" } else { "no" }, self.languages.join(", "));
        if let Some(p) = &self.packs {
            s.push_str(&format!("packs = {}\n", p.join(", ")));
        }
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

    /// The word packs that are on: the chosen ones, or with none chosen yet the installed packs that are on by default.
    pub fn active_packs(&self) -> Vec<String> {
        if !self.enabled {
            return vec![];
        }
        match &self.packs {
            Some(p) => p.clone(),
            None => crate::packs::installed().into_iter().filter(|p| p.default_on).map(|p| p.id).collect(),
        }
    }

    /// The languages to check with: the chosen ones that are installed; with none chosen, the system's language (LANG) when its
    /// dictionary is installed, else every installed English dictionary's first one. Empty when spell checking is off.
    pub fn active(&self) -> Vec<String> {
        if !self.enabled {
            return vec![];
        }
        let installed: Vec<String> = crate::dicts::installed().into_iter().map(|i| i.code).collect();
        let chosen: Vec<String> = self.languages.iter().filter(|c| installed.contains(c)).cloned().collect();
        if !chosen.is_empty() {
            return chosen;
        }
        let lang = std::env::var("LANG").unwrap_or_default();
        let sys = lang.split(['.', '@']).next().unwrap_or("").to_string();
        if installed.contains(&sys) {
            return vec![sys];
        }
        installed.into_iter().find(|c| c.starts_with("en_") && !c.contains('-')).into_iter().collect()
    }
}

/// A list of plain ids (dictionary codes, pack ids): anything else is dropped.
fn ids(v: &str) -> Vec<String> {
    v.split([',', ' ']).filter(|s| !s.is_empty() && s.chars().all(|ch| ch.is_ascii_alphanumeric() || "_-".contains(ch))).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_unknown_lines() {
        let c = SpellConf::parse("enabled = no\nlanguages = en_US, de_DE\npacks = tech, gaming\nfuture = 1\nlanguages_bad = x\n");
        assert!(!c.enabled);
        assert_eq!(c.languages, ["en_US", "de_DE"]);
        assert_eq!(c.packs.as_deref(), Some(&["tech".to_string(), "gaming".to_string()][..]));
        assert_eq!(SpellConf::parse("enabled = yes\n").packs, None, "no packs line: the defaults");
        let again = SpellConf::parse(&c.render());
        assert_eq!(again, c);
        assert!(c.render().contains("future = 1"));
    }

    #[test]
    fn odd_language_values_are_dropped() {
        let c = SpellConf::parse("languages = en_US, ../../etc, de;rm\n");
        assert_eq!(c.languages, ["en_US"]);
    }
}
