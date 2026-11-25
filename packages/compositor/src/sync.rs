//! Waiting for a program's GPU before its picture is used.
//!
//! A program draws into a GPU buffer and commits it before the GPU has finished drawing. Mesa's drivers attach the work to
//! the buffer itself (implicit sync): the kernel knows when the buffer is ready. NVIDIA's driver does not, so a compositor that
//! uses the buffer at once may show half-drawn or old frames (flicker, frames out of order). Two ways to wait, both before
//! the commit is applied (a blocker on the surface's transaction, cleared from the event loop, nothing polled):
//!
//! - **Explicit sync** (linux-drm-syncobj-v1, what NVIDIA's Vulkan and OpenGL, Xwayland and newer Mesa use when offered):
//!   the program names a point on a DRM timeline that its GPU signals when the frame is drawn (the acquire point), and one
//!   the compositor signals when it no longer reads the buffer (the release point, which Smithay signals when the last
//!   reference to the buffer goes). Offered only when the GPU can wait on a timeline through an eventfd (`syncobj_eventfd`,
//!   Linux 6.6 and newer), as Smithay asks.
//! - **Implicit sync**, for every other GPU buffer: the commit waits until the buffer's own fences say it can be read.
//!
//! `YGGDRASIL_COMPOSITOR_EXPLICIT_SYNC=0` leaves the protocol out (for a driver that gets it wrong); the implicit wait stays.

use std::os::fd::OwnedFd;
use std::path::Path;

use smithay::backend::drm::DrmDeviceFd;
use smithay::reexports::calloop::Interest;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::utils::DeviceFd;
use smithay::wayland::compositor::{add_blocker, add_pre_commit_hook, with_states, BufferAssignment, CompositorHandler, SurfaceAttributes};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjCachedState, DrmSyncobjHandler, DrmSyncobjState};

use crate::state::App;

/// Whether explicit sync may be offered: yes unless YGGDRASIL_COMPOSITOR_EXPLICIT_SYNC says 0, no, off or false.
pub fn explicit_allowed(var: Option<&str>) -> bool {
    !matches!(var.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("0" | "no" | "off" | "false"))
}

/// Offers linux-drm-syncobj-v1 for the GPU behind `render_node` (the node programs render on), when it can wait on a
/// timeline through an eventfd. Says which on stderr, once.
pub fn offer(app: &mut App, dh: &DisplayHandle, render_node: &Path) {
    if !explicit_allowed(std::env::var("YGGDRASIL_COMPOSITOR_EXPLICIT_SYNC").ok().as_deref()) {
        eprintln!("yggdrasil-compositor: explicit sync left out (YGGDRASIL_COMPOSITOR_EXPLICIT_SYNC)");
        return;
    }
    let file = match std::fs::OpenOptions::new().read(true).write(true).open(render_node) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("yggdrasil-compositor: no explicit sync ({}: {e})", render_node.display());
            return;
        }
    };
    let fd = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(file)));
    if supports_syncobj_eventfd(&fd) {
        app.syncobj_state = Some(DrmSyncobjState::new::<App>(dh, fd));
        eprintln!("yggdrasil-compositor: explicit sync (linux-drm-syncobj-v1) on {}", render_node.display());
    } else {
        eprintln!("yggdrasil-compositor: no explicit sync: {} cannot wait on a timeline (syncobj_eventfd)", render_node.display());
    }
}

impl DrmSyncobjHandler for App {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.syncobj_state.as_mut()
    }
}

/// Makes every commit of `surface` that brings a new GPU buffer wait until the buffer is drawn: its acquire point (explicit)
/// or its fences (implicit). Called for each new surface.
pub fn wait_for_gpu(surface: &WlSurface) {
    add_pre_commit_hook::<App, _>(surface, |app, _dh, surface| {
        let mut acquire = None;
        let dmabuf = with_states(surface, |states| {
            acquire.clone_from(&states.cached_state.get::<DrmSyncobjCachedState>().pending().acquire_point);
            states.cached_state.get::<SurfaceAttributes>().pending().buffer.as_ref().and_then(|b| match b {
                BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).cloned().ok(),
                _ => None,
            })
        });
        let Some(dmabuf) = dmabuf else { return };
        let Some(client) = surface.client() else { return };
        if let Some(point) = acquire {
            if let Ok((blocker, source)) = point.generate_blocker() {
                let c = client.clone();
                let added = app.loop_handle.insert_source(source, move |_, _, app| {
                    let dh = app.display_handle.clone();
                    app.client_compositor_state(&c).blocker_cleared(app, &dh);
                    Ok(())
                });
                if added.is_ok() {
                    add_blocker(surface, blocker);
                    return;
                }
            }
        }
        // no acquire point (or it could not be waited on): the buffer's own fences, when it has any not yet signalled
        if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
            let added = app.loop_handle.insert_source(source, move |_, _, app| {
                let dh = app.display_handle.clone();
                app.client_compositor_state(&client).blocker_cleared(app, &dh);
                Ok(())
            });
            if added.is_ok() {
                add_blocker(surface, blocker);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_sync_is_on_unless_switched_off() {
        assert!(explicit_allowed(None));
        assert!(explicit_allowed(Some("1")));
        assert!(explicit_allowed(Some("")));
        for off in ["0", "no", "OFF", " false "] {
            assert!(!explicit_allowed(Some(off)), "{off}");
        }
    }
}
