//! `org.freedesktop.impl.portal.Settings`: tells programs whether the desktop is dark or light and what the accent colour is
//! (the `org.freedesktop.appearance` namespace: GTK 4 / libadwaita, Qt 6, Firefox, Chromium and Electron all read it), and tells them
//! again the moment it changes. The values come from `~/.config/yggdrasil/appearance.conf` (yggdrasil_conf::appearance), which
//! Settings > Appearance writes; a change is noticed through inotify, so nothing polls.

use std::collections::HashMap;

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};

pub const NAMESPACE: &str = "org.freedesktop.appearance";

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.portal.Error")]
pub enum PortalError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotFound(String),
}

/// The appearance values, as the portal specification wants them: color-scheme 1 dark / 2 light, accent-color as three 0..1
/// doubles, contrast 0 (normal).
pub fn values(a: &yggdrasil_conf::appearance::Appearance) -> Vec<(&'static str, OwnedValue)> {
    let [r, g, b] = a.accent.unwrap_or_else(yggdrasil_conf::integrations::default_accent);
    let f = |x: u8| x as f64 / 255.0;
    vec![
        ("color-scheme", OwnedValue::from(if a.dark { 1u32 } else { 2u32 })),
        ("accent-color", Value::from((f(r), f(g), f(b))).try_to_owned().expect("plain values")),
        ("contrast", OwnedValue::from(0u32)),
    ]
}

/// Whether a ReadAll pattern asks for `ns`: an empty list or an empty pattern means everything; a trailing `*` is a prefix.
pub fn wanted(patterns: &[String], ns: &str) -> bool {
    patterns.is_empty() || patterns.iter().any(|p| p.is_empty() || p == ns || p.strip_suffix('*').is_some_and(|pre| ns.starts_with(pre)))
}

pub struct Settings;

#[zbus::interface(name = "org.freedesktop.impl.portal.Settings")]
impl Settings {
    fn read_all(&self, namespaces: Vec<String>) -> HashMap<String, HashMap<String, OwnedValue>> {
        let mut out = HashMap::new();
        if wanted(&namespaces, NAMESPACE) {
            out.insert(
                NAMESPACE.to_string(),
                values(&yggdrasil_conf::appearance::load()).into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            );
        }
        out
    }

    fn read(&self, namespace: String, key: String) -> Result<OwnedValue, PortalError> {
        if namespace == NAMESPACE {
            if let Some((_, v)) = values(&yggdrasil_conf::appearance::load()).into_iter().find(|(k, _)| *k == key) {
                return Ok(v);
            }
        }
        Err(PortalError::NotFound(format!("{namespace} {key}")))
    }

    #[zbus(signal)]
    pub async fn setting_changed(emitter: &SignalEmitter<'_>, namespace: &str, key: &str, value: Value<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

/// The file names in what inotify read (a run of events: each a header, then its name padded with NULs). The buffer is plain
/// bytes with no alignment, so each header is copied out rather than referenced in place: a reference to an unaligned header
/// is undefined behaviour, and debug builds stop on it (the portal ended so in CI, 2026-11-24). An event that runs past the end
/// ends the list.
fn event_names(buf: &[u8]) -> Vec<&[u8]> {
    let head = std::mem::size_of::<libc::inotify_event>();
    let mut names = vec![];
    let mut off = 0usize;
    while off + head <= buf.len() {
        // SAFETY: `head` bytes are there (checked above); read_unaligned needs no alignment
        let ev: libc::inotify_event = unsafe { std::ptr::read_unaligned(buf[off..].as_ptr().cast()) };
        let start = off + head;
        let Some(raw) = buf.get(start..start + ev.len as usize) else { break };
        names.push(&raw[..raw.iter().position(|&c| c == 0).unwrap_or(raw.len())]);
        off = start + ev.len as usize;
    }
    names
}

/// Watches the appearance file and tells programs what changed. Runs on its own thread; returns at once.
pub fn watch(conn: zbus::blocking::Connection, path: &'static str) {
    std::thread::spawn(move || {
        let file = yggdrasil_conf::appearance::path();
        let (Some(dir), Some(name)) = (file.parent().map(|d| d.to_path_buf()), file.file_name().map(|n| n.to_os_string())) else { return };
        let _ = std::fs::create_dir_all(&dir);
        let Ok(dir_c) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else { return };
        // SAFETY: plain inotify calls on a descriptor this thread owns
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if fd < 0 || unsafe { libc::inotify_add_watch(fd, dir_c.as_ptr(), libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO) } < 0 {
            return;
        }
        let mut last = yggdrasil_conf::appearance::load();
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                return;
            }
            // only our file matters (the folder holds the desktop's other settings too)
            let ours = event_names(&buf[..n as usize]).contains(&name.as_encoded_bytes());
            if !ours {
                continue;
            }
            let now = yggdrasil_conf::appearance::load();
            if now == last {
                continue;
            }
            let Ok(iface) = conn.object_server().interface::<_, Settings>(path) else { continue };
            let emitter = iface.signal_emitter().clone();
            let (old, new) = (values(&last), values(&now));
            for ((k, v), (_, before)) in new.into_iter().zip(old) {
                if v != before {
                    let _ = zbus::block_on(Settings::setting_changed(&emitter, NAMESPACE, k, Value::from(v)));
                }
            }
            last = now;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use yggdrasil_conf::appearance::Appearance;

    #[test]
    fn values_follow_the_spec() {
        let v = values(&Appearance { dark: false, accent: Some([255, 0, 51]) });
        assert_eq!(u32::try_from(&v[0].1).unwrap(), 2, "light is 2");
        let (r, g, b): (f64, f64, f64) = v[1].1.try_clone().unwrap().try_into().unwrap();
        assert_eq!((r, g, b), (1.0, 0.0, 0.2));
        assert_eq!(u32::try_from(&values(&Appearance { dark: true, accent: None })[0].1).unwrap(), 1, "dark is 1");
    }

    #[test]
    fn namespace_patterns() {
        assert!(wanted(&[], NAMESPACE));
        assert!(wanted(&["org.freedesktop.*".into()], NAMESPACE));
        assert!(wanted(&[NAMESPACE.into()], NAMESPACE));
        assert!(!wanted(&["org.gnome.*".into()], NAMESPACE));
    }

    #[test]
    fn inotify_events_are_read_from_an_unaligned_buffer() {
        // two events, "appearance.conf" and "other", as the kernel lays them out
        let event = |name: &str| {
            let mut padded = name.as_bytes().to_vec();
            padded.resize(name.len().div_ceil(4) * 4 + 4, 0);
            let mut b = vec![0u8; std::mem::size_of::<libc::inotify_event>()];
            b[12..16].copy_from_slice(&(padded.len() as u32).to_ne_bytes());
            b.extend(padded);
            b
        };
        let mut bytes = event("appearance.conf");
        bytes.extend(event("other"));
        // one byte in, so the headers are not 4-byte aligned (as read() into a byte buffer may leave them)
        let mut buf = vec![0u8; 1];
        buf.extend(&bytes);
        let names = super::event_names(&buf[1..]);
        assert_eq!(names, vec![b"appearance.conf".as_slice(), b"other".as_slice()]);
        // an event cut short ends the list instead of reading past the end
        assert_eq!(super::event_names(&buf[1..buf.len() - 3]), vec![b"appearance.conf".as_slice()]);
    }
}
