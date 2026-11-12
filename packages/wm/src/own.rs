//! Yggdrasil's own compositor (packages/compositor), through its control socket ($XDG_RUNTIME_DIR/yggdrasil-compositor-
//! $WAYLAND_DISPLAY.ctl): one command per connection, answered with lines and then "ok" or "error: ...". The commands are its
//! desktop commands (src/wmctl.rs there); windows are named by the number `clients` gives them.

use std::path::PathBuf;
use std::time::Duration;

use crate::{Desktops, MonitorRef, Rect, Screen, Window};

const TIMEOUT: Duration = Duration::from_secs(2);
/// The answers are a line per window or output: far below this.
const MAX_REPLY: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Own {
    pub ctl: PathBuf,
}

/// A window number for a command: digits, or "active".
fn id(w: &str) -> Option<&str> {
    (w == "active" || (!w.is_empty() && w.len() <= 20 && w.chars().all(|c| c.is_ascii_digit()))).then_some(w)
}

impl Own {
    /// One command; its lines, or what went wrong.
    pub fn ask(&self, command: &str) -> Result<Vec<String>, String> {
        let answer = crate::sock::request_limited(&self.ctl, format!("{command}\n").as_bytes(), TIMEOUT, MAX_REPLY)
            .ok_or_else(|| format!("{}: no answer", self.ctl.display()))?;
        let mut lines: Vec<String> = answer.lines().map(str::to_string).collect();
        match lines.pop().as_deref() {
            Some("ok") => Ok(lines),
            Some(e) => Err(e.trim_start_matches("error: ").to_string()),
            None => Err("an empty answer".into()),
        }
    }

    fn act(&self, command: &str) -> bool {
        self.ask(command).is_ok()
    }

    pub fn windows(&self) -> Vec<Window> {
        self.ask("clients").map(|l| l.iter().filter_map(|l| parse_client(l)).collect()).unwrap_or_default()
    }

    pub fn active_window(&self) -> Option<Window> {
        self.ask("active").ok()?.first().and_then(|l| parse_client(l))
    }

    pub fn screens(&self) -> Vec<Screen> {
        let current = self.ask("workspaces").ok().and_then(|l| parse_workspaces(&l).current());
        self.ask("screens").map(|l| l.iter().enumerate().filter_map(|(i, l)| parse_screen(i, l, current)).collect()).unwrap_or_default()
    }

    pub fn cursor(&self) -> Option<(f64, f64)> {
        let l = self.ask("cursor").ok()?;
        let (x, y) = l.first()?.split_once(' ')?;
        Some((x.parse().ok()?, y.parse().ok()?))
    }

    pub fn desktops(&self) -> Desktops {
        self.ask("workspaces").map(|l| parse_workspaces(&l)).unwrap_or_default()
    }

    pub fn show_desktop(&self, n: &str) {
        if n.parse::<u32>().is_ok() {
            self.act(&format!("workspace {n}"));
        }
    }

    pub fn send_to_desktop(&self, window: &str, to: &str) {
        if let (Some(w), Ok(to)) = (id(window), to.parse::<u32>()) {
            self.act(&format!("send {w} {to}"));
        }
    }

    pub fn place(&self, window: &str, (x, y, w, h): Rect) -> Vec<(String, Option<String>)> {
        let Some(win) = id(window) else { return vec![] };
        let c = format!("place {win} {x} {y} {w} {h}");
        let r = self.ask(&c);
        vec![(c, Some(r.map(|_| "ok".to_string()).unwrap_or_else(|e| e)))]
    }

    pub fn super_held(&self) -> Option<bool> {
        Some(self.ask("modifiers").ok()?.first()?.split(' ').any(|m| m == "super"))
    }

    pub fn pointer_timeout(&self) -> Option<f32> {
        self.ask("pointer-timeout").ok()?.first()?.trim().parse().ok()
    }

    pub fn set_pointer_timeout(&self, secs: f32) {
        self.act(&format!("pointer-timeout {secs}"));
    }

    pub fn maximize_active(&self) -> bool {
        self.act("maximize active toggle")
    }

    pub fn minimize_active(&self) -> bool {
        self.act("minimize active")
    }

    pub fn close_active(&self) -> bool {
        self.act("close active")
    }

    pub fn snap_active(&self, (x, y, w, h): Rect) -> bool {
        self.act(&format!("place active {x} {y} {w} {h}"))
    }
}

