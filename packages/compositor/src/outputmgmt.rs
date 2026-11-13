//! wlr-output-management (version 4), the standard protocol programs use to list the screens with their modes and to change
//! their mode, scale, place and whether they are on: wlr-randr, kanshi, and Settings > Displays' kind of tool. Smithay does not
//! ship it, so it is written here. What a program applies goes through `App::configure_outputs` (outputs.rs) like Settings'
//! file, and lasts while the session runs (as on Hyprland: Settings' file is what is kept).
//!
//! Headless, any mode is accepted (a custom mode makes the picture that size); turning (transform) and variable refresh are not
//! offered (a configuration asking for them fails).

use std::sync::{Arc, Mutex};

use smithay::output::Mode;
use smithay::reexports::wayland_server::protocol::wl_output::Transform;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use smithay::utils::{Logical, Point};
use wayland_protocols_wlr::output_management::v1::server::{
    zwlr_output_configuration_head_v1::{self, ZwlrOutputConfigurationHeadV1},
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

use crate::outputs::{Head, Want};
use crate::state::App;

const VERSION: u32 = 4;

pub fn create_global(dh: &DisplayHandle) {
    dh.create_global::<App, ZwlrOutputManagerV1, ()>(VERSION, ());
}

/// A screen as programs are told it.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadState {
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    pub modes: Vec<Mode>,
    pub preferred: Option<Mode>,
    pub enabled: bool,
    pub current: Option<Mode>,
    pub at: Point<i32, Logical>,
    pub scale: f64,
}

impl HeadState {
    pub fn of(h: &Head) -> HeadState {
        let o = &h.output;
        let p = o.physical_properties();
        HeadState {
            name: o.name(),
            description: format!("{} {} ({})", p.make, p.model, o.name()),
            make: p.make,
            model: p.model,
            modes: o.modes(),
            preferred: o.preferred_mode(),
            enabled: h.enabled,
            current: o.current_mode(),
            at: o.current_location(),
            scale: o.current_scale().fractional_scale(),
        }
    }
}

/// One head object a program holds, with its mode objects and what it was told last.
struct BoundHead {
    head: ZwlrOutputHeadV1,
    modes: Vec<(ZwlrOutputModeV1, Mode)>,
    told: HeadState,
}

struct Bound {
    manager: ZwlrOutputManagerV1,
    heads: Vec<BoundHead>,
}

#[derive(Default)]
pub struct Mgmt {
    /// the configuration's number: a program's configuration made for an older one is cancelled
    serial: u32,
    heads: Vec<HeadState>,
    bound: Vec<Bound>,
}

/// What a configuration asks of one head; None: as it is.
#[derive(Default)]
pub struct HeadSet {
    mode: Option<Mode>,
    scale: Option<f64>,
    at: Option<Point<i32, Logical>>,
    /// turned, or variable refresh on: not offered here
    unsupported: bool,
}

pub struct Config {
    serial: u32,
    /// every head named, and what it is to be (None: off)
    heads: Vec<(String, Option<Arc<Mutex<HeadSet>>>)>,
    used: bool,
}

fn mode_object(dh: &DisplayHandle, client: &Client, head: &ZwlrOutputHeadV1, m: Mode, preferred: bool) -> Option<ZwlrOutputModeV1> {
    let name = head.data::<String>().cloned().unwrap_or_default();
    let mode = client.create_resource::<ZwlrOutputModeV1, (String, Mode), App>(dh, head.version(), (name, m)).ok()?;
    head.mode(&mode);
    mode.size(m.size.w, m.size.h);
    if m.refresh > 0 {
        mode.refresh(m.refresh);
    }
    if preferred {
        mode.preferred();
    }
    Some(mode)
}

/// Its place, mode and scale, when it is on (a head that is off says only that).
fn tell_current(b: &BoundHead, s: &HeadState) {
    if !s.enabled {
        return;
    }
    if let Some((m, _)) = b.modes.iter().find(|(_, m)| Some(*m) == s.current) {
        b.head.current_mode(m);
    }
    b.head.position(s.at.x, s.at.y);
    b.head.transform(Transform::Normal);
    b.head.scale(s.scale);
}

