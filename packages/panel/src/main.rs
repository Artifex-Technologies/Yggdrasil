//! labyrinth-panel: Labyrinth's top bar and dock, written for Wayland (wlr-layer-shell) and drawn
//! on the CPU. Windows come from KWin through the plasma-window-management protocol, which KWin
//! only gives to programs whose desktop file asks for it (labyrinth-panel.desktop).
//!
//! One full bar on every screen except the tiling screen (which gets a slim one), and the dock on
//! the main screen. Which screen is which comes from KDE (kscreen priority, the tiling setting);
//! it is re-read every few seconds, so changing it in Settings takes effect live.

mod a11y;
mod apps;
mod paint;
mod calendar;
mod convo;
mod devices;
mod fylgja;
mod notify;
mod osd;
mod stats;
mod voicekey;
mod voicewave;
mod shell;
mod status;
mod tray;
mod trayui;

use std::time::Instant;
use std::collections::HashMap;
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::process::Command;
use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use memmap2::MmapMut;
use resvg::tiny_skia::Pixmap;
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use xkbcommon::xkb;
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{delegate_noop, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{self, WpFractionalScaleV1};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols_plasma::plasma_shell::client::org_kde_plasma_shell::OrgKdePlasmaShell;
use wayland_protocols_plasma::plasma_shell::client::org_kde_plasma_surface::{self, OrgKdePlasmaSurface, Role};
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window::{self, OrgKdePlasmaWindow};
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management::{self, OrgKdePlasmaWindowManagement};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1};

use labyrinth_conf::{DockPos, PanelConf, StartTheme};
use shell::{Action, Hit, Model, Win};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Bar { slim: bool },
    /// A strip along the dock's screen edge, as wide as the screen (or as tall): the dock is drawn in it
    /// and only the dock itself takes clicks. Like the bar, a surface that spans its edge sits flush
    /// against it, which a small centred surface with its own exclusive zone does not under KWin.
    /// A strip along the dock's screen edge, as wide as the screen (or as tall): the dock is drawn in it
    /// and only the dock itself takes clicks. Like the bar (and Plasma's own panels) the strip spans its
    /// edge and reserves its thickness, so windows keep clear of the dock.
    Dock,
    /// An invisible layer-shell strip along the dock's screen edge. It only reserves the dock's room so
    /// windows keep clear of it (KWin takes that from layer surfaces, not from panel windows).
    Strut,
    /// A popover (the calendar): a transparent surface over the whole screen, so a click anywhere
    /// outside the card closes it. The card is drawn at the top right.
    Popup,
    /// The wallpaper, when Labyrinth draws it itself (own_wallpaper): a background layer under everything.
    Wallpaper,
    /// The volume / brightness pop-up: a small pill a little above the dock.
    Osd,
    /// Notification pop-ups, stacked at the bottom right of the main screen.
    Toast,
    /// The voice island: grows out of the notch while Sarina listens, thinks or speaks.
    Voice,
}

/// The panel's own D-Bus service, so a key press elsewhere (the Super key, through KWin) can reach it:
/// `busctl --user call org.labyrinth.Panel /Panel org.labyrinth.Panel ToggleStart`.
struct PanelBus(calloop::channel::Sender<BusMsg>);

/// What a request over D-Bus asks the panel to do.
#[derive(Clone, Copy, Debug)]
enum BusMsg {
    ToggleStart,
    ShowClipboard,
    AskSarina,
    ToggleCalendar,
    FylgjaSection(u32),
}

#[zbus::interface(name = "org.labyrinth.Panel")]
impl PanelBus {
    /// Open the start menu, or close it if it is open.
    fn toggle_start(&self) {
        let _ = self.0.send(BusMsg::ToggleStart);
    }

    /// Open Fylgja (the notch) on the clipboard history, or close it.
    fn show_clipboard(&self) {
        let _ = self.0.send(BusMsg::ShowClipboard);
    }

    /// Start recording a question for Sarina; call again to send it.
    fn ask_sarina(&self) {
        let _ = self.0.send(BusMsg::AskSarina);
    }

    /// Open the calendar (with the Notification Center pane when that is on), or close it.
    fn toggle_calendar(&self) {
        let _ = self.0.send(BusMsg::ToggleCalendar);
    }

    /// Open Fylgja on a section (0 Controls, 1 Today, 2 Stats, 3 Clipboard, 4 Notifications, 5 Wallpaper, 6 Power), or close it.
    fn show_fylgja(&self, section: u32) {
        let _ = self.0.send(BusMsg::FylgjaSection(section));
    }
}

/// A popover and what it shows.
struct Popup {
    output: u32,
    kind: PopupKind,
}

enum PopupKind {
    /// The calendar, this many months from the current one.
    Calendar { offset: i32 },
    Start(StartState),
    /// The tray: an app's menu, or the hidden icons.
    Tray(trayui::TrayPop),
    /// Fylgja, the notch, on this section.
    Fylgja(fylgja::FyState),
}

/// The start menu's state: what was typed, how far its lists are scrolled, and (Folders theme) which
/// folder is open at the side and whether the power choices are showing.
#[derive(Default)]
struct StartState {
    query: String,
    scroll: i32,
    /// Keyboard: where Tab has got to, and the selected row in the list, the folder at the side, and the power choices.
    focus: shell::StartFocus,
    sel: usize,
    sub_sel: usize,
    power_sel: usize,
    cat: Option<usize>,
    sub_scroll: i32,
    power_open: bool,
}

struct Buffer {
    buffer: WlBuffer,
    map: MmapMut,
    w: u32,
    h: u32,
    busy: Arc<AtomicBool>,
}

/// The wayland objects of the dock window.
struct DockWin {
    xs: XdgSurface,
    tl: XdgToplevel,
    ps: OrgKdePlasmaSurface,
}

struct Surf {
    id: usize,
    kind: Kind,
    /// Global name of the output this sits on.
    output: u32,
    wl: WlSurface,
    layer: Option<ZwlrLayerSurfaceV1>,
    /// The dock is a Plasma panel window (xdg toplevel with the panel role), not a layer surface.
    dock: Option<DockWin>,
    /// Logical size the compositor gave us, and the size we asked for.
    w: u32,
    h: u32,
    want: (u32, u32),
    /// The dock's float distance last sent to the compositor.
    gap: i32,
    configured: bool,
    bufs: Vec<Buffer>,
    hits: Vec<Hit>,
    hover: Option<usize>,
    /// Fractional scaling: the scale object and the viewport that lets the buffer be a fractional multiple of the surface.
    frac: Option<(WpFractionalScaleV1, WpViewport)>,
    /// The scale the compositor prefers for this surface (0 until it says).
    fscale: f32,
    /// What the wallpaper surface last drew (picture, width, height), so it is not redrawn for nothing.
    stamp: Option<(String, u32, u32)>,
}

impl Surf {
    fn destroy(self) {
        if let Some((f, v)) = &self.frac {
            f.destroy();
            v.destroy();
        }
        if let Some(l) = &self.layer {
            l.destroy();
        }
        if let Some(d) = &self.dock {
            d.ps.destroy();
            d.tl.destroy();
            d.xs.destroy();
        }
        self.wl.destroy();
    }
}

struct Out {
    wl: WlOutput,
    name: String,
    scale: i32,
    ready: bool,
    /// Where the compositor says this output is in the global space, in logical pixels (x, y, width, height), from xdg-output.
    logical: Option<(i32, i32, u32, u32)>,
    xdg: Option<ZxdgOutputV1>,
}

struct Assets {
    fonts: paint::Fonts,
    icons: paint::Icons,
}

/// Which screen does what, as KDE has it configured.
#[derive(Clone, Debug, Default, PartialEq)]
struct Layout {
    primary: String,
    tiling: Vec<String>,
    /// Each enabled output's place in the global space, in logical pixels (name, x, y, width, height), for
    /// placing the dock.
    sizes: Vec<(String, i32, i32, u32, u32)>,
}

struct State {
    qh: QueueHandle<State>,
    conn: Connection,
    compositor: WlCompositor,
    layer_shell: ZwlrLayerShellV1,
    xdg_wm: Option<XdgWmBase>,
    xdg_output_mgr: Option<ZxdgOutputManagerV1>,
    frac_mgr: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    plasma_shell: Option<OrgKdePlasmaShell>,
    shm: WlShm,
    model: Model,
    /// The panel for screen readers (AT-SPI); None when switched off with LABYRINTH_A11Y=off.
    a11y: Option<a11y::A11y>,
    assets: Assets,
    surfs: Vec<Surf>,
    next_surf: usize,
    outputs: HashMap<u32, Out>,
    layout: Layout,
    built_for: Option<Layout>,
    pointer_surface: Option<usize>,
    cursor: Option<WpCursorShapeDeviceV1>,
    cursor_mgr: Option<WpCursorShapeManagerV1>,
    seat: Option<WlSeat>,
    next_key: u32,
    dirty: bool,
    /// The calendar popover, when open: its output and how many months from this one it shows.
    popup: Option<Popup>,
    /// The popover card's rectangle in logical pixels.
    popup_card: (f32, f32, f32, f32),
    /// The dock app the pointer rests on, since when, where (screen x of its icon) and on which output; opens the window list after a second.
    dock_hover: Option<(String, Instant, f32, u32)>,
    /// When the window list should close if the pointer has not come back to it or the dock.
    preview_close: Option<Instant>,
    /// What can be clicked in the open start menu, and how far its list can scroll.
    start_hits: Vec<shell::StartHit>,
    start_max: i32,
    /// What can be clicked in Fylgja (the notch), and what its poller last read.
    fy_hits: Vec<fylgja::FHit>,
    fy_active: Arc<AtomicBool>,
    fy_shared: Arc<std::sync::Mutex<fylgja::Snapshot>>,
    /// A volume slider being dragged: which one, its rectangle, and the last level and time sent.
    drag: Option<(fylgja::Slider, (f32, f32, f32, f32))>,
    drag_sent: (std::time::Instant, u32),
    /// Nothing was playing when the small card was last asked for: do not ask again for a few seconds.
    peek_blocked: Option<std::time::Instant>,
    /// The volume or brightness pop-up being shown, and a count that tells its hide timer if it is the latest.
    osd: Option<osd::OsdState>,
    osd_gen: u64,
    osd_hide_tx: calloop::channel::Sender<u64>,
    /// Notification pop-ups on screen (ids, oldest first), and how their hide timers report.
    /// The Notification Center pane (when on): what can be clicked in its list, and where the list starts.
    pane_hits: Vec<(notify::Rect, notify::NAct)>,
    pane_top: f32,
    toasts: Vec<u32>,
    /// What can be clicked in the tray pop-up, and the sender the tray threads report through.
    tray_hits: Vec<(trayui::Rect, trayui::RowAct)>,
    tray_tx: Option<calloop::channel::Sender<tray::TrayMsg>>,
    /// Wallpaper pictures already scaled to a screen's size.
    wall_cache: Vec<((String, u32, u32), Pixmap)>,
    toast_hide_tx: calloop::channel::Sender<u32>,
    /// When Minos's generated icon file last changed (it follows the accent colour).
    icon_stamp: Option<std::time::SystemTime>,
    /// The icon pack chosen in KDE, to notice a change.
    icon_pack: Option<String>,
    /// The pointer is on the notch's tab (kept even when the small card was put away).
    tab_hover: bool,
    stats_shared: Arc<std::sync::Mutex<stats::StatsSnap>>,
    stats_wanted: Arc<AtomicBool>,
    /// Resting the pointer on the notch's tab opens Fylgja after a short delay: the count of the
    /// latest rest, whether the pointer is still on the tab, and its screen.
    tab_tx: calloop::channel::Sender<u64>,
    tab_gen: u64,
    tab_on: bool,
    tab_output: u32,
    /// A question for Sarina being recorded, and when it began; her answer comes back on `voice_tx`.
    rec: Option<(std::process::Child, std::path::PathBuf)>,
    rec_since: Option<std::time::Instant>,
    /// The voice island's state while it is showing, and whether its animation ticker should keep going.
    viz: Option<voicewave::Viz>,
    viz_alive: Arc<AtomicBool>,
    viz_tx: calloop::channel::Sender<()>,
    /// Sarina has been asked to speak (when); her loudness comes from the envelope file the speaking script writes.
    speech_since: Option<std::time::Instant>,
    speech_env: Option<(Vec<u8>, i64)>,
    speech_env_mtime: Option<std::time::SystemTime>,
    /// The speaking script was seen running (so its end means she is done).
    speech_seen: bool,
    /// Conversation: a tap listens until you stop talking, and after her answer it listens again; a hold is one question.
    conv: bool,
    /// The current recording ends by itself when you stop talking (a tap), instead of when the key comes up (a hold).
    vad_mode: bool,
    vad: convo::Vad,
    key_down_at: Option<std::time::Instant>,
    /// The key went down while a conversation was listening (a tap then stops it).
    down_in_conv: bool,
    /// The conversation's id (her session continues across its turns), and how many turns in a row had nothing to hear.
    conv_id: Option<String>,
    conv_misses: u32,
    last_tick: Option<std::time::Instant>,
    voice_tx: calloop::channel::Sender<Result<String, String>>,
    /// What Sarina understood of the question, sent as soon as it is known (before her answer).
    heard_tx: calloop::channel::Sender<String>,
    /// What was copied (in memory only) and the process that watches the clipboard.
    clips: Arc<std::sync::Mutex<Vec<String>>>,
    clip_child: Option<std::process::Child>,
    thumbs_started: bool,
    art_for: Option<String>,
    /// Fylgja sliding down (opening) or up (closing): when it began, and ticks that redraw it.
    fy_anim: Option<(std::time::Instant, bool)>,
    anim_tx: calloop::channel::Sender<()>,
    /// Turns key presses into letters (set when the compositor sends its keymap).
    xkb: Option<xkb::State>,
    pointer_pos: (f32, f32),
    /// The output of the surface that was just clicked.
    click_output: Option<u32>,
    keyboard: Option<WlKeyboard>,
    /// The scroll came from a wheel (counted in steps) rather than smoothly.
    axis_wheel: bool,
    axis_acc: f32,
}

// ------------------------------------------------------------------ small helpers

fn strftime_now(fmt: &std::ffi::CStr) -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let mut buf = [0u8; 64];
        let n = libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), fmt.as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).to_string()
    }
}

/// Today as (year, month, day).
fn today_ymd() -> (i32, u32, u32) {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32)
    }
}

/// The signed-in user's full name (or login) for the start menu.
fn user_name() -> String {
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return std::env::var("USER").unwrap_or_default();
        }
        let gecos = std::ffi::CStr::from_ptr((*pw).pw_gecos).to_string_lossy().to_string();
        let first = gecos.split(',').next().unwrap_or("").trim().to_string();
        if first.is_empty() { std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().to_string() } else { first }
    }
}

/// The time for the bar: 24 hours ("16:41"), or 12 with AM/PM.
fn clock_text(h24: bool) -> String {
    strftime_now(if h24 { c"%H:%M" } else { c"%-I:%M %p" })
}

/// The time for the calendar: 24 hours with seconds ("16:41:23"), or 12 with AM/PM.
fn secs_text(h24: bool) -> String {
    strftime_now(if h24 { c"%H:%M:%S" } else { c"%-I:%M %p" })
}

/// The date with the day of the week: "Sun 19 Dec".
fn date_text() -> String {
    strftime_now(c"%a %-d %b")
}

fn command_output(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

/// The main screen (kscreen priority 1) and the tiling screens, from KDE.
/// Whether kscreen reports an output as in use. Plasma 6.8's `kscreen-doctor -j` no longer has an `enabled` key (it was
/// there before): a connected output with a current mode is the same thing.
fn output_enabled(o: &serde_json::Value) -> bool {
    o["enabled"].as_bool().unwrap_or_else(|| o["connected"].as_bool().unwrap_or(false) && o["currentModeId"].as_str().map(|m| !m.is_empty()).unwrap_or(false))
}

fn read_layout() -> Layout {
    let json: serde_json::Value = serde_json::from_str(&command_output("kscreen-doctor", &["-j"])).unwrap_or(serde_json::Value::Null);
    let primary = json["outputs"]
        .as_array()
        .and_then(|a| a.iter().find(|o| o["priority"].as_i64() == Some(1) && output_enabled(o)))
        .and_then(|o| o["name"].as_str())
        .unwrap_or("")
        .to_string();
    let tiling = command_output("kreadconfig6", &["--file", "kwinrc", "--group", "Script-labyrinth-screen-keys", "--key", "tilingOutputs"])
        .trim()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let sizes = json["outputs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|o| output_enabled(o))
                .filter_map(|o| {
                    let name = o["name"].as_str()?.to_string();
                    // The current mode is in device pixels; 2 and 8 are the sideways rotations.
                    let (w, h) = (o["size"]["width"].as_f64()?, o["size"]["height"].as_f64()?);
                    let scale = o["scale"].as_f64().unwrap_or(1.0).max(0.5);
                    let (w, h) = if matches!(o["rotation"].as_i64(), Some(2) | Some(8)) { (h, w) } else { (w, h) };
                    let (x, y) = (o["pos"]["x"].as_i64().unwrap_or(0) as i32, o["pos"]["y"].as_i64().unwrap_or(0) as i32);
                    Some((name, x, y, (w / scale).round() as u32, (h / scale).round() as u32))
                })
                .collect()
        })
        .unwrap_or_default();
    Layout { primary, tiling, sizes }
}

