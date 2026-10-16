//! Notifications: the system's notification service (`org.freedesktop.Notifications`), the Heimdall alert
//! feed, and the list both of Labyrinth's places for them draw (Fylgja's Notifications section and the
//! right-hand pane).
//!
//! What it does:
//!   * Apps send notifications over D-Bus; they are kept (up to `MAX_KEPT`, in
//!     ~/.local/state/labyrinth/notifications.json) until the user opens or clears them.
//!   * Heimdall's alerts (warning and critical by default) come in as notifications from "Heimdall".
//!   * They are shown grouped by app, the groups ordered by their newest notification; a group with several
//!     opens to show them all. Opening one goes to the app (and asks the app to show what it is about).
//!   * A pop-up (a "toast") shows a new one at the bottom right for a few seconds.
//!
//! The pure parts (the store, grouping, the text clean-up, times) are tested below; the D-Bus and Wayland
//! parts are checked on the running desktop.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use resvg::tiny_skia::Pixmap;

use crate::paint::{self, Fonts, Icons, Rgba};

pub const MAX_KEPT: usize = 200;
const MAX_TEXT: usize = 400;
pub const HEIMDALL_APP: &str = "Heimdall";

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notif {
    pub id: u32,
    /// The sending program's name as it sent it.
    pub app: String,
    /// Its desktop file name (without .desktop), when it said (hint `desktop-entry`).
    pub entry: String,
    /// An icon name or path.
    pub icon: String,
    pub summary: String,
    pub body: String,
    /// 0 low, 1 normal, 2 critical.
    pub urgency: u8,
    pub time: u64,
    /// (key, label) pairs the app offered; the key "default" is what a click on the notification means.
    pub actions: Vec<(String, String)>,
    /// What to open for a notification that is not an app's (Heimdall's): a `labyrinth-settings` page.
    pub link: String,
}

impl Notif {
    /// What groups notifications: the desktop entry when known, else the app name.
    pub fn group_key(&self) -> String {
        if !self.entry.is_empty() {
            self.entry.to_lowercase()
        } else {
            self.app.to_lowercase()
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"id": self.id, "app": self.app, "entry": self.entry, "icon": self.icon, "summary": self.summary,
            "body": self.body, "urgency": self.urgency, "time": self.time, "link": self.link,
            "actions": self.actions.iter().map(|(k, l)| serde_json::json!([k, l])).collect::<Vec<_>>()})
    }

    fn from_json(v: &serde_json::Value) -> Option<Notif> {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some(Notif {
            id: v.get("id")?.as_u64()? as u32,
            app: s("app"),
            entry: s("entry"),
            icon: s("icon"),
            summary: s("summary"),
            body: s("body"),
            urgency: v.get("urgency").and_then(|x| x.as_u64()).unwrap_or(1).min(2) as u8,
            time: v.get("time").and_then(|x| x.as_u64()).unwrap_or(0),
            link: s("link"),
            actions: v
                .get("actions")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|p| Some((p.get(0)?.as_str()?.to_string(), p.get(1)?.as_str()?.to_string()))).collect())
                .unwrap_or_default(),
        })
    }
}

/// One app's notifications, newest first.
#[derive(Clone, Debug)]
pub struct Group {
    pub key: String,
    pub name: String,
    pub icon: String,
    pub items: Vec<Notif>,
}

#[derive(Default)]
pub struct Store {
    items: Vec<Notif>,
    /// Groups the user opened out.
    pub expanded: HashSet<String>,
    /// How many rows the list is scrolled down.
    pub scroll: usize,
    path: Option<PathBuf>,
}

pub fn state_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("labyrinth").join("notifications.json"))
}