fn introduce(dh: &DisplayHandle, client: &Client, manager: &ZwlrOutputManagerV1, s: &HeadState) -> Option<BoundHead> {
    let head = client.create_resource::<ZwlrOutputHeadV1, String, App>(dh, manager.version(), s.name.clone()).ok()?;
    manager.head(&head);
    head.name(s.name.clone());
    head.description(s.description.clone());
    head.physical_size(0, 0);
    let modes = s.modes.iter().filter_map(|m| Some((mode_object(dh, client, &head, *m, s.preferred == Some(*m))?, *m))).collect();
    head.enabled(s.enabled as i32);
    if head.version() >= 2 {
        head.make(s.make.clone());
        head.model(s.model.clone());
    }
    if head.version() >= 4 {
        head.adaptive_sync(zwlr_output_head_v1::AdaptiveSyncState::Disabled);
    }
    let b = BoundHead { head, modes, told: s.clone() };
    tell_current(&b, s);
    Some(b)
}

/// Tell a held head what changed.
fn update(dh: &DisplayHandle, client: &Client, b: &mut BoundHead, s: &HeadState) {
    for m in &s.modes {
        if !b.modes.iter().any(|(_, x)| x == m) {
            if let Some(o) = mode_object(dh, client, &b.head, *m, s.preferred == Some(*m)) {
                b.modes.push((o, *m));
            }
        }
    }
    b.modes.retain(|(o, m)| {
        let keep = s.modes.contains(m);
        if !keep {
            o.finished();
        }
        keep
    });
    if b.told.enabled != s.enabled {
        b.head.enabled(s.enabled as i32);
    }
    if b.told != *s {
        tell_current(b, s);
    }
    b.told = s.clone();
}

impl Mgmt {
    /// The screens now: every program that holds the manager is told what changed, then `done` with a new serial.
    pub fn changed(&mut self, dh: &DisplayHandle, heads: &[HeadState]) {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.heads = heads.to_vec();
        for bound in &mut self.bound {
            let Some(client) = bound.manager.client() else { continue };
            bound.heads.retain(|b| {
                let keep = heads.iter().any(|s| s.name == b.told.name);
                if !keep {
                    for (m, _) in &b.modes {
                        m.finished();
                    }
                    b.head.finished();
                }
                keep
            });
            for s in heads {
                match bound.heads.iter_mut().find(|b| b.told.name == s.name) {
                    Some(b) => update(dh, &client, b, s),
                    None => {
                        if let Some(b) = introduce(dh, &client, &bound.manager, s) {
                            bound.heads.push(b);
                        }
                    }
                }
            }
            bound.manager.done(self.serial);
        }
    }
}

impl GlobalDispatch<ZwlrOutputManagerV1, ()> for App {
    fn bind(
        state: &mut Self,
        dh: &DisplayHandle,
        client: &Client,
        resource: New<ZwlrOutputManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let manager = init.init(resource, ());
        let heads = state.heads.mgmt.heads.iter().filter_map(|s| introduce(dh, client, &manager, s)).collect();
        manager.done(state.heads.mgmt.serial);
        state.heads.mgmt.bound.push(Bound { manager, heads });
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        manager: &ZwlrOutputManagerV1,
        request: zwlr_output_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_output_manager_v1::Request::CreateConfiguration { id, serial } => {
                init.init(id, Mutex::new(Config { serial, heads: vec![], used: false }));
            }
            zwlr_output_manager_v1::Request::Stop => {
                state.heads.mgmt.bound.retain(|b| b.manager != *manager);
                manager.finished();
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: smithay::reexports::wayland_server::backend::ClientId, manager: &ZwlrOutputManagerV1, _: &()) {
        state.heads.mgmt.bound.retain(|b| b.manager != *manager);
    }
}

impl Dispatch<ZwlrOutputHeadV1, String> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        head: &ZwlrOutputHeadV1,
        request: zwlr_output_head_v1::Request,
        _: &String,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_output_head_v1::Request::Release = request {
            for b in &mut state.heads.mgmt.bound {
                b.heads.retain(|h| h.head != *head);
            }
        }
    }
}

