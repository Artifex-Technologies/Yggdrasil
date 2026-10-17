//! What the Marketplace lists and where it comes from: Labyrinth's own catalog (built in, reviewed), repositories the user adds
//! (a Labyrinth catalog file, or Omarchy's plugin registry), and what is installed. Nothing here installs anything by itself;
//! `install_script` and `remove_script` build the command that runs in a terminal, where the password prompt and the output are visible.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use serde_json::{json, Value};

pub const BUILTIN: &str = include_str!("../catalog.json");

/// Which edition of the OS this is: "labyrinth" or "chymaera" (from /etc/os-release, or LABYRINTH_EDITION to try the other one). Apps
/// meant for one edition (the penetration-testing tools are Chymaera's) are not listed in the other.
pub fn edition() -> String {
    if let Ok(e) = std::env::var("LABYRINTH_EDITION") {
        return e.to_lowercase();
    }
    let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("ID=") {
            return v.trim().trim_matches('"').to_lowercase();
        }
    }
    "labyrinth".into()
}

/// The apps of `catalog` that this edition lists.
pub fn for_edition(mut c: Catalog, edition: &str) -> Catalog {
    c.apps.retain(|a| a.editions.is_empty() || a.editions.iter().any(|e| e == edition));
    c
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Packages from the Arch repositories.
    Pacman(Vec<String>),
    /// Packages built from the AUR with yay.
    Aur(Vec<String>),
    Flatpak(String),
    /// One of Labyrinth's optional components (labyrinth-apps).
    Component(String),
    /// An Omarchy shell plugin: listed, not installable here yet.
    Plugin { repo: String },
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub subtitle: String,
    pub description: String,
    /// Create, Work, Play, Develop, Security, Internet, Utilities, Plugins ...
    pub category: String,
    pub subcategory: String,
    pub source: Source,
    pub icon: String,
    /// Looked at by Labyrinth (only its own catalog).
    pub reviewed: bool,
    pub note: String,
    pub tags: Vec<String>,
    /// The catalog this came from.
    pub from: String,
    pub launch: String,
    /// AppStream ids (only in Labyrinth's own catalog): the app's own, and its Flathub one.
    pub appstream: String,
    pub flathub: String,
    /// Which editions of the OS list it ("labyrinth", "chymaera"); empty means all of them.
    pub editions: Vec<String>,
    /// Part of the base system: it is listed (and can be reinstalled) but not removed from here.
    pub preinstalled: bool,
    /// Where the (open) source code is, if it is public: shown as a link on the app's page.
    pub repo: String,
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub name: String,
    pub apps: Vec<Entry>,
    /// (id, tagline, kicker)
    pub featured: Vec<(String, String, String)>,
    /// A colour chosen for an app's tile on Discover (otherwise one is made from its name): id -> (r, g, b).
    pub tile_colors: std::collections::HashMap<String, (u8, u8, u8)>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// Read a Labyrinth catalog file. `trusted` marks its entries as reviewed only for the built-in one.
pub fn parse_catalog(text: &str, from: &str, trusted: bool) -> Result<Catalog, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not a catalog: {e}"))?;
    let list = v.get("apps").and_then(|a| a.as_array()).ok_or("no \"apps\" list in this file")?;
    let mut apps = vec![];
    for a in list {
        let src = a.get("source").cloned().unwrap_or(Value::Null);
        let source = match src.get("type").and_then(|t| t.as_str()) {
            Some("pacman") => Source::Pacman(strings(&src["packages"])),
            Some("aur") => Source::Aur(strings(&src["packages"])),
            Some("flatpak") => Source::Flatpak(s(&src, "app")),
            // components exist only in Labyrinth's own catalog
            Some("component") if trusted => Source::Component(s(&src, "id")),
            _ => continue,
        };
        let id = s(a, "id");
        let name = s(a, "name");
        if id.is_empty() || name.is_empty() {
            continue;
        }
        // package and app names go to a shell: refuse anything but plain names
        let plain = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_alphanumeric() || "-_.+@".contains(c));
        let ok = match &source {
            Source::Pacman(p) | Source::Aur(p) => !p.is_empty() && p.iter().all(|x| plain(x)),
            Source::Flatpak(x) | Source::Component(x) => plain(x),
            Source::Plugin { .. } => false,
        };
        if !ok {
            continue;
        }
        apps.push(Entry {
            icon: if s(a, "icon").is_empty() { id.clone() } else { s(a, "icon") },
            subtitle: s(a, "subtitle"),
            description: s(a, "description"),
            category: if s(a, "category").is_empty() { "Other".into() } else { s(a, "category") },
            subcategory: s(a, "subcategory"),
            reviewed: trusted && a.get("reviewed").and_then(|r| r.as_bool()).unwrap_or(false),
            note: s(a, "note"),
            tags: strings(&a["tags"]),
            from: from.to_string(),
            launch: s(a, "launch"),
            editions: if trusted { strings(&a["editions"]) } else { vec![] },
            repo: if trusted && s(a, "repo").starts_with("https://") { s(a, "repo") } else { String::new() },
            preinstalled: trusted && a.get("preinstalled").and_then(|p| p.as_bool()).unwrap_or(false),
            appstream: if trusted { s(a, "appstream") } else { String::new() },
            flathub: if trusted { s(a, "flathub") } else { String::new() },
            id,
            name,
            source,
        });
    }
    let featured = v.get("featured").and_then(|f| f.as_array()).map(|f| f.iter().map(|x| (s(x, "id"), s(x, "tagline"), s(x, "kicker"))).collect()).unwrap_or_default();
    let mut tile_colors = std::collections::HashMap::new();
    if let Some(o) = v.get("tile_colors").and_then(|t| t.as_object()) {
        for (k, c) in o {
            let Some(h) = c.as_str().and_then(|c| c.strip_prefix('#')).filter(|h| h.len() == 6) else { continue };
            let part = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
            if let (Some(r), Some(g), Some(b)) = (part(0), part(2), part(4)) {
                tile_colors.insert(k.clone(), (r, g, b));
            }
        }
    }
    Ok(Catalog { name: if s(&v, "name").is_empty() { from.to_string() } else { s(&v, "name") }, apps, featured, tile_colors })
}

