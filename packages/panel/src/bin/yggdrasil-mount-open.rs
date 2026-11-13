//! yggdrasil-mount-open DEVICE: mounts a drive through udisks2 (polkit decides whether that is allowed) and opens it in Minos.
//! The panel runs it when you click the "Drive connected" notification.
//!
//! As the shell script of the same name: the device name checked (/dev/ and only letters, digits, `/`, `_`, `.`, `-`), where it
//! is mounted already from `findmnt -n -o TARGET --source DEVICE`, else `udisksctl mount -b DEVICE` and the folder its answer
//! names; then Minos in a session of its own (`setsid -f minos FOLDER`). Fixed: the folder is what follows "Mounted DEVICE at "
//! (the script's sed took what followed the last " at ", so a drive whose name has " at " in it opened the wrong folder).

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

fn main() {
    let dev = std::env::args().nth(1).unwrap_or_default();
    if !dev.starts_with("/dev/") {
        eprintln!("usage: yggdrasil-mount-open /dev/DEVICE");
        std::process::exit(2);
    }
    if !dev.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_.-".contains(&b)) {
        eprintln!("bad device name");
        std::process::exit(2);
    }
    let mut at = Command::new("findmnt")
        .args(["-n", "-o", "TARGET", "--source", &dev])
        .stderr(Stdio::null())
        .output()
        .map(|o| first_line(&o.stdout))
        .unwrap_or_default();
    if at.is_empty() {
        // $(udisksctl mount -b DEVICE 2>&1): its output and errors through one pipe
        let (ok, out) = both(Command::new("udisksctl").args(["mount", "-b", &dev]));
        let out = String::from_utf8_lossy(trim_newlines(&out)).into_owned();
        if !ok {
            eprintln!("{out}");
            std::process::exit(1);
        }
        at = mounted_at(&out, &dev).unwrap_or_default();
    }
    if at.is_empty() {
        std::process::exit(1);
    }
    let e = Command::new("setsid").args(["-f", "minos", &at]).exec();
    if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!("yggdrasil-mount-open: setsid: not found");
        std::process::exit(127)
    }
    eprintln!("yggdrasil-mount-open: setsid: {e}");
    std::process::exit(126)
}

/// `| head -n1` under $(...): the first line, without its newline.
fn first_line(b: &[u8]) -> String {
    String::from_utf8_lossy(b.split(|&c| c == b'\n').next().unwrap_or_default()).into_owned()
}

fn trim_newlines(mut b: &[u8]) -> &[u8] {
    while let [rest @ .., b'\n'] = b {
        b = rest;
    }
    b
}

/// The folder in udisksctl's "Mounted DEVICE at FOLDER." (each such line; more than one are joined by newlines, as the
/// script's $(sed ...) joined them).
fn mounted_at(out: &str, dev: &str) -> Option<String> {
    let head = format!("Mounted {dev} at ");
    let v: Vec<&str> = out.split('\n').filter_map(|l| l.strip_prefix(&head)?.strip_suffix('.')).filter(|f| !f.is_empty()).collect();
    (!v.is_empty()).then(|| v.join("\n"))
}

/// Runs a command with its output and errors in one pipe: (succeeded, what it wrote).
fn both(c: &mut Command) -> (bool, Vec<u8>) {
    use std::io::Read;
    let Ok((mut r, w)) = std::io::pipe() else { return (false, vec![]) };
    let Ok(w2) = w.try_clone() else { return (false, vec![]) };
    let mut child = match c.stdout(w).stderr(w2).spawn() {
        Ok(ch) => ch,
        Err(e) => {
            let prog = c.get_program().to_string_lossy().into_owned();
            let why = if e.kind() == std::io::ErrorKind::NotFound { "not found".to_string() } else { e.to_string() };
            return (false, format!("yggdrasil-mount-open: {prog}: {why}\n").into_bytes());
        }
    };
    c.stdout(Stdio::null()).stderr(Stdio::null());
    let mut out = vec![];
    let _ = r.read_to_end(&mut out);
    (child.wait().is_ok_and(|s| s.success()), out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_udisksctl_names() {
        assert_eq!(mounted_at("Mounted /dev/sdb1 at /run/media/me/USB.", "/dev/sdb1").as_deref(), Some("/run/media/me/USB"));
        // the script took "home" here (what followed the last " at ")
        assert_eq!(
            mounted_at("Mounted /dev/sdb1 at /run/media/me/Photos at home.", "/dev/sdb1").as_deref(),
            Some("/run/media/me/Photos at home")
        );
        assert_eq!(mounted_at("Mounted /dev/sdb2 at /x.", "/dev/sdb1"), None);
        assert_eq!(mounted_at("Error mounting /dev/sdb1: busy", "/dev/sdb1"), None);
        assert_eq!(first_line(b"/run/media/a\n/b\n"), "/run/media/a");
    }
}
