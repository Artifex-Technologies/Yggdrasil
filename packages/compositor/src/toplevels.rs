//! wlr-foreign-toplevel-management: tells the dock, the notch and other shell programs which windows exist (title, app id, whether
//! one is active, minimized, maximized or full screen) and lets them activate, minimize, maximize and close windows. Smithay does not
//! ship this protocol, so it is written here.
//!
//! The standard list, ext-foreign-toplevel-list (title and app id only), comes from Smithay and is kept here alongside: its handles
//! name the windows for the capture protocols (capture.rs: the panel's window previews).

use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use wayland_protocols_wlr::foreign_toplevel::v1::server::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use smithay::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::server::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1;
use smithay::wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelListHandler, ForeignToplevelListState};

use crate::state::{self, App};

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
    /// each window's handle in the standard list (ext-foreign-toplevel-list)
    listed: Vec<(WlSurface, ForeignToplevelHandle)>,
}

const STATE_MAXIMIZED: u32 = 0;
const STATE_MINIMIZED: u32 = 1;
const STATE_ACTIVATED: u32 = 2;
const STATE_FULLSCREEN: u32 = 3;

pub fn create_global(dh: &DisplayHandle) {
    dh.create_global::<App, ZwlrForeignToplevelManagerV1, ()>(3, ());
}

fn describe(app: &App, window: &Window, minimized: bool) -> Option<Told> {
    // Wayland and X11 windows alike (an X11 window is listed once XWayland has made its surface)
    let surface = state::surface_of(window)?;
    let (title, app_id) = state::title_app(window);
    let mut state = vec![];
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as S;
    if state::win_has(window, S::Maximized) {
        state.push(STATE_MAXIMIZED);
    }
    if minimized {
        state.push(STATE_MINIMIZED);
    }
    if app.focused_surface().as_ref() == Some(&surface) {
        state.push(STATE_ACTIVATED);
    }
    if state::win_has(window, S::Fullscreen) {
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
        let (Some(client), Some(surface)) = (mgr.client(), state::surface_of(window)) else { return };
        let dh = self.display_handle.clone();
        let Ok(handle) = client.create_resource::<ZwlrForeignToplevelHandleV1, WlSurface, App>(&dh, mgr.version(), surface.clone()) else {
            return;
        };
        mgr.toplevel(&handle);
        let told = describe(self, window, minimized).unwrap_or_default();
        send(&handle, &Told::default(), &told);
        self.toplevels.entries.push(Entry { surface, handle, told });
    }

    /// A window opened: every shell program that asked gets to know it.
    pub fn foreign_added(&mut self, window: &Window) {
        for m in self.toplevels.managers.clone() {
            self.new_handle(&m, window, false);
        }
        if let Some(surface) = state::surface_of(window) {
            if !self.toplevels.listed.iter().any(|(s, _)| *s == surface) {
                let (title, app_id) = state::title_app(window);
                let handle = self.toplevel_list.new_toplevel::<App>(title, app_id);
                self.toplevels.listed.push((surface, handle));
            }
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
        if let Some(i) = self.toplevels.listed.iter().position(|(s, _)| s == surface) {
            let (_, handle) = self.toplevels.listed.remove(i);
            self.toplevel_list.remove_toplevel(&handle);
        }
    }

    /// The window an ext-foreign-toplevel-list handle names (None when it closed).
    pub fn window_of_list_handle(&self, handle: &ExtForeignToplevelHandleV1) -> Option<WlSurface> {
        let h = ForeignToplevelHandle::from_resource(handle)?;
        let id = h.identifier();
        self.toplevels.listed.iter().find(|(_, x)| x.identifier() == id).map(|(s, _)| s.clone())
    }

    /// A window's title and app id in the standard list, when they changed (after every commit of the window, and every frame).
    pub fn list_refresh(&self, surface: &WlSurface) {
        let Some((_, handle)) = self.toplevels.listed.iter().find(|(s, _)| s == surface) else { return };
        let Some(w) = self.window_for(surface) else { return };
        let (title, app_id) = state::title_app(&w);
        let (t, a) = (handle.title() != title, handle.app_id() != app_id);
        if t {
            handle.send_title(&title);
        }
        if a {
            handle.send_app_id(&app_id);
        }
        if t || a {
            handle.send_done();
        }
    }

    /// Send what changed (title, focus, maximized, minimized, ...) since last time.
    pub fn foreign_refresh(&mut self) {
        // the standard list: titles and app ids
        for (surface, _) in &self.toplevels.listed {
            self.list_refresh(surface);
        }
        if self.toplevels.entries.is_empty() {
            return;
        }
        let windows: Vec<(Window, bool)> = self.all_windows();
        let mut entries = std::mem::take(&mut self.toplevels.entries);
        for e in entries.iter_mut() {
            let Some((w, min)) = windows.iter().find(|(w, _)| state::is_window_of(w, &e.surface)) else { continue };
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
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let mgr = init.init(resource, ());
        state.toplevels.managers.push(mgr.clone());
        let all: Vec<(Window, bool)> = state.all_windows();
        for (w, min) in all {
            state.new_handle(&mgr, &w, min);
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        mgr: &ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            mgr.finished();
            state.toplevels.managers.retain(|m| m != mgr);
        }
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, WlSurface> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        handle: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        surface: &WlSurface,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
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

impl ForeignToplevelListHandler for App {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.toplevel_list
    }
}
smithay::delegate_foreign_toplevel_list!(App);
