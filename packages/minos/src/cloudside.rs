//! Cloud Storage in the sidebar (YGG-170): every cloud drive Settings > Cloud Storage keeps
//! (`yggdrasil_clouddrives`), with its state. The list is read again when the settings folder changes (Settings wrote it); whether
//! a drive is mounted is read from /proc/self/mountinfo by a thread every few seconds, and only a change wakes the window.
//! rclone itself is never asked: mounting and unmounting go through the drive's systemd user unit, on a thread.

use super::*;
use std::sync::Mutex;
use yggdrasil_clouddrives::{self as cd, Drive, Kind, Sys};

/// How often the mount table is looked at while there are remotes to watch.
const WATCH_EVERY: Duration = Duration::from_secs(3);

enum Msg {
    /// The watched mount points that are mounted now.
    Mounted(Vec<PathBuf>),
    /// A mount or unmount finished: the drive, and what went wrong.
    Done(String, Result<(), String>),
}

pub(crate) struct CloudDrives {
    /// The drives from the list, Proton Drive left out (it has its own row and sign-in).
    pub(crate) list: Vec<Drive>,
    mounted: Vec<PathBuf>,
    /// Drives being mounted or unmounted, and whether to open each once mounted.
    busy: BTreeMap<String, bool>,
    errors: BTreeMap<String, String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// The mount points the watcher looks for (shared with it); its thread starts with the first remote.
    watched: Arc<Mutex<Vec<PathBuf>>>,
    watcher: bool,
}

impl CloudDrives {
    pub(crate) fn new() -> Self {
        let (tx, rx) = channel();
        let mut c = CloudDrives {
            list: vec![],
            mounted: vec![],
            busy: BTreeMap::new(),
            errors: BTreeMap::new(),
            tx,
            rx,
            watched: Default::default(),
            watcher: false,
        };
        c.reload_list();
        c.mounted = cd::mount_points(&cd::Real.mountinfo()).into_iter().filter(|m| c.list.iter().any(|d| d.path() == m)).collect();
        c
    }

    /// Reads the list again (Settings changed it).
    pub(crate) fn reload_list(&mut self) {
        self.list = cd::load(&cd::Paths::user()).into_iter().filter(|d| !d.is_proton()).collect();
        let points: Vec<PathBuf> =
            self.list.iter().filter(|d| matches!(d.kind, Kind::Remote { .. })).map(|d| d.path().to_path_buf()).collect();
        if let Ok(mut w) = self.watched.lock() {
            *w = points;
        }
        self.errors.retain(|n, _| self.list.iter().any(|d| d.name == *n));
    }

    /// Starts the mount watcher once there is a remote to watch.
    fn ensure_watcher(&mut self, ctx: &egui::Context) {
        if self.watcher || !self.list.iter().any(|d| matches!(d.kind, Kind::Remote { .. })) {
            return;
        }
        self.watcher = true;
        let (tx, watched, c) = (self.tx.clone(), self.watched.clone(), ctx.clone());
        let mut last = self.mounted.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(WATCH_EVERY);
            let points = watched.lock().map(|w| w.clone()).unwrap_or_default();
            if points.is_empty() {
                continue;
            }
            let now: Vec<PathBuf> = cd::mount_points(&cd::Real.mountinfo()).into_iter().filter(|m| points.contains(m)).collect();
            if now != last {
                last = now.clone();
                if tx.send(Msg::Mounted(now)).is_err() {
                    return;
                }
                c.request_repaint();
            }
        });
    }

    /// Takes what the threads found. A drive mounted from the sidebar opens in pane `focus` (returned) once it is there.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) -> (Option<PathBuf>, Option<String>) {
        self.ensure_watcher(ctx);
        let (mut open, mut say) = (None, None);
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Mounted(now) => self.mounted = now,
                Msg::Done(name, res) => {
                    let wants_open = self.busy.remove(&name).unwrap_or(false);
                    match res {
                        Ok(()) => {
                            self.errors.remove(&name);
                            self.mounted = cd::mount_points(&cd::Real.mountinfo())
                                .into_iter()
                                .filter(|m| self.list.iter().any(|d| d.path() == m))
                                .collect();
                            if let Some(d) = self.list.iter().find(|d| d.name == name) {
                                let up = self.mounted.iter().any(|m| m == d.path());
                                if wants_open && up {
                                    open = Some(d.path().to_path_buf());
                                }
                                say = Some(format!("{} is {}", d.label(), if up { "mounted" } else { "unmounted" }));
                            }
                        }
                        Err(e) => {
                            say = Some(e.clone());
                            self.errors.insert(name, e);
                        }
                    }
                }
            }
        }
        (open, say)
    }

    pub(crate) fn state(&self, d: &Drive) -> cd::State {
        cd::state_of(d, &self.mounted, None, None, &|p| p.is_dir())
    }

    /// Mounts (`up`) or unmounts a drive on a thread; mounting waits up to 20 seconds for the folder to be mounted.
    pub(crate) fn mount(&mut self, ctx: &egui::Context, name: &str, up: bool, open: bool) {
        let Some(d) = self.list.iter().find(|d| d.name == name).cloned() else { return };
        if self.busy.contains_key(name) {
            return;
        }
        self.busy.insert(name.to_string(), open);
        let (tx, c) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let sys = cd::Real;
            let res = if up {
                cd::mount(&sys, &cd::Paths::user(), &d).and_then(|()| {
                    for _ in 0..40 {
                        if cd::mount_points(&sys.mountinfo()).iter().any(|m| m == d.path()) {
                            return Ok(());
                        }
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    Err(format!("{} did not mount (Settings > Cloud Storage shows why)", d.label()))
                })
            } else {
                cd::unmount(&sys, &d)
            };
            let _ = tx.send(Msg::Done(d.name.clone(), res));
            c.request_repaint();
        });
    }
}