/// Whether the system style is dark (the colour scheme's name says so; Settings > Appearance).
fn system_dark() -> bool {
    let scheme = command_output("kreadconfig6", &["--file", "kdeglobals", "--group", "General", "--key", "ColorScheme"]);
    let scheme = scheme.trim().to_lowercase();
    scheme.is_empty() || scheme.contains("dark")
}

fn new_buffer(shm: &WlShm, qh: &QueueHandle<State>, w: u32, h: u32) -> Option<Buffer> {
    let stride = w * 4;
    let size = (stride * h) as usize;
    let fd = rustix::fs::memfd_create(c"labyrinth-panel", rustix::fs::MemfdFlags::CLOEXEC).ok()?;
    rustix::fs::ftruncate(&fd, size as u64).ok()?;
    let file = std::fs::File::from(fd);
    let map = unsafe { MmapMut::map_mut(&file).ok()? };
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let busy = Arc::new(AtomicBool::new(false));
    let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, wl_shm::Format::Argb8888, qh, busy.clone());
    pool.destroy();
    Some(Buffer { buffer, map, w, h, busy })
}

fn run(cmd: &[&str]) {
    let Some((p, a)) = cmd.split_first() else { return };
    let _ = std::process::Command::new(p).args(a).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

impl State {
    /// The scale to draw a surface at: the fractional one the compositor prefers, else its screen's whole-number scale.
    fn scale_of(&self, s: &Surf) -> f32 {
        if s.fscale > 0.0 {
            return s.fscale;
        }
        self.outputs.get(&s.output).map(|o| o.scale).unwrap_or(1).max(1) as f32
    }

    /// Ask for the fractional scale and a viewport on a surface, once; without them the whole-number scale is used.
    fn scale_surface(&mut self, i: usize) {
        let (Some(mgr), Some(vp)) = (self.frac_mgr.clone(), self.viewporter.clone()) else { return };
        let s = &mut self.surfs[i];
        if s.frac.is_none() {
            let f = mgr.get_fractional_scale(&s.wl, &self.qh, s.id);
            let v = vp.get_viewport(&s.wl, &self.qh, ());
            s.frac = Some((f, v));
        }
    }

    /// Which output the dock goes on: the main screen unless it is the tiling one.
    /// An output's place in the global space (x, y, width, height in logical pixels): from the compositor (xdg-output) when it says,
    /// else from kscreen-doctor's reading.
    fn area_of(&self, output: u32) -> Option<(i32, i32, u32, u32)> {
        let o = self.outputs.get(&output)?;
        o.logical.filter(|(_, _, w, h)| *w > 0 && *h > 0).or_else(|| self.layout.sizes.iter().find(|(n, ..)| *n == o.name).map(|(_, x, y, w, h)| (*x, *y, *w, *h)))
    }

    fn dock_output(&self) -> Option<u32> {
        let ready = || self.outputs.iter().filter(|(_, o)| o.ready && !o.name.is_empty());
        let is_tiling = |o: &Out| self.layout.tiling.contains(&o.name);
        ready()
            .find(|(_, o)| o.name == self.layout.primary && !is_tiling(o))
            .or_else(|| ready().find(|(_, o)| !is_tiling(o)))
            .or_else(|| ready().next())
            .map(|(k, _)| *k)
    }

    /// Make the surfaces match the screens and the layout: a bar on each, the dock on one.
    fn build_surfaces(&mut self) {
        let ready: Vec<(u32, String)> = self.outputs.iter().filter(|(_, o)| o.ready && !o.name.is_empty()).map(|(k, o)| (*k, o.name.clone())).collect();
        if ready.is_empty() || self.outputs.values().any(|o| !o.ready) && self.surfs.is_empty() && ready.len() < self.outputs.len() {
            return;
        }
        for s in self.surfs.drain(..) {
            s.destroy();
        }
        self.pointer_surface = None;
        self.popup = None;
        let dock_out = self.dock_output();
        let mut sorted = ready;
        sorted.sort_by(|a, b| a.1.cmp(&b.1));
        let own_wall = self.model.conf.own_wallpaper && labyrinth_conf::wallpaper().is_some();
        for (key, name) in sorted {
            let slim = self.layout.tiling.contains(&name);
            if own_wall {
                self.add_wallpaper(key);
            }
            self.add_surface(key, Kind::Bar { slim });
            if Some(key) == dock_out {
                if self.xdg_wm.is_some() && self.plasma_shell.is_some() {
                    self.add_dock(key);
                    self.add_strut(key);
                } else {
                    // no KWin: a layer-shell surface on the edge that reserves its own room
                    self.add_layer_dock(key);
                }
            }
        }
        self.ensure_voice_surface();
        self.built_for = Some(self.layout.clone());
    }

    /// The wallpaper: a layer-shell surface on the background layer that fills the screen, drawn from the picture in
    /// ~/.config/labyrinth/wallpaper. It takes no input and reserves no room.
    fn add_wallpaper(&mut self, output: u32) {
        let id = self.next_surf;
        self.next_surf += 1;
        let Some(out) = self.outputs.get(&output) else { return };
        let wl = self.compositor.create_surface(&self.qh, ());
        let layer = self.layer_shell.get_layer_surface(&wl, Some(&out.wl), Layer::Background, "labyrinth-wallpaper".into(), &self.qh, id);
        layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let region = self.compositor.create_region(&self.qh, ());
        wl.set_input_region(Some(&region));
        region.destroy();
        wl.commit();
        self.surfs.push(Surf { id, kind: Kind::Wallpaper, output, wl, layer: Some(layer), dock: None, w: 0, h: 0, want: (0, 0), gap: 0, configured: false, bufs: vec![], hits: vec![], hover: None, frac: None, fscale: 0.0, stamp: None });
    }

    fn add_surface(&mut self, output: u32, kind: Kind) {
        let id = self.next_surf;
        self.next_surf += 1;
        let Some(out) = self.outputs.get(&output) else { return };
        let wl = self.compositor.create_surface(&self.qh, ());
        let layer = self.layer_shell.get_layer_surface(&wl, Some(&out.wl), Layer::Top, "labyrinth-panel".into(), &self.qh, id);
        let size = (0, shell::BAR_H);
        layer.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
        layer.set_size(size.0, size.1);
        layer.set_exclusive_zone(shell::BAR_H as i32);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        wl.commit();
        self.surfs.push(Surf { id, kind, output, wl, layer: Some(layer), dock: None, w: size.0, h: size.1, want: size, gap: 0, configured: false, bufs: vec![], hits: vec![], hover: None, frac: None, fscale: 0.0, stamp: None });
    }

    /// The dock for compositors without KDE's panel role: a layer-shell surface anchored to the dock's edge (centred along it)
    /// that reserves its own room.
    fn add_layer_dock(&mut self, output: u32) {
        let id = self.next_surf;
        self.next_surf += 1;
        let Some(out) = self.outputs.get(&output) else { return };
        let want = shell::dock_size(&shell::dock_items(&self.model), self.model.conf.dock_icon, self.model.conf.dock_pos);
        let wl = self.compositor.create_surface(&self.qh, ());
        let layer = self.layer_shell.get_layer_surface(&wl, Some(&out.wl), Layer::Top, "labyrinth-dock".into(), &self.qh, id);
        self.configure_layer_dock(&layer, want);
        wl.commit();
        self.surfs.push(Surf { id, kind: Kind::Dock, output, wl, layer: Some(layer), dock: None, w: want.0, h: want.1, want, gap: self.model.conf.dock_gap as i32, configured: false, bufs: vec![], hits: vec![], hover: None, frac: None, fscale: 0.0, stamp: None });
    }

    /// The edge, size, reserved room and distance from the edge of a layer-shell dock.
    fn configure_layer_dock(&self, layer: &ZwlrLayerSurfaceV1, want: (u32, u32)) {
        let gap = self.model.conf.dock_gap as i32;
        let (anchor, zone, margin) = match self.model.conf.dock_pos {
            DockPos::Bottom => (Anchor::Bottom, want.1 as i32 + gap, (0, 0, gap, 0)),
            DockPos::Left => (Anchor::Left, want.0 as i32 + gap, (0, 0, 0, gap)),
            DockPos::Right => (Anchor::Right, want.0 as i32 + gap, (0, gap, 0, 0)),
        };
        layer.set_anchor(anchor);
        layer.set_size(want.0, want.1);
        layer.set_exclusive_zone(zone);
        layer.set_margin(margin.0, margin.1, margin.2, margin.3);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    }

    /// The dock: a Plasma panel window, the way Plasma's own panels are. KWin puts it exactly where it
    /// is told, so it can sit right on the screen edge, and keeps windows clear of it. (A layer-shell
    /// surface with an exclusive zone is placed above that zone by KWin, so it floated.)
    fn add_dock(&mut self, output: u32) {
        let (Some(wm), Some(shell_g)) = (self.xdg_wm.clone(), self.plasma_shell.clone()) else {
            eprintln!("labyrinth-panel: no xdg-shell or plasma-shell from the compositor; there will be no dock");
            return;
        };
        let id = self.next_surf;
        self.next_surf += 1;
        let Some(out) = self.outputs.get(&output) else { return };
        let want = shell::dock_size(&shell::dock_items(&self.model), self.model.conf.dock_icon, self.model.conf.dock_pos);
        let wl = self.compositor.create_surface(&self.qh, ());
        let xs = wm.get_xdg_surface(&wl, &self.qh, id);
        let tl = xs.get_toplevel(&self.qh, id);
        tl.set_title("Labyrinth dock".into());
        tl.set_app_id("labyrinth-dock".into());
        tl.set_min_size(want.0 as i32, want.1 as i32);
        tl.set_max_size(want.0 as i32, want.1 as i32);
        let ps = shell_g.get_surface(&wl, &self.qh, id);
        ps.set_role(Role::Panel as u32);
        ps.set_output(&out.wl);
        let (x, y) = self.dock_origin(output, want);
        ps.set_position(x, y);
        ps.set_skip_taskbar(1);
        ps.set_skip_switcher(1);
        ps.set_panel_takes_focus(0);
        wl.commit();
        if std::env::var_os("LABYRINTH_PANEL_DEBUG").is_some() {
            eprintln!("dock {id}: {}x{} at {x},{y}", want.0, want.1);
        }
        self.surfs.push(Surf { id, kind: Kind::Dock, output, wl, layer: None, dock: Some(DockWin { xs, tl, ps }), w: want.0, h: want.1, want, gap: self.model.conf.dock_gap as i32, configured: false, bufs: vec![], hits: vec![], hover: None, frac: None, fscale: 0.0, stamp: None });
    }

    fn toggle_calendar(&mut self) {
        self.toggle_popup(PopupKind::Calendar { offset: 0 });
    }

    fn toggle_start(&mut self) {
        self.toggle_popup(PopupKind::Start(StartState::default()));
    }

    /// Open a popover on the screen that was clicked, or close it if one is open.
    fn toggle_popup(&mut self, kind: PopupKind) {
        if self.popup.is_some() {
            self.close_popup();
            return;
        }
        let Some(output) = self.click_output.or_else(|| self.dock_output()) else { return };
        let Some(out) = self.outputs.get(&output) else { return };
        let id = self.next_surf;
        self.next_surf += 1;
        let wl = self.compositor.create_surface(&self.qh, ());
        let layer = self.layer_shell.get_layer_surface(&wl, Some(&out.wl), Layer::Overlay, "labyrinth-popup".into(), &self.qh, id);
        // KWin puts a surface that fits the free room (under the bar, above the dock's strip) at the top of that
        // room, and lifts one that is taller up by the overhang. So ask for exactly that room: then the
        // menus start just under the bar, where Fylgja's section titles stay in view.
        let full_h = self.area_of(output).map(|(_, _, _, h)| h).unwrap_or(0);
        let strut = if self.model.conf.dock_pos == DockPos::Bottom && Some(output) == self.dock_output() { shell::dock_h(self.model.conf.dock_icon) + self.model.conf.dock_gap } else { 0 };
        let h = if full_h > 0 { full_h.saturating_sub(shell::BAR_H + strut) } else { 0 };
        layer.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
        layer.set_size(0, h);
        layer.set_exclusive_zone(-1);
        // The start menu takes the keyboard at once, so you can type as soon as it opens.
        layer.set_keyboard_interactivity(if matches!(&kind, PopupKind::Start(_)) || matches!(&kind, PopupKind::Fylgja(f) if f.pinned) { KeyboardInteractivity::Exclusive } else { KeyboardInteractivity::OnDemand });
        wl.commit();
        self.surfs.push(Surf { id, kind: Kind::Popup, output, wl, layer: Some(layer), dock: None, w: 0, h: 0, want: (0, 0), gap: 0, configured: false, bufs: vec![], hits: vec![], hover: None, frac: None, fscale: 0.0, stamp: None });
        if matches!(kind, PopupKind::Fylgja(_)) {
            self.fy_active.store(true, Ordering::Relaxed);
            fylgja::spawn_poller(self.fy_active.clone(), self.fy_shared.clone());
            stats::spawn_poller(self.fy_active.clone(), self.stats_wanted.clone(), self.stats_shared.clone());
            self.start_anim(true);
            if !self.thumbs_started {
                self.thumbs_started = true;
                let scale = self.scale_of(self.surfs.last().unwrap()).ceil().max(1.0) as u32;
                let walls = fylgja::wallpaper_list();
                let map = Arc::new(std::sync::Mutex::new(HashMap::new()));
                fylgja::spawn_thumbs(walls.clone(), (94 * scale, 53 * scale), map.clone());
                self.model.fylgja.walls = walls;
                self.model.fylgja.thumbs = Some(map);
            }
            self.dirty = true;
        }
        self.popup = Some(Popup { output, kind });
    }

    /// The pointer has rested on the notch's tab: open Fylgja (the small card) on that screen.
    fn open_fylgja(&mut self, output: u32) {
        if self.popup.is_none() {
            self.click_output = Some(output);
            self.toggle_popup(PopupKind::Fylgja(fylgja::FyState::default()));
        }
    }

    /// The pointer is on the closed tab and the wheel turned (or it was clicked): open the full notch.
    fn open_full_from_tab(&mut self) -> bool {
        if self.popup.is_some() {
            return false;
        }
        if !self.tab_hover {
            return false;
        }
        self.click_output = Some(self.tab_output);
        self.toggle_popup(PopupKind::Fylgja(fylgja::FyState { section: 0, open: true, pinned: false, voice_ui: false, picker: false, mixer: false }));
        true
    }

    /// Open Fylgja on the clipboard history (Super+V), or close it if it is open.
    fn show_clipboard(&mut self) {
        let was_open = self.popup.is_some();
        self.close_popup();
        if !was_open {
            self.toggle_popup(PopupKind::Fylgja(fylgja::FyState { section: 3, open: true, pinned: true, voice_ui: false, picker: false, mixer: false }));
        }
    }

    /// Opens Fylgja pinned on a section, or closes it if anything is open.
    fn show_fylgja_section(&mut self, section: usize) {
        let was_open = self.popup.is_some();
        self.close_popup();
        if !was_open {
            let section = section.min(fylgja::SECTIONS.len() - 1);
            self.toggle_popup(PopupKind::Fylgja(fylgja::FyState { section, open: true, pinned: true, voice_ui: false, picker: false, mixer: false }));
        }
    }

    /// The shortcut for asking Sarina by D-Bus (a fallback when the held shortcut is not available): the first call records, the next sends it.
    fn ask_sarina_key(&mut self) {
        match self.model.fylgja.voice {
            fylgja::Voice::Listening(_) => self.send_recording(),
            fylgja::Voice::Thinking(_) => {}
            _ => self.begin_listening(),
        }
    }

    /// The shortcut went down or up. A tap starts a conversation (it listens until you stop talking, she answers, then it listens again; a tap stops
    /// it); holding the key is one question, sent when you let go. Pressing while she talks stops her and listens.
    fn voice_key(&mut self, k: voicekey::Key) {
        match k {
            voicekey::Key::Down => {
                self.key_down_at = Some(std::time::Instant::now());
                self.down_in_conv = self.conv && self.vad_mode && matches!(self.model.fylgja.voice, fylgja::Voice::Listening(_));
                if self.down_in_conv || matches!(self.model.fylgja.voice, fylgja::Voice::Thinking(_)) {
                    return; // already listening (this press may be the one that stops it) or busy answering
                }
                let interrupting = self.speech_since.is_some();
                self.begin_listening();
                if interrupting {
                    self.vad_mode = false; // decided when the key comes up
                }
            }
            voicekey::Key::Up => {
                let held = self.key_down_at.take().map(|t| t.elapsed().as_millis()).unwrap_or(0);
                if !matches!(self.model.fylgja.voice, fylgja::Voice::Listening(_)) {
                    return;
                }
                match convo::classify(held, self.down_in_conv) {
                    convo::Press::StopConversation => self.end_conversation(),
                    convo::Press::SingleQuestion => {
                        self.vad_mode = false;
                        self.conv = false;
                        self.send_recording();
                    }
                    convo::Press::StartConversation => {
                        // keep recording: it ends when you stop talking. With her voice off there is no spoken back and forth, so one question only.
                        self.vad_mode = true;
                        self.conv = self.model.conf.sarina_speaks;
                        self.vad = convo::Vad::new();
                        self.conv_misses = 0;
                        if self.conv && self.conv_id.is_none() {
                            self.conv_id = Some(format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)));
                        }
                    }
                }
                self.down_in_conv = false;
            }
        }
    }

    /// Ends the conversation: stops listening and talking, and forgets its session.
    fn end_conversation(&mut self) {
        self.conv = false;
        self.vad_mode = false;
        self.conv_misses = 0;
        if let (Some(id), Some(dir)) = (self.conv_id.take(), std::env::var_os("XDG_RUNTIME_DIR")) {
            let _ = std::fs::remove_file(std::path::Path::new(&dir).join("labyrinth-sarina").join(format!("conv-{id}.session")));
        }
        self.cancel_voice();
    }

    /// Starts recording a question. No card opens: the notch grows into the voice island and its wave follows the microphone.
    fn begin_listening(&mut self) {
        self.stop_speech();
        match fylgja::start_recording() {
            Ok((child, path)) => {
                self.rec = Some((child, path));
                self.rec_since = Some(std::time::Instant::now());
                self.model.fylgja.voice = fylgja::Voice::Listening(0);
                self.vad = convo::Vad::new();
                self.start_viz();
            }
            Err(e) => self.voice_failed(&e),
        }
    }

    /// She has finished talking: in a conversation, listen again.
    fn after_speech(&mut self) {
        if self.conv && matches!(self.model.fylgja.voice, fylgja::Voice::Idle) {
            self.vad_mode = true;
            self.begin_listening();
        }
    }

    /// Ends the recording and hands it to Sarina.
    fn send_recording(&mut self) {
        if let Some((mut child, path)) = self.rec.take() {
            fylgja::stop_recording(&mut child);
            self.rec_since = None;
            self.model.fylgja.voice = fylgja::Voice::Thinking(String::new());
            fylgja::ask_sarina(path, self.conv_id.clone(), self.heard_tx.clone(), self.voice_tx.clone());
            self.dirty = true;
        }
    }

    /// Drops a recording that was not sent, and stops Sarina talking.
    fn cancel_voice(&mut self) {
        if let Some((mut child, path)) = self.rec.take() {
            fylgja::stop_recording(&mut child);
            let _ = std::fs::remove_file(path);
        }
        self.rec_since = None;
        self.stop_speech();
        if !matches!(self.model.fylgja.voice, fylgja::Voice::Thinking(_)) {
            self.model.fylgja.voice = fylgja::Voice::Idle;
        }
    }

    /// No answer to show: a notification says why (and she says it, when she speaks).
    fn voice_failed(&mut self, why: &str) {
        self.model.fylgja.voice = fylgja::Voice::Idle;
        if self.conv && why.starts_with("I did not catch") {
            // nothing to hear in a conversation: listen once more quietly, and end it after two in a row
            self.conv_misses += 1;
            if self.conv_misses >= 2 {
                self.end_conversation();
            } else {
                self.vad_mode = true;
                self.begin_listening();
            }
            return;
        }
        if self.conv {
            self.conv = false; // a real failure ends the conversation (she says why below)
            self.vad_mode = false;
        }
        let _ = Command::new("notify-send").args(["-a", "Sarina", "-i", "audio-input-microphone", "Sarina", why]).stdin(std::process::Stdio::null()).spawn().map(|mut c| std::thread::spawn(move || { let _ = c.wait(); }));
        if self.model.conf.sarina_speaks {
            self.start_speech(why.split(['.', '!', '?']).next().unwrap_or(why));
        }
    }

    /// Her answer: said aloud when she speaks, otherwise shown as a notification. No card either way.
    fn voice_answer(&mut self, reply: &str) {
        self.model.fylgja.voice = fylgja::Voice::Idle;
        self.conv_misses = 0;
        let text = fylgja::spoken_part(reply);
        if self.model.conf.sarina_speaks {
            self.start_speech(&text);
        } else {
            let _ = Command::new("notify-send").args(["-a", "Sarina", "-i", "dialog-information", "Sarina", &text]).stdin(std::process::Stdio::null()).spawn().map(|mut c| std::thread::spawn(move || { let _ = c.wait(); }));
        }
    }

    fn start_speech(&mut self, text: &str) {
        self.speech_since = Some(std::time::Instant::now());
        self.speech_env = None;
        self.speech_env_mtime = None;
        self.speech_seen = false;
        fylgja::speak(text);
        self.start_viz();
    }

    fn stop_speech(&mut self) {
        if self.speech_since.take().is_some() {
            fylgja::stop_speaking();
        }
        self.speech_env = None;
    }

    /// The voice island's surface is made once, when the bars are, and stays (empty and see-through) until it is needed: a surface
    /// made on demand showed up in the wrong place for a moment (below the bar) before KWin put it at the top.
    fn ensure_voice_surface(&mut self) {
        if self.surfs.iter().any(|s| s.kind == Kind::Voice) {
            return;
        }
        let Some(output) = self.surfs.iter().find(|s| matches!(s.kind, Kind::Bar { slim: false })).map(|s| s.output).or_else(|| self.dock_output()) else { return };
        let Some(out) = self.outputs.get(&output) else { return };
        let id = self.next_surf;
        self.next_surf += 1;
        let wl = self.compositor.create_surface(&self.qh, ());
        let layer = self.layer_shell.get_layer_surface(&wl, Some(&out.wl), Layer::Overlay, "labyrinth-voice".into(), &self.qh, id);
        // A surface that spans the screen's top edge sits flush against it (a small centred one is pushed down below the bar's
        // reserved strip by KWin), so it spans the width, is see-through and takes no clicks, and the pill is drawn in its middle.
        layer.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
        layer.set_size(0, voicewave::HEIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        // it takes no clicks: they go to whatever is under it
        let region = self.compositor.create_region(&self.qh, ());
        wl.set_input_region(Some(&region));
        region.destroy();
        wl.commit();
        self.surfs.push(Surf { id, kind: Kind::Voice, output, wl, layer: Some(layer), dock: None, w: voicewave::WIDTH, h: voicewave::HEIGHT, want: (voicewave::WIDTH, voicewave::HEIGHT), gap: 0, configured: false, bufs: vec![], hits: vec![], hover: None, frac: None, fscale: 0.0, stamp: None });
    }

    /// Makes the island appear (if it is not) and starts its animation ticker.
    fn start_viz(&mut self) {
        if self.viz.is_none() {
            self.viz = Some(voicewave::Viz { mode: voicewave::Mode::Listening, level: 0.0, phase: 0.0, shown: 0.0 });
        }
        self.ensure_voice_surface();
        if !self.viz_alive.swap(true, Ordering::AcqRel) {
            let (alive, tx) = (self.viz_alive.clone(), self.viz_tx.clone());
            std::thread::spawn(move || {
                while alive.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(33));
                    if tx.send(()).is_err() {
                        break;
                    }
                }
            });
        }
    }

    /// One step of the island's animation (about 30 a second while it shows).
    fn viz_tick(&mut self) {
        let mode = match &self.model.fylgja.voice {
            fylgja::Voice::Listening(_) => Some(voicewave::Mode::Listening),
            fylgja::Voice::Thinking(_) => Some(voicewave::Mode::Thinking),
            _ if self.speech_since.is_some() => Some(voicewave::Mode::Speaking),
            _ => None,
        };
        let phase_now = self.viz.map(|v| v.phase).unwrap_or(0.0);
        let raw = match mode {
            Some(voicewave::Mode::Listening) => self.rec.as_ref().map(|(_, p)| voicewave::mic_level(p)).unwrap_or(0.0),
            Some(voicewave::Mode::Thinking) => 0.25 + 0.1 * (phase_now * 0.6).sin(),
            Some(voicewave::Mode::Speaking) => self.speech_level(),
            None => 0.0,
        };
        // a tapped recording ends when you stop talking
        let dt = self.last_tick.map(|t| t.elapsed().as_millis() as u32).unwrap_or(33).min(200);
        self.last_tick = Some(std::time::Instant::now());
        if mode == Some(voicewave::Mode::Listening) && self.vad_mode {
            match self.vad.feed(raw, dt) {
                convo::Event::EndOfSpeech => self.send_recording(),
                convo::Event::GaveUp => self.end_conversation(),
                _ => {}
            }
        }
        let Some(v) = self.viz.as_mut() else {
            self.viz_alive.store(false, Ordering::Release);
            return;
        };
        v.phase += 0.14;
        v.level = voicewave::smooth(v.level, raw);
        let target = if let Some(m) = mode {
            v.mode = m;
            1.0
        } else {
            0.0
        };
        v.shown += (target - v.shown) * 0.22;
        if mode.is_none() && v.shown < 0.03 {
            self.viz = None;
            self.viz_alive.store(false, Ordering::Release);
            // one last, empty frame: the surface stays, see-through
            if let Some(i) = self.surfs.iter().position(|s| s.kind == Kind::Voice) {
                self.draw(i);
                let _ = self.conn.flush();
            }
            return;
        }
        if let Some(i) = self.surfs.iter().position(|s| s.kind == Kind::Voice) {
            self.draw(i);
            let _ = self.conn.flush();
        }
    }

    /// How loud her voice is right now: from the envelope file the speaking script writes just before each sentence plays (one number per 50 ms),
    /// read again whenever it changes (the next sentence). While the first sentence is still being made, a gentle wave. Speech is over when the
    /// speaking script has been seen running and is no longer (or never showed up).
    fn speech_level(&mut self) -> f32 {
        let Some(since) = self.speech_since else { return 0.0 };
        let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").map(|d| std::path::PathBuf::from(d).join("labyrinth-sarina")) else { return 0.0 };
        let alive = std::fs::read_to_string(dir.join("speaking.pid")).ok().and_then(|t| t.trim().parse::<u32>().ok()).map(|p| std::path::Path::new(&format!("/proc/{p}")).exists()).unwrap_or(false);
        if alive {
            self.speech_seen = true;
        } else if self.speech_seen || since.elapsed() > Duration::from_secs(45) {
            self.speech_since = None; // she has finished (or the voice never started: no voice installed)
            self.speech_env = None;
            self.speech_seen = false;
            self.after_speech();
            return 0.0;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let p = dir.join("speech.env");
        let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
        if mtime.is_some() && mtime != self.speech_env_mtime {
            if let Ok(t) = std::fs::read_to_string(&p) {
                let mut lines = t.lines();
                let start: Option<i64> = lines.next().and_then(|l| l.trim().parse().ok());
                let env: Vec<u8> = lines.next().unwrap_or("").split_whitespace().filter_map(|n| n.parse().ok()).collect();
                if let Some(st) = start {
                    if !env.is_empty() && now - st < 60_000 {
                        self.speech_env = Some((env, st));
                        self.speech_env_mtime = mtime;
                    }
                }
            }
        }
        match &self.speech_env {
            Some((env, start)) => {
                let ms = (now - start).max(0) as u64;
                voicewave::envelope_at(env, ms).unwrap_or(0.0) // between sentences: flat until the next one
            }
            None => 0.2,
        }
    }

    /// Shows the volume or brightness pop-up (or updates it) and starts its hide timer.
    fn show_osd(&mut self, st: osd::OsdState) {
        // Fylgja shows the value itself while it is open, and a slider being dragged would flicker it
        if self.drag.is_some() || matches!(&self.popup, Some(Popup { kind: PopupKind::Fylgja(f), .. }) if f.open) {
            return;
        }
        self.osd = Some(st);
        self.osd_gen += 1;
        let (g, tx) = (self.osd_gen, self.osd_hide_tx.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1800));
            let _ = tx.send(g);
        });
        if !self.surfs.iter().any(|s| s.kind == Kind::Osd) {
            let Some(output) = self.dock_output() else { return };
            let Some(out) = self.outputs.get(&output) else { return };
            let id = self.next_surf;
            self.next_surf += 1;
            let wl = self.compositor.create_surface(&self.qh, ());
            let layer = self.layer_shell.get_layer_surface(&wl, Some(&out.wl), Layer::Overlay, "labyrinth-osd".into(), &self.qh, id);
            layer.set_anchor(Anchor::Bottom);
            layer.set_size(osd::WIDTH, osd::HEIGHT);
            layer.set_exclusive_zone(0);
            // KWin puts a surface anchored to the bottom above the dock's reserved room; with the dock
            // on a side there is none, so the margin has to make the room
            let margin = if self.model.conf.dock_pos == DockPos::Bottom { 14 } else { 64 };
            layer.set_margin(0, 0, margin, 0);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            // it takes no clicks: they go to whatever is under it
            let region = self.compositor.create_region(&self.qh, ());
            wl.set_input_region(Some(&region));
            region.destroy();
            wl.commit();
            self.surfs.push(Surf { id, kind: Kind::Osd, output, wl, layer: Some(layer), dock: None, w: osd::WIDTH, h: osd::HEIGHT, want: (osd::WIDTH, osd::HEIGHT), gap: 0, configured: false, bufs: vec![], hits: vec![], hover: None, frac: None, fscale: 0.0, stamp: None });
        }
        self.dirty = true;
    }

    /// Takes the pop-up away when its timer ran out and nothing newer replaced it.
    fn hide_osd(&mut self, gen: u64) {
        if gen != self.osd_gen {
            return;
        }
        if let Some(i) = self.surfs.iter().position(|s| s.kind == Kind::Osd) {
            let s = self.surfs.remove(i);
            s.destroy();
            self.pointer_surface = None;
        }
        self.osd = None;
    }

    /// A notification arrived from an app or from Heimdall (or an app asked to take one away).
    fn on_notif(&mut self, msg: notify::NMsg) {
        match msg {
            notify::NMsg::New(n) => {
                let (id, urgency) = (n.id, n.urgency);
                self.model.notifs.add(n);
                if self.model.conf.notify_toasts && urgency > 0 && (!self.model.conf.notify_dnd || urgency == 2) {
                    self.toasts.retain(|t| *t != id);
                    self.toasts.push(id);
                    while self.toasts.len() > 3 {
                        self.toasts.remove(0);
                    }
                    let (tx, secs) = (self.toast_hide_tx.clone(), if urgency == 2 { 14 } else { 7 });
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(secs));
                        let _ = tx.send(id);
                    });
                    self.sync_toasts();
                }
                self.dirty = true;
            }
            notify::NMsg::Close(id) => self.remove_notif(id, 3),
        }
    }

    /// Takes a notification away for good (reason 2: the user did it, 3: the app asked) and tells its app.
    fn remove_notif(&mut self, id: u32, reason: u32) {
        if self.model.notifs.remove(id).is_some() {
            notify::emit_closed(id, reason);
        }
        self.toasts.retain(|t| *t != id);
        let max = notify::max_scroll(&self.model.notifs);
        self.model.notifs.scroll = self.model.notifs.scroll.min(max);
        self.sync_toasts();
        self.dirty = true;
    }

    /// The pop-up's time is up: it goes from the screen, but stays in the Notification Center.
    fn hide_toast(&mut self, id: u32) {
        // keep it while the pointer is on the pop-ups
        let hovered = self.surfs.iter().any(|s| s.kind == Kind::Toast && s.hover.is_some());
        if hovered {
            let tx = self.toast_hide_tx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(2));
                let _ = tx.send(id);
            });
            return;
        }
        if self.toasts.contains(&id) {
            self.toasts.retain(|t| *t != id);
            self.sync_toasts();
        }
    }

    /// Makes the pop-up surface match the list: one while there is anything to show, none otherwise.
    fn sync_toasts(&mut self) {
        let have = self.surfs.iter().position(|s| s.kind == Kind::Toast);
        if self.toasts.is_empty() {
            if let Some(i) = have {
                let s = self.surfs.remove(i);
                s.destroy();
                self.pointer_surface = None;
            }
            return;
        }
        if have.is_none() {
            let Some(output) = self.dock_output() else { return };
            let Some(out) = self.outputs.get(&output) else { return };
            let (w, h) = (notify::TOAST_W, notify::surface_h());
            let id = self.next_surf;
            self.next_surf += 1;
            let wl = self.compositor.create_surface(&self.qh, ());
            let layer = self.layer_shell.get_layer_surface(&wl, Some(&out.wl), Layer::Overlay, "labyrinth-toast".into(), &self.qh, id);
            layer.set_anchor(Anchor::Bottom | Anchor::Right);
            layer.set_size(w, h);
            layer.set_exclusive_zone(0);
            layer.set_margin(0, 16, 16, 0);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            wl.commit();
            self.surfs.push(Surf { id, kind: Kind::Toast, output, wl, layer: Some(layer), dock: None, w, h, want: (w, h), gap: 0, configured: false, bufs: vec![], hits: vec![], hover: None, frac: None, fscale: 0.0, stamp: None });
        }
    }

    /// Goes to the app a notification came from: asks it to show what the notification is about (its default
    /// action), then brings its window forward, or starts it. Heimdall's alerts open their page in Settings.
    fn open_notification(&mut self, id: u32) {
        let Some(n) = self.model.notifs.get(id).cloned() else { return };
        if n.actions.iter().any(|(k, _)| k == "default") {
            notify::emit_action(id, "default");
        }
        if let Some(dev) = n.link.strip_prefix("mount:") {
            run(&["labyrinth-mount-open", dev]);
        } else if !n.link.is_empty() {
            run(&["labyrinth-settings", &n.link]);
        } else if n.app != notify::HEIMDALL_APP {
            self.focus_or_launch(&n);
        }
        self.remove_notif(id, 2);
    }

    fn focus_or_launch(&mut self, n: &notify::Notif) {
        let apps = &self.model.apps;
        let name = n.app.to_lowercase();
        let app = apps::for_app_id(apps, &n.entry).or_else(|| apps.iter().find(|a| a.name.to_lowercase() == name || a.id.to_lowercase() == name));
        let win = self.model.wins.iter().find(|w| {
            let by_app = app.map(|a| apps::for_app_id(apps, &w.app_id).map(|x| x.id == a.id).unwrap_or(false)).unwrap_or(false);
            by_app || (!w.app_id.is_empty() && w.app_id.to_lowercase() == name)
        });
        if let Some(w) = win {
            w.proxy.activate(self.seat.as_ref());
        } else if let Some(a) = app {
            apps::launch(a);
        }
    }

    /// A tray icon was clicked: tell its app (the screen position is just under the icon).
    fn tray_click(&mut self, key: &str, kind: &'static str) {
        let Some(it) = self.model.tray.iter().find(|t| t.key == key).cloned() else { return };
        let (x, y) = (self.pointer_pos.0 as i32, shell::BAR_H as i32);
        // an item with a dbusmenu gets Labyrinth's own menu (themed, placed under the bar); the rest are asked to show theirs
        if kind == "ContextMenu" && !it.menu_path.is_empty() {
            if let Some(tx) = self.tray_tx.clone() {
                let ax = self.pointer_pos.0;
                std::thread::spawn(move || match tray::fetch_menu(&it.bus, &it.menu_path) {
                    Some(items) if !items.is_empty() => {
                        let _ = tx.send(tray::TrayMsg::Menu(it.key.clone(), items, ax));
                    }
                    _ => tray::call(&it, "ContextMenu", x, y),
                });
                return;
            }
        }
        tray::call(&it, kind, x, y);
    }

    /// Opens (or replaces) the tray pop-up under the bar.
    fn open_tray_popup(&mut self, pop: trayui::TrayPop) {
        if self.popup.is_some() {
            self.close_popup();
        }
        self.toggle_popup(PopupKind::Tray(pop));
        self.dirty = true;
    }

    /// A row of the tray pop-up was clicked.
    fn tray_row(&mut self, act: trayui::RowAct) {
        use trayui::RowAct;
        match act {
            RowAct::Activate(key) => {
                self.close_popup();
                self.tray_click(&key, "Activate");
            }
            RowAct::Choose(key, id) if key.starts_with("dock:") => {
                self.close_popup();
                self.dock_menu_choose(&key["dock:".len()..], id);
            }
            RowAct::Choose(key, id) => {
                if let Some(it) = self.model.tray.iter().find(|t| t.key == key) {
                    tray::menu_event(&it.bus, &it.menu_path, id);
                }
                self.close_popup();
            }
            RowAct::Sub(id) => {
                if let Some(Popup { kind: PopupKind::Tray(trayui::TrayPop { mode: trayui::TrayMode::Menu { path, .. }, .. }), .. }) = &mut self.popup {
                    path.push(id);
                }
                self.dirty = true;
            }
            RowAct::Window(i) => {
                if let Some(w) = self.model.wins.get(i) {
                    w.proxy.activate(self.seat.as_ref());
                }
                self.close_popup();
            }
            RowAct::Back => {
                if let Some(Popup { kind: PopupKind::Tray(trayui::TrayPop { mode: trayui::TrayMode::Menu { path, .. }, .. }), .. }) = &mut self.popup {
                    path.pop();
                }
                self.dirty = true;
            }
        }
    }

    /// Something in the notification list was clicked.
    fn notif_act(&mut self, act: notify::NAct) {
        use notify::NAct;
        match act {
            NAct::Toggle(key) => self.model.notifs.toggle_group(&key),
            NAct::Open(id) => {
                self.open_notification(id);
                self.close_popup();
            }
            NAct::Dismiss(id) => self.remove_notif(id, 2),
            NAct::ClearGroup(key) => {
                for id in self.model.notifs.clear_group(&key) {
                    notify::emit_closed(id, 2);
                    self.toasts.retain(|t| *t != id);
                }
                self.sync_toasts();
            }
            NAct::ClearAll => {
                for id in self.model.notifs.clear_all() {
                    notify::emit_closed(id, 2);
                }
                self.toasts.clear();
                self.sync_toasts();
            }
            NAct::ToggleDnd => {
                self.model.conf.notify_dnd = !self.model.conf.notify_dnd;
                self.model.notifs.dnd = self.model.conf.notify_dnd;
                let _ = self.model.conf.save();
                if self.model.conf.notify_dnd {
                    self.toasts.clear();
                    self.sync_toasts();
                }
            }
            NAct::Scroll(by) => {
                let max = notify::max_scroll(&self.model.notifs) as i32;
                self.model.notifs.scroll = (self.model.notifs.scroll as i32 + by).clamp(0, max) as usize;
            }
        }
        let max = notify::max_scroll(&self.model.notifs);
        self.model.notifs.scroll = self.model.notifs.scroll.min(max);
        self.dirty = true;
    }

    /// Starts or stops watching the clipboard to follow the setting.
    fn sync_clip(&mut self) {
        let on = self.model.conf.clipboard_history;
        self.model.fylgja.history_on = on;
        if on && self.clip_child.is_none() {
            self.clip_child = fylgja::start_clip_watch(self.clips.clone());
        }
        if !on {
            if let Some(mut c) = self.clip_child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            if let Ok(mut g) = self.clips.lock() {
                g.clear();
            }
            self.model.fylgja.clips.clear();
        }
    }

    /// Starts Fylgja sliding down (`opening`) or back up, redrawing for as long as it takes.
    fn start_anim(&mut self, opening: bool) {
        self.fy_anim = Some((std::time::Instant::now(), opening));
        let tx = self.anim_tx.clone();
        std::thread::spawn(move || {
            for _ in 0..=(fylgja::SLIDE_MS / 16 + 2) {
                std::thread::sleep(Duration::from_millis(16));
                if tx.send(()).is_err() {
                    break;
                }
            }
        });
    }

    /// How far down Fylgja hangs now: 1 when settled, between 0 and 1 while it slides.
    fn fy_slide(&self) -> f32 {
        match self.fy_anim {
            Some((t, true)) => fylgja::ease(t.elapsed().as_millis()),
            Some((t, false)) => 1.0 - fylgja::ease(t.elapsed().as_millis()),
            None => 1.0,
        }
    }

    /// Closes Fylgja by sliding it back up into the bar.
    fn retract_fylgja(&mut self) {
        if !matches!(self.popup, Some(Popup { kind: PopupKind::Fylgja(_), .. })) {
            return;
        }
        if matches!(self.fy_anim, Some((_, false))) {
            return;
        }
        self.cancel_voice();
        self.tab_on = false;
        self.start_anim(false);
    }

    /// The bar's black tab widens to the card's width while Fylgja is open (and slides with it).
    fn sync_notch(&mut self) {
        let slide = self.fy_slide();
        let (open, w) = match &self.popup {
            Some(Popup { kind: PopupKind::Fylgja(st), .. }) if fylgja::visible(&self.model, st) => (true, 150.0 + (fylgja::card_size(&self.model, st).0 - 150.0) * slide),
            _ => (false, 0.0),
        };
        let tab = match &self.popup {
            Some(Popup { kind: PopupKind::Fylgja(st), .. }) if st.open && !(st.voice_ui && self.model.fylgja.voice != fylgja::Voice::Idle) => Some(st.section),
            _ => None,
        };
        // looking at the notification list reads it
        if tab == Some(fylgja::NOTIFS) || (self.model.conf.notify_pane && matches!(self.popup, Some(Popup { kind: PopupKind::Calendar { .. }, .. }))) {
            if self.model.notifs.unread_count() > 0 {
                self.model.notifs.mark_read();
                self.dirty = true;
            }
        }
        // the Stats readings are taken only while that section is showing
        self.stats_wanted.store(tab == Some(2), Ordering::Relaxed);
        if self.model.notch_open != open || self.model.notch_w != w || self.model.notch_tab != tab {
            self.model.notch_open = open;
            self.model.notch_w = w;
            self.model.notch_tab = tab;
        }
    }

    /// Right click on a dock app: its menu above the dock.
    fn open_dock_menu(&mut self, app_id: &str, x: f32, output: u32) {
        if app_id == shell::TRASH_ID {
            return;
        }
        if self.popup.is_some() {
            self.close_popup();
        }
        let items = shell::dock_items(&self.model);
        let running = items.iter().find(|i| i.app_id == app_id).map(|i| !i.wins.is_empty()).unwrap_or(false);
        let pinned = self.model.pinned.iter().any(|p| p == app_id);
        let mk = |id: i32, label: &str| tray::MenuItem { id, label: label.into(), enabled: true, separator: false, checked: None, children: vec![] };
        let mut menu = vec![mk(1, if running { "New window" } else { "Open" })];
        if running {
            menu.push(mk(2, "Close"));
        }
        menu.push(tray::MenuItem { id: 0, label: String::new(), enabled: false, separator: true, checked: None, children: vec![] });
        menu.push(mk(3, if pinned { "Remove from dock" } else { "Keep in dock" }));
        self.click_output = Some(output);
        self.toggle_popup(PopupKind::Tray(trayui::TrayPop { anchor_x: x, mode: trayui::TrayMode::Menu { key: format!("dock:{app_id}"), items: menu, path: vec![] } }));
        self.dirty = true;
    }

    fn dock_menu_choose(&mut self, app_id: &str, choice: i32) {
        match choice {
            1 => {
                if let Some(app) = self.model.apps.iter().find(|a| a.id == app_id) {
                    apps::launch(app);
                }
            }
            2 => {
                let items = shell::dock_items(&self.model);
                if let Some(item) = items.iter().find(|i| i.app_id == app_id) {
                    for &i in &item.wins {
                        self.model.wins[i].proxy.close();
                    }
                }
            }
            3 => {
                if let Some(pos) = self.model.pinned.iter().position(|p| p == app_id) {
                    self.model.pinned.remove(pos);
                } else {
                    self.model.pinned.push(app_id.to_string());
                }
                apps::save_pinned(&self.model.pinned);
                self.resize_dock();
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn windows_popup_open(&self) -> bool {
        matches!(self.popup, Some(Popup { kind: PopupKind::Tray(trayui::TrayPop { mode: trayui::TrayMode::Windows { .. }, .. }), .. }))
    }

    /// Open the list of a dock app's windows above the dock.
    fn open_windows_popup(&mut self, app_id: &str, x: f32, output: u32) {
        if self.popup.is_some() {
            return;
        }
        let items = shell::dock_items(&self.model);
        let Some(item) = items.iter().find(|i| i.app_id == app_id) else { return };
        if item.wins.is_empty() {
            return;
        }
        let rows: Vec<(usize, String, bool)> = item.wins.iter().map(|&i| (i, self.model.wins[i].title.clone(), self.model.wins[i].active())).collect();
        self.click_output = Some(output);
        self.toggle_popup(PopupKind::Tray(trayui::TrayPop { anchor_x: x, mode: trayui::TrayMode::Windows { rows } }));
        self.dirty = true;
    }

    /// Pointer moves while the window list is open: leaving the card (and its margin) closes it.
    fn preview_pointer_moved(&mut self) -> bool {
        if !self.windows_popup_open() {
            return false;
        }
        let on_popup = self.pointer_surface.and_then(|i| self.surfs.get(i)).map(|s| s.kind == Kind::Popup).unwrap_or(false);
        if on_popup {
            let (cx, cy, cw, ch) = self.popup_card;
            let (px, py) = self.pointer_pos;
            let m = 28.0;
            if px < cx - m || px > cx + cw + m || py < cy - m {
                self.close_popup();
                return true;
            }
            self.preview_close = None;
        }
        self.dirty = true; // the row under the pointer lights up
        false
    }

    fn close_popup(&mut self) {
        self.fy_anim = None;
        if matches!(self.popup, Some(Popup { kind: PopupKind::Fylgja(_), .. })) {
            self.cancel_voice();
        }
        self.tab_on = false;
        if let Some(i) = self.surfs.iter().position(|s| s.kind == Kind::Popup) {
            let s = self.surfs.remove(i);
            s.destroy();
            self.pointer_surface = None;
        }
        self.popup = None;
        self.fy_active.store(false, Ordering::Relaxed);
        self.dirty = true;
    }

    /// The wheel: another month in the calendar (+1 is the next), or a few rows down the app list.
    fn step_month(&mut self, by: i32) {
        let max = self.start_max;
        let pointer = self.pointer_pos;
        match self.popup.as_mut().map(|p| &mut p.kind) {
            Some(PopupKind::Calendar { .. }) if self.model.conf.notify_pane && pointer.1 >= self.pane_top => {
                let max = notify::max_scroll(&self.model.notifs) as i32;
                self.model.notifs.scroll = (self.model.notifs.scroll as i32 + by).clamp(0, max) as usize;
            }
            Some(PopupKind::Calendar { offset }) => *offset = (*offset + by).clamp(-1200, 1200),
            Some(PopupKind::Fylgja(st)) => {
                if st.voice_ui {
                    // the voice card has nothing to scroll
                } else if !st.open {
                    st.open = true; // the wheel opens the full notch at its first section
                    st.section = 0;
                } else if st.section == fylgja::NOTIFS && self.pointer_pos.1 > 40.0 {
                    // over the list the wheel scrolls it; over the section titles it changes section
                    let max = notify::max_scroll(&self.model.notifs) as i32;
                    self.model.notifs.scroll = (self.model.notifs.scroll as i32 + by).clamp(0, max) as usize;
                } else {
                    st.section = (st.section as i32 + by).clamp(0, fylgja::SECTIONS.len() as i32 - 1) as usize;
                }
            }
            Some(PopupKind::Start(st)) => {
                // over the folder's own list, the wheel scrolls that; elsewhere, the main list
                if st.cat.is_some() && pointer.0 > 316.0 {
                    st.sub_scroll = (st.sub_scroll + by * 3).clamp(0, 1000);
                } else {
                    st.scroll = (st.scroll + by * 3).clamp(0, max);
                }
            }
            Some(PopupKind::Tray(_)) | None => return,
        }
        self.dirty = true;
    }

    /// The pointer moved or arrived while Fylgja is open: leaving the card closes it (true then),
    /// unless a shortcut opened it.
    fn fylgja_pointer_moved(&mut self) -> bool {
        let pinned = match &self.popup {
            Some(Popup { kind: PopupKind::Fylgja(st), .. }) => st.pinned,
            _ => return false,
        };
        let on_popup = self.pointer_surface.and_then(|i| self.surfs.get(i)).map(|s| s.kind == Kind::Popup).unwrap_or(false);
        let (cx, cy, cw, ch) = self.popup_card;
        let (px, py) = self.pointer_pos;
        if !pinned && on_popup && (px < cx - 6.0 || px > cx + cw + 6.0 || py > cy + ch + 6.0) {
            self.retract_fylgja();
            return true;
        }
        self.dirty = true; // buttons light up under the pointer
        false
    }

    /// While a volume slider is held: follow the pointer (no more often than every 30 ms; `force` sends the last one).
    fn drag_apply(&mut self, force: bool) {
        let Some((who, rect)) = self.drag.clone() else { return };
        let percent = (((self.pointer_pos.0 - rect.0) / rect.2) * 100.0).round().clamp(0.0, 100.0) as u32;
        if percent == self.drag_sent.1 || (!force && self.drag_sent.0.elapsed() < Duration::from_millis(30)) {
            return;
        }
        fylgja::set_volume(&who, percent);
        self.drag_sent = (std::time::Instant::now(), percent);
        let snap = &mut self.model.fylgja.snap;
        match who {
            fylgja::Slider::Master => snap.master = snap.master.map(|(_, m)| (percent, m)),
            fylgja::Slider::App(id) => {
                if let Some(a) = snap.apps.iter_mut().find(|a| a.id == id) {
                    a.percent = percent;
                }
            }
        }
        self.dirty = true;
    }

    /// Something in Fylgja was clicked; `x` is where, for the volume sliders.
    fn fylgja_act(&mut self, hit: fylgja::FHit, x: f32) {
        use fylgja::FAct;
        match hit.act {
            FAct::Section(i) => {
                if let Some(Popup { kind: PopupKind::Fylgja(st), .. }) = &mut self.popup {
                    st.section = i;
                    self.dirty = true;
                }
            }
            FAct::Expand => {
                if let Some(Popup { kind: PopupKind::Fylgja(st), .. }) = &mut self.popup {
                    st.open = true;
                    st.section = 0;
                    self.dirty = true;
                }
            }
            FAct::Prev | FAct::PlayPause | FAct::Next => {
                if let Some(m) = self.model.fylgja.snap.media.clone() {
                    fylgja::media_control(&m.bus, match hit.act { FAct::Prev => "Previous", FAct::Next => "Next", _ => "PlayPause" });
                }
            }
            FAct::Slide(who) => {
                let percent = (((x - hit.rect.0) / hit.rect.2) * 100.0).round().clamp(0.0, 100.0) as u32;
                fylgja::set_volume(&who, percent);
                // show it at once; the next reading confirms it
                let snap = &mut self.model.fylgja.snap;
                match who {
                    fylgja::Slider::Master => snap.master = snap.master.map(|(_, m)| (percent, m)),
                    fylgja::Slider::App(id) => {
                        if let Some(a) = snap.apps.iter_mut().find(|a| a.id == id) {
                            a.percent = percent;
                        }
                    }
                }
                self.dirty = true;
            }
            FAct::Clip(i) => {
                if let Some(t) = self.model.fylgja.clips.get(i).cloned() {
                    fylgja::copy_text(&t);
                    self.close_popup();
                }
            }
            FAct::ClipClear => {
                if let Ok(mut g) = self.clips.lock() {
                    g.clear();
                }
                self.model.fylgja.clips.clear();
                self.dirty = true;
            }
            FAct::Notif(a) => self.notif_act(a),
            FAct::Wall(p) => {
                if self.model.conf.own_wallpaper {
                    let _ = labyrinth_conf::set_wallpaper(&p.display().to_string());
                    for s in &mut self.surfs {
                        s.stamp = None; // redraw the wallpaper with the new picture
                    }
                    self.dirty = true;
                } else {
                    run(&["plasma-apply-wallpaperimage", &p.display().to_string()]);
                }
            }
            FAct::WallPage(by) => {
                let pages = self.model.fylgja.walls.len().div_ceil(8).max(1) as i32;
                self.model.fylgja.wall_page = (self.model.fylgja.wall_page as i32 + by).rem_euclid(pages) as usize;
                self.dirty = true;
            }
            FAct::VoiceStop => self.send_recording(),
            FAct::TopToggle => {
                self.model.fylgja.top_ram = !self.model.fylgja.top_ram;
                self.dirty = true;
            }
            FAct::SpeedTest => {
                stats::run_speedtest(self.model.fylgja.speed.clone());
                self.dirty = true;
            }
            FAct::Wifi => {
                let on = !self.model.fylgja.snap.wifi.as_ref().map(|w| w.0).unwrap_or(false);
                if let Some(w) = &mut self.model.fylgja.snap.wifi {
                    w.0 = on;
                    run(&["nmcli", "radio", "wifi", if on { "on" } else { "off" }]);
                }
                self.dirty = true;
            }
            FAct::Bt => {
                let on = !self.model.fylgja.snap.bt.as_ref().map(|b| b.0).unwrap_or(false);
                if let Some(b) = &mut self.model.fylgja.snap.bt {
                    b.0 = on;
                    run(&["bluetoothctl", "power", if on { "on" } else { "off" }]);
                }
                self.dirty = true;
            }
            FAct::WifiMore | FAct::BtMore => {
                run(&["setsid", "-f", "labyrinth-settings", if hit.act == FAct::WifiMore { "network" } else { "bluetooth" }]);
                self.close_popup();
            }
            FAct::Mute => {
                run(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
                let snap = &mut self.model.fylgja.snap;
                snap.master = snap.master.map(|(p, m)| (p, !m));
                self.dirty = true;
            }
            FAct::Picker => {
                if let Some(Popup { kind: PopupKind::Fylgja(st), .. }) = &mut self.popup {
                    st.picker = !st.picker;
                    self.dirty = true;
                }
            }
            FAct::Night => {
                let on = !self.model.fylgja.snap.night;
                fylgja::set_night(on);
                self.model.fylgja.snap.night = on;
                self.dirty = true;
            }
            FAct::Dark => {
                let dark = !self.model.dark;
                fylgja::set_dark(dark);
                self.model.dark = dark;
                self.dirty = true;
            }
            FAct::Mixer => {
                if let Some(Popup { kind: PopupKind::Fylgja(st), .. }) = &mut self.popup {
                    st.mixer = !st.mixer;
                    self.dirty = true;
                }
            }
            FAct::PickerClose => {
                if let Some(Popup { kind: PopupKind::Fylgja(st), .. }) = &mut self.popup {
                    st.picker = false;
                    self.dirty = true;
                }
            }
            FAct::Sink(name) => {
                if let Some(Popup { kind: PopupKind::Fylgja(st), .. }) = &mut self.popup {
                    st.picker = false;
                }
                run(&["pactl", "set-default-sink", &name]);
                self.model.fylgja.snap.default_sink = name;
                self.dirty = true;
            }
            FAct::Start(a) => self.start_act(a),
        }
    }

    /// The Folders theme: a folder opens to the side while the pointer is on it, and closes when it is on something else.
    fn start_hover(&mut self) {
        let (px, py) = self.pointer_pos;
        let over = self.start_hits.iter().rev().find(|h| px >= h.rect.0 && px < h.rect.0 + h.rect.2 && py >= h.rect.1 && py < h.rect.1 + h.rect.3).map(|h| h.act.clone());
        let Some(Popup { kind: PopupKind::Start(st), .. }) = &mut self.popup else { return };
        match over {
            Some(shell::StartAct::Folder(i)) if st.cat != Some(i) => {
                st.cat = Some(i);
                st.sub_scroll = 0;
            }
            Some(shell::StartAct::Launch(_)) if px < 316.0 => st.cat = None,
            _ => {}
        }
    }

    /// Something in the start menu was chosen.
    fn start_act(&mut self, act: shell::StartAct) {
        use shell::{PowerAct, StartAct};
        match act {
            StartAct::Launch(id) => {
                if let Some(app) = self.model.apps.iter().find(|a| a.id == id) {
                    apps::launch(app);
                }
            }
            StartAct::Folder(i) => {
                if let Some(Popup { kind: PopupKind::Start(st), .. }) = &mut self.popup {
                    st.cat = Some(i);
                    st.sub_scroll = 0;
                }
                self.dirty = true;
                return;
            }
            StartAct::TogglePower => {
                if let Some(Popup { kind: PopupKind::Start(st), .. }) = &mut self.popup {
                    st.power_open = !st.power_open;
                }
                self.dirty = true;
                return;
            }
            StartAct::Power(p) => match p {
                PowerAct::Lock => run(&["loginctl", "lock-session"]),
                PowerAct::Sleep => run(&["systemctl", "suspend"]),
                PowerAct::Hibernate => run(&["systemctl", "hibernate"]),
                // Plasma's own logout (it saves the session) when it is there, else end the login session
                PowerAct::Logout => run(&["sh", "-c", "qdbus6 org.kde.Shutdown /Shutdown org.kde.Shutdown.logout 2>/dev/null || loginctl terminate-session \"${XDG_SESSION_ID:-self}\""]),
                PowerAct::Restart => run(&["systemctl", "reboot"]),
                PowerAct::Shutdown => run(&["systemctl", "poweroff"]),
            },
        }
        self.close_popup();
    }

    /// A key was pressed while a popover has the keyboard.
    fn key_pressed(&mut self, key: u32) {
        if key == 1 {
            self.close_popup(); // Escape
            return;
        }
        let typed = self.xkb.as_ref().map(|x| x.key_get_utf8(xkb::Keycode::new(key + 8))).unwrap_or_default();
        let folders_theme = self.model.conf.start_theme == StartTheme::Folders;
        let (n_fav, n_folders) = shell::folders_counts(&self.model);
        let folder_apps = |m: &shell::Model, i: usize| apps::folders(&m.apps).get(i).map(|(_, l)| l.len()).unwrap_or(0);
        let power = shell::power_items(&self.model);
        let results: Vec<String> = match &self.popup {
            Some(Popup { kind: PopupKind::Start(st), .. }) => shell::search_apps(&self.model.apps, &st.query).iter().map(|a| a.id.clone()).collect(),
            _ => return,
        };
        let Some(Popup { kind: PopupKind::Start(st), .. }) = &mut self.popup else { return };
        use shell::StartFocus as F;
        let searching = !st.query.trim().is_empty();
        // what Enter does, worked out first (it needs the menu's data, not just its state)
        let mut act: Option<shell::StartAct> = None;
        match key {
            15 if folders_theme => {
                // Tab: search -> the list -> the power choices -> search (the list is skipped while there are search results)
                let next = match st.focus {
                    F::Search if searching => F::Power,
                    F::Search => F::List,
                    F::List | F::Sub => F::Power,
                    F::Power => F::Search,
                };
                st.focus = next;
                st.power_open = next == F::Power;
                if next == F::Search {
                    st.cat = None;
                }
                st.power_sel = 0;
                if next == F::List {
                    st.sel = st.sel.min((n_fav + n_folders).saturating_sub(1));
                }
            }
            103 | 108 if folders_theme => {
                let down = key == 108;
                match st.focus {
                    F::Search if searching => {
                        let last = results.len().min(12).saturating_sub(1);
                        st.sel = if down { (st.sel + 1).min(last) } else { st.sel.saturating_sub(1) };
                    }
                    F::Search => {
                        if down {
                            st.focus = F::List;
                        }
                    }
                    F::List => {
                        if down {
                            st.sel = (st.sel + 1).min((n_fav + n_folders).saturating_sub(1));
                        } else if st.sel == 0 {
                            st.focus = F::Search;
                        } else {
                            st.sel -= 1;
                        }
                        // a folder under the selection shows its apps at the side, like hovering it
                        st.cat = if st.sel >= n_fav { Some(st.sel - n_fav) } else { None };
                        st.sub_sel = 0;
                        st.sub_scroll = 0;
                    }
                    F::Sub => {
                        let last = st.cat.map(|c| folder_apps(&self.model, c)).unwrap_or(0).saturating_sub(1);
                        st.sub_sel = if down { (st.sub_sel + 1).min(last) } else { st.sub_sel.saturating_sub(1) };
                    }
                    F::Power => {
                        let last = power.len().saturating_sub(1);
                        st.power_sel = if down { (st.power_sel + 1).min(last) } else { st.power_sel.saturating_sub(1) };
                    }
                }
            }
            106 if folders_theme && st.focus == F::List && st.sel >= n_fav => {
                // Right on a folder: into its apps
                st.cat = Some(st.sel - n_fav);
                st.sub_sel = 0;
                st.sub_scroll = 0;
                st.focus = F::Sub;
            }
            105 if folders_theme && st.focus == F::Sub => st.focus = F::List,
            28 | 96 => match st.focus {
                F::Search => {
                    let i = if folders_theme { st.sel.min(results.len().saturating_sub(1)) } else { 0 };
                    act = results.get(i).map(|id| shell::StartAct::Launch(id.clone()));
                }
                F::List if st.sel < n_fav => {
                    act = shell::pinned_apps(&self.model).get(st.sel).map(|a| shell::StartAct::Launch(a.id.clone()));
                }
                F::List => {
                    st.cat = Some(st.sel - n_fav);
                    st.sub_sel = 0;
                    st.sub_scroll = 0;
                    st.focus = F::Sub;
                }
                F::Sub => {
                    act = st.cat.and_then(|c| apps::folders(&self.model.apps).get(c).and_then(|(_, l)| l.get(st.sub_sel).map(|a| shell::StartAct::Launch(a.id.clone()))));
                }
                F::Power => act = power.get(st.power_sel).map(|(_, a)| shell::StartAct::Power(*a)),
            },
            14 => {
                st.focus = F::Search;
                st.power_open = false;
                st.query.pop(); // Backspace
                st.scroll = 0;
                st.sel = 0;
            }
            _ => {
                let before = st.query.len();
                let chars: String = typed.chars().filter(|c| !c.is_control()).collect();
                if !chars.is_empty() {
                    // typing goes to the search box from anywhere
                    st.focus = F::Search;
                    st.power_open = false;
                    st.query.push_str(&chars);
                }
                if st.query.len() != before {
                    st.scroll = 0;
                    st.sel = 0;
                }
            }
        }
        if let Some(a) = act {
            self.start_act(a);
            return;
        }
        self.dirty = true;
    }

    fn add_strut(&mut self, output: u32) {
        let id = self.next_surf;
        self.next_surf += 1;
        let Some(out) = self.outputs.get(&output) else { return };
        let dock = shell::dock_size(&shell::dock_items(&self.model), self.model.conf.dock_icon, self.model.conf.dock_pos);
        let (anchor, size, zone) = self.strut_geometry(dock);
        let wl = self.compositor.create_surface(&self.qh, ());
        let layer = self.layer_shell.get_layer_surface(&wl, Some(&out.wl), Layer::Background, "labyrinth-dock-room".into(), &self.qh, id);
        layer.set_anchor(anchor);
        layer.set_size(size.0, size.1);
        layer.set_exclusive_zone(zone);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        wl.commit();
        self.surfs.push(Surf { id, kind: Kind::Strut, output, wl, layer: Some(layer), dock: None, w: size.0, h: size.1, want: (zone as u32, zone as u32), gap: 0, configured: false, bufs: vec![], hits: vec![], hover: None, frac: None, fscale: 0.0, stamp: None });
    }

    /// The strip's anchor, requested size and zone (the dock's thickness plus the gap it floats at).
    fn strut_geometry(&self, dock: (u32, u32)) -> (Anchor, (u32, u32), i32) {
        let gap = self.model.conf.dock_gap;
        match self.model.conf.dock_pos {
            DockPos::Bottom => (Anchor::Bottom | Anchor::Left | Anchor::Right, (0, 1), (dock.1 + gap) as i32),
            DockPos::Left => (Anchor::Left | Anchor::Top | Anchor::Bottom, (1, 0), (dock.0 + gap) as i32),
            DockPos::Right => (Anchor::Right | Anchor::Top | Anchor::Bottom, (1, 0), (dock.0 + gap) as i32),
        }
    }

    /// Where the dock's top left corner goes in the global space: against the edge it is on (the gap
    /// away from it), centred along it.
    fn dock_origin(&self, output: u32, dock: (u32, u32)) -> (i32, i32) {
        let gap = self.model.conf.dock_gap as i32;
        let (dw, dh) = (dock.0 as i32, dock.1 as i32);
        let area = self.area_of(output).map(|(x, y, w, h)| (x, y, w as i32, h as i32)).unwrap_or((0, 0, 1920, 1080));
        let (ox, oy, ow, oh) = area;
        match self.model.conf.dock_pos {
            DockPos::Bottom => (ox + (ow - dw) / 2, oy + oh - dh - gap),
            DockPos::Left => (ox + gap, oy + shell::BAR_H as i32 + (oh - shell::BAR_H as i32 - dh) / 2),
            DockPos::Right => (ox + ow - dw - gap, oy + shell::BAR_H as i32 + (oh - shell::BAR_H as i32 - dh) / 2),
        }
    }

    /// Ask for a new dock size if the items changed it.
    fn resize_dock(&mut self) {
        let pos = self.model.conf.dock_pos;
        let want = shell::dock_size(&shell::dock_items(&self.model), self.model.conf.dock_icon, pos);
        let gap = self.model.conf.dock_gap as i32;
        let (_, _, zone) = self.strut_geometry(want);
        let mut layer_updates: Vec<(usize, ZwlrLayerSurfaceV1)> = vec![];
        for i in 0..self.surfs.len() {
            if self.surfs[i].kind == Kind::Strut {
                if self.surfs[i].want.0 != zone as u32 {
                    let s = &mut self.surfs[i];
                    s.want = (zone as u32, zone as u32);
                    if let Some(l) = &s.layer {
                        l.set_exclusive_zone(zone);
                    }
                    s.wl.commit();
                }
                continue;
            }
            if self.surfs[i].kind != Kind::Dock || (self.surfs[i].want == want && self.surfs[i].gap == gap) {
                continue;
            }
            let (x, y) = self.dock_origin(self.surfs[i].output, want);
            let s = &mut self.surfs[i];
            s.want = want;
            s.gap = gap;
            s.w = want.0;
            s.h = want.1;
            if let Some(d) = &s.dock {
                d.tl.set_min_size(want.0 as i32, want.1 as i32);
                d.tl.set_max_size(want.0 as i32, want.1 as i32);
                d.ps.set_position(x, y);
            }
            if s.dock.is_none() {
                if let Some(l) = s.layer.clone() {
                    layer_updates.push((i, l));
                }
            }
        }
        // a layer-shell dock (no KWin): ask for the new size and room
        for (i, l) in layer_updates {
            self.configure_layer_dock(&l, want);
            self.surfs[i].wl.commit();
        }
    }

    fn redraw(&mut self) {
        self.sync_notch();
        self.resize_dock();
        for i in 0..self.surfs.len() {
            self.draw(i);
        }
        let _ = self.conn.flush();
        self.dirty = false;
    }

    fn draw(&mut self, i: usize) {
        let slide = self.fy_slide();
        let (kind, w, h, hover, configured) = {
            let s = &self.surfs[i];
            (s.kind, s.w, s.h, s.hover, s.configured)
        };
        if !configured || w == 0 || h == 0 {
            return;
        }
        self.scale_surface(i);
        let s = self.scale_of(&self.surfs[i]);
        let (pw, ph) = ((w as f32 * s).round() as u32, (h as f32 * s).round() as u32);
        let wall_path = if kind == Kind::Wallpaper { labyrinth_conf::wallpaper() } else { None };
        if kind == Kind::Wallpaper {
            let key = (wall_path.clone().unwrap_or_default(), pw, ph);
            if self.surfs[i].stamp.as_ref() == Some(&key) {
                return; // already showing this picture at this size
            }
            self.surfs[i].stamp = Some(key);
        }
        let Some(mut pix) = Pixmap::new(pw, ph) else { return };
        let mut start_scroll: Option<(i32, i32)> = None;
        let hits = match kind {
            Kind::Strut => vec![],
            Kind::Wallpaper => {
                let path = wall_path.unwrap_or_default();
                let cached = self.wall_cache.iter().position(|(k, _)| k.0 == path && k.1 == pw && k.2 == ph);
                let idx = match cached {
                    Some(i) => Some(i),
                    None => fylgja::load_cover(&path, pw, ph).map(|pm| {
                        self.wall_cache.push(((path.clone(), pw, ph), pm));
                        while self.wall_cache.len() > 4 {
                            self.wall_cache.remove(0);
                        }
                        self.wall_cache.len() - 1
                    }),
                };
                match idx {
                    Some(i) => paint::draw_pixmap(&mut pix, &self.wall_cache[i].1, 0, 0),
                    None => pix.fill(paint::color((12, 12, 17, 255))),
                }
                vec![]
            }
            Kind::Osd => {
                if let Some(st) = self.osd {
                    osd::paint(&mut pix, s, &st, &self.assets.fonts, &mut self.assets.icons);
                }
                vec![]
            }
            Kind::Voice => {
                if let Some(v) = self.viz {
                    voicewave::paint(&mut pix, s, &v, voicewave::palette(self.model.conf.sarina_wave));
                }
                vec![]
            }
            Kind::Toast => {
                let list: Vec<notify::Notif> = self.toasts.iter().filter_map(|id| self.model.notifs.get(*id).cloned()).collect();
                let hits = notify::paint_toasts(&mut pix, s, &list, hover, &self.assets.fonts, &mut self.assets.icons);
                // only the cards take clicks: the rest of this surface is see-through, for whatever is under it
                let region = self.compositor.create_region(&self.qh, ());
                for h in &hits {
                    if matches!(h.action, Action::ToastOpen(_)) {
                        region.add((h.rect.0 / s) as i32, (h.rect.1 / s) as i32, (h.rect.2 / s).ceil() as i32, (h.rect.3 / s).ceil() as i32);
                    }
                }
                self.surfs[i].wl.set_input_region(Some(&region));
                region.destroy();
                hits
            }
            Kind::Popup => {
                match &self.popup {
                    Some(Popup { kind: PopupKind::Start(st), .. }) => {
                        let sp = if self.model.conf.start_theme == StartTheme::Folders {
                            let view = shell::FoldersView { query: &st.query, scroll: st.scroll, focus: st.focus, sel: st.sel, sub_sel: st.sub_sel, power_sel: st.power_sel, cat: st.cat, sub_scroll: st.sub_scroll, power_open: st.power_open };
                            let sp = shell::paint_start_folders(&mut pix, s, &self.model, &view, self.pointer_pos, &self.assets.fonts, &mut self.assets.icons);
                            start_scroll = Some((sp.scroll, sp.sub_scroll));
                            sp
                        } else {
                            shell::paint_start(&mut pix, s, &self.model, &st.query, st.scroll, self.pointer_pos, &self.assets.fonts, &mut self.assets.icons)
                        };
                        self.popup_card = sp.card;
                        self.start_hits = sp.hits;
                        self.start_max = sp.max_scroll;
                    }
                    Some(Popup { kind: PopupKind::Fylgja(st), .. }) => {
                        let (card, hits) = fylgja::paint(&mut pix, s, &self.model, st, slide, self.pointer_pos, &self.assets.fonts, &mut self.assets.icons);
                        self.popup_card = card;
                        self.fy_hits = hits;
                    }
                    Some(Popup { kind: PopupKind::Tray(pop), .. }) => {
                        let pop = pop.clone();
                        let (card, hits) = trayui::paint(&mut pix, s, &pop, &self.model, self.pointer_pos, &self.assets.fonts, &mut self.assets.icons);
                        self.popup_card = card;
                        self.tray_hits = hits;
                    }
                    Some(Popup { kind: PopupKind::Calendar { offset }, .. }) => {
                        let pane = self.model.conf.notify_pane;
                        let card = shell::paint_calendar(&mut pix, s, &self.model, *offset, &self.assets.fonts, pane);
                        if pane {
                            // the Notification Center: the list under the calendar, in a pane down the right edge
                            let (w, h) = (pix.width() as f32 / s, pix.height() as f32 / s);
                            self.popup_card = shell::pane_rect(w, h);
                            let top = card.1 + card.3 + 10.0;
                            let area = (card.0, top, card.2, h - top - 10.0);
                            self.pane_hits = notify::paint_list(&mut pix, s, area, &self.model.notifs, self.pointer_pos, notify::now_secs(), &self.assets.fonts, &mut self.assets.icons);
                            self.pane_top = top;
                        } else {
                            self.popup_card = card;
                            self.pane_hits.clear();
                        }
                    }
                    None => {}
                }
                vec![]
            }
            Kind::Bar { slim } => shell::paint_bar(&mut pix, s, slim, &self.model, &self.assets.fonts, &mut self.assets.icons, hover),
            Kind::Dock => shell::paint_dock(&mut pix, s, &self.model, &self.assets.fonts, &mut self.assets.icons, hover),
        };
        if let Some((sc, ssc)) = start_scroll {
            if let Some(Popup { kind: PopupKind::Start(st), .. }) = &mut self.popup {
                st.scroll = sc;
                st.sub_scroll = ssc;
            }
        }
        let surf = &mut self.surfs[i];
        surf.hits = hits;
        let idx = match surf.bufs.iter().position(|b| !b.busy.load(Ordering::Acquire) && b.w == pw && b.h == ph) {
            Some(i) => i,
            None => {
                surf.bufs.retain(|b| b.busy.load(Ordering::Acquire) || (b.w == pw && b.h == ph));
                match new_buffer(&self.shm, &self.qh, pw, ph) {
                    Some(b) => {
                        surf.bufs.push(b);
                        surf.bufs.len() - 1
                    }
                    None => return,
                }
            }
        };
        let b = &mut surf.bufs[idx];
        // tiny-skia is premultiplied RGBA; wl_shm's ARGB8888 (little-endian) is BGRA in memory.
        for (dst, src) in b.map.chunks_exact_mut(4).zip(pix.data().chunks_exact(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        b.busy.store(true, Ordering::Release);
        match &surf.frac {
            // the buffer is `s` times the surface; the viewport says how big the surface is
            Some((_, vp)) => vp.set_destination(w as i32, h as i32),
            None => surf.wl.set_buffer_scale(s.round().max(1.0) as i32),
        }
        surf.wl.attach(Some(&b.buffer), 0, 0);
        surf.wl.damage_buffer(0, 0, pw as i32, ph as i32);
        surf.wl.commit();
    }

    // -------------------------------------------------------------- actions

    /// Describe the bar, dock and pop-ups to screen readers (only sent when something a reader can see changed).
    fn a11y_sync(&mut self) {
        let Some(a) = self.a11y.as_mut() else { return };
        let mut parts: Vec<(a11y::Part, Vec<Hit>)> = vec![];
        for s in &self.surfs {
            let part = match s.kind {
                Kind::Bar { .. } => a11y::Part::Bar,
                Kind::Dock => a11y::Part::Dock,
                Kind::Toast => a11y::Part::Toasts,
                _ => continue,
            };
            parts.push((part, s.hits.clone()));
        }
        a.sync(&self.model, &parts);
    }

    fn click(&mut self, action: Action) {
        match action {
            Action::Logo => self.toggle_start(),
            Action::Clock => self.toggle_calendar(),
            Action::Notch => {
                self.open_full_from_tab();
            }
            Action::NotchSection(i) => {
                if let Some(Popup { kind: PopupKind::Fylgja(st), .. }) = &mut self.popup {
                    st.section = i;
                    self.dirty = true;
                }
            }
            Action::Workspace(i) => {
                if let Some(d) = self.model.status.desktops.get(i) {
                    run(&["busctl", "--user", "set-property", "org.kde.KWin", "/VirtualDesktopManager", "org.kde.KWin.VirtualDesktopManager", "current", "s", &d.id]);
                    self.model.status.current = i;
                    self.dirty = true;
                }
            }
            Action::NewWorkspace => {
                let n = self.model.status.desktops.len();
                if n < status::MAX_DESKTOPS {
                    run(&["busctl", "--user", "call", "org.kde.KWin", "/VirtualDesktopManager", "org.kde.KWin.VirtualDesktopManager", "createDesktop", "us", &n.to_string(), &format!("Desktop {}", n + 1)]);
                }
            }
            Action::Network => run(&["labyrinth-settings", "network"]),
            Action::Bluetooth => run(&["labyrinth-settings", "bluetooth"]),
            Action::Dock(app_id) => self.click_dock(&app_id),
            Action::DockWin(_, wi) => {
                if let Some(w) = self.model.wins.get(wi) {
                    w.proxy.activate(self.seat.as_ref());
                }
            }
            Action::ToastOpen(id) => self.open_notification(id),
            Action::ToastAction(id, i) => {
                if let Some(n) = self.model.notifs.get(id).cloned() {
                    if let Some((key, _)) = notify::toast_actions(&n).get(i) {
                        notify::emit_action(id, key);
                    }
                }
                self.remove_notif(id, 2);
            }
            Action::Tray(key) => self.tray_click(&key, "Activate"),
            Action::TrayMore => {
                let x = self.pointer_pos.0;
                self.open_tray_popup(trayui::TrayPop { anchor_x: x, mode: trayui::TrayMode::Overflow });
            }
            Action::Notifications => {
                if self.model.conf.notify_pane {
                    self.toggle_calendar();
                } else {
                    self.show_fylgja_section(fylgja::NOTIFS);
                }
            }
            Action::ToastClose(id) => {
                self.toasts.retain(|t| *t != id);
                self.sync_toasts();
                self.dirty = true;
            }
        }
    }

    fn click_dock(&mut self, app_id: &str) {
        let items = shell::dock_items(&self.model);
        let Some(item) = items.iter().find(|i| i.app_id == app_id) else { return };
        if app_id == shell::TRASH_ID {
            // Minos, on the trash folder.
            if let Some(dir) = shell::trash_dir() {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("setsid").arg("-f").arg("minos").arg(dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
            }
            return;
        }
        if item.wins.is_empty() {
            if let Some(app) = self.model.apps.iter().find(|a| a.id == app_id) {
                apps::launch(app);
            }
            return;
        }
        // One window: focus it, or minimize it if it already has focus. Several: go to the next.
        let wins = &self.model.wins;
        let active_pos = item.wins.iter().position(|&i| wins[i].active());
        let target = match active_pos {
            Some(p) if item.wins.len() == 1 => {
                wins[item.wins[p]].proxy.minimize();
                return;
            }
            Some(p) => item.wins[(p + 1) % item.wins.len()],
            None => item.wins[0],
        };
        wins[target].proxy.activate(self.seat.as_ref());
    }

    fn surf_index(&self, wl: &WlSurface) -> Option<usize> {
        self.surfs.iter().position(|s| &s.wl == wl)
    }

    fn set_cursor(&self, serial: u32) {
        if let Some(c) = &self.cursor {
            c.set_shape(serial, Shape::Default);
        }
    }

    /// Which dock app the pointer is on (its id, its icon's centre along the screen), if any.
    fn dock_item_at(&self, i: usize, new: Option<usize>) -> Option<(String, f32)> {
        if self.surfs[i].kind != Kind::Dock {
            return None;
        }
        let hit = self.surfs[i].hits.get(new?)?;
        let id = match &hit.action {
            Action::Dock(id) | Action::DockWin(id, _) => id.clone(),
            _ => return None,
        };
        let s = self.scale_of(&self.surfs[i]);
        let origin = self.dock_origin(self.surfs[i].output, self.surfs[i].want);
        let pos = self.model.conf.dock_pos;
        let (w, _) = self.surfs[i].want;
        let along = if pos.vertical() {
            match pos {
                DockPos::Left => origin.0 as f32 + w as f32 + 150.0 + 8.0,
                _ => origin.0 as f32 - 150.0 - 8.0,
            }
        } else {
            origin.0 as f32 + (hit.rect.0 + hit.rect.2 / 2.0) / s
        };
        Some((id, along))
    }

    fn hover_at(&mut self, x: f32, y: f32) {
        let Some(i) = self.pointer_surface else { return };
        let s = self.scale_of(&self.surfs[i]);
        let new = self.surfs[i].hits.iter().position(|h| h.contains(x * s, y * s));
        // resting on a dock app starts the second after which its windows are listed
        match self.dock_item_at(i, new) {
            Some((id, along)) if id != shell::TRASH_ID => {
                if self.dock_hover.as_ref().map(|h| h.0 != id).unwrap_or(true) {
                    self.dock_hover = Some((id.clone(), Instant::now(), along, self.surfs[i].output));
                    if matches!(self.popup, Some(Popup { kind: PopupKind::Tray(trayui::TrayPop { mode: trayui::TrayMode::Windows { .. }, .. }), .. })) {
                        self.close_popup();
                    }
                }
                self.preview_close = None;
            }
            _ => {
                if self.surfs[i].kind == Kind::Dock {
                    self.dock_hover = None;
                    if self.preview_close.is_none() && self.windows_popup_open() {
                        self.preview_close = Some(Instant::now() + Duration::from_millis(350));
                    }
                }
            }
        }
        if new != self.surfs[i].hover {
            self.surfs[i].hover = new;
            self.dirty = true;
        }
        // Fylgja opens when the pointer has rested on the notch's tab for a moment, and closes when it leaves
        if matches!(self.surfs[i].kind, Kind::Bar { .. }) {
            let on_tab = new.and_then(|h| self.surfs[i].hits.get(h)).map(|h| h.action == Action::Notch || matches!(h.action, Action::NotchSection(_))).unwrap_or(false);
            let out = self.surfs[i].output;
            self.tab_hover = on_tab;
            if on_tab {
                self.tab_output = out;
                if !self.tab_on && self.popup.is_none() {
                    self.tab_on = true;
                    self.tab_output = out;
                    self.tab_gen += 1;
                    let (g, tx) = (self.tab_gen, self.tab_tx.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(150));
                        let _ = tx.send(g);
                    });
                }
            } else {
                self.tab_on = false;
                if matches!(&self.popup, Some(Popup { kind: PopupKind::Fylgja(st), .. }) if !st.pinned) {
                    self.retract_fylgja();
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Wayland dispatch

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(state: &mut Self, registry: &WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_output" => {
                let wl = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, name);
                let xdg = state.xdg_output_mgr.as_ref().map(|m| m.get_xdg_output(&wl, qh, name));
                state.outputs.insert(name, Out { wl, name: String::new(), scale: 1, ready: false, logical: None, xdg });
            }
            wl_registry::Event::GlobalRemove { name } => {
                if state.outputs.remove(&name).is_some() {
                    state.built_for = None; // a screen went away: rebuild
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for State {
    fn event(state: &mut Self, _: &WlOutput, event: wl_output::Event, key: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(o) = state.outputs.get_mut(key) else { return };
        match event {
            wl_output::Event::Name { name } => o.name = name,
            wl_output::Event::Scale { factor } => {
                o.scale = factor.max(1);
                state.dirty = true;
            }
            wl_output::Event::Done => {
                if !o.ready {
                    o.ready = true;
                    state.built_for = None;
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ZxdgOutputManagerV1);

impl Dispatch<ZxdgOutputV1, u32> for State {
    fn event(state: &mut Self, _: &ZxdgOutputV1, event: zxdg_output_v1::Event, key: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(o) = state.outputs.get_mut(key) else { return };
        let (mut x, mut y, mut w, mut h) = o.logical.unwrap_or((0, 0, 0, 0));
        match event {
            zxdg_output_v1::Event::LogicalPosition { x: nx, y: ny } => {
                x = nx;
                y = ny;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                w = width.max(0) as u32;
                h = height.max(0) as u32;
            }
            zxdg_output_v1::Event::Done if w > 0 && h > 0 => {}
            _ => return,
        }
        let new = Some((x, y, w, h));
        if new != o.logical && w > 0 && h > 0 {
            o.logical = new;
            state.built_for = None; // the screen's size or place changed: place the dock and the popups again
            state.dirty = true;
        } else if w == 0 || h == 0 {
            o.logical = new;
        }
    }
}

impl Dispatch<WlRegion, ()> for State {
    fn event(_: &mut Self, _: &WlRegion, _: <WlRegion as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(_: &mut Self, wm: &XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, usize> for State {
    fn event(state: &mut Self, xs: &XdgSurface, event: xdg_surface::Event, id: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            xs.ack_configure(serial);
            let Some(i) = state.surfs.iter().position(|s| s.id == *id) else { return };
            let want = state.surfs[i].want;
            state.surfs[i].w = want.0;
            state.surfs[i].h = want.1;
            state.surfs[i].configured = true;
            state.draw(i);
            let _ = state.conn.flush();
        }
    }
}

impl Dispatch<XdgToplevel, usize> for State {
    fn event(_: &mut Self, _: &XdgToplevel, _: xdg_toplevel::Event, _: &usize, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<OrgKdePlasmaShell, ()> for State {
    fn event(_: &mut Self, _: &OrgKdePlasmaShell, _: <OrgKdePlasmaShell as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<OrgKdePlasmaSurface, usize> for State {
    fn event(_: &mut Self, _: &OrgKdePlasmaSurface, _: org_kde_plasma_surface::Event, _: &usize, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlSurface, ()> for State {
    fn event(_: &mut Self, _: &WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlBuffer, Arc<AtomicBool>> for State {
    fn event(_: &mut Self, _: &WlBuffer, event: wl_buffer::Event, busy: &Arc<AtomicBool>, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event {
            busy.store(false, Ordering::Release);
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, usize> for State {
    fn event(state: &mut Self, layer: &ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, id: &usize, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer.ack_configure(serial);
                let Some(i) = state.surfs.iter().position(|s| s.id == *id) else { return };
                if std::env::var_os("LABYRINTH_PANEL_DEBUG").is_some() {
                    eprintln!("configure surface {} ({:?}): {}x{}", id, state.surfs[i].kind, width, height);
                }
                let s = &mut state.surfs[i];
                if width > 0 {
                    s.w = width;
                }
                if height > 0 {
                    s.h = height;
                }
                s.configured = true;
                state.draw(i);
                let _ = state.conn.flush();
            }
            zwlr_layer_surface_v1::Event::Closed => {
                // The screen went away or the compositor revoked the surface; rebuild from scratch.
                if let Some(i) = state.surfs.iter().position(|s| s.id == *id) {
                    let s = state.surfs.remove(i);
                    s.destroy();
                    state.pointer_surface = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = event {
            if c.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if c.contains(wl_seat::Capability::Pointer) && state.seat.is_none() {
                state.seat = Some(seat.clone());
                let p = seat.get_pointer(qh, ());
                if let Some(m) = &state.cursor_mgr {
                    state.cursor = Some(m.get_pointer(&p, qh, ()));
                }
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(state: &mut Self, _: &WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Keymap { format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1), fd, size } => {
                let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                // SAFETY: the compositor hands us a read-only descriptor of exactly `size` bytes.
                if let Ok(Some(km)) = unsafe { xkb::Keymap::new_from_fd(&ctx, fd, size as usize, xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS) } {
                    state.xkb = Some(xkb::State::new(&km));
                }
            }
            wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                if let Some(x) = &mut state.xkb {
                    x.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            wl_keyboard::Event::Key { key, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } => state.key_pressed(key),
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(state: &mut Self, _: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                state.pointer_surface = state.surf_index(&surface);
                state.pointer_pos = (surface_x as f32, surface_y as f32);
                state.set_cursor(serial);
                if state.fylgja_pointer_moved() || state.preview_pointer_moved() {
                    return;
                }
                state.hover_at(surface_x as f32, surface_y as f32);
            }
            wl_pointer::Event::Leave { .. } => {
                if state.popup.is_none() {
                    state.tab_on = false;
                    state.tab_hover = false;
                }
                if let Some(i) = state.pointer_surface {
                    if state.surfs.get(i).map(|s| s.kind == Kind::Dock).unwrap_or(false) {
                        state.dock_hover = None;
                        if state.windows_popup_open() && state.preview_close.is_none() {
                            state.preview_close = Some(Instant::now() + Duration::from_millis(350));
                        }
                    }
                }
                if let Some(i) = state.pointer_surface.take() {
                    if state.surfs.get_mut(i).map(|s| s.hover.take().is_some()).unwrap_or(false) {
                        state.dirty = true;
                    }
                }
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                state.pointer_pos = (surface_x as f32, surface_y as f32);
                if state.drag.is_some() {
                    state.drag_apply(false);
                    return;
                }
                if state.fylgja_pointer_moved() || state.preview_pointer_moved() {
                    return;
                }
                if matches!(state.popup, Some(Popup { kind: PopupKind::Start(_), .. })) {
                    state.dirty = true; // the row under the pointer lights up
                    state.start_hover();
                }
                state.hover_at(surface_x as f32, surface_y as f32)
            }
            wl_pointer::Event::AxisSource { axis_source } => state.axis_wheel = matches!(axis_source, WEnum::Value(wl_pointer::AxisSource::Wheel)),
            // a wheel click: one step
            wl_pointer::Event::AxisDiscrete { discrete, .. } => {
                if state.popup.is_some() {
                    state.step_month(discrete.signum());
                } else {
                    state.open_full_from_tab();
                }
            }
            // smooth scrolling (a touchpad): a step for each stretch of scrolling
            wl_pointer::Event::Axis { axis: WEnum::Value(wl_pointer::Axis::VerticalScroll), value, .. } => {
                if state.popup.is_some() && !state.axis_wheel {
                    state.axis_acc += value as f32;
                    if state.axis_acc.abs() >= 20.0 {
                        let by = state.axis_acc.signum() as i32;
                        state.axis_acc = 0.0;
                        state.step_month(by);
                    }
                }
            }
            wl_pointer::Event::Button { state: WEnum::Value(wl_pointer::ButtonState::Released), .. } => {
                if state.drag.is_some() {
                    state.drag_apply(true);
                    state.drag = None;
                }
            }
            wl_pointer::Event::Button { state: WEnum::Value(wl_pointer::ButtonState::Pressed), button, .. } => {
                // 272 = BTN_LEFT
                if button == 272 {
                    // On the popover: the header takes you back to this month; outside the card closes it.
                    if state.pointer_surface.and_then(|i| state.surfs.get(i)).map(|s| s.kind == Kind::Popup).unwrap_or(false) {
                        let (px, py) = state.pointer_pos;
                        let (cx, cy, cw, ch) = state.popup_card;
                        if px < cx || px > cx + cw || py < cy || py > cy + ch {
                            state.close_popup();
                        } else if matches!(state.popup, Some(Popup { kind: PopupKind::Fylgja(_), .. })) {
                            // the last drawn is on top (the buttons over the card)
                            let hit = state.fy_hits.iter().rev().find(|h| px >= h.rect.0 && px < h.rect.0 + h.rect.2 && py >= h.rect.1 && py < h.rect.1 + h.rect.3).cloned();
                            if let Some(hit) = hit {
                                if let fylgja::FAct::Slide(who) = &hit.act {
                                    // pressing on a slider sets the level at once and then follows the pointer
                                    state.drag = Some((who.clone(), hit.rect));
                                    state.drag_sent.1 = 101;
                                    state.drag_apply(true);
                                } else {
                                    state.fylgja_act(hit, px);
                                }
                            }
                        } else if matches!(state.popup, Some(Popup { kind: PopupKind::Start(_), .. })) {
                            // the last drawn is on top (the power choices over the list)
                            let hit = state.start_hits.iter().rev().find(|h| px >= h.rect.0 && px < h.rect.0 + h.rect.2 && py >= h.rect.1 && py < h.rect.1 + h.rect.3).map(|h| h.act.clone());
                            if let Some(act) = hit {
                                state.start_act(act);
                            } else if let Some(Popup { kind: PopupKind::Start(st), .. }) = &mut state.popup {
                                st.power_open = false; // a click elsewhere puts the power choices away
                                state.dirty = true;
                            }
                        } else if matches!(state.popup, Some(Popup { kind: PopupKind::Tray(_), .. })) {
                            let hit = state.tray_hits.iter().rev().find(|(r, _)| px >= r.0 && px < r.0 + r.2 && py >= r.1 && py < r.1 + r.3).map(|(_, a)| a.clone());
                            if let Some(a) = hit {
                                state.tray_row(a);
                            }
                        } else if state.model.conf.notify_pane && py >= state.pane_top {
                            let hit = state.pane_hits.iter().rev().find(|(r, _)| px >= r.0 && px < r.0 + r.2 && py >= r.1 && py < r.1 + r.3).map(|(_, a)| a.clone());
                            if let Some(a) = hit {
                                state.notif_act(a);
                            }
                        } else if py < cy + 62.0 {
                            // the calendar's header: back to this month
                            if let Some(Popup { kind: PopupKind::Calendar { offset }, .. }) = &mut state.popup {
                                *offset = 0;
                            }
                            state.dirty = true;
                        }
                        return;
                    }
                    if let Some(i) = state.pointer_surface {
                        state.click_output = state.surfs.get(i).map(|s| s.output);
                        if let Some(h) = state.surfs.get(i).and_then(|s| s.hover) {
                            if let Some(hit) = state.surfs[i].hits.get(h).cloned() {
                                state.click(hit.action);
                            }
                        }
                    }
                } else if button == 273 {
                    // on a dock app: its menu (new window, close, keep in dock)
                    if let Some(i) = state.pointer_surface {
                        let hover = state.surfs.get(i).and_then(|s| s.hover);
                        if let Some((id, along)) = state.dock_item_at(i, hover) {
                            let out = state.surfs[i].output;
                            state.dock_hover = None;
                            state.open_dock_menu(&id, along, out);
                            return;
                        }
                    }
                    // 273 = BTN_RIGHT: on a tray icon it asks the app for its menu
                    if let Some(i) = state.pointer_surface {
                        if let Some(h) = state.surfs.get(i).and_then(|s| s.hover) {
                            if let Some(Hit { action: Action::Tray(key), .. }) = state.surfs[i].hits.get(h).cloned() {
                                state.tray_click(&key, "ContextMenu");
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<OrgKdePlasmaWindowManagement, ()> for State {
    fn event(state: &mut Self, wm: &OrgKdePlasmaWindowManagement, event: org_kde_plasma_window_management::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let org_kde_plasma_window_management::Event::WindowWithUuid { uuid, .. } = event {
            state.next_key += 1;
            let key = state.next_key;
            let proxy = wm.get_window_by_uuid(uuid.clone(), qh, key);
            state.model.wins.push(Win { key, uuid, title: String::new(), app_id: String::new(), flags: 0, ready: false, menu: None, proxy: shell::WinHandle::Plasma(proxy) });
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(state: &mut Self, _: &ZwlrForeignToplevelManagerV1, event: zwlr_foreign_toplevel_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.next_key += 1;
            let key = state.next_key;
            state.model.wins.push(Win { key, uuid: format!("wlr-{key}"), title: String::new(), app_id: String::new(), flags: 0, ready: false, menu: None, proxy: shell::WinHandle::Wlr(toplevel) });
        }
    }

    wayland_client::event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(state: &mut Self, h: &ZwlrForeignToplevelHandleV1, event: zwlr_foreign_toplevel_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_foreign_toplevel_handle_v1::Event as E;
        let Some(pos) = state.model.wins.iter().position(|w| matches!(&w.proxy, shell::WinHandle::Wlr(x) if x == h)) else { return };
        match event {
            E::Title { title } => state.model.wins[pos].title = title,
            E::AppId { app_id } => state.model.wins[pos].app_id = app_id,
            E::State { state: bytes } => {
                // an array of u32 states: 0 maximized, 1 minimized, 2 activated, 3 fullscreen
                let mut flags = 0;
                for c in bytes.chunks_exact(4) {
                    match u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) {
                        1 => flags |= shell::ST_MINIMIZED,
                        2 => flags |= shell::ST_ACTIVE,
                        _ => {}
                    }
                }
                state.model.wins[pos].flags = flags;
            }
            E::Done => state.model.wins[pos].ready = true,
            E::Closed => {
                let w = state.model.wins.remove(pos);
                w.proxy.destroy();
            }
            _ => return,
        }
        state.dirty = true;
    }
}

impl Dispatch<OrgKdePlasmaWindow, u32> for State {
    fn event(state: &mut Self, _: &OrgKdePlasmaWindow, event: org_kde_plasma_window::Event, key: &u32, _: &Connection, _: &QueueHandle<Self>) {
        use org_kde_plasma_window::Event as E;
        let Some(pos) = state.model.wins.iter().position(|w| w.key == *key) else { return };
        match event {
            E::TitleChanged { title } => state.model.wins[pos].title = title,
            E::AppIdChanged { app_id } => state.model.wins[pos].app_id = app_id,
            E::StateChanged { flags } => state.model.wins[pos].flags = flags,
            E::ApplicationMenu { service_name, object_path } => state.model.wins[pos].menu = Some((service_name, object_path)),
            E::InitialState => state.model.wins[pos].ready = true,
            E::Unmapped => {
                let w = state.model.wins.remove(pos);
                w.proxy.destroy();
            }
            _ => return,
        }
        state.dirty = true;
    }
}

impl Dispatch<WpFractionalScaleV1, usize> for State {
    fn event(state: &mut Self, _: &WpFractionalScaleV1, event: wp_fractional_scale_v1::Event, id: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            if let Some(s) = state.surfs.iter_mut().find(|s| s.id == *id) {
                s.fscale = scale as f32 / 120.0;
                state.dirty = true;
            }
        }
    }
}
delegate_noop!(State: WpFractionalScaleManagerV1);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);

impl Dispatch<WpCursorShapeManagerV1, ()> for State {
    fn event(_: &mut Self, _: &WpCursorShapeManagerV1, _: <WpCursorShapeManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WpCursorShapeDeviceV1, ()> for State {
    fn event(_: &mut Self, _: &WpCursorShapeDeviceV1, _: <WpCursorShapeDeviceV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(State: WlCompositor);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ZwlrLayerShellV1);

// ------------------------------------------------------------------ main

fn bind_outputs(globals: &GlobalList, state: &mut State, qh: &QueueHandle<State>) {
    let existing: Vec<(u32, u32)> = globals.contents().clone_list().iter().filter(|g| g.interface == "wl_output").map(|g| (g.name, g.version)).collect();
    for (name, version) in existing {
        let wl = globals.registry().bind::<WlOutput, _, _>(name, version.min(4), qh, name);
        let xdg = state.xdg_output_mgr.as_ref().map(|m| m.get_xdg_output(&wl, qh, name));
        state.outputs.insert(name, Out { wl, name: String::new(), scale: 1, ready: false, logical: None, xdg });
    }
}

fn main() {
    let Some(fonts) = paint::Fonts::load() else {
        eprintln!("labyrinth-panel: no sans-serif font found (fc-match)");
        std::process::exit(1);
    };
    let conn = Connection::connect_to_env().unwrap_or_else(|e| {
        eprintln!("labyrinth-panel: cannot connect to Wayland: {e}");
        std::process::exit(1);
    });
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = queue.handle();
    if std::env::var_os("LABYRINTH_PANEL_DEBUG").is_some() {
        globals.contents().with_list(|l| {
            for g in l {
                eprintln!("global {} v{}", g.interface, g.version);
            }
        });
    }

    let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).expect("wl_compositor");
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
    let xdg_wm: Option<XdgWmBase> = globals.bind(&qh, 1..=6, ()).ok();
    let xdg_output_mgr: Option<ZxdgOutputManagerV1> = globals.bind(&qh, 1..=3, ()).ok();
    let frac_mgr: Option<WpFractionalScaleManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    let plasma_shell: Option<OrgKdePlasmaShell> = globals.bind(&qh, 1..=8, ()).ok();
    let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).unwrap_or_else(|_| {
        eprintln!("labyrinth-panel: the compositor has no wlr-layer-shell");
        std::process::exit(1);
    });
    let _seat: Option<WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
    let cursor_mgr: Option<WpCursorShapeManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    let wm: Option<OrgKdePlasmaWindowManagement> = globals.bind(&qh, 1..=18, ()).ok();
    // Without KWin's protocol the standard one (Hyprland, Sway, labwc ...) gives the window list
    let wlr_wm: Option<ZwlrForeignToplevelManagerV1> = if wm.is_none() { globals.bind(&qh, 1..=3, ()).ok() } else { None };
    if wm.is_none() && wlr_wm.is_none() {
        eprintln!("labyrinth-panel: the compositor offers no window management (KWin needs labyrinth-panel.desktop installed with X-KDE-Wayland-Interfaces); the dock will only launch apps");
    } else if wlr_wm.is_some() {
        eprintln!("labyrinth-panel: window list through wlr-foreign-toplevel-management");
    }

    let (tab_tx, tab_rx) = calloop::channel::channel::<u64>();
    let (anim_tx, anim_rx) = calloop::channel::channel::<()>();
    let (osd_hide_tx, osd_hide_rx) = calloop::channel::channel::<u64>();
    let (toast_hide_tx, toast_hide_rx) = calloop::channel::channel::<u32>();
    let (voice_tx, voice_rx) = calloop::channel::channel::<Result<String, String>>();
    let (heard_tx, heard_rx) = calloop::channel::channel::<String>();
    let (viz_tx, viz_rx) = calloop::channel::channel::<()>();
    let (vkey_tx, vkey_rx) = calloop::channel::channel::<voicekey::Key>();
    let mut state = State {
        qh: qh.clone(),
        conn: conn.clone(),
        compositor,
        layer_shell,
        xdg_wm,
        xdg_output_mgr,
        frac_mgr,
        viewporter,
        plasma_shell,
        shm,
        model: Model { apps: apps::load_all(), pinned: apps::pinned(), wins: vec![], clock: clock_text(PanelConf::load().clock_24h), clock_secs: secs_text(PanelConf::load().clock_24h), date: date_text(), today: today_ymd(), user: user_name(), hibernate: shell::can_hibernate(), conf: PanelConf::load(), dark: system_dark(), status: Default::default(), trash_full: shell::trash_has_files(), notch_open: false, fylgja: Default::default(), notch_w: 0.0, notch_tab: None, notifs: notify::Store::load(notify::state_path()), tray: vec![] },
        a11y: None,
        assets: Assets { fonts, icons: paint::Icons::new() },
        surfs: vec![],
        next_surf: 0,
        outputs: HashMap::new(),
        layout: read_layout(),
        built_for: None,
        pointer_surface: None,
        cursor: None,
        cursor_mgr,
        seat: None,
        next_key: 0,
        dirty: false,
        popup: None,
        popup_card: (0.0, 0.0, 0.0, 0.0),
        dock_hover: None,
        preview_close: None,
        start_hits: vec![],
        start_max: 0,
        fy_hits: vec![],
        fy_active: Arc::new(AtomicBool::new(false)),
        fy_shared: Arc::new(std::sync::Mutex::new(Default::default())),
        drag: None,
        drag_sent: (std::time::Instant::now(), 101),
        peek_blocked: None,
        osd: None,
        osd_gen: 0,
        osd_hide_tx,
        pane_hits: vec![],
        pane_top: 0.0,
        toasts: vec![],
        tray_hits: vec![],
        tray_tx: None,
        wall_cache: vec![],
        toast_hide_tx,
        icon_stamp: None,
        icon_pack: paint::selected_icon_theme(),
        tab_hover: false,
        stats_shared: Arc::new(std::sync::Mutex::new(Default::default())),
        stats_wanted: Arc::new(AtomicBool::new(false)),
        tab_tx,
        tab_gen: 0,
        tab_on: false,
        tab_output: 0,
        rec: None,
        rec_since: None,
        viz: None,
        viz_alive: Arc::new(AtomicBool::new(false)),
        viz_tx,
        speech_since: None,
        speech_env: None,
        speech_env_mtime: None,
        speech_seen: false,
        conv: false,
        vad_mode: false,
        vad: convo::Vad::new(),
        key_down_at: None,
        down_in_conv: false,
        conv_id: None,
        conv_misses: 0,
        last_tick: None,
        voice_tx,
        heard_tx,
        clips: Arc::new(std::sync::Mutex::new(Vec::new())),
        clip_child: None,
        thumbs_started: false,
        art_for: None,
        fy_anim: None,
        anim_tx,
        xkb: None,
        pointer_pos: (0.0, 0.0),
        click_output: None,
        keyboard: None,
        axis_wheel: false,
        axis_acc: 0.0,
    };
    bind_outputs(&globals, &mut state, &qh);
    state.sync_clip();

    let mut event_loop: EventLoop<State> = EventLoop::try_new().expect("event loop");
    // Learn the outputs' names before drawing anything.
    queue.roundtrip(&mut state).expect("roundtrip");
    queue.roundtrip(&mut state).expect("roundtrip");
    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).expect("wayland source");

    // The D-Bus service runs on its own thread and hands requests to the event loop through a channel.
    let (bus_tx, bus_rx) = calloop::channel::channel::<BusMsg>();
    let key_tx = bus_tx.clone();
    std::thread::spawn(move || {
        let conn = zbus::blocking::connection::Builder::session().and_then(|b| b.name("org.labyrinth.Panel")).and_then(|b| b.serve_at("/Panel", PanelBus(bus_tx))).and_then(|b| b.build());
        match conn {
            Ok(c) => {
                // The Super key on its own: KWin 6.7 hands a bare modifier tap to Plasma's global shortcut
                // "activate application launcher" (kwinrc's ModifierOnlyShortcuts is no longer read), and
                // kglobalacceld announces every press on the bus, so the menu opens when that one fires.
                let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface("org.kde.kglobalaccel.Component").and_then(|b| b.member("globalShortcutPressed")).map(|b| b.build());
                match rule.and_then(|r| zbus::blocking::MessageIterator::for_match_rule(r, &c, None)) {
                    Ok(iter) => {
                        for msg in iter.flatten() {
                            if let Ok((component, action, _t)) = msg.body().deserialize::<(String, String, i64)>() {
                                if component == "plasmashell" && action == "activate application launcher" {
                                    let _ = key_tx.send(BusMsg::ToggleStart);
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("labyrinth-panel: cannot listen for the Super key ({e})"),
                }
                loop {
                    std::thread::park();
                }
            }
            Err(e) => eprintln!("labyrinth-panel: no D-Bus service ({e}); the Super key will not open the start menu"),
        }
    });
    event_loop
        .handle()
        .insert_source(bus_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(m) = event {
                st.click_output = None; // the main screen
                match m {
                    BusMsg::ToggleStart => st.toggle_start(),
                    BusMsg::ShowClipboard => st.show_clipboard(),
                    BusMsg::AskSarina => st.ask_sarina_key(),
                    BusMsg::ToggleCalendar => st.toggle_calendar(),
                    BusMsg::FylgjaSection(n) => st.show_fylgja_section(n as usize),
                }
            }
        })
        .expect("bus channel");
    // the pointer has rested on the notch's tab
    event_loop
        .handle()
        .insert_source(tab_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(g) = event {
                let blocked = st.peek_blocked.map(|t| t.elapsed() < Duration::from_secs(4)).unwrap_or(false);
                if g == st.tab_gen && st.tab_on && st.popup.is_none() && !blocked {
                    let out = st.tab_output;
                    st.open_fylgja(out);
                }
            }
        })
        .expect("tab channel");
    // the volume or brightness changed: show the pop-up; its timer ran out: put it away
    let (osd_tx, osd_rx) = calloop::channel::channel::<osd::OsdState>();
    osd::spawn_watchers(osd_tx);
    event_loop
        .handle()
        .insert_source(osd_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(o) = event {
                st.show_osd(o);
            }
        })
        .expect("osd channel");
    event_loop
        .handle()
        .insert_source(osd_hide_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(g) = event {
                st.hide_osd(g);
            }
        })
        .expect("osd hide channel");
    // notifications: from apps (D-Bus) and from Heimdall; a pop-up's timer ran out
    let (note_tx, note_rx) = calloop::channel::channel::<notify::NMsg>();
    notify::set_next_id(state.model.notifs.max_id());
    state.model.notifs.dnd = state.model.conf.notify_dnd;
    notify::spawn_server(note_tx.clone());
    notify::spawn_heimdall(note_tx.clone());
    devices::spawn(note_tx);
    event_loop
        .handle()
        .insert_source(note_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(m) = event {
                st.on_notif(m);
            }
        })
        .expect("notification channel");
    event_loop
        .handle()
        .insert_source(toast_hide_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(id) = event {
                st.hide_toast(id);
            }
        })
        .expect("toast hide channel");
    // the system tray: apps' icons come and go
    let (tray_tx, tray_rx) = calloop::channel::channel::<tray::TrayMsg>();
    tray::spawn(tray_tx.clone());
    state.tray_tx = Some(tray_tx);
    event_loop
        .handle()
        .insert_source(tray_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(m) = event {
                match m {
                    tray::TrayMsg::Set(it) => match st.model.tray.iter().position(|t| t.key == it.key) {
                        Some(i) => st.model.tray[i] = it,
                        None => st.model.tray.push(it),
                    },
                    tray::TrayMsg::Remove(k) => st.model.tray.retain(|t| t.key != k),
                    tray::TrayMsg::Menu(key, items, x) => st.open_tray_popup(trayui::TrayPop { anchor_x: x, mode: trayui::TrayMode::Menu { key, items, path: vec![] } }),
                }
                st.dirty = true;
            }
        })
        .expect("tray channel");
    // screen readers: the bar, dock and pop-ups as an AT-SPI tree, and a reader's "click" back as a real click
    if std::env::var("LABYRINTH_A11Y").map(|v| v != "off").unwrap_or(true) {
        let (a11y_tx, a11y_rx) = calloop::channel::channel::<Action>();
        state.a11y = Some(a11y::A11y::new(a11y_tx));
        event_loop
            .handle()
            .insert_source(a11y_rx, |event, _, st| {
                if let calloop::channel::Event::Msg(a) = event {
                    st.click(a);
                }
            })
            .expect("accessibility channel");
        event_loop
            .handle()
            .insert_source(Timer::from_duration(Duration::from_millis(500)), |_, _, st| {
                st.a11y_sync();
                TimeoutAction::ToDuration(Duration::from_millis(500))
            })
            .expect("accessibility timer");
    }
    // resting on a dock app for a second lists its windows; the list goes when the pointer has left it
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_millis(120)), |_, _, st| {
            if st.popup.is_none() {
                if let Some((id, at, x, out)) = st.dock_hover.clone() {
                    if at.elapsed() >= Duration::from_millis(1000) {
                        st.dock_hover = None;
                        st.open_windows_popup(&id, x, out);
                    }
                }
            }
            if st.preview_close.is_some_and(|t| Instant::now() >= t) {
                st.preview_close = None;
                if st.windows_popup_open() {
                    st.close_popup();
                }
            }
            TimeoutAction::ToDuration(Duration::from_millis(120))
        })
        .expect("dock preview timer");
    // each step of Fylgja sliding: redraw, and finish a close once it has gone up
    event_loop
        .handle()
        .insert_source(anim_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(()) = event {
                if let Some((t, opening)) = st.fy_anim {
                    if t.elapsed().as_millis() >= fylgja::SLIDE_MS {
                        st.fy_anim = None;
                        if !opening {
                            st.close_popup();
                        }
                    }
                    st.dirty = true;
                }
            }
        })
        .expect("animation channel");
    // what she understood, while she is still working on the answer
    event_loop
        .handle()
        .insert_source(heard_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(h) = event {
                if matches!(st.model.fylgja.voice, fylgja::Voice::Thinking(_)) {
                    st.model.fylgja.voice = fylgja::Voice::Thinking(h);
                    st.dirty = true;
                }
            }
        })
        .expect("heard channel");
    // Sarina's answer (or why there is none): said aloud or shown as a notification, never a card
    event_loop
        .handle()
        .insert_source(voice_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(r) = event {
                match r {
                    Ok(t) if !t.is_empty() => st.voice_answer(&t),
                    Ok(_) => st.voice_failed("Sarina had nothing to say."),
                    Err(e) => st.voice_failed(&e),
                }
                st.dirty = true;
            }
        })
        .expect("voice channel");
    // the voice island's animation, and the held shortcut
    event_loop
        .handle()
        .insert_source(viz_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(()) = event {
                st.viz_tick();
            }
        })
        .expect("voice island channel");
    event_loop
        .handle()
                .insert_source(vkey_rx, |event, _, st| {
            if let calloop::channel::Event::Msg(k) = event {
                st.voice_key(k);
            }
        })
        .expect("voice key channel");
    std::thread::spawn(move || {
        if let Err(e) = voicekey::run(vkey_tx) {
            eprintln!("labyrinth-panel: hold-to-talk is not available ({e}); the AskSarina D-Bus call still works");
        }
    });

    // Desktops, network and Bluetooth are read on a worker thread; the 1 s timer picks up the result.
    let shared = Arc::new(std::sync::Mutex::new(status::Status::default()));
    {
        let shared = shared.clone();
        std::thread::spawn(move || loop {
            let now = status::poll();
            if let Ok(mut g) = shared.lock() {
                *g = now;
            }
            std::thread::sleep(Duration::from_secs(2));
        });
    }
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(1)), move |_, _, st| {
            if let Ok(cur) = shared.lock() {
                if *cur != st.model.status {
                    st.model.status = cur.clone();
                    st.dirty = true;
                }
            }
            let h24 = st.model.conf.clock_24h;
            let now = clock_text(h24);
            if now != st.model.clock {
                st.model.clock = now;
                st.model.date = date_text();
                st.model.today = today_ymd();
                st.dirty = true;
            }
            // The calendar shows seconds: redraw it each second while it is open.
            let secs = secs_text(h24);
            if secs != st.model.clock_secs {
                st.model.clock_secs = secs;
                if matches!(st.popup, Some(Popup { kind: PopupKind::Calendar { .. }, .. }) | Some(Popup { kind: PopupKind::Fylgja(_), .. })) {
                    st.dirty = true;
                }
            }
            // a question being recorded: the seconds, and a recording that stopped by itself (no microphone)
            if let Some(since) = st.rec_since {
                let ended = st.rec.as_mut().map(|(c, _)| c.try_wait().map(|r| r.is_some()).unwrap_or(false)).unwrap_or(false);
                if ended {
                    if let Some((_, path)) = st.rec.take() {
                        let _ = std::fs::remove_file(path);
                    }
                    st.rec_since = None;
                    st.voice_failed("No microphone could be recorded. Check that one is plugged in and not muted, and that this app may use it.");
                } else {
                    st.model.fylgja.voice = fylgja::Voice::Listening(since.elapsed().as_secs());
                }
                st.dirty = true;
            }
            if matches!(st.popup, Some(Popup { kind: PopupKind::Fylgja(_), .. })) {
                let snap = st.fy_shared.lock().map(|g| g.clone()).unwrap_or_default();
                if snap != st.model.fylgja.snap && st.drag.is_none() {
                    st.model.fylgja.snap = snap;
                    st.dirty = true;
                }
                // the small card with nothing playing has nothing to show: put it away
                let nothing = matches!(&st.popup, Some(Popup { kind: PopupKind::Fylgja(f), .. }) if !f.open && !f.pinned && !f.voice_ui) && st.model.fylgja.snap.ready && st.model.fylgja.snap.media.is_none();
                if nothing {
                    st.peek_blocked = Some(std::time::Instant::now());
                    st.close_popup();
                }
                if st.model.notch_tab == Some(2) {
                    let stats = st.stats_shared.lock().map(|g| g.clone()).unwrap_or_default();
                    if stats != st.model.fylgja.stats {
                        st.model.fylgja.stats = stats;
                    }
                    st.dirty = true; // the speed test's progress shows on the next draw
                }
                if st.model.fylgja.history_on {
                    let clips = st.clips.lock().map(|g| g.clone()).unwrap_or_default();
                    if clips != st.model.fylgja.clips {
                        st.model.fylgja.clips = clips;
                        st.dirty = true;
                    }
                }
                // the cover of what is playing, loaded when the song changes
                let want = st.model.fylgja.snap.media.as_ref().and_then(|m| m.art.clone());
                if want != st.art_for {
                    let scale = st.surfs.iter().find(|x| x.kind == Kind::Popup).map(|x| st.scale_of(x)).unwrap_or(1.0).ceil().max(1.0) as u32;
                    st.model.fylgja.art = want.as_deref().and_then(|p| fylgja::load_image(p, 58 * scale, 58 * scale, 8.0 * scale as f32));
                    st.art_for = want;
                    st.dirty = true;
                }
            }
            let full = shell::trash_has_files();
            if full != st.model.trash_full {
                st.model.trash_full = full;
                st.dirty = true;
            }
            TimeoutAction::ToDuration(Duration::from_secs(1))
        })
        .expect("clock timer");
    // KDE's screen settings can change under us (Settings, kscreen-doctor, a plugged-in monitor).
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(4)), |_, _, st| {
            let l = read_layout();
            if l != st.layout {
                st.layout = l;
                st.built_for = None;
            }
            let dark = system_dark();
            if dark != st.model.dark {
                st.model.dark = dark;
                st.dirty = true;
            }
            // Minos's icon is redrawn in the new accent colour: take it up when its file changes
            let stamp = std::env::var_os("HOME").and_then(|h| std::fs::metadata(std::path::PathBuf::from(h).join(".local/share/icons/Labyrinth/apps/scalable/minos.svg")).ok()?.modified().ok());
            let pack = paint::selected_icon_theme();
            if pack != st.icon_pack {
                st.icon_pack = pack.clone();
                // Minos's icon for the new pack (its own, the pack's file manager's, or our logo)
                let _ = std::process::Command::new("labyrinth-minos-icon").status();
                st.assets.icons.set_pack(pack.as_deref());
                st.dirty = true;
            }
            if stamp != st.icon_stamp {
                st.icon_stamp = stamp;
                st.assets.icons.clear();
                st.dirty = true;
            }
            TimeoutAction::ToDuration(Duration::from_secs(4))
        })
        .expect("layout timer");

    // Settings writes ~/.config/labyrinth/panel.conf; pick changes up within a second.
    let mut seen = PanelConf::modified();
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(1)), move |_, _, st| {
            let now = PanelConf::modified();
            if now != seen {
                seen = now;
                let old = st.model.conf.dock_pos;
                let old_wall = st.model.conf.own_wallpaper;
                st.model.conf = PanelConf::load();
                if st.model.conf.own_wallpaper != old_wall {
                    st.built_for = None; // the wallpaper surface comes or goes
                }
                st.model.notifs.dnd = st.model.conf.notify_dnd;
                st.sync_clip();
                st.model.clock = clock_text(st.model.conf.clock_24h);
                st.model.clock_secs = secs_text(st.model.conf.clock_24h);
                if st.model.conf.dock_pos != old {
                    st.built_for = None; // another edge: the dock's surface is rebuilt
                }
                st.dirty = true;
            }
            TimeoutAction::ToDuration(Duration::from_secs(1))
        })
        .expect("config timer");

    event_loop
        .run(None, &mut state, |st| {
            if st.built_for.is_none() {
                st.build_surfaces();
            }
            if st.dirty {
                st.redraw();
            }
        })
        .expect("event loop run");
}

#[cfg(test)]
mod layout_tests {
    #[test]
    fn output_in_use_with_or_without_the_enabled_key() {
        let j = |t: &str| serde_json::from_str::<serde_json::Value>(t).unwrap();
        let old = j(r#"{"enabled":true,"connected":true}"#);
        let new_68 = j(r#"{"connected":true,"currentModeId":"6"}"#);
        let off = j(r#"{"connected":true,"enabled":false,"currentModeId":"6"}"#);
        let unplugged = j(r#"{"connected":false}"#);
        assert!(super::output_enabled(&old) && super::output_enabled(&new_68));
        assert!(!super::output_enabled(&off) && !super::output_enabled(&unplugged));
    }
}
