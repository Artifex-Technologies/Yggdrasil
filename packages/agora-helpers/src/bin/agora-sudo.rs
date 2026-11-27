//! /usr/lib/agora/sudo: a stand-in for `sudo` that asks through the desktop's password box (polkit) instead of a terminal prompt.
//! Agora gives it to `yay --sudo` and to an operating system's component command (AGORA_SUDO) so installs can run without a
//! terminal. `sudo -v` and other options do nothing.
//!
//! Options before the command are skipped (`--` ends them) and no command at all is success. `pacman ...` always goes through
//! pkg-root, which checks the command and whose password polkit remembers a few minutes for the program that asked
//! (org.yggdrasil.agora.packages), so one yay run asks once: with Agora's cancel file (AGORA_CANCEL_FILE) the install can be
//! cancelled and cleaned up; without one (device offers, the first login's components, makepkg) it gets a cancel file in the
//! user's runtime folder that nothing makes. The operating system's component command with `setup ID...` or `teardown ID...` (its
//! root steps beside the packages) goes through pkg-root the same way, so a component's whole install asks once. Anything else is
//! `pkexec COMMAND...`, asked every time. This process becomes pkexec.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let rest = command(&args);
    if rest.is_empty() {
        std::process::exit(0);
    }
    // SAFETY: getuid has no preconditions and cannot fail
    let uid = unsafe { libc::getuid() };
    // the component command is looked up (in the integration files) only for what could be its setup or teardown
    let component = (rest.len() >= 3 && (rest[1] == "setup" || rest[1] == "teardown"))
        .then(|| pkg_root::component_command(&pkg_root::INTEGRATIONS.map(PathBuf::from), 0).ok())
        .flatten();
    let cancel = std::env::var_os("AGORA_CANCEL_FILE");
    let e = Command::new("pkexec").args(pkexec_args(rest, cancel, component, uid, std::process::id())).exec();
    if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!("/usr/lib/agora/sudo: pkexec: not found");
        std::process::exit(127)
    }
    eprintln!("/usr/lib/agora/sudo: pkexec: {e}");
    std::process::exit(126)
}

const PKG_ROOT: &str = "/usr/lib/agora/pkg-root";

/// pkexec's arguments for `rest`: pacman, or the component command's setup or teardown (`component setup|teardown ID...`),
/// through pkg-root with Agora's cancel file, or without one (unset or empty) a file in the user's runtime folder named after
/// this process, which nothing makes; anything else as it is.
fn pkexec_args(rest: &[OsString], cancel: Option<OsString>, component: Option<PathBuf>, uid: u32, pid: u32) -> Vec<OsString> {
    let setup = rest.len() >= 3 && component.is_some_and(|c| rest[0] == c.as_os_str()) && (rest[1] == "setup" || rest[1] == "teardown");
    if rest[0] != "pacman" && !setup {
        return rest.to_vec();
    }
    let cancel = cancel.filter(|c| !c.is_empty()).unwrap_or_else(|| format!("/run/user/{uid}/agora/sudo-{pid}.cancel").into());
    let mut v = vec![OsString::from(PKG_ROOT), cancel];
    if setup {
        v.push("component".into());
        v.extend_from_slice(&rest[1..]);
    } else {
        v.extend_from_slice(rest);
    }
    v
}

/// The command after sudo's options: everything up to `--` (dropped) or the first word not starting with `-`.
fn command(args: &[OsString]) -> &[OsString] {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_encoded_bytes();
        if a == b"--" {
            return &args[i + 1..];
        }
        if !a.starts_with(b"-") {
            break;
        }
        i += 1;
    }
    &args[i..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(a: &[&str]) -> Vec<String> {
        let v: Vec<OsString> = a.iter().map(OsString::from).collect();
        command(&v).iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn options_before_the_command_are_skipped() {
        assert_eq!(cmd(&["-v"]), Vec::<String>::new());
        assert_eq!(cmd(&[]), Vec::<String>::new());
        assert_eq!(cmd(&["pacman", "-S", "x"]), ["pacman", "-S", "x"]);
        assert_eq!(cmd(&["-E", "-k", "pacman", "-U", "--", "f"]), ["pacman", "-U", "--", "f"]);
        assert_eq!(cmd(&["-E", "--", "-weird", "a"]), ["-weird", "a"]);
        assert_eq!(cmd(&["-", "x"]), ["x"]);
        assert_eq!(cmd(&["--", "--"]), ["--"]);
    }

    fn pkexec(rest: &[&str], cancel: Option<&str>) -> Vec<String> {
        with_component(rest, cancel, None)
    }

    fn with_component(rest: &[&str], cancel: Option<&str>, component: Option<&str>) -> Vec<String> {
        let rest: Vec<OsString> = rest.iter().map(OsString::from).collect();
        pkexec_args(&rest, cancel.map(OsString::from), component.map(PathBuf::from), 1000, 42)
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_components_setup_and_teardown_go_through_pkg_root_too() {
        let os = Some("/usr/bin/os-apps");
        assert_eq!(
            with_component(&["/usr/bin/os-apps", "setup", "waydroid", "razer"], None, os),
            ["/usr/lib/agora/pkg-root", "/run/user/1000/agora/sudo-42.cancel", "component", "setup", "waydroid", "razer"]
        );
        assert_eq!(
            with_component(&["/usr/bin/os-apps", "teardown", "waydroid"], Some("/run/user/1000/agora/x.cancel"), os),
            ["/usr/lib/agora/pkg-root", "/run/user/1000/agora/x.cancel", "component", "teardown", "waydroid"]
        );
        // anything else of it, another program's setup, or a system without a component command: as they are
        for rest in [&["/usr/bin/os-apps", "install", "x"][..], &["/usr/bin/other", "setup", "x"], &["/usr/bin/os-apps", "setup"]] {
            assert_eq!(with_component(rest, None, os), rest, "{rest:?}");
        }
        assert_eq!(with_component(&["/usr/bin/os-apps", "setup", "x"], None, None), ["/usr/bin/os-apps", "setup", "x"]);
    }

    #[test]
    fn pacman_always_goes_through_pkg_root_so_the_password_is_asked_once() {
        // Agora's own install: its cancel file
        assert_eq!(
            pkexec(&["pacman", "-U", "--", "a.pkg.tar.zst"], Some("/run/user/1000/agora/x.cancel")),
            ["/usr/lib/agora/pkg-root", "/run/user/1000/agora/x.cancel", "pacman", "-U", "--", "a.pkg.tar.zst"]
        );
        // device offers, the first login's components, makepkg: no cancel file, so one that is never made
        for cancel in [None, Some("")] {
            let a = pkexec(&["pacman", "-S", "--needed", "x"], cancel);
            assert_eq!(a[0], "/usr/lib/agora/pkg-root");
            assert_eq!(&a[2..], ["pacman", "-S", "--needed", "x"]);
            assert_eq!(a[1], "/run/user/1000/agora/sudo-42.cancel");
            assert!(pkg_root::valid_cancel_file(&a[1], "1000"), "pkg-root takes it: {}", a[1]);
        }
        // anything else is pkexec COMMAND as before
        assert_eq!(pkexec(&["systemctl", "enable", "x.service"], None), ["systemctl", "enable", "x.service"]);
        assert_eq!(pkexec(&["/usr/bin/pacman", "-S", "x"], None), ["/usr/bin/pacman", "-S", "x"]);
    }
}