impl Store {
    pub fn load(path: Option<PathBuf>) -> Store {
        let mut st = Store { path, ..Store::default() };
        if let Some(p) = &st.path {
            if let Ok(t) = std::fs::read_to_string(p) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    st.items = v.as_array().map(|a| a.iter().filter_map(Notif::from_json).collect()).unwrap_or_default();
                }
            }
        }
        st
    }

    pub fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = serde_json::Value::Array(self.items.iter().map(Notif::to_json).collect()).to_string();
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
    }

    pub fn max_id(&self) -> u32 {
        self.items.iter().map(|n| n.id).max().unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, id: u32) -> Option<&Notif> {
        self.items.iter().find(|n| n.id == id)
    }

    /// Adds a notification; one with the same id (an app updating its own) is replaced in place.
    pub fn add(&mut self, n: Notif) {
        if let Some(old) = self.items.iter_mut().find(|o| o.id == n.id) {
            *old = n;
        } else {
            self.items.push(n);
            if self.items.len() > MAX_KEPT {
                let drop = self.items.len() - MAX_KEPT;
                self.items.drain(..drop); // the oldest
            }
        }
        self.save();
    }

    pub fn remove(&mut self, id: u32) -> Option<Notif> {
        let i = self.items.iter().position(|n| n.id == id)?;
        let n = self.items.remove(i);
        self.save();
        Some(n)
    }

    pub fn clear_group(&mut self, key: &str) -> Vec<u32> {
        let ids: Vec<u32> = self.items.iter().filter(|n| n.group_key() == key).map(|n| n.id).collect();
        self.items.retain(|n| n.group_key() != key);
        self.expanded.remove(key);
        self.save();
        ids
    }

    pub fn clear_all(&mut self) -> Vec<u32> {
        let ids = self.items.iter().map(|n| n.id).collect();
        self.items.clear();
        self.expanded.clear();
        self.scroll = 0;
        self.save();
        ids
    }

    /// The groups, the one with the newest notification first; inside a group the newest first too.
    pub fn groups(&self) -> Vec<Group> {
        let mut map: HashMap<String, Group> = HashMap::new();
        for n in &self.items {
            let g = map.entry(n.group_key()).or_insert_with(|| Group { key: n.group_key(), name: n.app.clone(), icon: String::new(), items: vec![] });
            if g.icon.is_empty() && !n.icon.is_empty() {
                g.icon = n.icon.clone();
            }
            if n.app.len() > g.name.len() && g.name.is_empty() {
                g.name = n.app.clone();
            }
            g.items.push(n.clone());
        }
        let mut out: Vec<Group> = map.into_values().collect();
        for g in &mut out {
            g.items.sort_by(|a, b| b.time.cmp(&a.time).then(b.id.cmp(&a.id)));
            if g.name.is_empty() {
                g.name = if g.key.is_empty() { "Other".into() } else { g.key.clone() };
            }
        }
        out.sort_by(|a, b| b.items[0].time.cmp(&a.items[0].time).then(b.items[0].id.cmp(&a.items[0].id)));
        out
    }

    pub fn toggle_group(&mut self, key: &str) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_string());
        }
    }
}

// ------------------------------------------------------------------------------------------------ text

/// A notification's text without markup: the tags apps use (<b>, <i>, <a href>, <img>) taken out, the
/// entities put back, and cut to a sane length.
pub fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            '\r' => {}
            c if c.is_control() && c != '\n' => {}
            c => out.push(c),
        }
    }
    let out = out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&amp;", "&");
    let out = out.trim().to_string();
    if out.chars().count() > MAX_TEXT {
        let cut: String = out.chars().take(MAX_TEXT).collect();
        format!("{cut}…")
    } else {
        out
    }
}

/// "now", "5m", "3h", "2d": how long ago, short.
pub fn ago(then: u64, now: u64) -> String {
    let d = now.saturating_sub(then);
    match d {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", d / 60),
        3600..=86399 => format!("{}h", d / 3600),
        _ => format!("{}d", d / 86400),
    }
}

// ------------------------------------------------------------------------------------------------ messages

