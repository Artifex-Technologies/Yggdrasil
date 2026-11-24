//! ext-workspace-v1: tells other programs (a bar such as waybar, a pager) which workspaces there are and which one shows,
//! and lets them switch. Smithay does not ship this protocol, so it is written here.
//!
//! The workspaces are the compositor's, shared by every screen: one group holds them all and enters every screen the program
//! knows. Each workspace is named and numbered from 1, can be activated, and is marked active while it shows. Workspaces 1 to 4
//! are always there; one past them made (shown, a window sent there, or the group's `create_workspace`) is announced, and one
//! gone again (left empty) is removed (`state::WORKSPACES`). A program's requests wait for its `commit`, as the protocol says;
//! the last activate before it wins, after a create.

use smithay::reexports::wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
    ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use crate::state::App;

/// One program's view: its manager, the group, a handle per workspace, and what it was last told.
struct Bound {
    manager: ExtWorkspaceManagerV1,
    group: ExtWorkspaceGroupHandleV1,
    handles: Vec<ExtWorkspaceHandleV1>,
    active: usize,
    outputs: Vec<WlOutput>,
    /// the workspace an `activate` asked for, applied at `commit`
    asked: Option<usize>,
    /// a `create_workspace` waiting for `commit`
    create: bool,
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

/// A handle for workspace `n` (0-based), told to the program and entered into its group.
fn announce(dh: &DisplayHandle, client: &Client, b: &Bound, n: usize, active: bool) -> Option<ExtWorkspaceHandleV1> {
    let h = client.create_resource::<ExtWorkspaceHandleV1, usize, App>(dh, b.manager.version(), n).ok()?;
    b.manager.workspace(&h);
    h.id(format!("{}", n + 1));
    h.name(format!("{}", n + 1));
    h.coordinates((n as u32).to_ne_bytes().to_vec());
    h.state(state_of(active));
    h.capabilities(ext_workspace_handle_v1::WorkspaceCapabilities::Activate);
    b.group.workspace_enter(&h);
    Some(h)
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
        let count = self.workspace_count();
        let dh = self.display_handle.clone();
        let mut bound = std::mem::take(&mut self.workspaces.bound);
        for b in bound.iter_mut() {
            let mut changed = false;
            // workspaces made and gone since: new handles, and the gone ones left and removed
            while b.handles.len() > count {
                if let Some(h) = b.handles.pop() {
                    b.group.workspace_leave(&h);
                    h.removed();
                }
                changed = true;
            }
            if let Some(client) = b.manager.client() {
                // told as not shown: when one is shown, the switch below tells both it and the one left
                while b.handles.len() < count {
                    let n = b.handles.len();
                    let Some(h) = announce(&dh, &client, b, n, false) else { break };
                    b.handles.push(h);
                    changed = true;
                }
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
        group.capabilities(ext_workspace_group_handle_v1::GroupCapabilities::CreateWorkspace);
        let outputs = state.outputs_of(client);
        for o in &outputs {
            group.output_enter(o);
        }
        let active = state.workspace();
        let mut b = Bound { manager, group, handles: vec![], active, outputs, asked: None, create: false };
        for n in 0..state.workspace_count() {
            let Some(h) = announce(dh, client, &b, n, n == active) else { return };
            b.handles.push(h);
        }
        b.manager.done();
        state.workspaces.bound.push(b);
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
                let Some(b) = state.workspaces.bound.iter_mut().find(|b| &b.manager == manager) else { return };
                let (create, asked) = (std::mem::take(&mut b.create), b.asked.take());
                if create {
                    state.make_workspace();
                }
                if let Some(n) = asked {
                    state.switch_workspace(n);
                }
                if create || asked.is_some() {
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
        state: &mut Self,
        _: &Client,
        group: &ExtWorkspaceGroupHandleV1,
        request: ext_workspace_group_handle_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // a new workspace past the last, at the next commit (its name is its number, whatever was asked); destroy needs nothing
        if let ext_workspace_group_handle_v1::Request::CreateWorkspace { .. } = request {
            if let Some(b) = state.workspaces.bound.iter_mut().find(|b| &b.group == group) {
                b.create = true;
            }
        }
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
