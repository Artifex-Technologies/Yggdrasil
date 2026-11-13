//! Settings > Peripherals > Camera and Microphone, each a row on Peripherals only when the computer has one.
//!
//! - Camera: the cameras (/sys/class/video4linux, capture nodes only), which one is the default (WirePlumber's default video
//!   source, `wpctl set-default`), and a live preview of the chosen one while the page is open (ffmpeg reads the camera and
//!   hands raw frames over a pipe, five a second (each one redraws the window); it is stopped when the page is left).
//! - Microphone: the input device, its volume and mute (moved here from Sound), and a level meter while the page is open
//!   (`pw-record` at 16 kHz mono, read in 50 ms pieces; stopped when the page is left).
//!
//! Nothing is captured unless the preview or the meter is on screen.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, vec2, CornerRadius, RichText, Sense};
use yggdrasil_ui::theme::Palette;

use crate::{sys, ui, App};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sub {
    Camera,
    Microphone,
}

/// A camera: its name and its device node.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub name: String,
    pub dev: PathBuf,
}

/// The cameras under `root` (/sys/class/video4linux): capture nodes only (index 0; a camera's other nodes carry metadata).
pub fn cameras_in(root: &Path) -> Vec<Camera> {
    let mut v: Vec<Camera> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let d = e.path();
            let read = |f: &str| std::fs::read_to_string(d.join(f)).ok().map(|s| s.trim().to_string());
            if read("index").as_deref().unwrap_or("0") != "0" {
                return None;
            }
            let node = e.file_name().to_string_lossy().to_string();
            Some(Camera { name: read("name").filter(|n| !n.is_empty()).unwrap_or_else(|| node.clone()), dev: Path::new("/dev").join(node) })
        })
        .collect();
    v.sort_by(|a, b| a.dev.cmp(&b.dev));
    v
}

/// The cameras of this computer (a fixture's `cameras` instead, with made-up nodes).
pub fn cameras() -> Vec<Camera> {
    match crate::fixture::get() {
        Some(f) => f
            .cameras
            .iter()
            .enumerate()
            .map(|(i, n)| Camera { name: n.clone(), dev: PathBuf::from(format!("/dev/video{}", i * 2)) })
            .collect(),
        None => cameras_in(Path::new("/sys/class/video4linux")),
    }
}

/// WirePlumber's video sources from `wpctl status`: (id, name, the default).
pub fn parse_video_sources(status: &str) -> Vec<(u32, String, bool)> {
    let mut out = Vec::new();
    let (mut video, mut sources) = (false, false);
    for line in status.lines() {
        if !line.starts_with([' ', '│', '├', '└']) {
            video = line.trim() == "Video";
            sources = false;
            continue;
        }
        if !video {
            continue;
        }
        let t = line.trim_start_matches([' ', '│', '├', '└', '─']).trim();
        if t.ends_with(':') {
            sources = t == "Sources:";
            continue;
        }
        if !sources || t.is_empty() {
            continue;
        }
        let default = t.starts_with('*');
        let t = t.trim_start_matches('*').trim();
        let Some((id, name)) = t.split_once(". ") else { continue };
        let Ok(id) = id.trim().parse() else { continue };
        let name = name.split(" [").next().unwrap_or(name).trim().to_string();
        out.push((id, name, default));
    }
    out
}

/// What the Camera and Microphone pages read when Peripherals opens.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub cameras: Vec<Camera>,
    pub video_sources: Vec<(u32, String, bool)>,
    pub sound: Option<sys::Sound>,
}

impl Info {
    pub fn has_camera(&self) -> bool {
        !self.cameras.is_empty()
    }

    pub fn has_microphone(&self) -> bool {
        self.sound.as_ref().is_some_and(|s| !s.inputs.is_empty())
    }

    /// The WirePlumber source of a camera (its name begins with the camera's), and whether it is the default.
    pub fn source_of(&self, c: &Camera) -> Option<(u32, bool)> {
        self.video_sources.iter().find(|(_, n, _)| n.starts_with(&c.name)).map(|(id, _, d)| (*id, *d))
    }
}

pub fn load() -> Info {
    Info {
        cameras: cameras(),
        video_sources: parse_video_sources(&sys::run("wpctl", &["status"]).unwrap_or_default()),
        sound: Some(sys::sound()),
    }
}

