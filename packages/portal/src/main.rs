//! yggdrasil-portal: the xdg-desktop-portal backend that makes Minos the open/save dialog of every application
//! (`org.freedesktop.impl.portal.FileChooser`). When a program asks for a file, this starts `minos --pick ...`, reads the
//! paths Minos prints and answers with file:// URIs. It decides nothing about access itself; the portal frontend and the
//! sandbox do that. Nothing is logged but errors.
//!
//! It also answers `org.freedesktop.impl.portal.Settings` (settings.rs): dark or light, and the accent colour, for every program.
//!
//! The dialog opens in the folder the same program last picked from (remembered per program in
//! `~/.local/state/yggdrasil/picker-folders`), unless the program asks for a folder of its own. "The same program" is the
//! app id the portal gives, else the window with the focus when the dialog is asked for (Hyprland; that is the program
//! asking), else the dialog's title.

mod settings;

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

type Options = HashMap<String, OwnedValue>;
type Filters = Vec<(String, Vec<(u32, String)>)>;
type Results = HashMap<String, OwnedValue>;

/// `file://` URI for a path (percent-encoding everything outside the unreserved set and `/`).
fn uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A byte-string option (`ay`, NUL-terminated) as text.
fn bytes_text(b: Vec<u8>) -> String {
    String::from_utf8_lossy(&b).trim_end_matches('\0').to_string()
}

fn opt_bool(o: &Options, k: &str) -> bool {
    o.get(k).and_then(|v| bool::try_from(v).ok()).unwrap_or(false)
}

fn opt_str(o: &Options, k: &str) -> Option<String> {
    o.get(k).and_then(|v| String::try_from(v.clone()).ok()).filter(|s| !s.is_empty())
}

fn opt_path(o: &Options, k: &str) -> Option<String> {
    o.get(k).and_then(|v| Vec::<u8>::try_from(v.clone()).ok()).map(bytes_text).filter(|s| !s.is_empty())
}

/// Globs for a MIME type, from the system's shared-mime-info table (`weight:type:glob` lines).
fn mime_globs(table: &str, mime: &str) -> Vec<String> {
    table
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, ':');
            let (_, m, g) = (p.next()?, p.next()?, p.next()?);
            (m == mime && g.starts_with("*.")).then(|| g.to_string())
        })
        .collect()
}

/// The `--filter` arguments for the filters an application sent.
fn filter_args(filters: &Filters, mime_table: &str) -> Vec<String> {
    let mut a = vec![];
    for (label, pats) in filters {
        let mut globs: Vec<String> = vec![];
        for (kind, pat) in pats {
            if *kind == 0 {
                globs.push(pat.clone());
            } else {
                globs.extend(mime_globs(mime_table, pat));
            }
        }
        a.push("--filter".to_string());
        a.push(format!("{label}|{}", globs.join(";")));
    }
    a
}

struct Chooser {
    /// Running Minos children by request handle, so Close can stop one.
    running: Arc<Mutex<HashMap<String, u32>>>,
}

struct Request {
    handle: String,
    running: Arc<Mutex<HashMap<String, u32>>>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Request")]
impl Request {
    fn close(&self) {
        if let Some(pid) = self.running.lock().unwrap().remove(&self.handle) {
            unsafe { libc_kill(pid as i32) };
        }
    }
}

extern "C" {
    #[link_name = "kill"]
    fn kill(pid: i32, sig: i32) -> i32;
}

unsafe fn libc_kill(pid: i32) {
    kill(pid, 15);
}

/// Where the last folder of each program is kept: `key<TAB>folder` lines.
fn folders_file() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state")))?;
    Some(base.join("yggdrasil/picker-folders"))
}

fn remembered(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(folders_file()?).ok()?;
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .find(|(k, _)| *k == key)
        .map(|(_, f)| f.to_string())
        .filter(|f| std::path::Path::new(f).is_dir())
}

