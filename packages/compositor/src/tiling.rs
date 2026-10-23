//! Workspace profiles: each workspace is either **standard** (windows float and you place them) or **tiling** (windows are laid out
//! automatically, Hyprland-style, "dwindle": the first takes the left half, the next the top right, and so on, each splitting the
//! space that is left). A default profile can be chosen (or "ask each time" a workspace is opened empty) with exceptions per workspace.
//!
//! `~/.config/yggdrasil/workspaces.conf` (written by Settings > Appearance > Workspaces):
//!
//!     default = standard        # standard, tiling or ask
//!     2 = tiling                # workspace 2 is always tiling (1 to 4)

use smithay::utils::{Logical, Rectangle};

pub const WORKSPACES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Standard,
    Tiling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultProfile {
    Fixed(Profile),
    /// Ask whenever an empty workspace is opened.
    Ask,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conf {
    pub default: DefaultProfile,
    pub per: [Option<Profile>; WORKSPACES],
}

impl Default for Conf {
    fn default() -> Self {
        Conf { default: DefaultProfile::Fixed(Profile::Standard), per: [None; WORKSPACES] }
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
            c.default = if v == "ask" { DefaultProfile::Ask } else { profile(v).map(DefaultProfile::Fixed).unwrap_or(c.default) };
        } else if let Ok(n) = k.parse::<usize>() {
            if (1..=WORKSPACES).contains(&n) {
                c.per[n - 1] = profile(v);
            }
        }
    }
    c
}

impl Conf {
    /// The profile workspace `ws` (0-based) has, when it is already decided; None means: ask.
    pub fn profile_of(&self, ws: usize) -> Option<Profile> {
        match (self.per.get(ws).copied().flatten(), self.default) {
            (Some(p), _) => Some(p),
            (None, DefaultProfile::Fixed(p)) => Some(p),
            (None, DefaultProfile::Ask) => None,
        }
    }
}

pub fn config_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("YGGDRASIL_COMPOSITOR_WORKSPACES") {
        return p.into();
    }
    yggdrasil_conf::dirs::config().join("workspaces.conf")
}

/// Where `n` tiled windows go inside `zone`, with `gap` pixels between them and around the edge. Dwindle: each window takes half of what is
/// left, splitting left/right, then top/bottom, and so on; the last takes the rest.
pub fn dwindle(zone: Rectangle<i32, Logical>, n: usize, gap: i32) -> Vec<Rectangle<i32, Logical>> {
    let mut out = Vec::with_capacity(n);
    let mut area = Rectangle::new((zone.loc.x + gap, zone.loc.y + gap).into(), ((zone.size.w - 2 * gap).max(1), (zone.size.h - 2 * gap).max(1)).into());
    for i in 0..n {
        if i == n - 1 {
            out.push(area);
            break;
        }
        let (x, y, w, h) = (area.loc.x, area.loc.y, area.size.w, area.size.h);
        if i % 2 == 0 {
            let first = (w - gap) / 2;
            out.push(Rectangle::new((x, y).into(), (first, h).into()));
            area = Rectangle::new((x + first + gap, y).into(), ((w - first - gap).max(1), h).into());
        } else {
            let first = (h - gap) / 2;
            out.push(Rectangle::new((x, y).into(), (w, first).into()));
            area = Rectangle::new((x, y + first + gap).into(), (w, (h - first - gap).max(1)).into());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn config_parses() {
        let c = parse("default = tiling\n2 = standard\n9 = tiling\nbogus\n3 = tiling # three\n");
        assert_eq!(c.default, DefaultProfile::Fixed(Profile::Tiling));
        assert_eq!(c.profile_of(1), Some(Profile::Standard));
        assert_eq!(c.profile_of(2), Some(Profile::Tiling));
        assert_eq!(c.profile_of(0), Some(Profile::Tiling));
        assert_eq!(parse("default = ask").profile_of(0), None);
        assert_eq!(parse("").profile_of(0), Some(Profile::Standard));
        assert_eq!(parse("default = ask\n1 = tiling").profile_of(0), Some(Profile::Tiling));
    }

    #[test]
    fn one_window_takes_everything() {
        assert_eq!(dwindle(r(0, 0, 1000, 600), 1, 10), vec![r(10, 10, 980, 580)]);
    }

    #[test]
    fn two_windows_split_left_and_right() {
        let v = dwindle(r(0, 0, 1000, 600), 2, 10);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].loc.x, 10);
        assert!(v[1].loc.x > v[0].loc.x + v[0].size.w, "a gap between them");
        assert_eq!(v[0].size.h, 580);
        assert_eq!(v[1].size.h, 580);
    }

    #[test]
    fn three_windows_split_the_right_half_top_and_bottom() {
        let v = dwindle(r(0, 0, 1000, 600), 3, 10);
        assert_eq!(v[1].loc.x, v[2].loc.x);
        assert!(v[2].loc.y > v[1].loc.y + v[1].size.h - 1);
    }

    #[test]
    fn no_window_overlaps_another() {
        for n in 1..9 {
            let v = dwindle(r(0, 40, 1920, 1000), n, 8);
            for (i, a) in v.iter().enumerate() {
                for b in &v[i + 1..] {
                    assert!(!a.overlaps(*b), "{n} windows: {a:?} and {b:?}");
                }
            }
        }
    }
}
