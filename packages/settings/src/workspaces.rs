//! Settings > Appearance > Workspaces: which profile windows follow on each workspace. Written to ~/.config/yggdrasil/workspaces.conf,
//! which the compositor reads (and reads again when it changes).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Standard,
    Tiling,
}

/// What a new workspace gets when nothing says otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Default {
    Fixed(Profile),
    /// Asks whenever an empty workspace is opened.
    Ask,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conf {
    pub default: Default,
    /// Workspaces 1 to 4: Some = always this profile.
    pub per: [Option<Profile>; 4],
}

impl std::default::Default for Conf {
    fn default() -> Self {
        Conf { default: Default::Fixed(Profile::Standard), per: [None; 4] }
    }
}

fn profile(s: &str) -> Option<Profile> {
    match s.trim() {
        "standard" => Some(Profile::Standard),
        "tiling" => Some(Profile::Tiling),
        _ => None,
    }
}

pub fn parse(text: &str) -> Conf {
    let mut c = Conf::default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k == "default" {
            c.default = if v == "ask" { Default::Ask } else { profile(v).map(Default::Fixed).unwrap_or(c.default) };
        } else if let Ok(n) = k.parse::<usize>() {
            if (1..=4).contains(&n) {
                c.per[n - 1] = profile(v);
            }
        }
    }
    c
}

pub fn text(c: &Conf) -> String {
    let name = |p: Profile| if p == Profile::Tiling { "tiling" } else { "standard" };
    let mut s = String::from("# Written by Settings > Appearance > Workspaces. The compositor reads it (and again when it changes).\n");
    s.push_str(&format!("default = {}\n", match c.default {
        Default::Ask => "ask",
        Default::Fixed(p) => name(p),
    }));
    for (i, p) in c.per.iter().enumerate() {
        if let Some(p) = p {
            s.push_str(&format!("{} = {}\n", i + 1, name(*p)));
        }
    }
    s
}

pub fn path() -> std::path::PathBuf {
    yggdrasil_conf::dirs::config().join("workspaces.conf")
}

pub fn load() -> Conf {
    std::fs::read_to_string(path()).map(|t| parse(&t)).unwrap_or_default()
}

pub fn save(c: &Conf) -> Result<(), String> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, text(c)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let c = Conf { default: Default::Ask, per: [Some(Profile::Tiling), None, Some(Profile::Standard), None] };
        assert_eq!(parse(&text(&c)), c);
        let d = Conf { default: Default::Fixed(Profile::Tiling), per: [None; 4] };
        assert_eq!(parse(&text(&d)), d);
    }

    #[test]
    fn nothing_means_standard() {
        assert_eq!(parse(""), Conf::default());
    }
}