/// What the D-Bus thread and the Heimdall thread tell the panel.
#[derive(Debug)]
pub enum NMsg {
    New(Notif),
    /// An app asked to take this one away.
    Close(u32),
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
static CONN: OnceLock<zbus::blocking::Connection> = OnceLock::new();

pub fn set_next_id(after: u32) {
    NEXT_ID.store(after + 1, Ordering::Relaxed);
}

pub fn next_id() -> u32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Tells the sending app a notification closed (reason 2: dismissed by the user, 3: closed by a call).
pub fn emit_closed(id: u32, reason: u32) {
    if let Some(c) = CONN.get() {
        let _ = c.emit_signal(None::<&str>, "/org/freedesktop/Notifications", "org.freedesktop.Notifications", "NotificationClosed", &(id, reason));
    }
}

/// Tells the sending app one of its actions was chosen.
pub fn emit_action(id: u32, key: &str) {
    if let Some(c) = CONN.get() {
        let _ = c.emit_signal(None::<&str>, "/org/freedesktop/Notifications", "org.freedesktop.Notifications", "ActionInvoked", &(id, key));
    }
}

struct Server {
    tx: calloop::channel::Sender<NMsg>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<String> {
        ["body", "actions", "icon-static", "persistence"].iter().map(|s| s.to_string()).collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(&self, app_name: String, replaces_id: u32, app_icon: String, summary: String, body: String, actions: Vec<String>, hints: HashMap<String, zbus::zvariant::OwnedValue>, _expire_timeout: i32) -> u32 {
        let id = if replaces_id != 0 { replaces_id } else { next_id() };
        let urgency = hints.get("urgency").and_then(|v| u8::try_from(v).ok()).unwrap_or(1).min(2);
        let entry = hints.get("desktop-entry").and_then(|v| <&str>::try_from(v).ok()).unwrap_or("").trim_end_matches(".desktop").to_string();
        let icon = if app_icon.is_empty() { hints.get("image-path").and_then(|v| <&str>::try_from(v).ok()).unwrap_or("").to_string() } else { app_icon };
        let acts: Vec<(String, String)> = actions.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].clone(), clean(&c[1]))).collect();
        let _ = self.tx.send(NMsg::New(Notif {
            id,
            app: clean(&app_name),
            entry,
            icon,
            summary: clean(&summary),
            body: clean(&body),
            urgency,
            time: now_secs(),
            actions: acts,
            link: String::new(),
        }));
        id
    }

    fn close_notification(&self, id: u32) {
        let _ = self.tx.send(NMsg::Close(id));
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        ("Labyrinth".into(), "Labyrinth".into(), env!("CARGO_PKG_VERSION").into(), "1.2".into())
    }
}

/// Serves the notification interface. If another program (mako, Plasma) holds the name it tries again every
/// ten seconds, so stopping that program is all it takes to switch over.
pub fn spawn_server(tx: calloop::channel::Sender<NMsg>) {
    std::thread::spawn(move || {
        let mut told = false;
        loop {
            let built = zbus::blocking::connection::Builder::session()
                .and_then(|b| b.name("org.freedesktop.Notifications")).map(|b| b.replace_existing_names(true))
                .and_then(|b| b.serve_at("/org/freedesktop/Notifications", Server { tx: tx.clone() }))
                .and_then(|b| b.build());
            match built {
                Ok(conn) => {
                    let _ = CONN.set(conn.clone());
                    eprintln!("labyrinth-panel: serving notifications");
                    // the connection serves on its own threads; keep this one (and the connection) alive
                    loop {
                        std::thread::sleep(Duration::from_secs(3600));
                    }
                }
                Err(e) => {
                    if !told {
                        eprintln!("labyrinth-panel: notifications not served (another program holds the name?): {e}");
                        told = true;
                    }
                    std::thread::sleep(Duration::from_secs(10));
                }
            }
        }
    });
}

// ------------------------------------------------------------------------------------------------ Heimdall

fn severity_rank(s: &str) -> u8 {
    match s {
        "critical" => 2,
        "warning" => 1,
        _ => 0,
    }
}

/// The notification for one Heimdall alert (a JSON object from /v1/alerts), if it is at least `min`
/// ("info", "warning", "critical"). The summary can quote what a hostile file said, so it is only ever
/// shown as text.
pub fn from_alert(v: &serde_json::Value, min: &str, id: u32) -> Option<Notif> {
    let sev = v.get("severity")?.as_str()?;
    if severity_rank(sev) < severity_rank(min) {
        return None;
    }
    let detector = v.get("detector_id").and_then(|x| x.as_str()).unwrap_or("alert");
    let summary = clean(v.get("summary").and_then(|x| x.as_str()).unwrap_or(""));
    // a blocked app that connected has a page of its own in Settings
    let link = v
        .get("event")
        .and_then(|e| e.get("Fields").or_else(|| e.get("fields")))
        .and_then(|f| f.get("app"))
        .and_then(|a| a.as_str())
        .filter(|a| !a.is_empty() && detector == "connection-watch")
        .map(|a| format!("apps:{a}"))
        .unwrap_or_default();
    Some(Notif {
        id,
        app: HEIMDALL_APP.into(),
        entry: String::new(),
        icon: "security-high".into(),
        summary: format!("{} · {}", if sev == "critical" { "Critical" } else { "Warning" }, detector),
        body: summary,
        urgency: if sev == "critical" { 2 } else { 1 },
        time: now_secs(),
        actions: vec![],
        link,
    })
}

fn http_get(addr: &str, path: &str) -> Option<String> {
    let mut s = std::net::TcpStream::connect_timeout(&addr.parse().ok()?, Duration::from_secs(2)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes()).ok()?;
    let mut text = String::new();
    s.read_to_string(&mut text).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    head.starts_with("HTTP/1.0 200").then(|| body.to_string()).or_else(|| head.starts_with("HTTP/1.1 200").then(|| body.to_string()))
}

