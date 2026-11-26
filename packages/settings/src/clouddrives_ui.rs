//! Settings > Apps > Minos > Cloud drives (YGG-170): the cloud drives Minos lists under Cloud Storage, with their state (mounted,
//! not mounted, an error), Add, Mount / Unmount, "Mount at sign-in" and Remove. The drives, their units and rclone's remotes are
//! `yggdrasil_clouddrives`; this is the page.
//!
//! - Add: the provider's own sign-in through rclone. Google Drive, OneDrive, Dropbox and Box: `rclone config create NAME TYPE`
//!   runs as a child of Settings, which shows what it prints while the browser sign-in waits (Cancel stops it). Proton Drive,
//!   WebDAV / Nextcloud and S3 want a user name and a password or keys: rclone's own questions (`... --all`) in a terminal, so the
//!   password is typed into rclone and never passes through Settings or a command line; the page waits for the remote to appear.
//!   No account of ours is involved. An existing rclone remote, or a folder a sync app keeps, can be added too.
//! - Remove asks first, and removes the remote and the mount unit only, never files in the cloud.
//! - When rclone is missing the page says so and shows the install command; nothing is installed.
//!
//! Everything slow (rclone, systemctl) runs on a thread. While the page is open the state is read again every few seconds (the
//! mount table and one `systemctl --user show`); rclone's remotes when the page opens, after a change, and while a sign-in is
//! awaited. A debug build with a fixture answers from it (fixture.rs) and changes nothing.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, RichText};
use yggdrasil_clouddrives::{self as cd, Drive, Kind, Provider, Remote, SignIn, State, Sys};
use yggdrasil_ui::theme::Palette;

use crate::icons::Icon;
use crate::ui;

/// How often the state is read again while the page is open.
const REFRESH: Duration = Duration::from_secs(3);
/// The most lines of a sign-in's output kept on the page.
const OUTPUT_LINES: usize = 200;

/// The system as Settings sees it: a fixture's made-up answers in a debug build started with one, the real one otherwise.
struct SettingsSys;

impl cd::Sys for SettingsSys {
    fn run(&self, cmd: &str, args: &[&str]) -> Option<String> {
        crate::sys::run(cmd, args)
    }

    fn act(&self, cmd: &str, args: &[&str]) -> Result<(), String> {
        crate::sys::act(cmd, args)
    }

    fn mountinfo(&self) -> String {
        if crate::fixture::get().is_some() {
            return crate::sys::run("cat", &["/proc/self/mountinfo"]).unwrap_or_default();
        }
        cd::Real.mountinfo()
    }

    fn find(&self, program: &str) -> Option<PathBuf> {
        match crate::fixture::get() {
            Some(f) => f.tools.iter().any(|t| t == program).then(|| PathBuf::from("/usr/bin").join(program)),
            None => cd::Real.find(program),
        }
    }
}

/// One drive as shown: the drive, its state, and whether it is mounted at sign-in.
#[derive(Clone, Debug)]
struct Shown {
    drive: Drive,
    state: State,
    at_login: bool,
}

enum Msg {
    /// The drives read again; with rclone's remotes when they were asked for too.
    Loaded { drives: Vec<Shown>, remotes: Option<Result<Vec<Remote>, String>>, rclone: bool },
    /// A change finished: what to say.
    Done(Result<String, String>),
}