/// Opens Settings on its Cloud Storage page.
pub(crate) fn open_settings() -> Result<(), String> {
    std::process::Command::new("yggdrasil-settings")
        .arg("cloudstorage")
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open Settings: {e}"))
}

/// The theme icon names for a provider (the drawn cloud when the theme has none).
fn provider_icons(provider: Option<&str>) -> &'static [&'static str] {
    match provider {
        Some("drive") => &["google-drive", "folder-gdrive", "folder-cloud"],
        Some("onedrive") => &["onedrive", "folder-onedrive", "folder-cloud"],
        Some("dropbox") => &["dropbox", "folder-dropbox", "folder-cloud"],
        Some("webdav") => &["nextcloud", "folder-nextcloud", "folder-cloud"],
        _ => &["folder-cloud"],
    }
}

impl App {
    /// The cloud drives' rows under Cloud Storage: a mounted drive (or a sync app's folder that is there) opens on a click; one not
    /// mounted is greyed and mounts on a click or with Mount on right-click.
    pub(crate) fn cloud_drive_rows(&self, ui: &mut egui::Ui, pal: &Palette, actions: &mut Vec<Action>) {
        for d in &self.cloud_drives.list {
            let state = self.cloud_drives.state(d);
            let busy = self.cloud_drives.busy.contains_key(&d.name);
            let error = self.cloud_drives.errors.get(&d.name);
            let is_remote = matches!(d.kind, Kind::Remote { .. });
            let selected = {
                let p = &self.panes[self.focus];
                p.tabs[p.active].dir() == d.path()
            };
            let label = if busy {
                format!("{} ({})", d.label(), if state.usable() { "unmounting..." } else { "mounting..." })
            } else {
                d.label().to_string()
            };
            let r = if state.usable() {
                side_row(ui, pal, Icon::Cloud, pal.dim, provider_icons(d.provider()), &label, selected, None)
            } else {
                ui.scope(|ui| {
                    ui.multiply_opacity(0.5);
                    side_row(ui, pal, Icon::Cloud, pal.dim, provider_icons(d.provider()), &label, selected, None)
                })
                .inner
            };
            // the state as a dot: green mounted, red an error
            let dot = if error.is_some() || state.is_error() {
                Some(pal.red)
            } else if state == cd::State::Mounted {
                Some(pal.green)
            } else {
                None
            };
            if let Some(c) = dot {
                ui.painter().circle_filled(pos2(r.rect.right() - 12.0, r.rect.top() + 14.0), 3.5, c);
            }
            let kind = d.provider().map(cd::provider_label).unwrap_or("Folder kept by a sync app");
            let tip = match (error, state) {
                (Some(e), _) => format!("{kind}: {e}"),
                (None, s) if s.usable() => format!("{kind}: {}\n{}", s.label(), d.path().display()),
                (None, cd::State::Missing) => format!("{kind}: the folder {} is not there", d.path().display()),
                (None, s) => format!("{kind}: {}. Click or right-click > Mount", s.label()),
            };
            let r = r.on_hover_text(tip);
            if r.clicked() && !busy {
                if state.usable() {
                    actions.push(Action::Go(self.focus, d.path().to_path_buf()));
                } else if is_remote {
                    actions.push(Action::CloudMount(d.name.clone(), true));
                }
            }
            if r.middle_clicked() && state.usable() {
                actions.push(Action::OpenTab(self.focus, d.path().to_path_buf()));
            }
            if state.usable() {
                if let Some(p) = r.dnd_release_payload::<DragPaths>() {
                    actions.push(Action::Move(p.0.clone(), d.path().to_path_buf()));
                }
            }
            r.context_menu(|ui| {
                if ui.add_enabled(state.usable(), egui::Button::new("Open")).clicked() {
                    actions.push(Action::Go(self.focus, d.path().to_path_buf()));
                    ui.close();
                }
                if is_remote {
                    let mounted = state == cd::State::Mounted;
                    if ui.add_enabled(!busy && !mounted, egui::Button::new("Mount")).clicked() {
                        actions.push(Action::CloudMount(d.name.clone(), false));
                        ui.close();
                    }
                    if ui.add_enabled(!busy && mounted, egui::Button::new("Unmount")).clicked() {
                        actions.push(Action::CloudUnmount(d.name.clone()));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Manage cloud drives...").clicked() {
                    actions.push(Action::ManageCloud);
                    ui.close();
                }
            });
        }
    }
}