/// Polls Heimdall's local API for alerts. What is already there when the panel starts is not announced.
pub fn spawn_heimdall(tx: calloop::channel::Sender<NMsg>) {
    std::thread::spawn(move || {
        let mut seen: HashSet<String> = HashSet::new();
        let mut first = true;
        loop {
            let min = labyrinth_conf::PanelConf::load().heimdall_alerts.as_str();
            if min != "off" {
                if let Some(body) = http_get("127.0.0.1:8930", "/v1/alerts?n=60") {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                        let list = v.as_array().cloned().or_else(|| v.get("alerts").and_then(|a| a.as_array()).cloned()).unwrap_or_default();
                        // oldest first, so they arrive in order
                        let mut fresh = vec![];
                        for a in &list {
                            let key = format!("{}|{}|{}", a.get("time").and_then(|x| x.as_str()).unwrap_or(""), a.get("detector_id").and_then(|x| x.as_str()).unwrap_or(""), a.get("summary").and_then(|x| x.as_str()).unwrap_or(""));
                            if seen.insert(key) && !first {
                                fresh.push(a.clone());
                            }
                        }
                        fresh.sort_by(|a, b| a.get("time").and_then(|x| x.as_str()).cmp(&b.get("time").and_then(|x| x.as_str())));
                        for a in fresh.iter().take(5) {
                            if let Some(n) = from_alert(a, &min, next_id()) {
                                let _ = tx.send(NMsg::New(n));
                            }
                        }
                        if seen.len() > 2000 {
                            seen.clear();
                            first = true; // do not re-announce the whole history after trimming
                        } else {
                            first = false;
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

// ------------------------------------------------------------------------------------------------ the list

#[derive(Clone, Debug, PartialEq)]
pub enum NAct {
    /// Open a group out or fold it.
    Toggle(String),
    /// Open this notification (go to the app).
    Open(u32),
    /// Dismiss one without opening it.
    Dismiss(u32),
    ClearGroup(String),
    ClearAll,
    /// Scroll the list by this many rows.
    Scroll(i32),
}

pub type Rect = (f32, f32, f32, f32);

const WHITE: Rgba = (255, 255, 255, 255);
const DIM: Rgba = (150, 150, 165, 255);
const ACCENT: Rgba = (124, 84, 214, 255);
const RED: Rgba = (232, 84, 84, 255);
const HEAD_H: f32 = 54.0;
const ITEM_H: f32 = 46.0;
const FOOT_H: f32 = 34.0;

enum Row<'a> {
    Head(&'a Group),
    Item(&'a Notif, &'a Group),
}

fn rows<'a>(groups: &'a [Group], expanded: &HashSet<String>) -> Vec<Row<'a>> {
    let mut v = vec![];
    for g in groups {
        v.push(Row::Head(g));
        if g.items.len() > 1 && expanded.contains(&g.key) {
            for n in &g.items {
                v.push(Row::Item(n, g));
            }
        }
    }
    v
}

fn over(p: (f32, f32), r: Rect) -> bool {
    p.0 >= r.0 && p.0 < r.0 + r.2 && p.1 >= r.1 && p.1 < r.1 + r.3
}

/// The app's icon, or a coloured disc with its first letter when the theme has none.
pub fn draw_app_icon(pix: &mut Pixmap, s: f32, icons: &mut Icons, fonts: &Fonts, name: &str, icon: &str, x: f32, y: f32, size: f32, urgent: bool) {
    let px = (size * s) as u32;
    let candidates = [icon.to_string(), name.to_lowercase(), name.to_lowercase().replace(' ', "-")];
    for c in candidates.iter().filter(|c| !c.is_empty() && !c.contains('/')) {
        if let Some(ic) = icons.get(c, px) {
            paint::draw_pixmap(pix, ic, (x * s) as i32, (y * s) as i32);
            return;
        }
    }
    let col = if urgent { RED } else { ACCENT };
    paint::fill_circle(pix, (x + size / 2.0) * s, (y + size / 2.0) * s, size / 2.0 * s, col);
    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
    let lw = paint::text_width(&fonts.bold, size * 0.5 * s, &letter);
    paint::draw_text(pix, &fonts.bold, size * 0.5 * s, (x + size / 2.0) * s - lw / 2.0, (y + size / 2.0) * s, WHITE, &letter);
}

fn fit(font: &ab_glyph::FontVec, size: f32, text: &str, max: f32) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    crate::fylgja::fit(font, size, line, max)
}

/// Draws the grouped list inside `area` (logical px; `s` the scale) and returns what can be clicked,
/// last drawn on top. The bottom strip holds "Clear all".
pub fn paint_list(pix: &mut Pixmap, s: f32, area: Rect, store: &Store, pointer: (f32, f32), now: u64, fonts: &Fonts, icons: &mut Icons) -> Vec<(Rect, NAct)> {
    let mut hits: Vec<(Rect, NAct)> = vec![];
    let (ax, ay, aw, ah) = area;
    let groups = store.groups();
    if groups.is_empty() {
        let msg = "No notifications";
        let w = paint::text_width(&fonts.regular, 13.0 * s, msg);
        paint::draw_text(pix, &fonts.regular, 13.0 * s, (ax + aw / 2.0) * s - w / 2.0, (ay + ah / 2.0 - 10.0) * s, DIM, msg);
        return hits;
    }
    let list_h = ah - FOOT_H;
    let all = rows(&groups, &store.expanded);
    let start = store.scroll.min(all.len().saturating_sub(1));
    let mut y = ay;
    let mut shown = 0;
    for row in all.iter().skip(start) {
        let h = match row {
            Row::Head(_) => HEAD_H,
            Row::Item(..) => ITEM_H,
        };
        if y + h > ay + list_h + 0.5 {
            break;
        }
        shown += 1;
        match row {
            Row::Head(g) => {
                let r = (ax, y, aw, HEAD_H - 4.0);
                let hot = over(pointer, r);
                paint::fill_rounded(pix, r.0 * s, r.1 * s, r.2 * s, r.3 * s, 12.0 * s, if hot { (255, 255, 255, 30) } else { (255, 255, 255, 16) });
                let top = &g.items[0];
                let urgent = g.items.iter().any(|n| n.urgency == 2);
                draw_app_icon(pix, s, icons, fonts, &g.name, &g.icon, ax + 10.0, y + 9.0, 30.0, urgent);
                let text_x = ax + 50.0;
                let right = ax + aw - 12.0;
                let t = ago(top.time, now);
                let tw = paint::text_width(&fonts.regular, 10.5 * s, &t) / s;
                paint::draw_text(pix, &fonts.regular, 10.5 * s, (right - tw) * s, (y + 15.0) * s, DIM, &t);
                let many = g.items.len() > 1;
                let mut name_right = right - tw - 8.0;
                if many {
                    let badge = g.items.len().to_string();
                    let bw = (paint::text_width(&fonts.bold, 10.5 * s, &badge) / s + 12.0).max(20.0);
                    paint::fill_rounded(pix, (right - bw) * s, (y + 27.0) * s, bw * s, 17.0 * s, 8.5 * s, if urgent { RED } else { ACCENT });
                    let lw = paint::text_width(&fonts.bold, 10.5 * s, &badge);
                    paint::draw_text(pix, &fonts.bold, 10.5 * s, (right - bw / 2.0) * s - lw / 2.0, (y + 35.5) * s, WHITE, &badge);
                    name_right = name_right.min(right - 4.0);
                }
                paint::draw_text(pix, &fonts.bold, 12.5 * s, text_x * s, (y + 15.0) * s, WHITE, &fit(&fonts.bold, 12.5 * s, &g.name, (name_right - text_x) * s));
                let preview = match (top.summary.is_empty(), top.body.is_empty()) {
                    (false, false) => format!("{}: {}", top.summary, top.body.lines().next().unwrap_or("")),
                    (true, _) => top.body.clone(),
                    _ => top.summary.clone(),
                };
                let pw = (right - text_x - if many { 30.0 } else { 0.0 }) * s;
                paint::draw_text(pix, &fonts.regular, 11.5 * s, text_x * s, (y + 34.0) * s, DIM, &fit(&fonts.regular, 11.5 * s, &preview, pw));
                hits.push((r, if many { NAct::Toggle(g.key.clone()) } else { NAct::Open(top.id) }));
                if hot {
                    let x = (right - tw - 22.0, y + 3.0, 14.0, 14.0);
                    // the small x on the header: clear the group
                    let hx = over(pointer, x);
                    paint::draw_text(pix, &fonts.bold, 13.0 * s, x.0 * s, (x.1 + 7.0) * s, if hx { WHITE } else { DIM }, "×");
                    hits.push((x, NAct::ClearGroup(g.key.clone())));
                }
            }
            Row::Item(n, _) => {
                let r = (ax + 14.0, y, aw - 14.0, ITEM_H - 4.0);
                let hot = over(pointer, r);
                paint::fill_rounded(pix, r.0 * s, r.1 * s, r.2 * s, r.3 * s, 10.0 * s, if hot { (255, 255, 255, 26) } else { (255, 255, 255, 10) });
                let right = r.0 + r.2 - 10.0;
                let t = ago(n.time, now);
                let tw = paint::text_width(&fonts.regular, 10.0 * s, &t) / s;
                paint::draw_text(pix, &fonts.regular, 10.0 * s, (right - tw) * s, (y + 13.0) * s, DIM, &t);
                if n.urgency == 2 {
                    paint::fill_circle(pix, (r.0 + 9.0) * s, (y + 13.0) * s, 3.0 * s, RED);
                }
                let tx = r.0 + 20.0;
                let title = if n.summary.is_empty() { &n.body } else { &n.summary };
                paint::draw_text(pix, &fonts.bold, 12.0 * s, tx * s, (y + 13.0) * s, WHITE, &fit(&fonts.bold, 12.0 * s, title, (right - tw - 8.0 - tx) * s));
                if !n.summary.is_empty() && !n.body.is_empty() {
                    paint::draw_text(pix, &fonts.regular, 11.0 * s, tx * s, (y + 30.0) * s, DIM, &fit(&fonts.regular, 11.0 * s, &n.body, (right - tx) * s));
                }
                hits.push((r, NAct::Open(n.id)));
                if hot {
                    let x = (right - tw - 16.0, y + 3.0, 14.0, 14.0);
                    let hx = over(pointer, x);
                    paint::draw_text(pix, &fonts.bold, 13.0 * s, x.0 * s, (x.1 + 7.0) * s, if hx { WHITE } else { DIM }, "×");
                    hits.push((x, NAct::Dismiss(n.id)));
                }
            }
        }
        y += h;
    }
    // scroll hints: rows above or below the ones shown
    if start > 0 {
        hits.push(((ax, ay, aw, 0.0), NAct::Scroll(-1)));
    }
    let _ = shown;
    // the foot: how many, and "Clear all"
    let fy = ay + ah - FOOT_H + 6.0;
    let count = format!("{} notification{}", store.len(), if store.len() == 1 { "" } else { "s" });
    paint::draw_text(pix, &fonts.regular, 11.5 * s, (ax + 6.0) * s, (fy + 11.0) * s, DIM, &count);
    let label = "Clear all";
    let lw = paint::text_width(&fonts.bold, 11.5 * s, label) / s;
    let b = (ax + aw - lw - 22.0, fy, lw + 20.0, 22.0);
    let hot = over(pointer, b);
    paint::fill_rounded(pix, b.0 * s, b.1 * s, b.2 * s, b.3 * s, 11.0 * s, if hot { (255, 255, 255, 40) } else { (255, 255, 255, 18) });
    paint::draw_text(pix, &fonts.bold, 11.5 * s, (b.0 + 10.0) * s, (b.1 + 11.0) * s, if hot { WHITE } else { (210, 210, 222, 255) }, label);
    hits.push((b, NAct::ClearAll));
    hits
}

// ------------------------------------------------------------------------------------------------ pop-ups

pub const TOAST_W: u32 = 360;
const TOAST_H: f32 = 78.0;
const TOAST_GAP: f32 = 8.0;
const TOAST_MAX: usize = 3;

/// The pop-up surface is as tall as three cards; the cards stack up from its bottom.
pub fn surface_h() -> u32 {
    (TOAST_MAX as f32 * (TOAST_H + TOAST_GAP)) as u32
}

/// Draws the pop-ups (oldest on top, newest at the bottom corner) and returns their hits in physical pixels:
/// each card's close button first, then the card, so the button wins where they overlap.
pub fn paint_toasts(pix: &mut Pixmap, s: f32, list: &[Notif], hover: Option<usize>, fonts: &Fonts, icons: &mut Icons) -> Vec<crate::shell::Hit> {
    use crate::shell::{Action, Hit};
    pix.fill(paint::color((0, 0, 0, 0)));
    let (w, h) = (pix.width() as f32 / s, pix.height() as f32 / s);
    let mut hits = vec![];
    let now = now_secs();
    for (k, n) in list.iter().rev().enumerate() {
        let y = h - (k as f32 + 1.0) * (TOAST_H + TOAST_GAP) + TOAST_GAP;
        let r = (0.0, y, w, TOAST_H);
        let card_hit = hits.len() + 1; // the card's hit comes after its close button's
        let lit = hover == Some(card_hit) || hover == Some(card_hit - 1);
        paint::fill_rounded(pix, r.0 * s, r.1 * s, r.2 * s, r.3 * s, 16.0 * s, if lit { (14, 14, 18, 245) } else { (0, 0, 0, 238) });
        paint::stroke_rounded(pix, (r.0 + 0.5) * s, (r.1 + 0.5) * s, (r.2 - 1.0) * s, (r.3 - 1.0) * s, 16.0 * s, s, if n.urgency == 2 { (232, 84, 84, 140) } else { (255, 255, 255, 34) });
        draw_app_icon(pix, s, icons, fonts, &n.app, &n.icon, 14.0, y + 14.0, 36.0, n.urgency == 2);
        let tx = 62.0;
        let right = w - 14.0;
        let t = ago(n.time, now);
        let tw = paint::text_width(&fonts.regular, 10.5 * s, &t) / s;
        paint::draw_text(pix, &fonts.regular, 10.5 * s, (right - tw - 18.0) * s, (y + 17.0) * s, DIM, &t);
        paint::draw_text(pix, &fonts.bold, 11.0 * s, tx * s, (y + 17.0) * s, (190, 190, 205, 255), &fit(&fonts.bold, 11.0 * s, &n.app, (right - tw - 30.0 - tx) * s));
        let title = if n.summary.is_empty() { &n.body } else { &n.summary };
        paint::draw_text(pix, &fonts.bold, 13.0 * s, tx * s, (y + 38.0) * s, WHITE, &fit(&fonts.bold, 13.0 * s, title, (right - tx) * s));
        if !n.summary.is_empty() && !n.body.is_empty() {
            paint::draw_text(pix, &fonts.regular, 12.0 * s, tx * s, (y + 58.0) * s, DIM, &fit(&fonts.regular, 12.0 * s, &n.body, (right - tx) * s));
        }
        let x = (right - 14.0, y + 6.0, 20.0, 20.0);
        paint::draw_text(pix, &fonts.bold, 14.0 * s, (x.0 + 4.0) * s, (x.1 + 10.0) * s, if hover == Some(card_hit - 1) { WHITE } else { DIM }, "×");
        let phys = |r: Rect| (r.0 * s, r.1 * s, r.2 * s, r.3 * s);
        hits.push(Hit { rect: phys(x), action: Action::ToastClose(n.id) });
        hits.push(Hit { rect: phys(r), action: Action::ToastOpen(n.id) });
    }
    hits
}

/// How many rows the list can scroll down to (for the wheel).
pub fn max_scroll(store: &Store) -> usize {
    rows(&store.groups(), &store.expanded).len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: u32, app: &str, summary: &str, time: u64) -> Notif {
        Notif { id, app: app.into(), entry: String::new(), icon: String::new(), summary: summary.into(), body: String::new(), urgency: 1, time, actions: vec![], link: String::new() }
    }

    #[test]
    fn groups_by_app_newest_group_first() {
        let mut s = Store::default();
        s.add(n(1, "Mail", "a", 100));
        s.add(n(2, "Chat", "b", 200));
        s.add(n(3, "Mail", "c", 300));
        let g = s.groups();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "Mail"); // its newest (300) beats Chat's (200)
        assert_eq!(g[0].items.iter().map(|x| x.id).collect::<Vec<_>>(), vec![3, 1]);
        assert_eq!(g[1].name, "Chat");
    }

    #[test]
    fn desktop_entry_groups_across_names() {
        let mut s = Store::default();
        let mut a = n(1, "Telegram", "x", 10);
        a.entry = "org.telegram.desktop".into();
        let mut b = n(2, "Telegram Desktop", "y", 20);
        b.entry = "org.telegram.desktop".into();
        s.add(a);
        s.add(b);
        assert_eq!(s.groups().len(), 1);
    }

    #[test]
    fn replace_keeps_one_and_remove_clear_work() {
        let mut s = Store::default();
        s.add(n(1, "A", "old", 1));
        s.add(n(1, "A", "new", 2));
        assert_eq!(s.len(), 1);
        assert_eq!(s.get(1).unwrap().summary, "new");
        s.add(n(2, "B", "x", 3));
        assert_eq!(s.clear_group("a"), vec![1]);
        assert_eq!(s.remove(2).map(|x| x.id), Some(2));
        assert!(s.is_empty());
        s.add(n(3, "C", "z", 4));
        assert_eq!(s.clear_all(), vec![3]);
    }

    #[test]
    fn keeps_at_most_max() {
        let mut s = Store::default();
        for i in 0..(MAX_KEPT as u32 + 10) {
            s.add(n(i + 1, "A", "x", i as u64));
        }
        assert_eq!(s.len(), MAX_KEPT);
        assert!(s.get(1).is_none()); // the oldest went
        assert!(s.get(MAX_KEPT as u32 + 10).is_some());
    }

    #[test]
    fn saved_and_loaded_back() {
        let dir = std::env::temp_dir().join(format!("lab-notify-{}", std::process::id()));
        let path = dir.join("n.json");
        let mut s = Store::load(Some(path.clone()));
        let mut a = n(7, "Mail", "hello", 55);
        a.actions = vec![("default".into(), "Open".into())];
        a.urgency = 2;
        s.add(a.clone());
        let back = Store::load(Some(path));
        assert_eq!(back.get(7), Some(&a));
        assert_eq!(back.max_id(), 7);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn markup_is_stripped() {
        assert_eq!(clean("<b>Hi</b> there &amp; <a href=\"x\">you</a>"), "Hi there & you");
        assert_eq!(clean("a\u{7}b"), "ab");
        assert!(clean(&"x".repeat(1000)).chars().count() <= MAX_TEXT + 1);
    }

    #[test]
    fn times_are_short() {
        assert_eq!(ago(100, 130), "now");
        assert_eq!(ago(0, 300), "5m");
        assert_eq!(ago(0, 7200), "2h");
        assert_eq!(ago(0, 3 * 86400), "3d");
    }

    #[test]
    fn heimdall_alert_to_notification() {
        let v = serde_json::json!({"time": "t", "detector_id": "connection-watch", "severity": "critical", "summary": "app \"x\" ...",
            "event": {"Fields": {"app": "x"}}});
        let note = from_alert(&v, "warning", 9).unwrap();
        assert_eq!(note.app, "Heimdall");
        assert_eq!(note.urgency, 2);
        assert_eq!(note.link, "apps:x");
        assert!(note.summary.starts_with("Critical"));
        let info = serde_json::json!({"detector_id": "connection-watch", "severity": "info", "summary": "s"});
        assert!(from_alert(&info, "warning", 1).is_none());
        assert!(from_alert(&info, "info", 1).is_some());
    }
}

#[cfg(test)]
mod render {
    use super::*;

    /// Writes what the list and the pop-ups look like to $NOTIFY_PNG_DIR (run by hand to see them).
    #[test]
    fn draws_the_list_and_toasts() {
        let Some(fonts) = Fonts::load() else { return };
        let mut icons = Icons::new();
        let mut s = Store::default();
        let now = now_secs();
        let mk = |id, app: &str, sum: &str, body: &str, age, urgency| Notif { id, app: app.into(), entry: String::new(), icon: String::new(), summary: sum.into(), body: body.into(), urgency, time: now - age, actions: vec![], link: String::new() };
        s.add(mk(1, "Mail", "Alice", "Are we still on for lunch?", 600, 1));
        s.add(mk(2, "Mail", "Bob", "Invoice attached for October", 120, 1));
        s.add(mk(3, "Heimdall", "Critical · connection-watch", "app \"x\" has its network switched off but curl connected", 30, 2));
        s.add(mk(4, "Chat", "Sam", "ok", 4000, 1));
        s.toggle_group("mail");
        let hits = {
            let mut pix = Pixmap::new(880, 744).unwrap();
            pix.fill(paint::color((0, 0, 0, 255)));
            let h = paint_list(&mut pix, 2.0, (20.0, 14.0, 400.0, 350.0), &s, (0.0, 0.0), now, &fonts, &mut icons);
            if let Some(dir) = std::env::var_os("NOTIFY_PNG_DIR") {
                pix.save_png(std::path::Path::new(&dir).join("list.png")).unwrap();
            }
            h
        };
        assert!(hits.iter().any(|(_, a)| *a == NAct::ClearAll));
        assert!(hits.iter().any(|(_, a)| matches!(a, NAct::Open(_))));
        let mut pix = Pixmap::new(720, 2 * surface_h()).unwrap();
        let list = vec![s.get(2).unwrap().clone(), s.get(3).unwrap().clone()];
        let th = paint_toasts(&mut pix, 2.0, &list, None, &fonts, &mut icons);
        assert_eq!(th.len(), 4);
        if let Some(dir) = std::env::var_os("NOTIFY_PNG_DIR") {
            pix.save_png(std::path::Path::new(&dir).join("toast.png")).unwrap();
        }
    }
}
