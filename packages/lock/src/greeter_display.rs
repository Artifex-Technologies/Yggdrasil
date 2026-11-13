//! /usr/lib/yggdrasil/greeter-display MAKE MODEL CONNECTOR: hands an account's main display to the login screen.
//!
//! The login screen runs as the `greeter` account and cannot read home folders, so the panel runs this through pkexec at each
//! sign-in and when the choice changes (polkit action org.yggdrasil.greeter.main-display: yes, without a password, for an account
//! signed in at this computer; no for anyone else, so it never asks). As root it writes the three values, and nothing else, to
//! /var/lib/yggdrasil/greeter/display.conf (yggdrasil_conf::display::SYSTEM_PATH), readable by all. Values the login screen would
//! not accept are refused here already (yggdrasil_conf::display::system_text). An empty maker and model with a connector is a
//! choice by connector alone.
//!
//! Exit status: 0 written, 1 could not write (or not run as root), 2 refused (the arguments).

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use yggdrasil_conf::display::{system_text, MainDisplay, SYSTEM_PATH};

/// Writes the copy at `path`: the folder made for it if missing (0755), the text written beside and renamed over the old one
/// (so the login screen never reads half a file), 0644 whatever the umask.
fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    std::fs::DirBuilder::new().recursive(true).mode(0o755).create(dir)?;
    if !std::fs::symlink_metadata(dir)?.is_dir() {
        return Err(std::io::Error::other("not a folder"));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
    let tmp = dir.join(format!(".display.conf.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).custom_flags(libc::O_NOFOLLOW).open(&tmp)?;
    let done = f
        .write_all(text.as_bytes())
        .and_then(|_| f.set_permissions(std::fs::Permissions::from_mode(0o644)))
        .and_then(|_| f.sync_all())
        .and_then(|_| std::fs::rename(&tmp, path));
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [make, model, connector] = a.as_slice() else {
        eprintln!("usage: greeter-display MAKE MODEL CONNECTOR");
        std::process::exit(2);
    };
    let c = MainDisplay { make: make.clone(), model: model.clone(), connector: connector.clone() };
    let Some(text) = system_text(&c) else {
        eprintln!("greeter-display: not a display the login screen can use");
        std::process::exit(2);
    };
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("greeter-display: only through pkexec");
        std::process::exit(1);
    }
    if let Err(e) = write(Path::new(SYSTEM_PATH), &text) {
        eprintln!("greeter-display: {SYSTEM_PATH}: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn the_copy_is_written_whole_and_readable_by_all() {
        let d = std::env::temp_dir().join(format!("ygg-greeter-display-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let f = d.join("greeter/display.conf");
        let c = MainDisplay { make: "Example Display".into(), model: "UW34".into(), connector: "DP-1".into() };
        let old = unsafe { libc::umask(0o077) };
        write(&f, &system_text(&c).unwrap()).unwrap();
        unsafe { libc::umask(old) };
        assert_eq!(std::fs::metadata(&f).unwrap().mode() & 0o777, 0o644, "whatever the umask");
        assert_eq!(std::fs::metadata(f.parent().unwrap()).unwrap().mode() & 0o777, 0o755);
        let me = unsafe { libc::geteuid() };
        assert_eq!(yggdrasil_conf::display::load_system(&f, me), Some(c));
        // the latest one wins, and nothing is left beside it
        let c2 = MainDisplay { connector: "DP-3".into(), ..Default::default() };
        write(&f, &system_text(&c2).unwrap()).unwrap();
        assert_eq!(yggdrasil_conf::display::load_system(&f, me), Some(c2));
        assert_eq!(std::fs::read_dir(f.parent().unwrap()).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