/// What is being added.
#[derive(Clone, Debug, PartialEq)]
enum Choice {
    Provider(&'static Provider),
    /// An rclone remote that is not listed yet (its name).
    Existing(String),
    Folder,
}

/// A sign-in running: the browser kind as a child whose output is shown, the terminal kind as a terminal window.
struct SignInRun {
    child: Child,
    name: String,
    provider: &'static Provider,
    mount: PathBuf,
    lines: Arc<Mutex<Vec<String>>>,
    started: Instant,
}

struct AddForm {
    choice: Option<Choice>,
    name: String,
    /// The folder; it follows the name until it is changed by hand.
    folder: String,
    folder_edited: bool,
    run: Option<SignInRun>,
    error: String,
}

pub struct Panel {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    drives: Option<Vec<Shown>>,
    remotes: Option<Result<Vec<Remote>, String>>,
    rclone: bool,
    loaded: Instant,
    loading: bool,
    busy: Option<String>,
    status: Result<String, String>,
    add: Option<AddForm>,
    confirm_remove: Option<Drive>,
    home: PathBuf,
}

/// The drives with their states, read on a thread. rclone's remotes are asked for when `cached` has none (the page opened, a
/// change was made, a sign-in is awaited); in between the last answer is used, so the quick reads start no rclone.
fn load(cached: Option<Result<Vec<Remote>, String>>) -> Msg {
    let sys = SettingsSys;
    let paths = cd::Paths::user();
    let rclone = sys.find("rclone").is_some();
    let asked = (cached.is_none() && rclone).then(|| cd::remotes(&sys));
    let remotes = cached.as_ref().or(asked.as_ref()).and_then(|r| r.as_ref().ok());
    let known: Vec<Remote> = remotes.cloned().unwrap_or_default();
    let drives = cd::with_proton(&cd::load(&paths), &known, &paths.home);
    let units = cd::units(&sys, &drives);
    let mounts = cd::mount_points(&sys.mountinfo());
    let shown = drives
        .into_iter()
        .map(|d| {
            let unit = d.unit().and_then(|u| units.get(&u).cloned());
            // whether it is signed in is known only when rclone's remotes could be read
            let signed_in = match d.kind {
                Kind::Remote { .. } if remotes.is_some() => Some(known.iter().any(|r| r.name == d.name && r.signed_in)),
                _ => None,
            };
            let state = cd::state_of(&d, &mounts, unit.as_ref(), signed_in, &|p| p.is_dir());
            Shown { at_login: unit.is_some_and(|u| u.enabled), drive: d, state }
        })
        .collect();
    Msg::Loaded { drives: shown, remotes: asked, rclone }
}

/// A choice in "What to add": filled with the accent when chosen.
fn chip<'a>(on: bool, pal: &Palette, label: &'a str) -> egui::Button<'a> {
    egui::Button::new(RichText::new(label).color(if on { pal.on_accent() } else { pal.text }))
        .fill(if on { pal.accent } else { pal.hover })
        .corner_radius(egui::CornerRadius::same(8))
        .min_size(vec2(0.0, 28.0))
}

/// `~/x` as a path under home; anything else as typed.
fn expand(home: &Path, typed: &str) -> PathBuf {
    match typed.trim().strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if typed.trim() == "~" => home.to_path_buf(),
        None => PathBuf::from(typed.trim()),
    }
}

