//! `org.freedesktop.impl.portal.Screenshot`: screenshots and picked colours for programs that ask through the portal, on
//! Yggdrasil's own compositor (the session on Hyprland uses Hyprland's portal for these). A screenshot is the whole desktop
//! (grim) or, when the program asks for it to be interactive, what the person chooses in yggdrasil-shot; a colour is what the
//! person picks in yggdrasil-colorpicker. Neither touches the person's clipboard, Pictures folder, colour history or
//! notifications (both tools' `--portal` mode): the program gets the picture as a file of the portal's own, in the runtime
//! folder (the last ten kept; gone at log-out), or the colour.
//!
//! Version 1: a sandboxed program's screenshot that is not interactive is allowed by the person first, through the portal
//! frontend's own question (its Access dialog), as the frontend does for a version 1 backend.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use zbus::object_server::ObjectServer;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::{opt_bool, uri, Options, Request, Results};

/// The screenshots kept for programs (the newest).
const KEPT: usize = 10;

/// Where the screenshots for programs go: the runtime folder's `yggdrasil-portal/screenshots`, readable by this user only.
fn shots_dir() -> Option<PathBuf> {
    let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute())?;
    let dir = run.join("yggdrasil-portal/screenshots");
    std::fs::create_dir_all(&dir).ok()?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(run.join("yggdrasil-portal"), std::fs::Permissions::from_mode(0o700)).ok()?;
    Some(dir)
}

/// Makes room for one more: the oldest screenshots go until `KEPT - 1` are left. Names sort by age (`screenshot-MILLIS-N`).
fn prune(dir: &Path) {
    let mut shots: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png")).collect())
        .unwrap_or_default();
    shots.sort_by_key(|p| shot_order(p));
    let extra = shots.len().saturating_sub(KEPT - 1);
    for p in &shots[..extra] {
        let _ = std::fs::remove_file(p);
    }
}

/// A screenshot's place in time, from its name.
fn shot_order(p: &Path) -> (u128, u64) {
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let mut parts = stem.strip_prefix("screenshot-").unwrap_or_default().split('-');
    let millis = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    let n = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    (millis, n)
}

/// Runs one of the tools for a request (Request.Close stops it); its output when it ran.
fn run_tool(handle: &str, running: &Arc<Mutex<HashMap<String, u32>>>, mut cmd: Command) -> Option<Output> {
    let child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yggdrasil-portal: could not start {:?}: {e}", cmd.get_program());
            return None;
        }
    };
    running.lock().unwrap().insert(handle.to_string(), child.id());
    let out = child.wait_with_output();
    running.lock().unwrap().remove(handle);
    out.ok()
}

/// The colour picker's answer: red, green and blue, each from 0 to 1.
fn parse_color(text: &str) -> Option<(f64, f64, f64)> {
    let v: Vec<f64> = text.split_whitespace().map(|n| n.parse::<f64>()).collect::<Result<_, _>>().ok()?;
    match v[..] {
        [r, g, b] if [r, g, b].iter().all(|c| (0.0..=1.0).contains(c)) => Some((r, g, b)),
        _ => None,
    }
}

pub struct Screenshot {
    /// the tools running, by request handle (shared with the other interfaces: Request.Close stops one)
    pub running: Arc<Mutex<HashMap<String, u32>>>,
    pub count: AtomicU64,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl Screenshot {
    async fn screenshot(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        handle: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        options: Options,
    ) -> (u32, Results) {
        let interactive = opt_bool(&options, "interactive");
        let Some(dir) = shots_dir() else { return (2, Results::new()) };
        prune(&dir);
        let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let path = dir.join(format!("screenshot-{millis}-{}.png", self.count.fetch_add(1, Ordering::Relaxed)));
        let _ = server.at(handle.as_str(), Request { handle: handle.to_string(), running: self.running.clone() }).await;
        let (h, run, p) = (handle.to_string(), self.running.clone(), path.clone());
        let ok = blocking::unblock(move || {
            let mut cmd = if interactive { Command::new("yggdrasil-shot") } else { Command::new("grim") };
            if interactive {
                cmd.arg("--portal");
            }
            cmd.arg(&p);
            run_tool(&h, &run, cmd).is_some_and(|o| o.status.success()) && p.is_file()
        })
        .await;
        let _ = server.remove::<Request, _>(handle.as_str()).await;
        if !ok {
            let _ = std::fs::remove_file(&path);
            // interactive: the person chose nothing; else the screen could not be taken
            return (if interactive { 1 } else { 2 }, Results::new());
        }
        let mut r = Results::new();
        if let Ok(v) = OwnedValue::try_from(Value::from(uri(&path.display().to_string()))) {
            r.insert("uri".into(), v);
        }
        (0, r)
    }

    async fn pick_color(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        handle: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        _options: Options,
    ) -> (u32, Results) {
        let _ = server.at(handle.as_str(), Request { handle: handle.to_string(), running: self.running.clone() }).await;
        let (h, run) = (handle.to_string(), self.running.clone());
        let out = blocking::unblock(move || {
            let mut cmd = Command::new("yggdrasil-colorpicker");
            cmd.arg("--portal");
            run_tool(&h, &run, cmd)
        })
        .await;
        let _ = server.remove::<Request, _>(handle.as_str()).await;
        let colour = out.filter(|o| o.status.success()).and_then(|o| parse_color(&String::from_utf8_lossy(&o.stdout)));
        let Some(c) = colour else { return (1, Results::new()) };
        let mut r = Results::new();
        if let Ok(v) = OwnedValue::try_from(Value::from(c)) {
            r.insert("color".into(), v);
        }
        (0, r)
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_three_fractions() {
        assert_eq!(parse_color("1 0 0.2\n"), Some((1.0, 0.0, 0.2)));
        assert_eq!(parse_color("1 0"), None);
        assert_eq!(parse_color("1 0 2"), None);
        assert_eq!(parse_color("#ff0033"), None);
        assert_eq!(parse_color(""), None);
    }

    #[test]
    fn only_the_newest_screenshots_are_kept() {
        let dir = std::env::temp_dir().join(format!("portal-prune-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // made in this order; the counter breaks a tie within one millisecond, and 10 sorts after 9
        for (ms, n) in [(5, 0), (7, 1), (7, 2), (9, 3), (10, 4), (11, 5), (12, 6), (13, 7), (14, 8), (15, 9), (15, 10)] {
            std::fs::write(dir.join(format!("screenshot-{ms}-{n}.png")), b"").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), b"").unwrap();
        prune(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left.len(), KEPT - 1 + 1, "nine screenshots and the other file");
        assert!(!left.contains(&"screenshot-5-0.png".to_string()) && !left.contains(&"screenshot-7-1.png".to_string()));
        assert!(left.contains(&"screenshot-15-10.png".to_string()) && left.contains(&"notes.txt".to_string()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