/// Omarchy's marketplace registry: a long list of plugin repositories. Listed for browsing; each is unreviewed and runs
/// unsandboxed in Omarchy's own shell, so there is no install button until a sandboxed host exists.
pub fn parse_omarchy(text: &str, from: &str) -> Result<Catalog, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not an Omarchy registry: {e}"))?;
    let sources = v.get("sources").and_then(|a| a.as_array()).ok_or("no \"sources\" list in this file")?;
    let mut apps = vec![];
    for src in sources {
        let repo = s(src, "repo");
        let caps = src.get("automatedSecurityBaseline").map(|b| strings(&b["capabilities"])).unwrap_or_default();
        let plugins = src.get("plugins").and_then(|p| p.as_object());
        let Some(plugins) = plugins else { continue };
        for (pid, p) in plugins {
            let tags = strings(&p["tags"]);
            let cat = s(p, "category");
            let games = tags.iter().any(|t| t == "games");
            let name = pid.rsplit('.').next().unwrap_or(pid).replace(['-', '_'], " ");
            let mut note = String::from("An Omarchy shell plugin (QML for Quickshell). It runs unsandboxed with your user's permissions in Omarchy's shell, and the listing's checks are a pattern scan, not an audit.");
            if !caps.is_empty() {
                note.push_str(&format!(" Flagged capabilities: {}.", caps.join(", ")));
            }
            apps.push(Entry {
                id: pid.clone(),
                name,
                subtitle: {
                    let owner = pid.rsplit_once('.').map(|x| x.0).unwrap_or("");
                    let base = if cat.is_empty() { "Omarchy plugin".to_string() } else { format!("Omarchy plugin: {cat}") };
                    if owner.is_empty() { base } else { format!("{base}  \u{b7}  {owner}") }
                },
                description: format!("{} Source: {repo}", if tags.is_empty() { String::new() } else { format!("Tags: {}.", tags.join(", ")) }),
                category: if games { "Play".into() } else { "Plugins".into() },
                subcategory: if games { "Plugin games".into() } else if cat.is_empty() { "Other".into() } else { cat },
                source: Source::Plugin { repo: repo.clone() },
                icon: String::new(),
                reviewed: false,
                note,
                tags,
                from: from.to_string(),
                launch: String::new(),
                appstream: String::new(),
                flathub: String::new(),
                editions: vec![],
                preinstalled: false,
                repo: String::new(),
            });
        }
    }
    Ok(Catalog { name: "Omarchy plugins".into(), apps, featured: vec![], tile_colors: Default::default() })
}

