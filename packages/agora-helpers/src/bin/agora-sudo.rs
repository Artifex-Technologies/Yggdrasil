//! /usr/lib/agora/sudo: a stand-in for `sudo` that asks through the desktop's password box (polkit) instead of a terminal prompt.
//! Agora gives it to `yay --sudo` and to an operating system's component command (AGORA_SUDO) so installs can run without a
//! terminal. `sudo -v` and other options do nothing.
//!
//! As the shell script it replaced: options before the command are skipped (`--` ends them), no command at all is success,
//! `pacman ...` goes through pkg-root when Agora gave a cancel file (AGORA_CANCEL_FILE), so the install can be cancelled and
//! cleaned up; anything else is `pkexec COMMAND...`. This process becomes pkexec.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let rest = command(&args);
    if rest.is_empty() {
        std::process::exit(0);
    }
    let mut c = Command::new("pkexec");
    match std::env::var_os("AGORA_CANCEL_FILE") {
        Some(cancel) if rest[0] == "pacman" && !cancel.is_empty() => {
            c.arg("/usr/lib/agora/pkg-root").arg(cancel).arg("pacman").args(&rest[1..]);
        }
        _ => {
            c.args(rest);
        }
    }
    let e = c.exec();
    if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!("/usr/lib/agora/sudo: pkexec: not found");
        std::process::exit(127)
    }
    eprintln!("/usr/lib/agora/sudo: pkexec: {e}");
    std::process::exit(126)
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
}
