//! Turning the displays off and on again through the compositor's output-power protocol (`zwlr_output_power_manager_v1`, which
//! Hyprland and other wlroots-style compositors offer). Where it is missing (KWin), `kscreen-doctor` is asked instead.

use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_output;
use wayland_client::{delegate_noop, QueueHandle};
use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1,
    zwlr_output_power_v1::{Mode, ZwlrOutputPowerV1},
};

use crate::App;

delegate_noop!(App: ignore ZwlrOutputPowerManagerV1);
delegate_noop!(App: ignore ZwlrOutputPowerV1);

pub struct Power {
    /// One control per output (the outputs present at start).
    controls: Vec<ZwlrOutputPowerV1>,
    /// The displays were turned off by us (so the first activity turns them on again).
    pub off: bool,
}

impl Power {
    /// None when the compositor has no output-power protocol.
    pub fn new(globals: &GlobalList, qh: &QueueHandle<App>) -> Option<Power> {
        let mgr: ZwlrOutputPowerManagerV1 = globals.bind(qh, 1..=1, ()).ok()?;
        let outputs: Vec<u32> = globals.contents().with_list(|l| l.iter().filter(|g| g.interface == "wl_output").map(|g| g.name).collect());
        let controls = outputs
            .into_iter()
            .map(|name| {
                let out = globals.registry().bind::<wl_output::WlOutput, _, _>(name, 1, qh, ());
                mgr.get_output_power(&out, qh, ())
            })
            .collect();
        Some(Power { controls, off: false })
    }

    pub fn set(&mut self, on: bool) {
        for c in &self.controls {
            c.set_mode(if on { Mode::On } else { Mode::Off });
        }
        self.off = !on;
    }
}
