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

/// Where `n` tiled windows go inside `zone`, as Hyprland's dwindle with the session's gaps: each window takes half of what is left,
/// splitting left/right, then top/bottom, and so on, the last takes the rest; then each box keeps `gaps_out` from the zone's edges
/// and `gaps_in` from its neighbours (so two windows are 2 x `gaps_in` apart). The boxes include the window's border.
pub fn dwindle(zone: Rectangle<i32, Logical>, n: usize, gaps_in: i32, gaps_out: i32) -> Vec<Rectangle<i32, Logical>> {
    let mut boxes = Vec::with_capacity(n);
    let mut area = zone;
    for i in 0..n {
        if i == n - 1 {
            boxes.push(area);
            break;
        }
        let (x, y, w, h) = (area.loc.x, area.loc.y, area.size.w, area.size.h);
        if i % 2 == 0 {
            let first = w / 2;
            boxes.push(Rectangle::new((x, y).into(), (first, h).into()));
            area = Rectangle::new((x + first, y).into(), (w - first, h).into());
        } else {
            let first = h / 2;
            boxes.push(Rectangle::new((x, y).into(), (w, first).into()));
            area = Rectangle::new((x, y + first).into(), (w, h - first).into());
        }
    }
    let (right, bottom) = (zone.loc.x + zone.size.w, zone.loc.y + zone.size.h);
    boxes
        .into_iter()
        .map(|b| {
            let gap = |at_edge: bool| if at_edge { gaps_out } else { gaps_in };
            let l = gap(b.loc.x == zone.loc.x);
            let t = gap(b.loc.y == zone.loc.y);
            let r = gap(b.loc.x + b.size.w == right);
            let bt = gap(b.loc.y + b.size.h == bottom);
            Rectangle::new((b.loc.x + l, b.loc.y + t).into(), ((b.size.w - l - r).max(1), (b.size.h - t - bt).max(1)).into())
        })
        .collect()
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
    fn one_window_takes_everything_but_the_outer_gap() {
        assert_eq!(dwindle(r(0, 0, 1000, 600), 1, 4, 8), vec![r(8, 8, 984, 584)]);
    }

    #[test]
    fn two_windows_split_left_and_right_with_the_sessions_gaps() {
        // 8 from the edges, 4 + 4 between them (Hyprland's gaps_out and gaps_in)
        assert_eq!(dwindle(r(0, 32, 1600, 804), 2, 4, 8), vec![r(8, 40, 788, 788), r(804, 40, 788, 788)]);
    }

    #[test]
    fn three_windows_split_the_right_half_top_and_bottom() {
        let v = dwindle(r(0, 0, 1000, 600), 3, 4, 8);
        assert_eq!(v, vec![r(8, 8, 488, 584), r(504, 8, 488, 288), r(504, 304, 488, 288)]);
    }

    #[test]
    fn no_window_overlaps_another() {
        for n in 1..9 {
            let v = dwindle(r(0, 40, 1920, 1000), n, 4, 8);
            for (i, a) in v.iter().enumerate() {
                for b in &v[i + 1..] {
                    assert!(!a.overlaps(*b), "{n} windows: {a:?} and {b:?}");
                }
            }
        }
    }
}
