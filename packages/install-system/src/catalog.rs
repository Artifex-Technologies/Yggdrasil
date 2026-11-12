//! The OS's lists: the browsers offered (and what choosing them means), the optional parts and the components they install,
//! and the files the default browser is written into (mimeapps.list, the dock's pins).

use std::collections::HashSet;

use crate::py::{decode, universal};
use crate::pyjson::{loads, Dict, Py, OTHER_NESTING};

/// The keys that make a browser the default (as /etc/xdg/mimeapps.list sets them).
pub const BROWSER_MIME: [&str; 6] = [
    "x-scheme-handler/http",
    "x-scheme-handler/https",
    "x-scheme-handler/about",
    "x-scheme-handler/unknown",
    "text/html",
    "application/xhtml+xml",
];

/// The entries listed under `key` in one of the OS's JSON files that have an id (text, as the window reads them); none when
/// the file cannot be read or is not such a list.
pub fn entries(path: Option<&str>, key: &str) -> Vec<Py> {
    let Some(path) = path else { return vec![] };
    let Ok(bytes) = std::fs::read(path) else {
        return vec![];
    };
    let Ok(text) = decode(&bytes) else {
        return vec![];
    };
    let Ok(data) = loads(&universal(&text), OTHER_NESTING) else {
        return vec![];
    };
    match data.get(key) {
        Some(Py::List(items)) => {
            items.iter().filter(|x| x.as_dict().is_some() && x.at("id").as_str().is_some_and(|i| !i.is_empty())).cloned().collect()
        }
        _ => vec![],
    }
}

/// An entry's id (entries() made sure it is text).
pub fn id(entry: &Py) -> String {
    entry.at("id").as_str().unwrap_or("").to_string()
}

/// The text items of a list in one of the OS's files (a list is expected there; anything else is no list).
pub fn texts(v: &Py) -> Vec<String> {
    match v {
        Py::List(l) => l.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => vec![],
    }
}

/// What choosing browsers means.
#[derive(Debug, PartialEq)]
pub struct BrowserPlan {
    /// the repository packages of the others (only those installed are removed)
    pub remove: Vec<String>,
    /// the chosen ones' packages
    pub install: Vec<String>,
    /// the chosen ones' Flathub apps
    pub flatpaks: Vec<String>,
}

pub fn browser_plan(catalog: &[Py], chosen: &[String]) -> BrowserPlan {
    let (mut remove, mut install, mut flatpaks) = (vec![], vec![], vec![]);
    for b in catalog {
        let pk = texts(b.at("packages"));
        if chosen.contains(&id(b)) {
            install.extend(pk);
            if let Some(f) = b.at("flatpak").as_str().filter(|f| !f.is_empty()) {
                flatpaks.push(f.to_string());
            }
        } else {
            remove.extend(pk);
        }
    }
    let keep: HashSet<&String> = install.iter().collect();
    let remove = remove.iter().filter(|x| !keep.contains(x)).cloned().collect();
    BrowserPlan { remove, install, flatpaks }
}

fn lstrip(s: &str) -> &str {
    s.trim_start_matches(pycompat::isspace)
}

