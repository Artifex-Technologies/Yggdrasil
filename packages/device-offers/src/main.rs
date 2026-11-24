//! yggdrasil-device-offers: offers the software for this computer's devices that is not installed yet (Razer, Corsair, Logitech,
//! gaming mice, printers, a laptop's fingerprint reader ...), once the computer is online. One notification with "Install" (the
//! operating system's component command, through Agora's password box) and "Don't ask again"; closing it means not now. Nothing
//! is installed without that click.
//!
//! systemd starts it in the signed-in user's session (data/): at sign-in, when a USB device is plugged in (udev), and when the
//! computer comes online (NetworkManager's dispatcher, device-offers-online). It looks once and exits; offline it exits at once.
//!
//! What it looks at: the USB devices plugged in (/sys), the Bluetooth devices paired that name a USB maker (BlueZ, through
//! busctl), and what the computer is. What it reads: the OS's `packages` integration (Yggdrasil's docs/INTEGRATIONS.md): `devices` (its list, the installer's
//! extras file), `components` and `component` (what Agora installs with), `first_login` (what the OS still installs by itself:
//! left to it). Asked once per session ($XDG_RUNTIME_DIR/yggdrasil/device-offers-asked); "Don't ask again" is kept in
//! ~/.local/state/yggdrasil/device-offers-never; what installing said goes to ~/.local/state/yggdrasil/device-offers.log.
//!
//!   yggdrasil-device-offers                  look now and offer
//!   yggdrasil-device-offers --print          what would be offered now (id and components, one per line); asks nothing, online or not
//!   yggdrasil-device-offers --json           this computer's offers as JSON, the ones said no to among them ("never": true), for
//!                                            Settings > Peripherals: [{"id", "name", "note", "components", "never"}]
//!   yggdrasil-device-offers --install ID...  installs those offers now (as the Install button does)
//!   yggdrasil-device-offers --ask-again ID...  undoes "Don't ask again" for them
//!
//! For tests only: DEVICE_OFFERS_ROOT (another / for /sys and /proc/cpuinfo), DEVICE_OFFERS_INTEGRATIONS (folders with integration files,
//! separated by ':'), DEVICE_OFFERS_AGORA_SUDO (another password box).

use device_offers::{self as offers, Hardware, Offer};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use yggdrasil_conf::integrations::{self, Integration};

const AGORA_SUDO: &str = "/usr/lib/agora/sudo";

/// What the OS's packages integration says.
struct Os {
    devices: PathBuf,
    components: PathBuf,
    command: Vec<String>,
    first_login: Option<PathBuf>,
}

fn os() -> Option<Os> {
    let all = match std::env::var_os("DEVICE_OFFERS_INTEGRATIONS") {
        Some(d) => integrations::all_in(&std::env::split_paths(&d).collect::<Vec<_>>()),
        None => integrations::all(),
    };
    let i: &Integration = all.iter().find(|i| i.kind == "packages" && i.get("devices").is_some())?;
    Some(Os {
        devices: i.path("devices")?,
        components: i.path("components")?,
        command: i.command("component")?,
        first_login: i.path("first_login"),
    })
}

