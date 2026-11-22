//! A screen's colour matrix (CTM), set by a program through hyprland-ctm-control-v1: the protocol hyprsunset speaks, and so
//! Settings' Night Light and the notch's tile (yggdrasil_conf::nightlight runs hyprsunset with its schedule and fade) work on
//! this compositor as they do on Hyprland.
//!
//! - One program at a time: the first manager bound while none is counts; another is told `blocked` (version 2) and what it
//!   sets is ignored. When the one that counts goes, every screen is back to no tint.
//! - A commit sets every screen at once: the ones it named to their matrix, the others to none (identity).
//! - The hardware backend puts a screen's matrix into its CRTC's gamma table (udev.rs): each colour is scaled by its row of the
//!   matrix (`channels`), exact for the diagonal matrices hyprsunset sends for a colour temperature, and costs nothing to draw.
//!   Headless, the matrices are only kept, for `ctl ctm` and the tests.
//!
//! protocols/hyprland-ctm-control-v1.xml is hyprland-protocols' (BSD-3-Clause, its notice inside the file).
use crate::state::App;
use smithay::output::Output;
use std::sync::Mutex;
use wayland_server::backend::ClientId;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak};

#[allow(dead_code, non_camel_case_types, non_upper_case_globals, non_snake_case, unused_imports, missing_docs, clippy::all)]
pub mod proto {
    use wayland_server;
    use wayland_server::protocol::*;
    pub mod __interfaces {
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/hyprland-ctm-control-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_server_code!("protocols/hyprland-ctm-control-v1.xml");
}

use proto::hyprland_ctm_control_manager_v1::{self, HyprlandCtmControlManagerV1};

/// No tint.
pub const IDENTITY: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

/// The matrices in force, and the program that sets them.
#[derive(Default)]
pub struct Ctm {
    /// the manager whose matrices count
    active: Option<Weak<HyprlandCtmControlManagerV1>>,
    /// each tinted screen's matrix, by name (a screen not listed has none)
    pub current: Vec<(String, [f64; 9])>,
}

/// One manager's side: told it is blocked, and the matrices set since its last commit.
pub struct Pending {
    blocked: bool,
    set: Vec<(String, [f64; 9])>,
}

pub fn create_global(dh: &DisplayHandle) {
    dh.create_global::<App, HyprlandCtmControlManagerV1, ()>(2, ());
}

/// What each colour of a screen is scaled by (red, green, blue): its row of the matrix summed (so a grey stays the grey the
/// matrix makes of it), from 0 to 1 (a gamma table cannot brighten past full).
pub fn channels(m: &[f64; 9]) -> [f64; 3] {
    [0, 1, 2].map(|r| (m[r * 3] + m[r * 3 + 1] + m[r * 3 + 2]).clamp(0.0, 1.0))
}

/// A gamma table of `len` steps for one colour scaled by `factor` (a straight line from 0 to `factor` of full).
pub fn ramp(factor: f64, len: usize) -> Vec<u16> {
    let last = len.saturating_sub(1).max(1) as f64;
    (0..len).map(|i| ((i as f64 / last) * factor.clamp(0.0, 1.0) * 65535.0).round() as u16).collect()
}

impl App {
    /// A screen's matrix now (identity when it has none).
    pub fn ctm_of(&self, name: &str) -> [f64; 9] {
        self.ctm.current.iter().find(|(n, _)| n == name).map(|(_, m)| *m).unwrap_or(IDENTITY)
    }

    /// Every screen's matrix (`ctm`): its name and the nine numbers, row by row.
    pub fn ctm_lines(&self) -> Vec<String> {
        self.space
            .outputs()
            .map(|o| {
                let m = self.ctm_of(&o.name());
                format!("{}\t{}", o.name(), m.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>().join(" "))
            })
            .collect()
    }

    fn set_ctm(&mut self, set: Vec<(String, [f64; 9])>) {
        self.ctm.current = set.into_iter().filter(|(_, m)| *m != IDENTITY).collect();
        self.wake();
    }
}

impl GlobalDispatch<HyprlandCtmControlManagerV1, ()> for App {
    fn bind(app: &mut App, _: &DisplayHandle, _: &Client, res: New<HyprlandCtmControlManagerV1>, _: &(), init: &mut DataInit<'_, App>) {
        let blocked = app.ctm.active.as_ref().is_some_and(|w| w.upgrade().is_ok());
        let m = init.init(res, Mutex::new(Pending { blocked, set: vec![] }));
        if !blocked {
            app.ctm.active = Some(m.downgrade());
        } else if m.version() >= 2 {
            m.blocked();
        }
    }
}

impl Dispatch<HyprlandCtmControlManagerV1, Mutex<Pending>> for App {
    fn request(
        app: &mut App,
        _: &Client,
        res: &HyprlandCtmControlManagerV1,
        request: hyprland_ctm_control_manager_v1::Request,
        data: &Mutex<Pending>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, App>,
    ) {
        use hyprland_ctm_control_manager_v1::{Error, Request};
        match request {
            Request::SetCtmForOutput { output, mat0, mat1, mat2, mat3, mat4, mat5, mat6, mat7, mat8 } => {
                let m = [mat0, mat1, mat2, mat3, mat4, mat5, mat6, mat7, mat8];
                if m.iter().any(|v| !v.is_finite() || *v < 0.0) {
                    res.post_error(Error::InvalidMatrix, "the values are from 0 up");
                    return;
                }
                let Some(o) = Output::from_resource(&output) else { return };
                let mut p = data.lock().unwrap();
                if !p.blocked {
                    p.set.retain(|(n, _)| *n != o.name());
                    p.set.push((o.name(), m));
                }
            }
            Request::Commit => {
                let set = {
                    let mut p = data.lock().unwrap();
                    if p.blocked {
                        return;
                    }
                    std::mem::take(&mut p.set)
                };
                app.set_ctm(set);
            }
            Request::Destroy => {}
        }
    }

    fn destroyed(app: &mut App, _: ClientId, res: &HyprlandCtmControlManagerV1, _: &Mutex<Pending>) {
        if app.ctm.active.as_ref().is_some_and(|w| w.id() == res.id()) {
            app.ctm.active = None;
            app.set_ctm(vec![]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tint_becomes_a_gamma_table_for_each_colour() {
        // hyprsunset's matrix for a warm temperature: a diagonal one
        assert_eq!(channels(&[1.0, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.55]), [1.0, 0.8, 0.55]);
        assert_eq!(channels(&IDENTITY), [1.0, 1.0, 1.0]);
        // more than full is full; a row's sum is what a grey gets
        assert_eq!(channels(&[2.0, 0.0, 0.0, 0.1, 0.2, 0.3, 0.0, 0.0, 0.0]), [1.0, 0.6000000000000001, 0.0]);
        let r = ramp(0.5, 256);
        assert_eq!((r.len(), r[0], r[255]), (256, 0, 32768));
        assert!(r.windows(2).all(|w| w[0] <= w[1]), "never darker for a brighter input");
        assert_eq!(*ramp(1.0, 1024).last().unwrap(), 65535);
    }
}