fn remember(key: &str, folder: &str) {
    let Some(path) = folders_file() else { return };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    // newest first, kept to the last 200 programs
    let mut lines: Vec<String> = text.lines().filter(|l| l.split_once('\t').is_some_and(|(k, _)| k != key)).map(str::to_string).collect();
    lines.insert(0, format!("{key}\t{folder}"));
    lines.truncate(200);
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&path, lines.join("\n") + "\n");
}

/// Which program is asking: its app id, else the class of the window with the focus (the compositor's, through yggdrasil-wm),
/// else the dialog's title.
fn program_key(app_id: &str, title: &str) -> String {
    if !app_id.is_empty() {
        return format!("app:{app_id}");
    }
    let class = yggdrasil_wm::Desk::detect().and_then(|d| d.active_window(false)).map(|w| w.class).filter(|c| !c.is_empty());
    match class {
        Some(class) => format!("window:{class}"),
        None => format!("title:{title}"),
    }
}

/// Run Minos as a chooser; Some(paths) when the user chose, None when they cancelled.
fn pick(handle: &str, running: &Arc<Mutex<HashMap<String, u32>>>, args: Vec<String>) -> Option<Vec<String>> {
    let child = Command::new("minos").arg("--pick").args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yggdrasil-portal: could not start minos: {e}");
            return None;
        }
    };
    running.lock().unwrap().insert(handle.to_string(), child.id());
    let out = child.wait_with_output();
    running.lock().unwrap().remove(handle);
    let out = out.ok()?;
    if !out.status.success() {
        return None;
    }
    let paths: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
    if paths.is_empty() {
        None
    } else {
        Some(paths)
    }
}

fn reply(paths: Option<Vec<String>>, filters: &Filters) -> (u32, Results) {
    let mut r = Results::new();
    let Some(paths) = paths else { return (1, r) };
    let uris: Vec<String> = paths.iter().map(|p| uri(p)).collect();
    r.insert("uris".into(), Value::from(uris).try_into().unwrap());
    let _ = filters;
    (0, r)
}

