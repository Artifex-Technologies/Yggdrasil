//! /usr/lib/yggdrasil/fingerprint: the fingerprint reader for Settings > Users (laptop::fingerprint).
//!
//!   fingerprint json            the reader, your enrolled fingers, and whether sudo takes a finger
//!   fingerprint enroll [FINGER] enrol a finger (right-index-finger unless named); each step on its own line
//!   fingerprint delete          remove your enrolled fingers
//!   fingerprint sudo on|off     as root only, through pkexec (org.yggdrasil.fingerprint.sudo): fingerprint for sudo

use std::path::Path;

use laptop::fingerprint;

const FINGERS: [&str; 10] = [
    "left-thumb",
    "left-index-finger",
    "left-middle-finger",
    "left-ring-finger",
    "left-little-finger",
    "right-thumb",
    "right-index-finger",
    "right-middle-finger",
    "right-ring-finger",
    "right-little-finger",
];

/// This user's login (fprintd keeps fingers per login).
fn login() -> String {
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_name.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
        }
    }
    String::new()
}

fn system() -> Result<zbus::blocking::Connection, String> {
    zbus::blocking::Connection::system().map_err(|e| e.to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r: Result<(), String> = match args.first().map(String::as_str) {
        Some("json") => system().map(|c| println!("{}", fingerprint::status(&c, &login(), Path::new(fingerprint::SUDO_PAM)).json())),
        Some("enroll") => {
            let finger = args.get(1).map(String::as_str).unwrap_or("right-index-finger");
            if !FINGERS.contains(&finger) {
                Err(format!("not a finger fprintd knows: {finger}"))
            } else {
                println!("Touch the reader with your finger");
                system().and_then(|c| {
                    fingerprint::enroll(&c, finger, |step| println!("{}", fingerprint::words(step)))
                        .map_err(|last| fingerprint::words(&last).to_string())
                })
            }
        }
        Some("delete") => system().and_then(|c| fingerprint::delete_all(&c)),
        Some("sudo") => match args.get(1).map(String::as_str) {
            _ if unsafe { libc::geteuid() } != 0 => Err("only through pkexec: Settings > Users".into()),
            Some(v @ ("on" | "off")) => fingerprint::set_sudo(Path::new(fingerprint::SUDO_PAM), v == "on", Path::new(fingerprint::MODULE))
                .map(|changed| println!("{}", if changed { "changed" } else { "unchanged" })),
            _ => Err("on or off".into()),
        },
        _ => Err("usage: fingerprint json | enroll [FINGER] | delete | sudo on|off".into()),
    };
    if let Err(e) = r {
        eprintln!("fingerprint: {e}");
        std::process::exit(1);
    }
}