fn pair<T: std::str::FromStr>(s: &str, sep: char) -> Option<(T, T)> {
    let (a, b) = s.split_once(sep)?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// A `clients` line: id, app id, title, place, size, workspace, states, output.
pub fn parse_client(line: &str) -> Option<Window> {
    let p: Vec<&str> = line.split('\t').collect();
    let [id, app, title, at, size, ws, states, output] = p.as_slice() else { return None };
    id.parse::<u64>().ok()?;
    let states: Vec<&str> = states.split(',').collect();
    let minimized = states.contains(&"minimized");
    Some(Window {
        id: id.to_string(),
        class: app.to_string(),
        initial_class: app.to_string(),
        title: title.to_string(),
        workspace: ws.parse().ok(),
        minimized,
        special: minimized,
        at: pair::<f64>(at, ','),
        size: pair::<f64>(size, 'x'),
        floating: Some(!states.contains(&"tiled")),
        fullscreen: states.contains(&"fullscreen"),
        mapped: true,
        hidden: false,
        monitor: Some(MonitorRef::Name(output.to_string())),
        focus_order: i64::MAX,
        pid: None,
    })
}

/// `workspaces`: number, windows, "shown" or "-". The desktops are those holding a window and the one shown, as on Hyprland.
pub fn parse_workspaces(lines: &[String]) -> Desktops {
    let mut ids = vec![];
    let mut current = None;
    for l in lines {
        let p: Vec<&str> = l.split('\t').collect();
        let [n, count, shown] = p.as_slice() else { continue };
        let (Ok(n), Ok(count)) = (n.parse::<i64>(), count.parse::<u32>()) else { continue };
        if *shown == "shown" {
            current = Some(n);
        }
        if count > 0 || *shown == "shown" {
            ids.push(n);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    let at = current.and_then(|c| ids.iter().position(|&id| id == c)).unwrap_or(0);
    Desktops { ids, at }
}

/// A `screens` line: name, place, mode size, scale, reserved strips, focused. `index` is its number (the output's order).
pub fn parse_screen(index: usize, line: &str, workspace: Option<i64>) -> Option<Screen> {
    let p: Vec<&str> = line.split('\t').collect();
    let [name, at, size, scale, reserved, focused] = p.as_slice() else { return None };
    let (x, y) = pair::<i32>(at, ',')?;
    let (width, height) = pair::<u32>(size, 'x')?;
    let r: Vec<i32> = reserved.split(',').filter_map(|v| v.parse().ok()).collect();
    let [l, t, rr, b] = r.as_slice() else { return None };
    Some(Screen {
        id: Some(index as i64),
        name: name.to_string(),
        x,
        y,
        width,
        height,
        scale: scale.parse().ok(),
        transform: 0,
        reserved: [*l, *t, *rr, *b],
        focused: *focused == "focused",
        disabled: false,
        active_workspace: workspace,
        special_workspace: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_sockets_lines_are_read() {
        let w = parse_client("7\torg.example.Editor\tnotes - Editor\t40,60\t800x600\t2\tactive,floating\tHEADLESS-1").unwrap();
        assert_eq!((w.id.as_str(), w.class.as_str(), w.title.as_str()), ("7", "org.example.Editor", "notes - Editor"));
        assert_eq!((w.at, w.size, w.workspace, w.floating), (Some((40.0, 60.0)), Some((800.0, 600.0)), Some(2), Some(true)));
        assert!(!w.minimized && !w.fullscreen);
        assert_eq!(w.monitor, Some(MonitorRef::Name("HEADLESS-1".into())));
        let m = parse_client("8\ta\tb\t0,0\t10x10\t1\ttiled,minimized\tHEADLESS-1").unwrap();
        assert!(m.minimized && m.special && m.floating == Some(false));
        assert_eq!(parse_client("x\ta\tb\t0,0\t10x10\t1\t\tH"), None, "the id is a number");
        assert_eq!(parse_client("1\ta\tb"), None);

        let s = parse_screen(1, "HEADLESS-2\t1280,0\t1600x900\t1.5\t0,32,0,64\tfocused", Some(3)).unwrap();
        assert_eq!((s.x, s.y, s.width, s.height, s.scale, s.reserved), (1280, 0, 1600, 900, Some(1.5), [0, 32, 0, 64]));
        assert!(s.focused && s.id == Some(1) && s.active_workspace == Some(3));
        assert_eq!(s.monitor().free_area(), (1280, 32, 1067, 504));

        let lines: Vec<String> = ["1\t2\t-", "2\t0\tshown", "3\t1\t-", "4\t0\t-"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse_workspaces(&lines), Desktops { ids: vec![1, 2, 3], at: 1 }, "an empty one exists while it is shown");
        assert_eq!(id("12"), Some("12"));
        assert_eq!(id("1; quit"), None);
    }
}