/// /etc/xdg/mimeapps.list with `desktop` opening web links and pages (the keys added under [Default Applications] if missing).
pub fn mimeapps_with_browser(text: &str, desktop: &str) -> String {
    let mut lines: Vec<String> = pycompat::splitlines(text).into_iter().map(str::to_string).collect();
    if !lines.iter().any(|l| pycompat::strip(l) == "[Default Applications]") {
        lines.push("[Default Applications]".into());
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = vec![];
    for l in &lines {
        let k = pycompat::strip(l.split('=').next().unwrap_or(""));
        match BROWSER_MIME.iter().find(|m| **m == k) {
            Some(m) if l.contains('=') && !lstrip(l).starts_with('#') => {
                out.push(format!("{k}={desktop}"));
                seen.insert(m);
            }
            _ => out.push(l.clone()),
        }
    }
    let at = out.iter().position(|l| pycompat::strip(l) == "[Default Applications]").map_or(0, |i| i + 1);
    for k in BROWSER_MIME.iter().filter(|k| !seen.contains(*k)).rev() {
        out.insert(at, format!("{k}={desktop}"));
    }
    out.join("\n") + "\n"
}

/// The dock's pinned apps with the browser pinned there (any of `browser_ids`) replaced by the default one, or taken out when
/// there is none; other browsers pinned are taken out.
pub fn dock_with_browser(text: &str, browser_ids: &HashSet<String>, default_id: Option<&str>) -> String {
    let mut out: Vec<&str> = vec![];
    let mut placed = false;
    for l in pycompat::splitlines(text) {
        if browser_ids.contains(pycompat::strip(l)) && !lstrip(l).starts_with('#') {
            if let (Some(d), false) = (default_id, placed) {
                out.push(d);
                placed = true;
            }
            continue;
        }
        out.push(l);
    }
    out.join("\n") + "\n"
}

/// The OS's optional components by id (the integration's "components" file): an id given twice keeps its first place and
/// takes the last entry.
pub fn components(path: Option<&str>) -> Dict {
    let mut d = Dict::default();
    for c in entries(path, "components") {
        d.insert(id(&c), c);
    }
    d
}

/// The names pacman knows a component by (a pkgbuild component installs its own name).
pub fn component_packages(c: &Py) -> Vec<String> {
    let mut v = texts(c.at("packages"));
    v.extend(texts(c.at("aur")));
    if let Some(p) = c.at("pkgbuild").as_str().filter(|p| !p.is_empty()) {
        v.push(p.to_string());
    }
    v
}

/// A component's name for messages (its id when it has none in text).
pub fn component_name(comps: &Dict, cid: &str) -> String {
    comps.get(cid).and_then(|c| c.at("name").as_str()).unwrap_or(cid).to_string()
}

/// What choosing optional parts means for the new system.
#[derive(Debug, PartialEq)]
pub struct ExtrasPlan {
    /// the packages of parts not chosen that the system has (and the chosen ones do not need)
    pub remove: Vec<String>,
    /// the chosen components it already has (kept: their services are switched on)
    pub keep: Vec<String>,
    /// the chosen components it lacks (installed at the first login; they need the internet)
    pub queue: Vec<String>,
}

pub fn extras_plan(catalog: &[Py], chosen: &[String], comps: &Dict, installed: &HashSet<String>) -> ExtrasPlan {
    let (mut keep, mut queue, mut drop) = (vec![], vec![], vec![]);
    for e in catalog {
        for cid in texts(e.at("components")) {
            let Some(c) = comps.get(&cid) else { continue };
            if chosen.contains(&id(e)) {
                if component_packages(c).iter().all(|x| installed.contains(x)) {
                    keep.push(cid);
                } else if !queue.contains(&cid) {
                    queue.push(cid);
                }
            } else {
                drop.push(cid);
            }
        }
    }
    let wanted: HashSet<String> =
        keep.iter().chain(queue.iter()).flat_map(|cid| comps.get(cid).map(component_packages).unwrap_or_default()).collect();
    let mut remove: Vec<String> = vec![];
    for cid in &drop {
        for x in comps.get(cid).map(component_packages).unwrap_or_default() {
            if installed.contains(&x) && !wanted.contains(&x) && !remove.contains(&x) {
                remove.push(x);
            }
        }
    }
    ExtrasPlan { remove, keep, queue }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf::tempdir::Dir;

    const BROWSERS: &str = r#"{"browsers": [
        {"id": "vivaldi", "name": "Vivaldi", "chosen": true, "packages": ["vivaldi", "vivaldi-ffmpeg-codecs"], "desktop": ["vivaldi-stable.desktop"]},
        {"id": "firefox", "name": "Firefox", "packages": ["firefox"], "desktop": ["firefox.desktop"]},
        {"id": "librewolf", "name": "LibreWolf", "packages": ["librewolf"], "desktop": ["librewolf.desktop"]},
        {"id": "opera-gx", "name": "Opera GX", "flatpak": "com.opera.opera-gx", "desktop": ["com.opera.opera-gx.desktop"]},
        {"name": "no id"}]}"#;
    const EXTRAS: &str = r#"{"extras": [{"id": "assistant", "name": "Assistant", "components": ["assistant"]},
        {"id": "monitor", "name": "Monitor", "components": ["monitor"]}]}"#;
    const COMPONENTS: &str = r#"{"components": [{"id": "assistant", "packages": ["assistant-git"]},
        {"id": "monitor", "packages": ["monitor-git", "clamav"], "enable_system": ["monitor.service"]}]}"#;

    fn written(d: &Dir, name: &str, text: &str) -> String {
        let p = d.path().join(name);
        std::fs::write(&p, text).unwrap();
        p.to_str().unwrap().to_string()
    }

    fn ids(v: &[Py]) -> Vec<String> {
        v.iter().map(id).collect()
    }

    fn sv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn entries_without_an_id_are_left_out() {
        let d = Dir::new();
        let cat = entries(Some(&written(&d, "b.json", BROWSERS)), "browsers");
        assert_eq!(ids(&cat), ["vivaldi", "firefox", "librewolf", "opera-gx"]);
        assert!(entries(Some("/nonexistent.json"), "browsers").is_empty());
        assert!(entries(None, "browsers").is_empty());
        assert!(entries(Some(&written(&d, "l.json", r#"[{"id": "x"}]"#)), "browsers").is_empty(), "not an object at the top");
        assert!(entries(Some(&written(&d, "o.json", r#"{"browsers": {"id": "x"}}"#)), "browsers").is_empty());
        assert_eq!(
            ids(&entries(
                Some(&written(&d, "n.json", r#"{"browsers": [{"id": 5}, {"id": ["x"]}, {"id": ""}, {"id": "ok"}]}"#)),
                "browsers"
            )),
            ["ok"]
        );
    }

    #[test]
    fn a_choice_removes_the_others_and_installs_its_own() {
        let d = Dir::new();
        let cat = entries(Some(&written(&d, "b.json", BROWSERS)), "browsers");
        let p = browser_plan(&cat, &sv(&["librewolf", "opera-gx"]));
        assert_eq!(p.remove, ["vivaldi", "vivaldi-ffmpeg-codecs", "firefox"]);
        assert_eq!(p.install, ["librewolf"]);
        assert_eq!(p.flatpaks, ["com.opera.opera-gx"]);
        let none = browser_plan(&cat, &[]);
        assert!(none.install.is_empty() && none.flatpaks.is_empty());
        let odd = entries(
            Some(&written(
                &d,
                "odd.json",
                r#"{"browsers": [{"id": "a", "packages": "abc", "flatpak": 5}, {"id": "b", "packages": ["b", 7]}]}"#,
            )),
            "browsers",
        );
        assert_eq!(browser_plan(&odd, &sv(&["a"])), BrowserPlan { remove: sv(&["b"]), install: vec![], flatpaks: vec![] });
    }

    #[test]
    fn the_default_browser_opens_links_and_sits_in_the_dock() {
        let mime = "# comment\n[Default Applications]\nx-scheme-handler/http=vivaldi-stable.desktop\ntext/html=vivaldi-stable.desktop\nvideo/mp4=yggdrasil-player.desktop\n";
        let out = mimeapps_with_browser(mime, "librewolf.desktop");
        assert!(out.contains("x-scheme-handler/http=librewolf.desktop"));
        assert!(out.contains("x-scheme-handler/https=librewolf.desktop"), "a missing key is added");
        assert!(out.contains("video/mp4=yggdrasil-player.desktop"));
        assert!(!out.contains("vivaldi"));
        assert!(mimeapps_with_browser("", "x.desktop").contains("text/html=x.desktop"));
        let dock = "# pins\nminos\nvivaldi-stable\nfirefox\nsteam\n";
        let ids: HashSet<String> = sv(&["vivaldi-stable", "firefox", "librewolf"]).into_iter().collect();
        assert_eq!(dock_with_browser(dock, &ids, Some("librewolf")), "# pins\nminos\nlibrewolf\nsteam\n");
        assert_eq!(dock_with_browser(dock, &ids, None), "# pins\nminos\nsteam\n");
    }

    #[test]
    fn on_the_medium_kept_or_removed_otherwise_left_for_later() {
        let d = Dir::new();
        let cat = entries(Some(&written(&d, "e.json", EXTRAS)), "extras");
        let comps = components(Some(&written(&d, "c.json", COMPONENTS)));
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<HashSet<String>>();
        let p = extras_plan(&cat, &sv(&["monitor"]), &comps, &set(&["assistant-git", "monitor-git", "clamav"]));
        assert_eq!((p.remove, p.keep, p.queue), (sv(&["assistant-git"]), sv(&["monitor"]), vec![]));
        let p = extras_plan(&cat, &sv(&["assistant", "monitor"]), &comps, &set(&[]));
        assert_eq!((p.remove, p.keep, p.queue), (vec![], vec![], sv(&["assistant", "monitor"])));
        let p = extras_plan(&cat, &[], &comps, &set(&["monitor-git"]));
        assert_eq!((p.remove, p.keep, p.queue), (sv(&["monitor-git"]), vec![], vec![]), "what the medium lacks is not removed");
        let odd = components(Some(&written(
            &d,
            "odd.json",
            r#"{"components": [{"id": "c", "packages": ["p"], "aur": "q", "pkgbuild": 5, "name": null}]}"#,
        )));
        assert_eq!(component_packages(odd.get("c").unwrap()), ["p"]);
        assert_eq!(component_name(&odd, "c"), "c");
    }
}