// ------------------------------------------------------------------ repositories the user added

#[derive(Clone, Debug, PartialEq)]
pub struct Repo {
    pub name: String,
    pub url: String,
    /// "labyrinth" (a catalog file) or "omarchy" (the plugin registry)
    pub kind: String,
    pub enabled: bool,
}

pub const OMARCHY_URL: &str = "https://raw.githubusercontent.com/omacom/omarchy-plugin-marketplace/main/registry.json";

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).unwrap_or_default().join("labyrinth/market")
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))).unwrap_or_default().join("labyrinth/market")
}

pub fn load_repos() -> Vec<Repo> {
    let text = std::fs::read_to_string(config_dir().join("repos.json")).unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    v.as_array()
        .map(|a| a.iter().map(|r| Repo { name: s(r, "name"), url: s(r, "url"), kind: if s(r, "kind") == "omarchy" { "omarchy".into() } else { "labyrinth".into() }, enabled: r.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true) }).filter(|r| r.url.starts_with("https://")).collect())
        .unwrap_or_default()
}

pub fn save_repos(repos: &[Repo]) {
    let v: Vec<Value> = repos.iter().map(|r| json!({"name": r.name, "url": r.url, "kind": r.kind, "enabled": r.enabled})).collect();
    let d = config_dir();
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::write(d.join("repos.json"), serde_json::to_string_pretty(&v).unwrap_or_default());
}

fn cache_path(url: &str) -> PathBuf {
    // a stable file name from the address
    let mut h: u64 = 1469598103934665603;
    for b in url.bytes() {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    cache_dir().join(format!("{h:016x}.json"))
}

/// Download a catalog (HTTPS only, size-limited) into the cache; returns its text.
pub fn fetch(url: &str) -> Result<String, String> {
    if !url.starts_with("https://") {
        return Err("only https:// addresses are accepted".into());
    }
    let out = std::process::Command::new("curl").args(["-fsSL", "--proto", "=https", "--max-time", "60", "--max-filesize", "40000000", url]).output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err("could not download it".into());
    }
    let text = String::from_utf8(out.stdout).map_err(|_| "not text".to_string())?;
    let p = cache_path(url);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, &text);
    Ok(text)
}

/// The last download of a repository, if any.
pub fn cached(url: &str) -> Option<String> {
    std::fs::read_to_string(cache_path(url)).ok()
}

pub fn load_repo(r: &Repo, text: &str) -> Result<Catalog, String> {
    if r.kind == "omarchy" { parse_omarchy(text, &r.name) } else { parse_catalog(text, &r.name, false) }
}

// ------------------------------------------------------------------ installed state

#[derive(Default, Clone)]
pub struct Installed {
    pub pacman: HashSet<String>,
    pub flatpak: HashSet<String>,
    /// component id -> its package names
    pub components: Vec<(String, Vec<String>)>,
    /// package names with an update waiting
    pub updates: Vec<String>,
}

fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

pub fn read_installed() -> Installed {
    let pacman: HashSet<String> = run("pacman", &["-Qq"]).split_whitespace().map(str::to_string).collect();
    let flatpak: HashSet<String> = run("flatpak", &["list", "--app", "--columns=application"]).split_whitespace().map(str::to_string).collect();
    let comps = std::fs::read_to_string("/usr/share/labyrinth/components.json").unwrap_or_default();
    let v: Value = serde_json::from_str(&comps).unwrap_or(Value::Null);
    let components = v.get("components").and_then(|c| c.as_array()).map(|c| c.iter().map(|x| {
        let mut p = strings(&x["packages"]);
        if let Some(b) = x.get("pkgbuild").and_then(|b| b.as_str()) {
            p.push(b.to_string());
        }
        (s(x, "id"), p)
    }).collect()).unwrap_or_default();
    // updates are looked for separately (they need the network): see check_updates
    Installed { pacman, flatpak, components, updates: vec![] }
}

impl Installed {
    pub fn has(&self, e: &Entry) -> bool {
        match &e.source {
            Source::Pacman(p) | Source::Aur(p) => p.iter().all(|x| self.pacman.contains(x)),
            Source::Flatpak(a) => self.flatpak.contains(a),
            Source::Component(id) => self.components.iter().find(|c| &c.0 == id).map(|c| !c.1.is_empty() && c.1.iter().all(|p| self.pacman.contains(p))).unwrap_or(false),
            Source::Plugin { .. } => false,
        }
    }

