//! wlr-foreign-toplevel-management: tells the dock, the notch and other shell programs which windows exist (title, app id, whether
//! one is active, minimized, maximized or full screen) and lets them activate, minimize, maximize and close windows. Smithay does not
//! ship this protocol, so it is written here.

use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use wayland_protocols_wlr::foreign_toplevel::v1::server::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use crate::state::App;

/// What a handle was last told, so only changes are sent.
#[derive(Default, Clone, PartialEq)]
struct Told {
    title: String,
    app_id: String,
    state: Vec<u32>,
}

pub struct Entry {
    surface: WlSurface,
    handle: ZwlrForeignToplevelHandleV1,
    told: Told,
}

#[derive(Default)]
pub struct Toplevels {
    managers: Vec<ZwlrForeignToplevelManagerV1>,
    entries: Vec<Entry>,
}

const STATE_MAXIMIZED: u32 = 0;
const STATE_MINIMIZED: u32 = 1;
const STATE_ACTIVATED: u32 = 2;
const STATE_FULLSCREEN: u32 = 3;

pub fn create_global(dh: &DisplayHandle) {
    dh.create_global::<App, ZwlrForeignToplevelManagerV1, ()>(3, ());
}

fn describe(app: &App, window: &Window, minimized: bool) -> Option<Told> {
    let t = window.toplevel()?;
    let (title, app_id) = with_states(t.wl_surface(), |s| {
        let d = s.data_map.get::<XdgToplevelSurfaceData>().map(|d| d.lock().unwrap());
        (d.as_ref().and_then(|d| d.title.clone()).unwrap_or_default(), d.as_ref().and_then(|d| d.app_id.clone()).unwrap_or_default())
    });
    let cur = t.current_state();
    let mut state = vec![];
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as S;
    if cur.states.contains(S::Maximized) {
        state.push(STATE_MAXIMIZED);
    }
    if minimized {
        state.push(STATE_MINIMIZED);
    }
    if app.keyboard.current_focus().map(|f| &f == t.wl_surface()).unwrap_or(false) {
        state.push(STATE_ACTIVATED);
    }
    if cur.states.contains(S::Fullscreen) {
        state.push(STATE_FULLSCREEN);
    }
    Some(Told { title, app_id, state })
}

fn send(handle: &ZwlrForeignToplevelHandleV1, old: &Told, new: &Told) {
    if old.title != new.title {
        handle.title(new.title.clone());
    }
    if old.app_id != new.app_id {
        handle.app_id(new.app_id.clone());
    }
    if old.state != new.state {
        handle.state(new.state.iter().flat_map(|s| s.to_ne_bytes()).collect());
    }
    handle.done();
}

impl App {
    fn new_handle(&mut self, mgr: &ZwlrForeignToplevelManagerV1, window: &Window, minimized: bool) {
        let (Some(client), Some(t)) = (mgr.client(), window.toplevel()) else { return };
        let dh = self.display_handle.clone();
        let Ok(handle) = client.create_resource::<ZwlrForeignToplevelHandleV1, WlSurface, App>(&dh, mgr.version(), t.wl_surface().clone()) else { return };
        mgr.toplevel(&handle);
        let told = describe(self, window, minimized).unwrap_or_default();
        send(&handle, &Told::default(), &told);
        self.toplevels.entries.push(Entry { surface: t.wl_surface().clone(), handle, told });
    }

    /// A window opened: every shell program that asked gets to know it.
    pub fn foreign_added(&mut self, window: &Window) {
        for m in self.toplevels.managers.clone() {
            self.new_handle(&m, window, false);
        }
    }

    /// A window closed.
    pub fn foreign_removed(&mut self, surface: &WlSurface) {
        self.toplevels.entries.retain(|e| {
            if &e.surface == surface {
                e.handle.closed();
                false
            } else {
                true
            }
        });
    }

    /// Send what changed (title, focus, maximized, minimized, ...) since last time.
    pub fn foreign_refresh(&mut self) {
        if self.toplevels.entries.is_empty() {
            return;
        }
        let windows: Vec<(Window, bool)> = self.space.elements().map(|w| (w.clone(), false)).chain(self.minimized.iter().map(|w| (w.clone(), true))).collect();
        let mut entries = std::mem::take(&mut self.toplevels.entries);
        for e in entries.iter_mut() {
            let Some((w, min)) = windows.iter().find(|(w, _)| w.toplevel().map(|t| t.wl_surface() == &e.surface).unwrap_or(false)) else { continue };
            if let Some(new) = describe(self, w, *min) {
                if new != e.told {
                    send(&e.handle, &e.told, &new);
                    e.told = new;
                }
            }
        }
        self.toplevels.entries = entries;
    }
}

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for App {
    fn bind(state: &mut Self, _: &DisplayHandle, _: &Client, resource: New<ZwlrForeignToplevelManagerV1>, _: &(), init: &mut DataInit<'_, Self>) {
        let mgr = init.init(resource, ());
        state.toplevels.managers.push(mgr.clone());
        let all: Vec<(Window, bool)> = state.space.elements().map(|w| (w.clone(), false)).chain(state.minimized.iter().map(|w| (w.clone(), true))).collect();
        for (w, min) in all {
            state.new_handle(&mgr, &w, min);
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for App {
    fn request(state: &mut Self, _: &Client, mgr: &ZwlrForeignToplevelManagerV1, request: zwlr_foreign_toplevel_manager_v1::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            mgr.finished();
            state.toplevels.managers.retain(|m| m != mgr);
        }
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, WlSurface> for App {
    fn request(state: &mut Self, _: &Client, handle: &ZwlrForeignToplevelHandleV1, request: zwlr_foreign_toplevel_handle_v1::Request, surface: &WlSurface, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {
        use zwlr_foreign_toplevel_handle_v1::Request as R;
        match request {
            R::Activate { .. } => state.activate_surface(surface),
            R::Close => state.close_surface(surface),
            R::SetMaximized => state.set_maximized(surface, true),
            R::UnsetMaximized => state.set_maximized(surface, false),
            R::SetMinimized => state.minimize_surface(surface),
            R::UnsetMinimized => state.activate_surface(surface),
            R::SetFullscreen { .. } => state.set_fullscreen(surface, true),
            R::UnsetFullscreen => state.set_fullscreen(surface, false),
            R::Destroy => state.toplevels.entries.retain(|e| &e.handle != handle),
            _ => {}
        }
    }
}
