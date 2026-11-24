//! ext-workspace-v1: tells other programs (a bar such as waybar, a pager) which workspaces there are and which one shows,
//! and lets them switch. Smithay does not ship this protocol, so it is written here.
//!
//! The workspaces are the compositor's (`state::WORKSPACES`), shared by every screen: one group holds them all and enters
//! every screen the program knows. Each workspace is named and numbered from 1, can be activated, and is marked active while
//! it shows. A program's requests wait for its `commit`, as the protocol says; the last activate before it wins.

use smithay::reexports::wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
    ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use crate::state::{App, WORKSPACES};

/// One program's view: its manager, the group, a handle per workspace, and what it was last told.
struct Bound {
    manager: ExtWorkspaceManagerV1,
    group: ExtWorkspaceGroupHandleV1,
    handles: Vec<ExtWorkspaceHandleV1>,
    active: usize,
    outputs: Vec<WlOutput>,
    /// the workspace an `activate` asked for, applied at `commit`
    asked: Option<usize>,
}

#[derive(Default)]
pub struct Workspaces {
    bound: Vec<Bound>,
}

pub fn create_global(dh: &DisplayHandle) {
    dh.create_global::<App, ExtWorkspaceManagerV1, ()>(1, ());
}

/// The state bits a workspace has: active while it shows.
fn state_of(active: bool) -> ext_workspace_handle_v1::State {
    if active {
        ext_workspace_handle_v1::State::Active
    } else {
        ext_workspace_handle_v1::State::empty()
    }
}

impl App {
    /// The screens a program has bound (wl_output), for the group's output_enter.
    fn outputs_of(&self, client: &Client) -> Vec<WlOutput> {
        self.space.outputs().flat_map(|o| o.client_outputs(client)).collect()
    }

    /// Sends what changed (the workspace that shows, the screens a program has) since last time; called every frame.
    pub fn workspaces_refresh(&mut self) {
        if self.workspaces.bound.is_empty() {
            return;
        }
        let now = self.workspace();
        let mut bound = std::mem::take(&mut self.workspaces.bound);
        for b in bound.iter_mut() {
            let mut changed = false;
            if let Some(client) = b.manager.client() {
                let outputs = self.outputs_of(&client);
                for o in outputs.iter().filter(|o| !b.outputs.contains(o)) {
                    b.group.output_enter(o);
                    changed = true;
                }
                for o in b.outputs.iter().filter(|o| !outputs.contains(o)) {
                    if o.is_alive() {
                        b.group.output_leave(o);
                    }
                    changed = true;
                }
                b.outputs = outputs;
            }
            if b.active != now {
                if let Some(h) = b.handles.get(b.active) {
                    h.state(state_of(false));
                }
                if let Some(h) = b.handles.get(now) {
                    h.state(state_of(true));
                }
                b.active = now;
                changed = true;
            }
            if changed {
                b.manager.done();
            }
        }
        self.workspaces.bound = bound;
    }
}

impl GlobalDispatch<ExtWorkspaceManagerV1, ()> for App {
    fn bind(
        state: &mut Self,
        dh: &DisplayHandle,
        client: &Client,
        resource: New<ExtWorkspaceManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let manager = init.init(resource, ());
        let Ok(group) = client.create_resource::<ExtWorkspaceGroupHandleV1, (), App>(dh, manager.version(), ()) else { return };
        manager.workspace_group(&group);
        // the number of workspaces is the compositor's: none can be made or removed from outside
        group.capabilities(ext_workspace_group_handle_v1::GroupCapabilities::empty());
        let outputs = state.outputs_of(client);
        for o in &outputs {
            group.output_enter(o);
        }
        let active = state.workspace();
        let mut handles = vec![];
        for n in 0..WORKSPACES {
            let Ok(h) = client.create_resource::<ExtWorkspaceHandleV1, usize, App>(dh, manager.version(), n) else { return };
            manager.workspace(&h);
            h.id(format!("{}", n + 1));
            h.name(format!("{}", n + 1));
            h.coordinates((n as u32).to_ne_bytes().to_vec());
            h.state(state_of(n == active));
            h.capabilities(ext_workspace_handle_v1::WorkspaceCapabilities::Activate);
            group.workspace_enter(&h);
            handles.push(h);
        }
        manager.done();
        state.workspaces.bound.push(Bound { manager, group, handles, active, outputs, asked: None });
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        manager: &ExtWorkspaceManagerV1,
        request: ext_workspace_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_workspace_manager_v1::Request::Commit => {
                let asked = state.workspaces.bound.iter_mut().find(|b| &b.manager == manager).and_then(|b| b.asked.take());
                if let Some(n) = asked {
                    state.switch_workspace(n);
                    state.workspaces_refresh();
                }
            }
            ext_workspace_manager_v1::Request::Stop => {
                manager.finished();
                state.workspaces.bound.retain(|b| &b.manager != manager);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for App {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtWorkspaceGroupHandleV1,
        _: ext_workspace_group_handle_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // create_workspace is not offered (no capability); destroy needs nothing here
    }
}

impl Dispatch<ExtWorkspaceHandleV1, usize> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        handle: &ExtWorkspaceHandleV1,
        request: ext_workspace_handle_v1::Request,
        n: &usize,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let ext_workspace_handle_v1::Request::Activate = request {
            if let Some(b) = state.workspaces.bound.iter_mut().find(|b| b.handles.contains(handle)) {
                b.asked = Some(*n);
            }
        }
    }
}