/// The rows Peripherals shows for these: Camera with a camera, Microphone with an input device.
pub fn rows(camera: bool, microphone: bool) -> Vec<Sub> {
    [(camera, Sub::Camera), (microphone, Sub::Microphone)].into_iter().filter(|(on, _)| *on).map(|(_, s)| s).collect()
}

const W: usize = 480;
const H: usize = 360;

/// The camera's picture while its page is open: ffmpeg, its frames, and the texture they are shown in.
pub struct Preview {
    child: Child,
    frame: Arc<Mutex<Option<Vec<u8>>>>,
    tex: Option<egui::TextureHandle>,
    pub dev: PathBuf,
}

impl Preview {
    /// Starts ffmpeg on `dev` (a fixture: ffmpeg's own test picture, no camera). None when ffmpeg cannot be started.
    pub fn start(dev: &Path, ctx: &egui::Context) -> Option<Preview> {
        let input: Vec<String> = if crate::fixture::get().is_some() {
            // -re: at its own pace, as a camera would deliver (a test picture is otherwise made as fast as it can be)
            vec!["-re".into(), "-f".into(), "lavfi".into(), "-i".into(), format!("testsrc2=size={W}x{H}:rate=5")]
        } else {
            vec!["-f".into(), "v4l2".into(), "-i".into(), dev.to_string_lossy().to_string()]
        };
        let mut child = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
            .args(&input)
            .args(["-vf", &format!("scale={W}:{H}:force_original_aspect_ratio=decrease,pad={W}:{H}:-1:-1,fps=5")])
            .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut out = child.stdout.take()?;
        let frame: Arc<Mutex<Option<Vec<u8>>>> = Default::default();
        let (f, ctx) = (frame.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut buf = vec![0u8; W * H * 3];
            while out.read_exact(&mut buf).is_ok() {
                *f.lock().unwrap_or_else(|e| e.into_inner()) = Some(buf.clone());
                // at most five redraws a second, whatever the camera sends
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        });
        Some(Preview { child, frame, tex: None, dev: dev.to_path_buf() })
    }

    fn show(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        if let Some(px) = self.frame.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let img = egui::ColorImage::from_rgb([W, H], &px);
            match &mut self.tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.tex = Some(ui.ctx().load_texture("camera-preview", img, egui::TextureOptions::LINEAR)),
            }
        }
        let width = ui.available_width().min(480.0);
        let size = vec2(width, width * H as f32 / W as f32);
        match &self.tex {
            Some(t) => {
                ui.add(egui::Image::new(t).fit_to_exact_size(size).corner_radius(10));
            }
            None => {
                let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(10), pal.bg);
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    "Starting the camera…",
                    egui::FontId::proportional(13.0),
                    pal.dim,
                );
            }
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The microphone's level while its page is open: pw-record's samples, the loudest of each 50 ms (0 to 1000).
pub struct Meter {
    child: Option<Child>,
    level: Arc<AtomicU32>,
}

/// The loudest sample of a piece of 16-bit little-endian audio, 0 to 1000.
pub fn peak(bytes: &[u8]) -> u32 {
    let m = bytes.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c).unsigned_abs() as u32).max().unwrap_or(0);
    m * 1000 / 32768
}