    pub fn update_for(&self, e: &Entry) -> bool {
        match &e.source {
            Source::Pacman(p) | Source::Aur(p) => p.iter().any(|x| self.updates.contains(x)),
            _ => false,
        }
    }
}

/// The command (a shell script) that installs an entry, run in a terminal.
pub fn install_script(e: &Entry) -> Option<String> {
    Some(match &e.source {
        Source::Pacman(p) => format!("sudo pacman -S --needed {}", p.join(" ")),
        Source::Aur(p) => format!("yay -S --needed {}", p.join(" ")),
        Source::Flatpak(a) => format!("flatpak install -y flathub {a}"),
        Source::Component(c) => format!("labyrinth-apps install {c}"),
        Source::Plugin { .. } => return None,
    })
}

pub fn remove_script(e: &Entry) -> Option<String> {
    Some(match &e.source {
        Source::Pacman(p) | Source::Aur(p) => format!("sudo pacman -Rs {}", p.join(" ")),
        Source::Flatpak(a) => format!("flatpak uninstall {a}"),
        Source::Component(c) => format!("labyrinth-apps remove {c}"),
        Source::Plugin { .. } => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub name: String,
    pub old: String,
    pub new: String,
    pub aur: bool,
}

/// Programs built here from Labyrinth's own build files (Sarina, Heimdall, Limbo ...): they share a name with unrelated AUR packages
/// (the AUR has a different "heimdall-git"), so they are never updated from the AUR.
pub fn local_builds() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir("/usr/share/labyrinth/pkgbuilds").map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect()).unwrap_or_default();
    // and the list built into the catalog (Labyrinth's own build files), for systems where that folder is not complete
    if let Ok(c) = serde_json::from_str::<Value>(BUILTIN) {
        v.extend(strings(&c["local_builds"]));
    }
    // the same programs as upstream (VS Code, Proton Pass): their updates do come from the AUR
    if let Ok(c) = serde_json::from_str::<Value>(BUILTIN) {
        let ok = strings(&c["aur_updates_ok"]);
        v.retain(|n| !ok.contains(n));
    }
    v.sort();
    v.dedup();
    v
}

fn parse_updates(text: &str, aur: bool) -> Vec<Update> {
    text.lines().filter_map(|l| {
        let mut it = l.split_whitespace();
        let (name, old, arrow, new) = (it.next()?, it.next()?, it.next()?, it.next()?);
        (arrow == "->").then(|| Update { name: name.to_string(), old: old.to_string(), new: new.to_string(), aur })
    }).collect()
}