impl Chooser {
    async fn run(
        &self,
        server: &zbus::object_server::ObjectServer,
        handle: OwnedObjectPath,
        app_id: &str,
        mode: &str,
        title: String,
        o: Options,
    ) -> (u32, Results) {
        let key = program_key(app_id, &title);
        let _ = server.at(handle.as_str(), Request { handle: handle.to_string(), running: self.running.clone() }).await;
        let table = std::fs::read_to_string("/usr/share/mime/globs2").unwrap_or_default();
        let filters: Filters = o.get("filters").and_then(|v| Filters::try_from(v.clone()).ok()).unwrap_or_default();
        let mut args = vec![mode.to_string(), "--title".into(), title.clone()];
        if opt_bool(&o, "multiple") {
            args.push("--multiple".into());
        }
        if let Some(l) = opt_str(&o, "accept_label") {
            args.push("--accept".into());
            args.push(l);
        }
        let mut folder = opt_path(&o, "current_folder");
        if let Some(f) = opt_path(&o, "current_file") {
            let p = std::path::Path::new(&f);
            if folder.is_none() {
                folder = p.parent().map(|d| d.display().to_string());
            }
            if let Some(n) = p.file_name() {
                args.push("--name".into());
                args.push(n.to_string_lossy().into_owned());
            }
        }
        if let Some(n) = opt_str(&o, "current_name") {
            args.push("--name".into());
            args.push(n);
        }
        // the program's own folder wins; else where it last picked from
        if let Some(f) = folder.or_else(|| remembered(&key)) {
            args.push("--folder".into());
            args.push(f);
        }
        args.extend(filter_args(&filters, &table));
        if let Some((label, _)) = o.get("current_filter").and_then(|v| <(String, Vec<(u32, String)>)>::try_from(v.clone()).ok()) {
            if let Some(i) = filters.iter().position(|f| f.0 == label) {
                args.push("--filter-index".into());
                args.push(i.to_string());
            }
        }
        let (h, run) = (handle.to_string(), self.running.clone());
        let paths = blocking::unblock(move || pick(&h, &run, args)).await;
        let _ = server.remove::<Request, _>(handle.as_str()).await;
        if let Some(first) = paths.as_ref().and_then(|p| p.first()) {
            let p = std::path::Path::new(first);
            let dir = if mode == "dir" || p.is_dir() { Some(p) } else { p.parent() };
            if let Some(d) = dir {
                remember(&key, &d.display().to_string());
            }
        }
        reply(paths, &filters)
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl Chooser {
    async fn open_file(
        &self,
        #[zbus(object_server)] server: &zbus::object_server::ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        _parent: String,
        title: String,
        options: Options,
    ) -> (u32, Results) {
        let mode = if opt_bool(&options, "directory") { "dir" } else { "open" };
        self.run(server, handle, &app_id, mode, title, options).await
    }

    async fn save_file(
        &self,
        #[zbus(object_server)] server: &zbus::object_server::ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        _parent: String,
        title: String,
        options: Options,
    ) -> (u32, Results) {
        self.run(server, handle, &app_id, "save", title, options).await
    }

    /// Save several files into one folder: pick the folder, answer with each file name inside it.
    async fn save_files(
        &self,
        #[zbus(object_server)] server: &zbus::object_server::ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        _parent: String,
        title: String,
        options: Options,
    ) -> (u32, Results) {
        let names: Vec<String> = options
            .get("files")
            .and_then(|v| Vec::<Vec<u8>>::try_from(v.clone()).ok())
            .unwrap_or_default()
            .into_iter()
            .map(bytes_text)
            .collect();
        let (code, mut r) = self.run(server, handle, &app_id, "dir", title, options).await;
        if code != 0 {
            return (code, r);
        }
        let dirs: Vec<String> = r.get("uris").and_then(|v| Vec::<String>::try_from(v.clone()).ok()).unwrap_or_default();
        if let Some(d) = dirs.first() {
            let uris: Vec<String> =
                names.iter().map(|n| format!("{}/{}", d.trim_end_matches('/'), uri(n).trim_start_matches("file://"))).collect();
            r.insert("uris".into(), Value::from(uris).try_into().unwrap());
        }
        (0, r)
    }
}

fn main() -> zbus::Result<()> {
    let running = Arc::new(Mutex::new(HashMap::new()));
    let conn = zbus::blocking::connection::Builder::session()?
        .name("org.freedesktop.impl.portal.desktop.yggdrasil")?
        .serve_at("/org/freedesktop/portal/desktop", Chooser { running })?
        .serve_at("/org/freedesktop/portal/desktop", settings::Settings)?
        .build()?;
    settings::watch(conn.clone(), "/org/freedesktop/portal/desktop");
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_are_encoded() {
        assert_eq!(uri("/home/a b/ü.txt"), "file:///home/a%20b/%C3%BC.txt");
    }

    #[test]
    fn mime_types_become_globs() {
        let t = "50:image/png:*.png\n50:image/jpeg:*.jpg\n10:image/jpeg:*.jpeg\n50:text/x-readme:README\n";
        assert_eq!(mime_globs(t, "image/jpeg"), vec!["*.jpg", "*.jpeg"]);
        assert!(mime_globs(t, "text/x-readme").is_empty());
    }

    #[test]
    fn filters_turn_into_arguments() {
        let f: Filters = vec![("Images".into(), vec![(1, "image/png".into())]), ("Text".into(), vec![(0, "*.txt".into())])];
        assert_eq!(filter_args(&f, "50:image/png:*.png\n"), vec!["--filter", "Images|*.png", "--filter", "Text|*.txt"]);
    }

    #[test]
    fn byte_strings_drop_the_nul() {
        assert_eq!(bytes_text(b"/tmp\0".to_vec()), "/tmp");
    }
}
