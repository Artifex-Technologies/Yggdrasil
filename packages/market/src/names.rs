//! Look-alike names in Agora's listings: an app whose name imitates another ("Vivаldi" with a Cyrillic а, fullwidth or
//! mathematical letters, invisible or direction-changing characters: yggdrasil_ui::lookalike), and an unreviewed listing (a
//! repository you added, Omarchy's plugin registry) that looks like an app of the operating system's reviewed list or takes its
//! very name. Such a listing gets "Look-alike name" in the lists and, on its page, the reason in plain words and what it really
//! is: where it installs from and under which package or id, and the program it starts when it is installed. Shown while the
//! privacy pack's `lookalike_warnings` is on (Settings > Security > Privacy; yggdrasil_ui::privacy_prefs).

use crate::catalog::{Entry, Source};
use yggdrasil_ui::lookalike;

/// Why a listing's name is marked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flag {
    /// The reason in plain words, one or two sentences.
    pub words: String,
}

/// The flag of each listing, by index (None: nothing to say). `os` is the reviewed list's owner ("Labyrinth OS").
pub fn flags(apps: &[Entry], os: &str) -> Vec<Option<Flag>> {
    let reviewed: Vec<&Entry> = apps.iter().filter(|a| a.reviewed).collect();
    apps.iter()
        .map(|e| {
            let r = lookalike::check(&e.name);
            let mut words = r.words();
            if !e.reviewed {
                // compared with the reviewed list: a different listing that looks like one of its apps, or has its very name
                if let Some(o) = reviewed.iter().find(|o| o.id != e.id && lookalike::confusable(&e.name, &o.name)) {
                    words = join(&words, &format!("It looks like \"{}\" from {os}'s list, but it is a different listing.", o.name));
                } else if let Some(o) = reviewed.iter().find(|o| o.name == e.name && (o.id != e.id || o.from != e.from)) {
                    words = join(&words, &format!("It has the same name as \"{}\" from {os}'s list, but comes from {}.", o.name, e.from));
                }
            }
            (!words.is_empty()).then_some(Flag { words })
        })
        .collect()
}

fn join(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        format!("{a} {b}")
    }
}

/// What a listing really installs, in plain words ("AUR package vivaldi-snapshot", "Flatpak com.example.App").
pub fn what_it_is(e: &Entry) -> String {
    match &e.source {
        Source::Pacman(p) => format!("{} {}", if p.len() == 1 { "package" } else { "packages" }, p.join(", ")),
        Source::Aur(p) => format!("AUR {} {}", if p.len() == 1 { "package" } else { "packages" }, p.join(", ")),
        Source::Flatpak(id) => format!("Flatpak {id}"),
        Source::Component(id) => format!("the system's optional component {id}"),
        Source::Plugin { repo } => format!("the plugin repository {repo}"),
    }
}

/// The program an installed desktop entry starts: its Exec line's first word (after `env` and its settings), found on PATH.
pub fn program_of_exec(exec: &str) -> String {
    let mut words = exec.split_whitespace().map(|w| w.trim_matches('"'));
    let mut first = words.next().unwrap_or_default();
    if first == "env" || first.ends_with("/env") {
        first = words.find(|w| !w.contains('=') && !w.starts_with('-')).unwrap_or_default();
    }
    if first.is_empty() || first.contains('/') {
        return first.to_string();
    }
    std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).map(|d| d.join(first)).find(|c| c.is_file()))
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|| first.to_string())
}

/// The Exec line of an installed desktop entry `id` (in the usual folders), if there is one.
pub fn exec_of(id: &str) -> Option<String> {
    let data = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let mut dirs: Vec<std::path::PathBuf> = data.split(':').filter(|d| !d.is_empty()).map(std::path::PathBuf::from).collect();
    if let Some(h) = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
    {
        dirs.insert(0, h);
    }
    dirs.push("/var/lib/flatpak/exports/share".into());
    let text = dirs.iter().find_map(|d| std::fs::read_to_string(d.join("applications").join(format!("{id}.desktop"))).ok())?;
    let mut inside = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            inside = l == "[Desktop Entry]";
        } else if inside {
            if let Some(v) = l.strip_prefix("Exec=") {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, reviewed: bool, from: &str, source: Source) -> Entry {
        Entry {
            id: id.into(),
            name: name.into(),
            subtitle: String::new(),
            description: String::new(),
            category: String::new(),
            subcategory: String::new(),
            source,
            icon: String::new(),
            reviewed,
            note: String::new(),
            tags: vec![],
            from: from.into(),
            launch: String::new(),
            appstream: String::new(),
            flathub: String::new(),
            editions: vec![],
            preinstalled: false,
            repo: String::new(),
            logo: String::new(),
            icon_url: String::new(),
            recommends: vec![],
        }
    }

    #[test]
    fn look_alikes_are_marked_and_ordinary_names_are_not() {
        let apps = vec![
            entry("vivaldi", "Vivaldi", true, "Labyrinth OS", Source::Pacman(vec!["vivaldi".into()])),
            entry("gimp", "GIMP", true, "Labyrinth OS", Source::Pacman(vec!["gimp".into()])),
            entry("viv2", "Viv\u{0430}ldi", false, "My repo", Source::Aur(vec!["vivaldi-free".into()])),
            entry("ff", "Fire\u{200B}fox", false, "My repo", Source::Flatpak("org.example.Fox".into())),
            entry("g2", "G1MP", false, "My repo", Source::Aur(vec!["gimp-plus".into()])),
            entry("v3", "Vivaldi", false, "My repo", Source::Aur(vec!["vivaldi-snapshot".into()])),
            entry("notes", "Блокнот", false, "My repo", Source::Pacman(vec!["notes".into()])),
            entry("own", "Krita", false, "My repo", Source::Pacman(vec!["krita".into()])),
        ];
        let f = flags(&apps, "Labyrinth OS");
        assert!(f[0].is_none() && f[1].is_none(), "the reviewed apps themselves");
        let w = &f[2].as_ref().unwrap().words;
        assert!(w.contains("Latin and Cyrillic") && w.contains("looks like \"Vivaldi\" from Labyrinth OS's list"), "{w}");
        assert!(f[3].as_ref().unwrap().words.contains("invisible"));
        assert!(f[4].as_ref().unwrap().words.contains("looks like \"GIMP\""), "ASCII look-alikes against the reviewed list");
        assert!(f[5].as_ref().unwrap().words.contains("same name as \"Vivaldi\""), "the very name, from another source");
        assert!(f[6].is_none(), "a real name in another alphabet");
        assert!(f[7].is_none(), "no reviewed app of that name");
    }

    #[test]
    fn what_a_listing_really_is() {
        let e = |s| entry("x", "X", false, "r", s);
        assert_eq!(what_it_is(&e(Source::Aur(vec!["vivaldi-snapshot".into()]))), "AUR package vivaldi-snapshot");
        assert_eq!(what_it_is(&e(Source::Pacman(vec!["a".into(), "b".into()]))), "packages a, b");
        assert_eq!(what_it_is(&e(Source::Flatpak("com.example.App".into()))), "Flatpak com.example.App");
        assert_eq!(program_of_exec("env A=1 /opt/x/run %U"), "/opt/x/run");
        assert_eq!(program_of_exec("/usr/bin/gimp %U"), "/usr/bin/gimp");
    }
}