impl Dispatch<ZwlrOutputModeV1, (String, Mode)> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        mode: &ZwlrOutputModeV1,
        request: zwlr_output_mode_v1::Request,
        _: &(String, Mode),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_output_mode_v1::Request::Release = request {
            for b in &mut state.heads.mgmt.bound {
                for h in &mut b.heads {
                    h.modes.retain(|(m, _)| m != mode);
                }
            }
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, Mutex<Config>> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        config: &ZwlrOutputConfigurationV1,
        request: zwlr_output_configuration_v1::Request,
        data: &Mutex<Config>,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_output_configuration_v1::{Error, Request};
        let mut c = data.lock().unwrap();
        let named = |c: &Config, head: &ZwlrOutputHeadV1| {
            let name = head.data::<String>().cloned().unwrap_or_default();
            let twice = c.heads.iter().any(|(n, _)| *n == name);
            (name, twice)
        };
        match request {
            Request::EnableHead { id, head } => {
                let set = Arc::new(Mutex::new(HeadSet::default()));
                let (name, twice) = named(&c, &head);
                init.init(id, (name.clone(), set.clone()));
                if c.used {
                    config.post_error(Error::AlreadyUsed, "the configuration was already applied or tested");
                } else if twice {
                    config.post_error(Error::AlreadyConfiguredHead, "that head is already in the configuration");
                } else {
                    c.heads.push((name, Some(set)));
                }
            }
            Request::DisableHead { head } => {
                let (name, twice) = named(&c, &head);
                if c.used {
                    config.post_error(Error::AlreadyUsed, "the configuration was already applied or tested");
                } else if twice {
                    config.post_error(Error::AlreadyConfiguredHead, "that head is already in the configuration");
                } else {
                    c.heads.push((name, None));
                }
            }
            Request::Apply | Request::Test => {
                if c.used {
                    config.post_error(Error::AlreadyUsed, "the configuration was already applied or tested");
                    return;
                }
                c.used = true;
                if c.serial != state.heads.mgmt.serial {
                    config.cancelled();
                    return;
                }
                let mut changes = vec![];
                for h in &state.heads.list {
                    let name = h.output.name();
                    let Some((_, set)) = c.heads.iter().find(|(n, _)| *n == name) else {
                        config.post_error(Error::UnconfiguredHead, format!("{name} is not in the configuration"));
                        return;
                    };
                    let now = h.now();
                    let want = match set {
                        None => Want { enabled: false, ..now },
                        Some(set) => {
                            let s = set.lock().unwrap();
                            if s.unsupported {
                                config.failed();
                                return;
                            }
                            Want {
                                enabled: true,
                                mode: s.mode.unwrap_or(now.mode),
                                scale: s.scale.unwrap_or(now.scale),
                                at: s.at.or(now.at),
                            }
                        }
                    };
                    changes.push((name, want));
                }
                let test = matches!(request, Request::Test);
                match state.configure_outputs(changes, test) {
                    Ok(()) => config.succeeded(),
                    Err(e) => {
                        eprintln!("yggdrasil-compositor: a program's screen configuration was refused: {e}");
                        config.failed();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, (String, Arc<Mutex<HeadSet>>)> for App {
    fn request(
        _: &mut Self,
        _: &Client,
        res: &ZwlrOutputConfigurationHeadV1,
        request: zwlr_output_configuration_head_v1::Request,
        data: &(String, Arc<Mutex<HeadSet>>),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use zwlr_output_configuration_head_v1::{Error, Request};
        let mut s = data.1.lock().unwrap();
        match request {
            Request::SetMode { mode } => match mode.data::<(String, Mode)>() {
                Some((head, m)) if *head == data.0 => {
                    if s.mode.is_some() {
                        res.post_error(Error::AlreadySet, "the mode is already set");
                    }
                    s.mode = Some(*m);
                }
                _ => res.post_error(Error::InvalidMode, "that mode is another head's"),
            },
            Request::SetCustomMode { width, height, refresh } => {
                if width <= 0 || height <= 0 || refresh < 0 {
                    res.post_error(Error::InvalidCustomMode, "a mode needs a size, and a refresh rate of 0 or more");
                } else if s.mode.is_some() {
                    res.post_error(Error::AlreadySet, "the mode is already set");
                } else {
                    s.mode = Some(Mode { size: (width, height).into(), refresh: if refresh > 0 { refresh } else { 60_000 } });
                }
            }
            Request::SetPosition { x, y } => {
                if s.at.is_some() {
                    res.post_error(Error::AlreadySet, "the position is already set");
                }
                s.at = Some((x, y).into());
            }
            Request::SetTransform { transform } => match transform.into_result() {
                Ok(Transform::Normal) => {}
                Ok(_) => s.unsupported = true,
                Err(_) => res.post_error(Error::InvalidTransform, "not a transform"),
            },
            Request::SetScale { scale } => {
                if scale.is_nan() || scale <= 0.0 {
                    res.post_error(Error::InvalidScale, "the scale must be more than 0");
                } else if s.scale.is_some() {
                    res.post_error(Error::AlreadySet, "the scale is already set");
                } else {
                    s.scale = Some(scale);
                }
            }
            Request::SetAdaptiveSync { state } => {
                if matches!(state.into_result(), Ok(zwlr_output_head_v1::AdaptiveSyncState::Enabled)) {
                    s.unsupported = true;
                }
            }
            _ => {}
        }
    }
}