/// A path shown with ~ for home.
fn short(home: &Path, p: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

impl Panel {
    pub fn new(ctx: &egui::Context) -> Panel {
        let (tx, rx) = channel();
        let mut p = Panel {
            tx,
            rx,
            drives: None,
            remotes: None,
            rclone: true,
            loaded: Instant::now(),
            loading: false,
            busy: None,
            status: Ok(String::new()),
            add: None,
            confirm_remove: None,
            home: cd::Paths::user().home,
        };
        p.refresh(ctx, true);
        p
    }

    fn refresh(&mut self, ctx: &egui::Context, full: bool) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.loaded = Instant::now();
        let cached = if full { None } else { self.remotes.clone() };
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(load(cached));
            ctx.request_repaint();
        });
    }

    /// Runs a change on a thread; the page reads everything again when it is done.
    fn act(&mut self, ctx: &egui::Context, what: String, job: impl FnOnce() -> Result<String, String> + Send + 'static) {
        if self.busy.is_some() {
            return;
        }
        if crate::fixture::get().is_some() {
            self.status = Err("A made-up system (a fixture): nothing is changed.".into());
            return;
        }
        self.busy = Some(what);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Done(job()));
            ctx.request_repaint();
        });
    }

    fn names_taken(&self) -> Vec<String> {
        let mut v: Vec<String> = self.drives.iter().flatten().map(|s| s.drive.name.clone()).collect();
        v.extend(self.remotes.iter().flatten().flatten().map(|r| r.name.clone()));
        v
    }

    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Loaded { drives, remotes, rclone } => {
                    self.loading = false;
                    self.drives = Some(drives);
                    self.rclone = rclone;
                    if remotes.is_some() {
                        self.remotes = remotes;
                    }
                }
                Msg::Done(r) => {
                    self.busy = None;
                    self.status = r;
                    self.loading = false;
                    self.refresh(ctx, true);
                }
            }
        }
        self.poll_sign_in(ctx);
        // read again every few seconds while the window is in front, or while a sign-in in a terminal (in front instead) is awaited
        let waiting = self.add.as_ref().is_some_and(|a| a.run.is_some());
        let background = ctx.input(|i| i.viewport().focused == Some(false) || i.viewport().minimized == Some(true));
        if waiting || !background {
            if self.loaded.elapsed() >= REFRESH {
                self.refresh(ctx, waiting);
            }
            ctx.request_repaint_after(REFRESH);
        }

        if !self.rclone {
            ui::group(ui, pal, None, |ui| {
                ui::row(
                    ui,
                    pal,
                    true,
                    "rclone is not installed",
                    Some(&format!(
                        "Cloud drives are signed in to and mounted with rclone. Install it with your system's package manager; on \
                         Arch-based systems: {}",
                        cd::INSTALL_HINT
                    )),
                    |ui| {
                        if ui::quiet(ui, pal, "Copy command").clicked() {
                            ui.ctx().copy_text(cd::INSTALL_HINT.into());
                        }
                    },
                );
            });
        }

        let Some(drives) = self.drives.clone() else {
            ui.add_space(14.0);
            ui.label(RichText::new("Reading your cloud drives...").color(pal.dim));
            return;
        };
        let busy = self.busy.is_some();
        let mut mount: Option<(Drive, bool)> = None;
        let mut login: Option<(Drive, bool)> = None;
        ui::group(ui, pal, Some("Cloud drives"), |ui| {
            if drives.is_empty() {
                ui::row(ui, pal, true, "No cloud drives yet", Some("Add one below; it appears in Minos under Cloud Storage"), |_| {});
            }
            for (i, s) in drives.iter().enumerate() {
                let d = &s.drive;
                let kind = d.provider().map(cd::provider_label).unwrap_or("Folder kept by a sync app");
                let sub = format!("{kind} \u{b7} {} \u{b7} {}", short(&self.home, d.path()), s.state.label());
                ui::row_with(
                    ui,
                    pal,
                    i == 0,
                    |ui| {
                        ui.horizontal(|ui| {
                            let dot = match s.state {
                                State::Mounted | State::Ready => pal.green,
                                st if st.is_error() => pal.red,
                                State::Starting => pal.amber,
                                _ => pal.dim,
                            };
                            let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), egui::Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.0, dot);
                            ui.add(egui::Label::new(RichText::new(d.label()).size(14.0)).wrap());
                        });
                    },
                    Some(&sub),
                    |ui| {
                        if ui::icon_button(ui, pal, Icon::Trash, "Remove from this computer", !busy, true).clicked() {
                            self.confirm_remove = Some(d.clone());
                        }
                        if matches!(d.kind, Kind::Remote { .. }) {
                            let up = s.state == State::Mounted;
                            let label = if up { "Unmount" } else { "Mount" };
                            if ui
                                .add_enabled_ui(!busy && self.rclone && s.state != State::Starting, |ui| ui::quiet(ui, pal, label))
                                .inner
                                .clicked()
                            {
                                mount = Some((d.clone(), !up));
                            }
                            ui.add_space(8.0);
                            let mut on = s.at_login;
                            let r = ui.add_enabled_ui(!busy && self.rclone, |ui| ui::switch(ui, pal, &mut on)).inner;
                            if r.changed() {
                                login = Some((d.clone(), on));
                            }
                            ui.label(RichText::new("At sign-in").color(pal.dim));
                        }
                    },
                );
            }
        });
        ui::footnote(
            ui,
            pal,
            "Mounted drives appear in Minos under Cloud Storage. Each one is mounted with rclone (rclone mount --vfs-cache-mode full) \
             by a service of your own account, so it stays mounted when Minos is closed; \"At sign-in\" mounts it whenever you sign \
             in. Removing a drive removes its sign-in and its mount from this computer, never your files in the cloud.",
        );
        if let Some((d, up)) = mount {
            let what = format!("{} {}", if up { "Mounting" } else { "Unmounting" }, d.label());
            self.act(ctx, what, move || {
                let r = if up { cd::mount(&SettingsSys, &cd::Paths::user(), &d) } else { cd::unmount(&SettingsSys, &d) };
                r.map(|()| format!("{} is {}", d.label(), if up { "mounted" } else { "unmounted" }))
            });
        }
        if let Some((d, on)) = login {
            self.act(ctx, format!("Changing {}", d.label()), move || {
                cd::set_at_login(&SettingsSys, &cd::Paths::user(), &d, on).map(|()| {
                    if on {
                        format!("{} is mounted whenever you sign in", d.label())
                    } else {
                        format!("{} is no longer mounted at sign-in", d.label())
                    }
                })
            });
        }

        ui.add_space(10.0);
        if self.add.is_none() {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                if ui::action(ui, pal, "Add a cloud drive").clicked() {
                    self.add = Some(AddForm {
                        choice: None,
                        name: String::new(),
                        folder: String::new(),
                        folder_edited: false,
                        run: None,
                        error: String::new(),
                    });
                    self.refresh(ctx, true);
                }
                if let Some(b) = &self.busy {
                    ui.add_space(8.0);
                    ui.spinner();
                    ui.label(RichText::new(format!("{b}...")).color(pal.dim));
                }
            });
        } else {
            self.add_form(ui, pal, ctx);
        }
        match &self.status {
            Ok(s) if !s.is_empty() => {
                ui.add_space(8.0);
                ui.label(RichText::new(s).color(pal.dim));
            }
            Err(e) => {
                ui.add_space(8.0);
                ui.label(RichText::new(e).color(pal.red));
            }
            _ => {}
        }
        self.remove_window(ctx, pal);
    }

    /// Add a cloud drive: choose what, name it, sign in (or add it at once).
    fn add_form(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let taken = self.names_taken();
        let home = self.home.clone();
        let existing: Vec<Remote> = self
            .remotes
            .iter()
            .flatten()
            .flatten()
            .filter(|r| r.name != cd::PROTON_REMOTE && !self.drives.iter().flatten().any(|s| s.drive.name == r.name))
            .cloned()
            .collect();
        let proton_there = self.remotes.iter().flatten().flatten().any(|r| r.name == cd::PROTON_REMOTE);
        let rclone = self.rclone;
        let Some(f) = self.add.as_mut() else { return };
        let mut close = false;
        let mut start: Option<(&'static Provider, String, PathBuf)> = None;
        let mut add_now: Option<Drive> = None;
        ui::group(ui, pal, Some("Add a cloud drive"), |ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("What to add").color(pal.dim));
            ui.horizontal_wrapped(|ui| {
                let running = f.run.is_some();
                for p in cd::PROVIDERS.iter() {
                    let on = f.choice.as_ref() == Some(&Choice::Provider(p));
                    if ui.add_enabled(!running && rclone, chip(on, pal, p.label)).on_hover_text(p.about).clicked() {
                        f.choice = Some(Choice::Provider(p));
                        f.name = if p.kind == cd::PROTON_TYPE { cd::PROTON_REMOTE.into() } else { cd::default_name(p, &taken) };
                        f.folder_edited = false;
                        f.error.clear();
                    }
                }
                let on = matches!(f.choice, Some(Choice::Existing(_)));
                if ui
                    .add_enabled(!running && rclone && !existing.is_empty(), chip(on, pal, "An existing rclone remote"))
                    .on_hover_text("A remote you set up with rclone before")
                    .on_disabled_hover_text("rclone has no remote that is not listed already")
                    .clicked()
                {
                    f.choice = existing.first().map(|r| Choice::Existing(r.name.clone()));
                    f.name = existing.first().map(|r| r.name.clone()).unwrap_or_default();
                    f.folder_edited = false;
                    f.error.clear();
                }
                let on = f.choice == Some(Choice::Folder);
                if ui
                    .add_enabled(!running, chip(on, pal, "A folder a sync app keeps"))
                    .on_hover_text("The folder of a cloud's own desktop app (Proton Drive, Nextcloud...): listed, not mounted")
                    .clicked()
                {
                    f.choice = Some(Choice::Folder);
                    f.name.clear();
                    f.folder.clear();
                    f.folder_edited = true;
                    f.error.clear();
                }
            });
            ui.add_space(8.0);
            let Some(choice) = f.choice.clone() else {
                ui.label(RichText::new("Choose a provider: you sign in on its own page, through rclone.").color(pal.dim));
                ui.add_space(8.0);
                return;
            };
            let proton = matches!(choice, Choice::Provider(p) if p.kind == cd::PROTON_TYPE);
            if proton {
                // the remote and its folder are the ones Minos's own Proton Drive sign-in and its mount service use
                f.folder = "~/ProtonDrive".into();
            } else if !f.folder_edited {
                f.folder = short(&home, &cd::default_mount(&home, &f.name));
            }
            let running = f.run.is_some();
            if let Choice::Existing(cur) = &choice {
                let mut pick = cur.clone();
                ui.horizontal(|ui| {
                    ui.label("Remote");
                    egui::ComboBox::from_id_salt("cloud-existing").selected_text(pick.clone()).show_ui(ui, |ui| {
                        for r in &existing {
                            ui.selectable_value(&mut pick, r.name.clone(), format!("{} ({})", r.name, cd::provider_label(&r.provider)));
                        }
                    });
                });
                if pick != *cur {
                    f.choice = Some(Choice::Existing(pick.clone()));
                    f.name = pick;
                }
            } else {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add_enabled_ui(!running && !proton, |ui| ui::field(ui, pal, &mut f.name, "Name", 220.0, false));
                });
            }
            ui.horizontal(|ui| {
                ui.label(if choice == Choice::Folder { "Folder" } else { "Mounted at" });
                let r = ui.add_enabled_ui(!running && !proton, |ui| ui::field(ui, pal, &mut f.folder, "~/Cloud/Name", 320.0, false)).inner;
                if r.changed() {
                    f.folder_edited = true;
                }
            });
            if let Choice::Provider(p) = &choice {
                ui.label(RichText::new(p.about).color(pal.dim));
                if p.sign_in == SignIn::Terminal {
                    ui.label(
                        RichText::new("A terminal opens with rclone's questions; answer them there. This page waits until it is done.")
                            .color(pal.dim),
                    );
                }
            }
            if proton && proton_there {
                ui.label(RichText::new("Proton Drive is added already (it is in the list above).").color(pal.amber));
            }
            ui.add_space(6.0);
            let folder = expand(&home, &f.folder);
            ui.horizontal(|ui| {
                let label = match &choice {
                    Choice::Provider(_) if running => "Signing in...",
                    Choice::Provider(_) => "Sign in",
                    _ => "Add",
                };
                let ok = !running && !(proton && proton_there);
                if ui.add_enabled_ui(ok, |ui| ui::action(ui, pal, label)).inner.clicked() {
                    f.error.clear();
                    let name = f.name.trim().to_string();
                    match &choice {
                        Choice::Folder => {
                            if name.is_empty() || name.chars().any(char::is_control) || taken.contains(&name) {
                                f.error = "Give it a name that is not used yet".into();
                            } else if !cd::valid_path(&folder) {
                                f.error = "Give the folder as a full path (~/ for your home folder)".into();
                            } else {
                                add_now = Some(Drive { name, kind: Kind::Folder { path: folder.clone() } });
                            }
                        }
                        Choice::Existing(r) => {
                            let provider = existing.iter().find(|x| x.name == *r).map(|x| x.provider.clone()).unwrap_or_default();
                            if !cd::valid_name(r) {
                                f.error = "This remote's name has characters a mount cannot use (only letters, digits, - and _); rename \
                                           it in rclone first"
                                    .into();
                            } else if !cd::valid_path(&folder) {
                                f.error = "Give the folder as a full path (~/ for your home folder)".into();
                            } else {
                                add_now = Some(Drive { name: r.clone(), kind: Kind::Remote { provider, mount: folder.clone() } });
                            }
                        }
                        Choice::Provider(p) => {
                            if !proton && (!cd::valid_name(&name) || taken.iter().any(|t| t.eq_ignore_ascii_case(&name))) {
                                f.error = "Give it a name that is not used yet: letters, digits, - and _".into();
                            } else if !cd::valid_path(&folder) {
                                f.error = "Give the folder as a full path (~/ for your home folder)".into();
                            } else {
                                start = Some((p, name, folder.clone()));
                            }
                        }
                    }
                }
                if ui::quiet(ui, pal, "Cancel").clicked() {
                    close = true;
                }
                if running {
                    ui.spinner();
                }
            });
            if !f.error.is_empty() {
                ui.label(RichText::new(&f.error).color(pal.red));
            }
            if let Some(run) = &f.run {
                let lines = run.lines.lock().map(|l| l.clone()).unwrap_or_default();
                if run.provider.sign_in == SignIn::Browser || !lines.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("What rclone says").color(pal.dim));
                    egui::Frame::new().fill(pal.bg).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(8)).show(
                        ui,
                        |ui| {
                            egui::ScrollArea::vertical().max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                for l in &lines {
                                    ui.label(RichText::new(l).monospace().size(12.0));
                                }
                            });
                        },
                    );
                }
            }
            ui.add_space(8.0);
        });
        if let Some(d) = add_now {
            let label = d.label().to_string();
            let r = cd::add(&cd::Paths::user(), d);
            self.status = r.map(|()| format!("{label} added: it is in Minos under Cloud Storage"));
            close = true;
            self.loading = false;
            self.refresh(ctx, true);
        }
        if let Some((p, name, mount)) = start {
            self.start_sign_in(ctx, p, name, mount);
        }
        if close {
            if let Some(mut run) = self.add.take().and_then(|f| f.run) {
                // the sign-in is ours: stopped with the form
                let _ = run.child.kill();
                let _ = run.child.wait();
            }
        }
    }

    fn start_sign_in(&mut self, ctx: &egui::Context, p: &'static Provider, name: String, mount: PathBuf) {
        let Some(f) = self.add.as_mut() else { return };
        if crate::fixture::get().is_some() {
            f.error = "A made-up system (a fixture): nothing is changed.".into();
            return;
        }
        let mut argv: Vec<String> = vec!["rclone".into()];
        argv.extend(cd::sign_in_args(&name, p));
        let lines: Arc<Mutex<Vec<String>>> = Default::default();
        let child = match p.sign_in {
            SignIn::Browser => {
                Command::new(&argv[0]).args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map(
                    |mut c| {
                        for out in [
                            c.stdout.take().map(|o| Box::new(o) as Box<dyn std::io::Read + Send>),
                            c.stderr.take().map(|e| Box::new(e) as _),
                        ]
                        .into_iter()
                        .flatten()
                        {
                            let (lines, ctx) = (lines.clone(), ctx.clone());
                            std::thread::spawn(move || {
                                for l in BufReader::new(out).lines().map_while(Result::ok) {
                                    if let Ok(mut v) = lines.lock() {
                                        v.push(l);
                                        let extra = v.len().saturating_sub(OUTPUT_LINES);
                                        v.drain(..extra);
                                    }
                                    ctx.request_repaint();
                                }
                            });
                        }
                        c
                    },
                )
            }
            SignIn::Terminal => {
                let installed = |n: &str| cd::Real.find(n).is_some();
                match cd::in_terminal(&argv, std::env::var("TERMINAL").ok().as_deref(), &installed) {
                    Some(cmd) => Command::new(&cmd[0]).args(&cmd[1..]).stdin(Stdio::null()).spawn(),
                    None => Err(std::io::Error::other("no terminal program is installed")),
                }
            }
        };
        match child {
            Ok(child) => f.run = Some(SignInRun { child, name, provider: p, mount, lines, started: Instant::now() }),
            Err(e) => f.error = format!("Could not start the sign-in: {e}"),
        }
    }

    /// A sign-in that finished: a browser one by its exit, a terminal one when rclone has the remote (some terminals hand the
    /// window to a running copy of themselves and return at once, so their exit says nothing).
    fn poll_sign_in(&mut self, ctx: &egui::Context) {
        let Some(f) = self.add.as_mut() else { return };
        let Some(run) = f.run.as_mut() else { return };
        let exited = run.child.try_wait().ok().flatten();
        let has_remote = self.remotes.iter().flatten().flatten().any(|r| r.name == run.name && r.signed_in);
        let finished = match run.provider.sign_in {
            SignIn::Browser => exited.is_some(),
            SignIn::Terminal => has_remote,
        };
        if !finished {
            if run.provider.sign_in == SignIn::Terminal && run.started.elapsed() > Duration::from_secs(30 * 60) {
                f.error = "Stopped waiting for the sign-in in the terminal".into();
                f.run = None;
            }
            return;
        }
        let ok = match exited {
            Some(st) if run.provider.sign_in == SignIn::Browser => st.success(),
            _ => true,
        };
        let (name, provider, mount) = (run.name.clone(), run.provider, run.mount.clone());
        if !ok {
            f.error = "The sign-in did not finish (see what rclone says above)".into();
            f.run = None;
            return;
        }
        f.run = None;
        self.add = None;
        if provider.kind == cd::PROTON_TYPE {
            self.status = Ok("Proton Drive is added: Mount it to see its files".into());
        } else {
            let d = Drive { name: name.clone(), kind: Kind::Remote { provider: provider.kind.into(), mount } };
            self.status = cd::add(&cd::Paths::user(), d).map(|()| format!("{name} is added: Mount it to see its files"));
        }
        self.loading = false;
        self.refresh(ctx, true);
    }

    /// "Remove NAME?": what goes and what stays, then the removal on a thread.
    fn remove_window(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some(d) = self.confirm_remove.clone() else { return };
        let (mut go, mut cancel) = (false, ctx.input(|i| i.key_pressed(egui::Key::Escape)));
        egui::Window::new(format!("Remove {}?", d.label()))
            .id(egui::Id::new("cloud-remove"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                let what = match d.kind {
                    Kind::Remote { .. } => format!(
                        "{} is unmounted and its rclone sign-in and mount service are removed from this computer. Your files in \
                         the cloud are not touched; add it again to see them.",
                        d.label()
                    ),
                    Kind::Folder { .. } => format!("{} leaves the list. The folder and its files stay where they are.", d.label()),
                };
                ui.add(egui::Label::new(what).wrap());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Remove").color(egui::Color32::WHITE)).fill(pal.red)).clicked() {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            self.confirm_remove = None;
        }
        if go {
            self.confirm_remove = None;
            self.act(ctx, format!("Removing {}", d.label()), move || {
                cd::remove(&SettingsSys, &cd::Paths::user(), &d).map(|()| format!("{} is removed", d.label()))
            });
        }
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        // leaving the page stops a browser sign-in still waiting (a terminal one goes on in its own window)
        if let Some(run) = self.add.as_mut().and_then(|f| f.run.as_mut()) {
            if run.provider.sign_in == SignIn::Browser {
                let _ = run.child.kill();
                let _ = run.child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_folders_are_typed_and_shown_with_a_tilde() {
        let home = Path::new("/home/me");
        assert_eq!(expand(home, "~/Cloud/x"), PathBuf::from("/home/me/Cloud/x"));
        assert_eq!(expand(home, " /mnt/c "), PathBuf::from("/mnt/c"));
        assert_eq!(expand(home, "~"), PathBuf::from("/home/me"));
        assert_eq!(short(home, Path::new("/home/me/Cloud/x")), "~/Cloud/x");
        assert_eq!(short(home, Path::new("/mnt/c")), "/mnt/c");
    }
}
