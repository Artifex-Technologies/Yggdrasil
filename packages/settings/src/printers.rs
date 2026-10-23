//! Settings > Printers: the printers CUPS knows, their state and ink or toner, their settings (paper, two-sided, quality), the print
//! queue, and adding the printers found on the network. Everything goes through CUPS's own tools (lpstat, lpoptions, lp, cancel, ipptool,
//! ippfind); adding and removing a printer asks for the administrator password (pkexec lpadmin). Every parser is a pure function of the
//! tool's output, tested without CUPS.

use std::collections::BTreeMap;

use crate::sys::{act, run};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marker {
    pub name: String,
    /// 0-100, or None when the printer does not report it (-1, -2, -3 in IPP)
    pub level: Option<u8>,
    pub color: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Choice {
    pub key: String,
    pub label: String,
    pub values: Vec<String>,
    pub current: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Printer {
    pub name: String,
    pub uri: String,
    /// idle, printing, stopped
    pub state: String,
    /// what the printer says about itself (low toner, paper jam), when anything
    pub message: String,
    pub default: bool,
    pub markers: Vec<Marker>,
    pub options: Vec<Choice>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Job {
    pub id: String,
    pub printer: String,
    pub owner: String,
    pub size: String,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    /// CUPS is installed and answering
    pub ready: bool,
    pub printers: Vec<Printer>,
    /// printers on the network (or on USB through ipp-usb) that are not added yet: (a name to show, its address)
    pub found: Vec<(String, String)>,
    pub jobs: Vec<Job>,
}

/// `lpstat -p`: name and state of each printer, with the indented reason lines that follow a stopped one.
pub fn parse_printers(text: &str) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = vec![];
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("printer ") {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            let state = if rest.contains(" is idle") {
                "idle"
            } else if rest.contains(" now printing") {
                "printing"
            } else if rest.contains("disabled") {
                "stopped"
            } else {
                "idle"
            };
            out.push((name, state.into(), String::new()));
        } else if line.starts_with('\t') || line.starts_with("  ") {
            if let Some(last) = out.last_mut() {
                let t = line.trim();
                if !t.is_empty() {
                    if !last.2.is_empty() {
                        last.2.push(' ');
                    }
                    last.2.push_str(t);
                }
            }
        }
    }
    out
}

/// `lpstat -d`: the default printer's name.
pub fn parse_default(text: &str) -> String {
    text.lines().find_map(|l| l.strip_prefix("system default destination: ")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// `lpstat -v`: each printer's address.
pub fn parse_devices(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("device for "))
        .filter_map(|l| l.split_once(": "))
        .map(|(n, u)| (n.trim().to_string(), u.trim().to_string()))
        .collect()
}

/// `lpstat -o`: the jobs waiting or printing.
pub fn parse_jobs(text: &str) -> Vec<Job> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 3 {
                return None;
            }
            let (printer, _) = f[0].rsplit_once('-')?;
            Some(Job { id: f[0].to_string(), printer: printer.to_string(), owner: f[1].to_string(), size: f[2].to_string() })
        })
        .collect()
}

/// `lpoptions -p NAME -l`: "Duplex/2-Sided Printing: *None DuplexNoTumble DuplexTumble" (the current choice has a *).
pub fn parse_options(text: &str) -> Vec<Choice> {
    text.lines()
        .filter_map(|l| {
            let (head, vals) = l.split_once(": ")?;
            let (key, label) = head.split_once('/').unwrap_or((head, head));
            let mut current = String::new();
            let values = vals
                .split_whitespace()
                .map(|v| {
                    if let Some(c) = v.strip_prefix('*') {
                        current = c.to_string();
                        c.to_string()
                    } else {
                        v.to_string()
                    }
                })
                .collect();
            Some(Choice { key: key.trim().to_string(), label: label.trim().to_string(), values, current })
        })
        .collect()
}

fn attr<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find_map(|l| {
        let t = l.trim();
        let rest = t.strip_prefix(name)?;
        if !rest.starts_with(" (") {
            return None;
        }
        rest.split_once(" = ").map(|(_, v)| v.trim())
    })
}

