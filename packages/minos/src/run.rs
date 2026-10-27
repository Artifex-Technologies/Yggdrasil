//! "Run in terminal": a program or script you have marked as runnable (the execute permission) is started in a terminal window, in its
//! own folder, so you see everything it prints and can answer its questions. Double-click still only opens a file to read: running
//! is always this deliberate choice, and a file that is not marked runnable is never offered.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Whether `p` is a program or script you may run: a regular file with your execute permission that starts like one (`#!` for a
/// script, the ELF header for a program).
pub fn runnable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else { return false };
    if !meta.is_file() || meta.permissions().mode() & 0o100 == 0 {
        return false;
    }
    let mut head = [0u8; 4];
    let n = std::fs::File::open(p).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    head[..n].starts_with(b"#!") || head[..n] == *b"\x7fELF"
}

/// Terminals Minos knows how to start a command in: (program, the arguments that come before the command).
const TERMINALS: [(&str, &[&str]); 7] = [
    ("ghostty", &["-e"]),
    ("foot", &[]),
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("konsole", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("xterm", &["-e"]),
];

/// The command that runs `file` in a terminal and keeps the window open afterwards, so its last words can be read. `$TERMINAL` (one
/// of the known terminals) is tried first, then the known ones in order; `installed` says whether a program is on this system.
pub fn command(file: &Path, terminal_env: Option<&str>, installed: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let preferred = terminal_env.and_then(|t| TERMINALS.iter().find(|(name, _)| *name == t)).filter(|(name, _)| installed(name));
    let (term, before) = preferred.or_else(|| TERMINALS.iter().find(|(name, _)| installed(name)))?;
    let mut cmd: Vec<String> = std::iter::once(term.to_string()).chain(before.iter().map(|s| s.to_string())).collect();
    // the file is an argument to sh, never part of the script text, so no name can change what runs
    cmd.extend(["sh", "-c", "\"$1\"; code=$?; echo; echo \"Finished (exit code $code). Press Enter to close.\"; read -r _", "sh"].map(String::from));
    cmd.push(file.to_string_lossy().into_owned());
    Some(cmd)
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// Starts `file` in a terminal, in its own folder.
pub fn run_in_terminal(file: &Path) -> Result<(), String> {
    if !runnable(file) {
        return Err("it is not marked as runnable".into());
    }
    let cmd = command(file, std::env::var("TERMINAL").ok().as_deref(), &on_path).ok_or("no terminal program is installed")?;
    let dir = file.parent().unwrap_or(Path::new("/"));
    std::process::Command::new(&cmd[0]).args(&cmd[1..]).current_dir(dir).spawn().map(|_| ()).map_err(|e| format!("could not start {}: {e}", cmd[0]))
}

/// Installs a package from the distribution's repositories in a terminal, where pacman asks for the password and shows what it
/// does. The name is an argument to sh, never script text.
pub fn install_in_terminal(package: &str) -> Result<(), String> {
    if package.is_empty() || !package.chars().all(|c| c.is_ascii_alphanumeric() || "-_.+".contains(c)) {
        return Err("not a package name".into());
    }
    let (term, before) = TERMINALS.iter().find(|(name, _)| on_path(name)).ok_or("no terminal program is installed")?;
    let script = "echo \"Installing $1\"; sudo pacman -S --needed \"$1\"; code=$?; echo; echo \"Finished (exit code $code). Press Enter to close.\"; read -r _";
    std::process::Command::new(term).args(*before).args(["sh", "-c", script, "sh", package]).spawn().map(|_| ()).map_err(|e| format!("could not start {term}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn only_files_marked_runnable_that_look_like_programs() {
        let d = std::env::temp_dir().join(format!("minos-run-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let file = |name: &str, body: &[u8], mode: u32| {
            let p = d.join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p
        };
        assert!(runnable(&file("a.sh", b"#!/bin/sh\necho hi\n", 0o755)));
        assert!(runnable(&file("prog", b"\x7fELF....", 0o700)));
        assert!(!runnable(&file("b.sh", b"#!/bin/sh\necho hi\n", 0o644)), "not marked runnable");
        assert!(!runnable(&file("notes.txt", b"hello", 0o755)), "runnable bit on a text file");
        assert!(!runnable(&d), "a folder");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_terminal_is_chosen_and_the_name_stays_an_argument() {
        let f = PathBuf::from("/home/u/Desktop/my script; rm -rf ~.sh");
        let only = |names: &'static [&'static str]| move |p: &str| names.contains(&p);
        let c = command(&f, None, &only(&["konsole", "ghostty"])).unwrap();
        assert_eq!(&c[..4], &["ghostty", "-e", "sh", "-c"]);
        assert_eq!(c.last().unwrap(), "/home/u/Desktop/my script; rm -rf ~.sh", "the path is passed whole, as $1");
        assert!(!c[4].contains("rm -rf"));
        assert_eq!(command(&f, Some("foot"), &only(&["foot", "ghostty"])).unwrap()[..3], ["foot", "sh", "-c"]);
        assert_eq!(command(&f, Some("weird-term"), &only(&["xterm"])).unwrap()[..2], ["xterm", "-e"], "an unknown $TERMINAL is ignored");
        assert!(command(&f, None, &only(&[])).is_none());
    }
}