fn root() -> PathBuf {
    std::env::var_os("DEVICE_OFFERS_ROOT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Online now, without waiting: NetworkManager's last answer ("full"), or without it a default route (this network namespace's).
fn online() -> bool {
    match Command::new("nmcli").args(["-t", "networking", "connectivity"]).stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim() == "full",
        Err(_) => offers::default_route(Path::new("/")),
    }
}

/// The installed packages (pacman -Qq), as Agora reads them.
fn installed() -> HashSet<String> {
    Command::new("pacman")
        .arg("-Qq")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The paired Bluetooth devices that name a USB maker (BlueZ over the system bus, one `busctl` call: it fails at once where
/// BlueZ is not running, where bluetoothctl would wait for it).
fn bluetooth() -> Vec<offers::Usb> {
    Command::new("busctl")
        .args(["--system", "--json=short", "call", "org.bluez", "/", "org.freedesktop.DBus.ObjectManager", "GetManagedObjects"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| offers::bluetooth_paired(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

fn asked_file() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()).map(|d| d.join("yggdrasil/device-offers-asked"))
}

fn never_file() -> PathBuf {
    yggdrasil_conf::dirs::state().join("device-offers-never")
}

/// What to offer now, leaving out `asked` (this session's) and, unless `with_never`, the ones said no to.
fn look(os: &Os, asked: &HashSet<String>, with_never: bool) -> Vec<Offer> {
    let root = root();
    let mut hw = Hardware::read(&root);
    hw.usb.extend(bluetooth());
    let list = std::fs::read_to_string(&os.devices).unwrap_or_default();
    let comps = offers::component_packages(&std::fs::read_to_string(&os.components).unwrap_or_default());
    let user = std::env::var("USER").unwrap_or_default();
    let pending =
        os.first_login.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| offers::pending(&t, &user)).unwrap_or_default();
    let mut leave = if with_never { HashSet::new() } else { offers::read_ids(&never_file()) };
    leave.extend(asked.iter().cloned());
    let exists = |p: &str| root.join(p.trim_start_matches('/')).exists();
    offers::offers(&list, &comps, &hw, &exists, &installed(), &leave, &pending)
}

fn names(list: &[Offer]) -> String {
    list.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// The notification's title and text.
fn wording(list: &[Offer]) -> (String, String) {
    let tail = "Installing asks for your password. Agora can add or remove it later.";
    match list {
        [one] => {
            let note = if one.note.is_empty() { String::new() } else { format!("{} ", one.note) };
            (format!("Software for {}", one.name), format!("Found on this computer. {note}{tail}"))
        }
        _ => ("Software for your devices".into(), format!("Found on this computer: {}. {tail}", names(list))),
    }
}

/// A notification without buttons (no -A: notify-send would wait for an answer).
fn tell(title: &str, body: &str) {
    let _ = Command::new("notify-send").args(["-a", "Agora", "-i", "agora", title, body]).stdin(Stdio::null()).status();
}

enum Answer {
    Install,
    Never,
    /// closed, or no answer
    Later,
    /// no notification server to ask through
    NoOne,
}

/// Asks with "Install" and "Don't ask again" and waits for the answer (notify-send -A waits until a button is clicked or the
/// notification is closed). Just after sign-in the panel that shows notifications may not be up yet: tried for at most 30 s.
fn ask(list: &[Offer]) -> Answer {
    let (title, body) = wording(list);
    let start = Instant::now();
    loop {
        let out = Command::new("notify-send")
            .args(["-a", "Agora", "-i", "agora", "-A", "install=Install", "-A", "never=Don't ask again", &title, &body])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        match out {
            Ok(o) if o.status.success() => {
                return match String::from_utf8_lossy(&o.stdout).trim() {
                    "install" => Answer::Install,
                    "never" => Answer::Never,
                    _ => Answer::Later,
                }
            }
            Ok(_) if start.elapsed() < Duration::from_secs(30) => std::thread::sleep(Duration::from_secs(3)),
            _ => return Answer::NoOne,
        }
    }
}

/// Installs the offers' components with the OS's component command (`install ID...`), its root steps through Agora's password box;
/// true when it did.
fn install(os: &Os, list: &[Offer]) -> bool {
    let ids: Vec<String> = list.iter().flat_map(|o| o.components.iter().cloned()).collect();
    let names = names(list);
    tell(&format!("Installing {names}"), "Your password is asked once; this can take a while.");
    let log_path = yggdrasil_conf::dirs::state().join("device-offers.log");
    let _ = std::fs::create_dir_all(log_path.parent().unwrap_or(Path::new("/")));
    let log = std::fs::OpenOptions::new().append(true).create(true).open(&log_path);
    let mut c = Command::new(&os.command[0]);
    c.args(&os.command[1..]).arg("install").args(&ids).stdin(Stdio::null());
    match log.and_then(|l| Ok((l.try_clone()?, l))) {
        Ok((o, e)) => {
            c.stdout(o).stderr(e);
        }
        Err(_) => {
            c.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    let sudo = std::env::var("DEVICE_OFFERS_AGORA_SUDO").unwrap_or_else(|_| AGORA_SUDO.into());
    if Path::new(&sudo).exists() {
        c.env("AGORA_SUDO", &sudo);
    }
    if c.status().is_ok_and(|s| s.success()) {
        tell("Installed", &format!("Ready: {names}."));
        true
    } else {
        tell(&format!("Could not install {names}"), "Agora can install it too. Details: ~/.local/state/yggdrasil/device-offers.log");
        false
    }
}

/// The offers as Settings reads them (`--json`).
fn offers_json(list: &[Offer], never: &HashSet<String>) -> String {
    let v: Vec<serde_json::Value> = list
        .iter()
        .map(
            |o| serde_json::json!({"id": o.id, "name": o.name, "note": o.note, "components": o.components, "never": never.contains(&o.id)}),
        )
        .collect();
    serde_json::Value::Array(v).to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || -> ! {
        eprintln!("usage: yggdrasil-device-offers [--print | --json | --install ID... | --ask-again ID...]");
        std::process::exit(2);
    };
    let ids: Vec<String> = args.iter().skip(1).cloned().collect();
    let mode = match (args.first().map(String::as_str), ids.is_empty()) {
        (None, _) => "",
        (Some("--print"), true) => "print",
        (Some("--json"), true) => "json",
        (Some("--install"), false) => "install",
        (Some("--ask-again"), false) => "ask-again",
        _ => usage(),
    };
    if mode == "ask-again" {
        // the user's own list: no OS needed to take a name off it
        if let Err(e) = offers::remove_ids(&never_file(), &ids) {
            eprintln!("yggdrasil-device-offers: {}: {e}", never_file().display());
            std::process::exit(1);
        }
        return;
    }
    // an OS without a device list: nothing to do
    let Some(os) = os() else {
        if mode == "json" {
            println!("[]");
        }
        return;
    };
    match mode {
        "print" => {
            for o in look(&os, &HashSet::new(), false) {
                println!("{}\t{}", o.id, o.components.join(" "));
            }
            return;
        }
        "json" => {
            println!("{}", offers_json(&look(&os, &HashSet::new(), true), &offers::read_ids(&never_file())));
            return;
        }
        "install" => {
            let list: Vec<Offer> = look(&os, &HashSet::new(), true).into_iter().filter(|o| ids.contains(&o.id)).collect();
            if list.is_empty() {
                eprintln!("yggdrasil-device-offers: nothing of {} is missing on this computer", ids.join(", "));
                std::process::exit(1);
            }
            std::process::exit(if install(&os, &list) { 0 } else { 1 });
        }
        _ => {}
    }
    let asked_path = asked_file();
    let mut asked = asked_path.as_deref().map(offers::read_ids).unwrap_or_default();
    // a device plugged in while the question was open is looked at when it is answered; each offer is asked once a session
    while online() {
        let list = look(&os, &asked, false);
        if list.is_empty() {
            return;
        }
        let ids: Vec<String> = list.iter().map(|o| o.id.clone()).collect();
        let answer = ask(&list);
        if matches!(answer, Answer::NoOne) {
            return;
        }
        asked.extend(ids.iter().cloned());
        if let Some(p) = &asked_path {
            let _ = offers::add_ids(p, &ids);
        }
        match answer {
            Answer::Install => {
                install(&os, &list);
            }
            Answer::Never => {
                let _ = offers::add_ids(&never_file(), &ids);
            }
            _ => {}
        }
    }
}