/// ipptool's verbose answer to Get-Printer-Attributes: the supplies (marker-names, marker-levels, marker-colors).
pub fn parse_markers(text: &str) -> Vec<Marker> {
    let names: Vec<String> = attr(text, "marker-names").map(|v| v.split(',').map(|s| s.trim().to_string()).collect()).unwrap_or_default();
    let levels: Vec<i32> = attr(text, "marker-levels").map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect()).unwrap_or_default();
    let colors: Vec<String> = attr(text, "marker-colors").map(|v| v.split(',').map(|s| s.trim().to_string()).collect()).unwrap_or_default();
    names
        .iter()
        .enumerate()
        .map(|(i, n)| Marker {
            name: n.clone(),
            level: levels.get(i).copied().filter(|l| (0..=100).contains(l)).map(|l| l as u8),
            color: colors.get(i).cloned().unwrap_or_default(),
        })
        .collect()
}

/// What `ippfind` printed (one address a line), minus the printers already added: (a readable name, the address).
pub fn parse_found(text: &str, known: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut out = vec![];
    for uri in text.lines().map(str::trim).filter(|l| l.starts_with("ipp://") || l.starts_with("ipps://")) {
        if known.values().any(|u| u == uri) || out.iter().any(|(_, u): &(String, String)| u == uri) {
            continue;
        }
        let host = uri.split("://").nth(1).unwrap_or("").split(['/', ':']).next().unwrap_or("").trim_end_matches(".local").replace("%20", " ");
        out.push((if host.is_empty() { uri.to_string() } else { host }, uri.to_string()));
    }
    out
}

/// A name CUPS accepts for a new printer (letters, digits, - and _), made from the shown name.
pub fn queue_name(shown: &str) -> String {
    let n: String = shown.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let n = n.trim_matches('_').to_string();
    if n.is_empty() { "Printer".into() } else { n.chars().take(60).collect() }
}

fn valid_queue(name: &str) -> bool {
    !name.is_empty() && name.len() <= 127 && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.@".contains(c))
}

const MARKERS_TEST: &str = "{\n OPERATION Get-Printer-Attributes\n GROUP operation-attributes-tag\n ATTR charset attributes-charset utf-8\n ATTR naturalLanguage attributes-natural-language en\n ATTR uri printer-uri $uri\n ATTR keyword requested-attributes marker-names,marker-levels,marker-colors,printer-state-message\n}\n";

fn markers_of(name: &str) -> Vec<Marker> {
    let path = std::env::temp_dir().join(format!("yggdrasil-printers-{}.test", std::process::id()));
    if std::fs::write(&path, MARKERS_TEST).is_err() {
        return vec![];
    }
    let out = run("ipptool", &["-tv", "-T", "4", &format!("ipp://localhost/printers/{name}"), path.to_str().unwrap_or("")]).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    parse_markers(&out)
}

pub fn load() -> State {
    let Some(status) = run("lpstat", &["-r"]) else { return State::default() };
    if !status.contains("is running") {
        return State { ready: false, ..Default::default() };
    }
    let default = run("lpstat", &["-d"]).map(|t| parse_default(&t)).unwrap_or_default();
    let devices = run("lpstat", &["-v"]).map(|t| parse_devices(&t)).unwrap_or_default();
    let printers = run("lpstat", &["-p"])
        .map(|t| parse_printers(&t))
        .unwrap_or_default()
        .into_iter()
        .map(|(name, state, message)| Printer {
            uri: devices.get(&name).cloned().unwrap_or_default(),
            default: name == default,
            markers: markers_of(&name),
            options: run("lpoptions", &["-p", &name, "-l"]).map(|t| parse_options(&t)).unwrap_or_default(),
            name,
            state,
            message,
        })
        .collect();
    let found = run("ippfind", &["-T", "3"]).map(|t| parse_found(&t, &devices)).unwrap_or_default();
    let jobs = run("lpstat", &["-o"]).map(|t| parse_jobs(&t)).unwrap_or_default();
    State { ready: true, printers, found, jobs }
}

pub fn set_default(name: &str) -> Result<(), String> {
    if !valid_queue(name) {
        return Err("not a printer name".into());
    }
    act("lpoptions", &["-d", name])
}

pub fn set_option(name: &str, key: &str, value: &str) -> Result<(), String> {
    let plain = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !valid_queue(name) || !plain(key) || !plain(value) {
        return Err("not a printer setting".into());
    }
    act("lpoptions", &["-p", name, "-o", &format!("{key}={value}")])
}

