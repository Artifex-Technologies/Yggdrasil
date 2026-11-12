//! wlr-output-power-management: yggdrasil-idle turns the screens off after the idle time and on again at the first input
//! (`packages/idle/src/power.rs`; on Hyprland the same protocol). Smithay does not ship it, so it is written here.
//!
//! Only the headless backend offers it today: an output that is off is drawn black and says so in the control socket's `outputs`.
//! The hardware backend will switch the connector off (DPMS) instead.

use smithay::output::Output;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};
use wayland_protocols_wlr::output_power_management::v1::server::{
    zwlr_output_power_manager_v1::{self, ZwlrOutputPowerManagerV1},
    zwlr_output_power_v1::{self, Mode, ZwlrOutputPowerV1},
};

use crate::state::App;

#[derive(Default)]
pub struct OutputPower {
    /// the names of the outputs that are off
    pub off: Vec<String>,
    /// every control a program holds, and its output
    controls: Vec<(ZwlrOutputPowerV1, Output)>,
}

impl OutputPower {
    pub fn is_on(&self, o: &Output) -> bool {
        !self.off.contains(&o.name())
    }
}

pub fn create_global(dh: &DisplayHandle) {
    dh.create_global::<App, ZwlrOutputPowerManagerV1, ()>(1, ());
}

impl App {
    /// Turn an output on or off and tell every program that holds a control for it.
    pub fn set_output_power(&mut self, o: &Output, on: bool) {
        let name = o.name();
        self.power.off.retain(|n| n != &name);
        if !on {
            self.power.off.push(name);
        }
        let mode = if on { Mode::On } else { Mode::Off };
        for (c, out) in &self.power.controls {
            if out == o {
                c.mode(mode);
            }
        }
        self.wake();
    }
}

impl GlobalDispatch<ZwlrOutputPowerManagerV1, ()> for App {
    fn bind(_: &mut Self, _: &DisplayHandle, _: &Client, resource: New<ZwlrOutputPowerManagerV1>, _: &(), init: &mut DataInit<'_, Self>) {
        init.init(resource, ());
    }
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrOutputPowerManagerV1,
        request: zwlr_output_power_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_output_power_manager_v1::Request::GetOutputPower { id, output } = request {
            let control = init.init(id, ());
            match Output::from_resource(&output) {
                Some(o) => {
                    control.mode(if state.power.is_on(&o) { Mode::On } else { Mode::Off });
                    state.power.controls.push((control, o));
                }
                None => control.failed(),
            }
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, ()> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        control: &ZwlrOutputPowerV1,
        request: zwlr_output_power_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_output_power_v1::Request::SetMode { mode } => {
                let Some(o) = state.power.controls.iter().find(|(c, _)| c == control).map(|(_, o)| o.clone()) else { return };
                let on = !matches!(mode.into_result(), Ok(Mode::Off));
                state.set_output_power(&o, on);
            }
            zwlr_output_power_v1::Request::Destroy => state.power.controls.retain(|(c, _)| c != control),
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: smithay::reexports::wayland_server::backend::ClientId, control: &ZwlrOutputPowerV1, _: &()) {
        state.power.controls.retain(|(c, _)| c != control);
    }
}