impl Meter {
    pub fn start(ctx: &egui::Context) -> Meter {
        let level = Arc::new(AtomicU32::new(0));
        if crate::fixture::get().is_some() {
            // made up: a steady level, nothing recorded
            level.store(420, Ordering::Release);
            return Meter { child: None, level };
        }
        let child = Command::new("pw-record")
            .args(["--rate", "16000", "--channels", "1", "--format", "s16", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok();
        let mut child = child;
        if let Some(mut out) = child.as_mut().and_then(|c| c.stdout.take()) {
            let (l, ctx) = (level.clone(), ctx.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 1600];
                while out.read_exact(&mut buf).is_ok() {
                    l.store(peak(&buf), Ordering::Release);
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            });
        }
        Meter { child, level }
    }

    pub fn running(&self) -> bool {
        self.child.is_some() || crate::fixture::get().is_some()
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl App {
    /// The Camera and Microphone rows on Peripherals' first page (only for what this computer has).
    pub(crate) fn media_rows(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(info) = &self.media_info else { return };
        let rows = rows(info.has_camera(), info.has_microphone());
        if rows.is_empty() {
            return;
        }
        let mut open = None;
        ui::group(ui, pal, Some("Camera and microphone"), |ui| {
            for (i, r) in rows.iter().enumerate() {
                let (title, sub) = match r {
                    Sub::Camera => ("Camera", "The cameras, the default one and a preview"),
                    Sub::Microphone => ("Microphone", "The input device, its level and volume"),
                };
                if ui::nav_row(ui, pal, i == 0, title, Some(sub), |_| {}) {
                    open = Some(*r);
                }
            }
        });
        if open.is_some() {
            self.media = open;
        }
    }

    pub(crate) fn page_media(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, sub: Sub) {
        let Some(info) = self.media_info.clone() else { return crate::loading(ui) };
        match sub {
            Sub::Camera => self.page_camera(ui, pal, ctx, &info),
            Sub::Microphone => self.page_microphone(ui, pal, ctx, &info),
        }
    }

    fn page_camera(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, info: &Info) {
        if info.cameras.is_empty() {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "No camera found", None, |_| {}));
            return;
        }
        // the camera shown: the one picked here, else the default, else the first
        let shown = self
            .cam_pick
            .clone()
            .filter(|d| info.cameras.iter().any(|c| &c.dev == d))
            .or_else(|| info.cameras.iter().find(|c| info.source_of(c).is_some_and(|(_, d)| d)).map(|c| c.dev.clone()))
            .unwrap_or_else(|| info.cameras[0].dev.clone());
        let mut pick: Option<(PathBuf, Option<u32>)> = None;
        ui::group(ui, pal, Some("Cameras"), |ui| {
            for (i, c) in info.cameras.iter().enumerate() {
                let src = info.source_of(c);
                let sub = match src {
                    Some((_, true)) => "Default · apps use this one",
                    Some(_) => "Click to make it the default",
                    None => "Not listed by PipeWire",
                };
                ui::row(ui, pal, i == 0, &c.name, Some(sub), |ui| {
                    if src.is_some_and(|(_, d)| d) {
                        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                        crate::icons::paint(ui.painter(), r, crate::icons::Icon::Check, pal.accent);
                    } else if ui::quiet(ui, pal, "Use").clicked() {
                        pick = Some((c.dev.clone(), src.map(|(id, _)| id)));
                    }
                    if c.dev != shown && ui::quiet(ui, pal, "Preview").clicked() {
                        pick = Some((c.dev.clone(), None));
                    }
                });
            }
        });
        ui::group(ui, pal, Some("Preview"), |ui| {
            ui.add_space(8.0);
            if self.cam_preview.as_ref().is_none_or(|p| p.dev != shown) {
                self.cam_preview = Preview::start(&shown, ctx);
            }
            match &mut self.cam_preview {
                Some(p) => p.show(ui, pal),
                None => {
                    ui.label(RichText::new("A live preview needs ffmpeg (the ffmpeg package).").color(pal.dim));
                }
            }
            ui.add_space(8.0);
        });
        ui::footnote(ui, pal, "The camera is on only while this page is open; leaving it turns the preview off.");
        if let Some((dev, id)) = pick {
            self.cam_pick = Some(dev);
            if let Some(id) = id {
                let id = id.to_string();
                self.change(ctx, move || sys::act("wpctl", &["set-default", &id]));
            }
        }
    }

    fn page_microphone(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, info: &Info) {
        let Some(s) = info.sound.clone().filter(|s| !s.inputs.is_empty()) else {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "No microphone found", None, |_| {}));
            return;
        };
        let cur = s.inputs.iter().find(|d| d.name == s.default_input).unwrap_or(&s.inputs[0]).clone();
        let mut cmds: Vec<Vec<String>> = vec![];
        if self.mic_meter.is_none() {
            self.mic_meter = Some(Meter::start(ctx));
        }
        let level = self.mic_meter.as_ref().map(|m| (m.level.load(Ordering::Acquire), m.running()));
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Input device", None, |ui| {
                egui::ComboBox::from_id_salt("mic-dev")
                    .selected_text(&cur.description)
                    .width((ui.available_width() * 0.6).clamp(160.0, 320.0))
                    .show_ui(ui, |ui| {
                        for d in &s.inputs {
                            if ui.selectable_label(d.name == cur.name, &d.description).clicked() {
                                cmds.push(vec!["set-default-source".into(), d.name.clone()]);
                            }
                        }
                    });
            });
            ui::row(ui, pal, false, "Input level", Some("Speak to see it move"), |ui| {
                let (r, _) = ui.allocate_exact_size(vec2((ui.available_width() * 0.55).clamp(120.0, 260.0), 10.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(5), pal.hover);
                match level {
                    Some((l, true)) => {
                        let mut f = r;
                        f.set_width(r.width() * (l as f32 / 1000.0).sqrt().min(1.0));
                        let c = if l > 900 { pal.red } else { pal.green };
                        ui.painter().rect_filled(f, CornerRadius::same(5), c);
                    }
                    _ => {
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            "pw-record is missing",
                            egui::FontId::proportional(10.0),
                            pal.dim,
                        );
                    }
                }
            });
            let mut vol = cur.volume as f32;
            ui::row(ui, pal, false, "Volume", None, |ui| {
                let r = ui::slider(ui, pal, &mut vol);
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    cmds.push(vec!["set-source-volume".into(), cur.name.clone(), format!("{}%", vol as u32)]);
                }
            });
            let mut muted = cur.muted;
            ui::row(ui, pal, false, "Mute", None, |ui| {
                if ui::switch(ui, pal, &mut muted).changed() {
                    cmds.push(vec!["set-source-mute".into(), cur.name.clone(), if muted { "1".into() } else { "0".into() }]);
                }
            });
        });
        ui::footnote(ui, pal, "The microphone is listened to only for the level, and only while this page is open; nothing is kept.");
        if !cmds.is_empty() {
            // the meter follows a new device: started again on the next frame
            self.mic_meter = None;
            self.change(ctx, move || {
                for c in cmds {
                    let a: Vec<&str> = c.iter().map(String::as_str).collect();
                    sys::act("pactl", &a)?;
                }
                // read again for the page (the change reloads Peripherals)
                Ok(())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_show_only_for_what_is_there() {
        assert_eq!(rows(false, false), []);
        assert_eq!(rows(true, false), [Sub::Camera]);
        assert_eq!(rows(false, true), [Sub::Microphone]);
        assert_eq!(rows(true, true), [Sub::Camera, Sub::Microphone]);
    }

    #[test]
    fn capture_nodes_are_cameras() {
        let root = std::env::temp_dir().join(format!("ygg-v4l-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (n, idx, name) in [("video0", "0", "Example Webcam"), ("video1", "1", "Example Webcam"), ("video2", "0", "")] {
            std::fs::create_dir_all(root.join(n)).unwrap();
            std::fs::write(root.join(n).join("index"), idx).unwrap();
            std::fs::write(root.join(n).join("name"), name).unwrap();
        }
        let c = cameras_in(&root);
        assert_eq!(
            c.iter().map(|c| (c.name.as_str(), c.dev.to_str().unwrap())).collect::<Vec<_>>(),
            [("Example Webcam", "/dev/video0"), ("video2", "/dev/video2")]
        );
        assert!(cameras_in(&root.join("none")).is_empty(), "a desktop without a camera");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn wpctl_lists_the_video_sources_and_the_default() {
        let s = "Audio\n ├─ Sources:\n │  *   40. Example Microphone [vol: 0.50]\n │\n\nVideo\n ├─ Devices:\n │      58. Example Webcam                   [v4l2]\n │\n ├─ Sources:\n │  *   62. Example Webcam (V4L2)\n │      63. Other Camera (V4L2)\n │\n └─ Streams:\n\nSettings\n └─ Default Configured Devices:\n";
        assert_eq!(
            parse_video_sources(s),
            [(62, "Example Webcam (V4L2)".to_string(), true), (63, "Other Camera (V4L2)".to_string(), false)]
        );
    }

    #[test]
    fn the_meter_takes_the_loudest_sample() {
        let quiet: Vec<u8> = [100i16, -200, 50].iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(peak(&quiet), 6);
        let loud: Vec<u8> = [i16::MIN, 0].iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(peak(&loud), 1000);
    }
}