fn newer(new: &str, old: &str) -> bool {
    std::process::Command::new("vercmp").args([new, old]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1").unwrap_or(true)
}

/// What can be updated right now. The repositories are asked afresh (a private copy of the sync databases is refreshed without root, like
/// checkupdates does), the AUR through yay; Labyrinth's own builds and anything that would go backwards are left out. Needs the network.
pub fn check_updates() -> Vec<Update> {
    let mut out: Vec<Update> = vec![];
    let run_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp")).join("labyrinth-market");
    let tmp = run_dir.join(format!("updates-db-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let made = std::fs::create_dir_all(tmp.join("sync")).is_ok() && std::os::unix::fs::symlink("/var/lib/pacman/local", tmp.join("local")).is_ok();
    if made {
        if let Ok(rd) = std::fs::read_dir("/var/lib/pacman/sync") {
            for e in rd.flatten() {
                let _ = std::fs::copy(e.path(), tmp.join("sync").join(e.file_name()));
            }
        }
        let db = tmp.to_string_lossy().into_owned();
        // A copy of pacman.conf that does not check signatures: without root the keyring cannot be read, so repositories that
        // require signed databases (CachyOS's) would fail. This only lists names and versions; the real update still checks everything.
        let conf: String = std::fs::read_to_string("/etc/pacman.conf").unwrap_or_default().lines().filter(|l| !l.trim_start().starts_with("SigLevel") && !l.trim_start().starts_with("LocalFileSigLevel")).map(|l| if l.trim() == "[options]" { "[options]\nSigLevel = Never".to_string() } else { l.to_string() }).collect::<Vec<_>>().join("\n");
        let conf_path = tmp.join("pacman.conf");
        let _ = std::fs::write(&conf_path, conf);
        let cf = conf_path.to_string_lossy().into_owned();
        let _ = std::process::Command::new("fakeroot").args(["--", "pacman", "-Sy", "--config", &cf, "--dbpath", &db, "--logfile", "/dev/null"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        out = parse_updates(&run("pacman", &["-Qu", "--config", &cf, "--dbpath", &db]), false);
    } else {
        out = parse_updates(&run("pacman", &["-Qu"]), false);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    let skip = local_builds();
    for u in parse_updates(&run("yay", &["-Qua"]), true) {
        if !skip.contains(&u.name) && newer(&u.new, &u.old) && !out.iter().any(|x| x.name == u.name) {
            out.push(u);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Distinct sub-categories of a category, sorted.
pub fn subcategories(apps: &[Entry], category: &str) -> Vec<String> {
    let set: BTreeSet<String> = apps.iter().filter(|a| a.category == category && !a.subcategory.is_empty()).map(|a| a.subcategory.clone()).collect();
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_catalog_parses_and_every_app_has_a_category() {
        let c = parse_catalog(BUILTIN, "Labyrinth", true).unwrap();
        assert!(c.apps.len() > 50);
        assert!(c.apps.iter().all(|a| !a.category.is_empty() && !a.subcategory.is_empty() && a.reviewed || !a.reviewed));
        assert!(c.featured.iter().all(|(id, _, _)| c.apps.iter().any(|a| &a.id == id)), "featured apps exist");
    }

    #[test]
    fn a_repository_cannot_use_components_or_odd_package_names() {
        let t = r#"{"name":"x","apps":[
          {"id":"a","name":"A","category":"Work","source":{"type":"component","id":"steam"}},
          {"id":"b","name":"B","category":"Work","source":{"type":"pacman","packages":["ok-name","bad; rm -rf ~"]}},
          {"id":"c","name":"C","category":"Work","reviewed":true,"source":{"type":"pacman","packages":["fine"]}}]}"#;
        let c = parse_catalog(t, "x", false).unwrap();
        assert_eq!(c.apps.len(), 1);
        assert_eq!(c.apps[0].id, "c");
        assert!(!c.apps[0].reviewed, "an added repository cannot mark itself reviewed");
    }

    #[test]
    fn omarchy_plugins_are_listed_but_not_installable() {
        let t = r#"{"sources":[{"repo":"https://github.com/a/b","plugins":{"io.github.a.snake":{"category":"Widgets","tags":["games","quickshell"]}}}]}"#;
        let c = parse_omarchy(t, "Omarchy").unwrap();
        assert_eq!(c.apps[0].category, "Play");
        assert!(install_script(&c.apps[0]).is_none());
    }

    #[test]
    fn chymaera_only_apps_are_not_listed_in_labyrinth() {
        let c = parse_catalog(BUILTIN, "Labyrinth", true).unwrap();
        let lab = for_edition(c.clone(), "labyrinth");
        let chy = for_edition(c, "chymaera");
        assert!(lab.apps.iter().all(|a| a.id != "ghidra" && a.id != "wireshark"));
        assert!(chy.apps.iter().any(|a| a.id == "ghidra") && chy.apps.iter().any(|a| a.id == "krita"));
        assert!(!lab.apps.iter().any(|a| a.description.to_lowercase().contains("pentest") || a.subtitle.to_lowercase().contains("pentest")), "nothing in the Labyrinth listing talks about penetration testing");
    }

    #[test]
    fn update_lines_are_read() {
        let u = parse_updates("krita 5.3.4-1 -> 5.3.5-1\nvivaldi 7.0-1 -> 7.1-1 [3d]\nnoise\n", false);
        assert_eq!(u.len(), 2);
        assert_eq!((u[0].name.as_str(), u[0].old.as_str(), u[0].new.as_str()), ("krita", "5.3.4-1", "5.3.5-1"));
    }

    #[test]
    fn install_commands() {
        let c = parse_catalog(BUILTIN, "Labyrinth", true).unwrap();
        let e = c.apps.iter().find(|a| a.id == "krita").unwrap();
        assert_eq!(install_script(e).unwrap(), "sudo pacman -S --needed krita");
        let a = c.apps.iter().find(|a| a.id == "pdfcraft").unwrap();
        assert_eq!(install_script(a).unwrap(), "yay -S --needed pdfcraft-bin");
    }
}
