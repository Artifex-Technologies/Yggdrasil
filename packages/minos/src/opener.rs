//! Opening a file with its default app. Where the operating system has an app launcher with per-app rules (the `apps`
//! integration: Labyrinth OS's labyrinth-launch), the default app is started through it, so the app's rules apply to a file
//! opened here as they do from the dock: its network setting (viewer apps start on "ask", the privacy pack) and its sandbox.
//! Elsewhere, or when the default app cannot be told or the launcher does not start it, xdg-open as before.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

/// A desktop id the launcher accepts: 1 to 120 of A-Z a-z 0-9 . _ + -, not starting with a dot ("org.gnome.Evince").
pub fn valid_id(id: &str) -> bool {
    let n = id.chars().count();
    (1..=120).contains(&n) && id.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c)) && !id.starts_with('.')
}

/// The desktop id in `xdg-mime query default`'s answer ("org.gnome.Evince.desktop\n" -> "org.gnome.Evince").
pub fn id_of(answer: &str) -> Option<String> {
    let first = answer.lines().next()?.trim();
    let id = first.strip_suffix(".desktop").unwrap_or(first);
    valid_id(id).then(|| id.to_string())
}

fn output(cmd: &mut Command) -> Option<String> {
    let o = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// The default app of a file, from its type (xdg-mime, argv only).
pub fn default_app(path: &Path) -> Option<String> {
    let mime = output(Command::new("xdg-mime").args(["query", "filetype"]).arg(path))?;
    let mime = mime.trim();
    if mime.is_empty() || !mime.contains('/') || mime.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    id_of(&output(Command::new("xdg-mime").args(["query", "default", mime]))?)
}

/// The launcher's command for an app and a file.
pub fn launch_argv(launch: &[String], id: &str, path: &Path) -> Vec<OsString> {
    let mut v: Vec<OsString> = launch.iter().map(OsString::from).collect();
    v.push(id.into());
    v.push(path.as_os_str().to_os_string());
    v
}

/// Opens a file (on a thread: asking for the default app takes a moment).
pub fn open(path: &Path) {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let launch = yggdrasil_conf::integrations::first("apps").and_then(|i| i.command("launch"));
        if let (Some(launch), Some(id)) = (launch, default_app(&path)) {
            let argv = launch_argv(&launch, &id, &path);
            let ok = Command::new(&argv[0]).args(&argv[1..]).stdin(Stdio::null()).status().is_ok_and(|s| s.success());
            if ok {
                return;
            }
        }
        if let Ok(mut c) = Command::new("xdg-open").arg(&path).spawn() {
            let _ = c.wait();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_ids_are_read_and_checked() {
        assert_eq!(id_of("org.gnome.Evince.desktop\n"), Some("org.gnome.Evince".into()));
        assert_eq!(id_of("vlc.desktop"), Some("vlc".into()));
        assert_eq!(id_of("libreoffice-writer.desktop\nother.desktop\n"), Some("libreoffice-writer".into()));
        assert_eq!(id_of(""), None);
        assert_eq!(id_of("../evil.desktop"), None);
        assert_eq!(id_of("a b.desktop"), None);
        assert_eq!(id_of(".hidden.desktop"), None);
        let argv = launch_argv(&["/usr/bin/labyrinth-launch".into()], "vlc", Path::new("/m/-x film.mkv"));
        assert_eq!(argv, ["/usr/bin/labyrinth-launch", "vlc", "/m/-x film.mkv"].map(OsString::from));
    }
}
