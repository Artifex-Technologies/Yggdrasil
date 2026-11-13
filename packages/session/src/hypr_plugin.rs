//! yggdrasil-hypr-plugin: loads Yggdrasil's Hyprland plugin (minimize buttons work: see
//! /usr/share/yggdrasil/hyprland/plugin/minimize.cpp). Hyprland only loads a plugin built against its own version, so it is built
//! here, once per Hyprland version (about two seconds), into ~/.cache/yggdrasil/hyprland-plugin/<Hyprland's commit>/, and loaded
//! from there. Run at the session's start.
//!
//! As the script, which read Hyprland's version with python3 (the session package needed Python for that alone): the same
//! calls of hyprctl, pkg-config and g++ (by argv: pkg-config's flags are split at blanks, as the shell split them), the same
//! files, messages and exit codes. Not 1:1: a "commit" that is not a JSON string (Python printed any value) counts as no answer.

use crate::run;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const ME: &str = "yggdrasil-hypr-plugin";
const SRC: &str = "/usr/share/yggdrasil/hyprland/plugin/minimize.cpp";

fn log(msg: &str) {
    eprintln!("{ME}: {msg}");
}

pub fn main() {
    std::process::exit(load())
}

fn load() -> i32 {
    let commit = commit(&run::output(Command::new("hyprctl").args(["-j", "version"])));
    let Some(commit) = commit else {
        log("Hyprland is not answering");
        return 1;
    };
    let list = Command::new("hyprctl").args(["plugin", "list"]).stderr(Stdio::null()).output().map(|o| o.stdout).unwrap_or_default();
    if list.split(|&b| b == b'\n').any(|l| l.starts_with(b"Plugin yggdrasil-minimize ")) {
        return 0;
    }
    let dir = run::xdg("XDG_CACHE_HOME", ".cache").join("yggdrasil/hyprland-plugin");
    let so = dir.join(&commit).join("yggdrasil-minimize.so");
    if !non_empty(&so) || older(&so, Path::new(SRC)) {
        if let Err(code) = build(&dir, &commit, &so) {
            return code;
        }
    }
    // $(hyprctl plugin load SO 2>&1): its output and its errors through one pipe, in the order written
    let mut text = both(Command::new("hyprctl").args(["plugin", "load"]).arg(&so));
    while text.last() == Some(&b'\n') {
        text.pop();
    }
    if !text.starts_with(b"ok") {
        log(&format!("loading failed: {}", String::from_utf8_lossy(&text)));
        return 1;
    }
    log("loaded");
    0
}

fn build(dir: &Path, commit: &str, so: &Path) -> Result<(), i32> {
    if run::which("g++").is_none() {
        log("no C++ compiler (g++), so the plugin cannot be built");
        return Err(1);
    }
    let flags = Command::new("pkg-config").args(["--cflags", "hyprland", "pixman-1", "libdrm"]).stderr(Stdio::null()).output();
    let flags = match flags {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => {
            log("Hyprland's headers are not installed");
            return Err(1);
        }
    };
    let here = dir.join(commit);
    if let Err(e) = std::fs::create_dir_all(&here) {
        eprintln!("mkdir: cannot create directory '{}': {}", here.display(), e);
    }
    let tmp = PathBuf::from(format!("{}.tmp", so.display()));
    let built = std::fs::File::create(here.join("build.log")).ok().is_some_and(|logf| {
        let mut c = Command::new("g++");
        c.args(["-shared", "-fPIC", "-std=c++26", "-O2"]).args(flags.split([' ', '\t', '\n']).filter(|w| !w.is_empty()));
        c.arg(SRC).arg("-o").arg(&tmp).stderr(logf);
        c.status().is_ok_and(|s| s.success())
    });
    if !built {
        log(&format!("the plugin did not build for Hyprland {commit} (see {}/build.log)", here.display()));
        let _ = std::fs::remove_file(&tmp);
        return Err(1);
    }
    if let Err(e) = std::fs::rename(&tmp, so) {
        eprintln!("mv: cannot move '{}' to '{}': {}", tmp.display(), so.display(), e);
    }
    // builds for older Hyprland versions are of no use any more
    if let Ok(d) = std::fs::read_dir(dir) {
        for e in d.flatten() {
            if e.file_name() != commit && e.file_type().is_ok_and(|t| t.is_dir()) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    log(&format!("built for Hyprland {commit}"));
    Ok(())
}

/// Hyprland's commit from `hyprctl -j version`: a JSON object's "commit", 7 to 64 lowercase hex digits (the newlines at its
/// end dropped, as the shell's $(...) dropped them).
pub fn commit(json: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let c = v.get("commit")?.as_str()?.trim_end_matches('\n');
    ((7..=64).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).then(|| c.to_string())
}

/// What a command wrote to its standard output and error together (one pipe for both); nothing when it could not start.
fn both(c: &mut Command) -> Vec<u8> {
    use std::io::Read;
    let Ok((mut r, w)) = std::io::pipe() else { return vec![] };
    let Ok(w2) = w.try_clone() else { return vec![] };
    let Ok(mut child) = c.stdout(w).stderr(w2).spawn() else { return vec![] };
    // the writing ends went to the child with the command; reading stops when it (and anything it left running) closes them
    c.stdout(Stdio::null()).stderr(Stdio::null());
    let mut out = vec![];
    let _ = r.read_to_end(&mut out);
    let _ = child.wait();
    out
}

/// [[ -s FILE ]]
fn non_empty(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.len() > 0)
}

/// [[ A -ot B ]]: A's modification time is before B's, or B exists and A does not.
fn older(a: &Path, b: &Path) -> bool {
    let (ma, mb) = (std::fs::metadata(a).and_then(|m| m.modified()), std::fs::metadata(b).and_then(|m| m.modified()));
    match (ma, mb) {
        (Ok(x), Ok(y)) => x < y,
        (Err(_), Ok(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commit_hyprland_reports() {
        assert_eq!(commit(br#"{"branch":"","commit":"0123456789abcdef","tag":"v0.57.1"}"#).as_deref(), Some("0123456789abcdef"));
        assert_eq!(commit(b" {\"commit\": \"abcdef1\\n\"}\n").as_deref(), Some("abcdef1"));
        assert_eq!(commit(br#"{"commit":"abcdef"}"#), None, "too short");
        assert_eq!(commit(br#"{"commit":"ABCDEF1"}"#), None, "uppercase");
        assert_eq!(commit(br#"{"commit":"abcdef1; rm -rf ~"}"#), None);
        assert_eq!(commit(br#"{"commit":1234567}"#), None);
        assert_eq!(commit(br#"["commit"]"#), None);
        assert_eq!(commit(b"Hyprland is not running"), None);
        assert_eq!(commit(b""), None);
    }
}