pub fn test_page(name: &str) -> Result<(), String> {
    if !valid_queue(name) {
        return Err("not a printer name".into());
    }
    act("lp", &["-d", name, "-t", "Test page", "/usr/share/cups/data/testprint"])
}

pub fn cancel(job: &str) -> Result<(), String> {
    if !job.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        return Err("not a job".into());
    }
    act("cancel", &[job])
}

/// Adds a printer found on the network as a driverless (IPP Everywhere) queue: no driver to choose.
pub fn add(shown: &str, uri: &str) -> Result<(), String> {
    if !(uri.starts_with("ipp://") || uri.starts_with("ipps://")) || uri.chars().any(|c| c.is_whitespace() || c == '\'' || c == '"') {
        return Err("not a printer address".into());
    }
    act("pkexec", &["lpadmin", "-p", &queue_name(shown), "-E", "-v", uri, "-m", "everywhere"])
}

pub fn remove(name: &str) -> Result<(), String> {
    if !valid_queue(name) {
        return Err("not a printer name".into());
    }
    act("pkexec", &["lpadmin", "-x", name])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printers_and_their_state() {
        let t = "printer Brother_HL is idle.  enabled since Wed 22 Oct 2026 10:00:00 AM\nprinter Office now printing Office-12.  enabled since Wed\nprinter Old disabled since Wed 22 Oct 2026 -\n\tPaused\n";
        let p = parse_printers(&t);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], ("Brother_HL".into(), "idle".into(), "".into()));
        assert_eq!(p[1].1, "printing");
        assert_eq!(p[2], ("Old".into(), "stopped".into(), "Paused".into()));
    }

    #[test]
    fn default_devices_jobs() {
        assert_eq!(parse_default("system default destination: Brother_HL\n"), "Brother_HL");
        assert_eq!(parse_default("no system default destination"), "");
        let d = parse_devices("device for Brother_HL: ipp://brother.local/ipp/print\ndevice for PDF: cups-pdf:/\n");
        assert_eq!(d["Brother_HL"], "ipp://brother.local/ipp/print");
        let j = parse_jobs("Brother_HL-12            zlothra          1024   Wed 22 Oct 2026 10:00:00 AM MDT\n");
        assert_eq!(j, vec![Job { id: "Brother_HL-12".into(), printer: "Brother_HL".into(), owner: "zlothra".into(), size: "1024".into() }]);
    }

    #[test]
    fn options_with_current_choice() {
        let o = parse_options("PageSize/Media Size: Letter *A4 Legal\nDuplex/2-Sided Printing: *None DuplexNoTumble DuplexTumble\n");
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].key, "PageSize");
        assert_eq!(o[0].label, "Media Size");
        assert_eq!(o[0].current, "A4");
        assert_eq!(o[1].values, vec!["None", "DuplexNoTumble", "DuplexTumble"]);
    }

    #[test]
    fn supplies() {
        let t = "        marker-names (1setOf nameWithoutLanguage) = Black Toner,Cyan Toner\n        marker-levels (1setOf integer) = 45,-3\n        marker-colors (1setOf nameWithoutLanguage) = #000000,#00FFFF\n";
        let m = parse_markers(t);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], Marker { name: "Black Toner".into(), level: Some(45), color: "#000000".into() });
        assert_eq!(m[1].level, None);
    }

    #[test]
    fn found_on_the_network() {
        let mut known = BTreeMap::new();
        known.insert("Office".to_string(), "ipp://office.local:631/ipp/print".to_string());
        let f = parse_found("ipp://office.local:631/ipp/print\nipps://Brother%20HL-L2350DW.local:443/ipp/print\nnot a uri\n", &known);
        assert_eq!(f, vec![("Brother HL-L2350DW".into(), "ipps://Brother%20HL-L2350DW.local:443/ipp/print".into())]);
        assert_eq!(queue_name("Brother HL-L2350DW"), "Brother_HL-L2350DW");
    }

    #[test]
    fn odd_input_is_refused() {
        assert!(set_default("a b").is_err());
        assert!(set_option("P", "Page;rm", "A4").is_err());
        assert!(add("x", "file:///etc/passwd").is_err());
        assert!(add("x", "ipp://a b").is_err());
        assert!(remove("").is_err());
        assert!(cancel("1;2").is_err());
    }
}
