//! The plan, checked before anything is done (check_plan): an error message, or None when it can be installed. The patterns
//! are matched by hand, whole (the script's fullmatch), on ASCII as its character classes were.

use crate::catalog;
use crate::conf::Os;
use crate::py;
use crate::pyjson::{Py, NONE};

pub const MIB: i128 = 1024 * 1024;
/// manual partitioning: what each mount point may be formatted as
pub const MOUNTS: [(&str, &[&str]); 7] = [
    ("/", &["btrfs", "ext4", "xfs"]),
    ("/home", &["btrfs", "ext4", "xfs"]),
    ("/var", &["btrfs", "ext4", "xfs"]),
    ("/srv", &["btrfs", "ext4", "xfs"]),
    ("/opt", &["btrfs", "ext4", "xfs"]),
    ("/boot/efi", &["vfat"]),
    ("swap", &["swap"]),
];
/// accounts the live medium or the login screen use, whatever the OS
pub const RESERVED: [&str; 3] = ["root", "live", "greeter"];

pub fn formats_of(mount: &str) -> Option<&'static [&'static str]> {
    MOUNTS.iter().find(|(m, _)| *m == mount).map(|(_, f)| *f)
}

fn all(s: &str, ok: impl Fn(u8) -> bool) -> bool {
    s.bytes().all(ok)
}

fn word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// [a-z_][a-z0-9_-]{0,31}
pub fn login_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && all(&s[1..], |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// [A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?
pub fn hostname_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && all(s, |c| c.is_ascii_alphanumeric() || c == b'-')
}

/// [A-Za-z_]+(\.[A-Za-z0-9-]+)?(@[a-z]+)?
pub fn locale_ok(s: &str) -> bool {
    let b = s.as_bytes();
    let run = |mut i: usize, ok: &dyn Fn(u8) -> bool| {
        while i < b.len() && ok(b[i]) {
            i += 1;
        }
        i
    };
    let mut i = run(0, &|c| c.is_ascii_alphabetic() || c == b'_');
    if i == 0 {
        return false;
    }
    if i < b.len() && b[i] == b'.' {
        let j = run(i + 1, &|c| c.is_ascii_alphanumeric() || c == b'-');
        if j == i + 1 {
            return false;
        }
        i = j;
    }
    if i < b.len() && b[i] == b'@' {
        let j = run(i + 1, &|c| c.is_ascii_lowercase());
        if j == i + 1 {
            return false;
        }
        i = j;
    }
    i == b.len()
}

/// [A-Za-z0-9_-]+
pub fn keymap_ok(s: &str) -> bool {
    !s.is_empty() && all(s, word)
}

/// /dev/[A-Za-z0-9/_-]+
pub fn device_ok(s: &str) -> bool {
    s.strip_prefix("/dev/").is_some_and(|rest| !rest.is_empty() && all(rest, |c| word(c) || c == b'/'))
}

/// [A-Za-z0-9_-]{1,16}
pub fn part_id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 16 && all(s, word)
}

/// fits(pattern, v): v is text and all of it fits.
fn fits(v: &Py, ok: fn(&str) -> bool) -> bool {
    v.as_str().is_some_and(ok)
}

/// v.get(key, default) of an object that has been made sure of.
fn get<'a>(p: &'a Py, key: &str, default: &'a Py) -> &'a Py {
    p.get(key).unwrap_or(default)
}

/// Whether an account name belongs to the system: the live medium's accounts are the new system's too (`exists` asks the
/// account database, as pwd.getpwnam did).
pub fn login_taken(os: &Os, login: &str, exists: &mut dyn FnMut(&str) -> bool) -> bool {
    RESERVED.contains(&login) || os.reserved_extra.contains(login) || exists(login)
}

/// What check_plan needs from the computer: the zoneinfo folder and the account database.
pub struct Ask<'a> {
    pub zoneinfo: &'a str,
    pub account_exists: &'a mut dyn FnMut(&str) -> bool,
}

/// The plan, checked: an error message, or None when it can be installed. A value of the wrong kind (a number where text
/// belongs, say) gets the message of that value.
pub fn check_plan(p: &Py, os: &Os, ask: &mut Ask) -> Option<String> {
    let empty = Py::Str(String::new());
    if p.as_dict().is_none() {
        return Some("the plan is not a JSON object".into());
    }
    let mode = p.at("mode").as_str();
    if mode != Some("erase") && mode != Some("manual") {
        return Some("unknown way to install (erase or manual)".into());
    }
    if !fits(get(p, "disk", &empty), device_ok) {
        return Some("no disk chosen".into());
    }
    if mode == Some("manual") {
        if let Some(problem) = check_manual(p) {
            return Some(problem);
        }
    }
    let empty_user = Py::Dict(Default::default());
    let u = if p.at("user").as_dict().is_some() { p.at("user") } else { &empty_user };
    let (login, password, name) = (get(u, "login", &empty), get(u, "password", &empty), get(u, "name", &empty));
    let Some(login) = login.as_str().filter(|l| login_ok(l)) else {
        return Some("the account name must start with a small letter and use only small letters, digits, - and _".into());
    };
    if login_taken(os, login, ask.account_exists) {
        return Some("that account name is taken by the system".into());
    }
    let Some(password) = password.as_str().filter(|pw| !pw.is_empty()) else {
        return Some("the account needs a password".into());
    };
    // chpasswd reads "login:password" lines: after a line break the rest would be another account's line (root's, say)
    if password.contains(['\r', '\n', '\0']) {
        return Some("the account's password may not contain a line break".into());
    }
    if name.as_str().is_none_or(|n| n.contains([':', '\n', '\0'])) {
        return Some("the full name may not contain : or a line break".into());
    }
    if !fits(get(p, "hostname", &empty), hostname_ok) {
        return Some("the computer's name may use letters, digits and - (not at the start or end)".into());
    }
    if p.at("encrypt").truthy() {
        match get(p, "encrypt_password", &empty).as_str() {
            Some(secret) if secret.chars().count() >= 8 => {
                // typed in GRUB, where Enter ends it: one with a line break could never be typed
                if secret.contains(['\r', '\n', '\0']) {
                    return Some("the encryption password may not contain a line break".into());
                }
            }
            _ => return Some("the encryption password needs at least 8 characters".into()),
        }
    }
    let tz_ok = get(p, "timezone", &empty).as_str().is_some_and(|tz| {
        // (os.path.join drops the folder before an absolute name)
        !tz.is_empty() && !tz.starts_with('/') && !tz.contains("..") && py::isfile(&py::join(ask.zoneinfo, tz))
    });
    if !tz_ok {
        return Some("unknown time zone".into());
    }
    if !fits(get(p, "locale", &empty), locale_ok) {
        return Some("unknown language setting".into());
    }
    if !fits(get(p, "keymap", &empty), keymap_ok) {
        return Some("unknown keyboard layout".into());
    }
    if p.at("keymap_variant").truthy() && !fits(p.at("keymap_variant"), keymap_ok) {
        return Some("unknown keyboard variant".into());
    }
    if let Some(chosen) = p.get("browsers") {
        let known: Vec<String> = catalog::entries(os.browsers_file.as_deref(), "browsers").into_iter().map(|b| catalog::id(&b)).collect();
        let fine = chosen.as_list().is_some_and(|l| l.iter().all(|b| b.as_str().is_some_and(|b| known.iter().any(|k| k == b))));
        if !fine {
            return Some("unknown browser chosen".into());
        }
        let default = p.at("default_browser");
        if default.truthy() && !chosen.as_list().unwrap_or(&[]).iter().any(|c| c == default) {
            return Some("the default browser must be one of those chosen".into());
        }
    }
    if let Some(chosen) = p.get("extras") {
        let known: Vec<String> = catalog::entries(os.extras_file.as_deref(), "extras").into_iter().map(|e| catalog::id(&e)).collect();
        if !chosen.as_list().is_some_and(|l| l.iter().all(|e| e.as_str().is_some_and(|e| known.iter().any(|k| k == e)))) {
            return Some("unknown optional part chosen".into());
        }
    }
    None
}

/// A list in the plan read as the script reads it: `p.get(key) or []`, Err(()) when it is something else that is true.
fn list_or_empty<'a>(p: &'a Py, key: &str) -> Result<&'a [Py], ()> {
    let v = p.at(key);
    if !v.truthy() {
        return Ok(&[]);
    }
    v.as_list().ok_or(())
}

/// The shape of a manual plan (the disk itself is checked in preflight): an error, or None. Something that is not an object
/// where an object belongs reads as an empty one.
pub fn check_manual(p: &Py) -> Option<String> {
    // only true or false: "false" (text) was true, and a new table erases every partition on the disk
    if !matches!(p.at("new_table"), Py::None | Py::Bool(_)) {
        return Some("a new partition table is chosen with true or false".into());
    }
    let Ok(creates) = list_or_empty(p, "create") else {
        return Some("a new partition is not described properly".into());
    };
    let mut ids: Vec<&str> = vec![];
    for c in creates {
        let c = if c.as_dict().is_some() { c } else { &NONE };
        let id = c.at("id").as_str().filter(|i| part_id_ok(i));
        let Some(id) = id.filter(|i| !ids.contains(i)) else {
            return Some("a new partition has no proper id".into());
        };
        ids.push(id);
        let kind_ok = matches!(c.at("kind").as_str(), Some("linux" | "efi" | "swap"));
        if !kind_ok || c.at("start").whole().is_none() || c.at("size").whole().is_none_or(|s| s < MIB) {
            return Some("a new partition is not described properly".into());
        }
    }
    match list_or_empty(p, "delete") {
        Ok(deletes) if deletes.iter().all(|n| n.whole().is_some_and(|n| n > 0)) => {}
        _ => return Some("a partition to delete is not a number".into()),
    }
    let uses: Vec<&Py> = match p.at("use") {
        v if !v.truthy() => vec![],
        Py::List(l) => l.iter().map(|u| if u.as_dict().is_some() { u } else { &NONE }).collect(),
        _ => vec![],
    };
    let empty = Py::Str(String::new());
    for u in &uses {
        let (m, fmt) = (u.at("mount"), u.at("format"));
        let part = get(u, "part", &empty).py_str();
        let Some(formats) = m.as_str().and_then(formats_of) else {
            return Some(format!("{} cannot be chosen as a mount point", m.py_str()));
        };
        if *fmt != Py::None && !fmt.as_str().is_some_and(|f| formats.contains(&f)) {
            return Some(format!("{} cannot be formatted as {}", m.py_str(), fmt.py_str()));
        }
        if let Some(id) = part.strip_prefix("new:") {
            if !ids.contains(&id) {
                return Some(format!("{part} is not one of the new partitions"));
            }
            if *fmt == Py::None {
                return Some("a new partition has to be formatted".into());
            }
        } else if !device_ok(&part) {
            return Some("a partition is not named properly".into());
        }
    }
    let mounts: Vec<&str> = uses.iter().map(|u| u.at("mount").as_str().unwrap_or("")).collect();
    if mounts.iter().filter(|m| **m == "/").count() != 1 {
        return Some("choose exactly one partition for / (the system)".into());
    }
    if mounts.iter().filter(|m| **m == "/boot/efi").count() != 1 {
        return Some("choose an EFI system partition for /boot/efi".into());
    }
    let mut not_swap: Vec<&str> = mounts.iter().copied().filter(|m| *m != "swap").collect();
    let n = not_swap.len();
    not_swap.sort();
    not_swap.dedup();
    if not_swap.len() != n {
        return Some("a mount point is chosen twice".into());
    }
    if uses.iter().find(|u| u.at("mount").as_str() == Some("/")).is_some_and(|u| *u.at("format") == Py::None) {
        return Some("the partition for / has to be formatted (the system is copied onto an empty one)".into());
    }
    let mut parts: Vec<&str> = uses.iter().map(|u| u.at("part").as_str().unwrap_or("")).collect();
    parts.sort();
    parts.dedup();
    if parts.len() != uses.len() {
        return Some("a partition is used twice".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_patterns() {
        for (s, want) in [
            ("a", true),
            ("_x-1", true),
            ("A", false),
            ("1a", false),
            (&"a".repeat(32) as &str, true),
            (&"a".repeat(33), false),
            ("ada\n", false),
            ("", false),
        ] {
            assert_eq!(login_ok(s), want, "{s:?}");
        }
        for (s, want) in [
            ("x", true),
            ("my-pc", true),
            ("-x", false),
            ("x-", false),
            (&"a".repeat(63) as &str, true),
            (&"a".repeat(64), false),
            ("x\n", false),
            ("x.y", false),
        ] {
            assert_eq!(hostname_ok(s), want, "{s:?}");
        }
        for (s, want) in [
            ("en_US.UTF-8", true),
            ("de_DE@euro", true),
            ("C", true),
            ("en_US.UTF-8@euro", true),
            ("en_US.", false),
            ("en_US@", false),
            ("en_US@Euro", false),
            (".x", false),
            ("en_US.UTF-8\n", false),
            ("é", false),
        ] {
            assert_eq!(locale_ok(s), want, "{s:?}");
        }
        for (s, want) in
            [("/dev/vda", true), ("/dev/", false), ("/dev//x", true), ("/dev/a.b", false), ("dev/a", false), ("/dev/a\n", false)]
        {
            assert_eq!(device_ok(s), want, "{s:?}");
        }
        assert!(part_id_ok("a-_9") && !part_id_ok("") && !part_id_ok(&"a".repeat(17)) && !part_id_ok("a b"));
        assert!(keymap_ok("us") && !keymap_ok("dvorak; rm") && !keymap_ok(""));
    }
}
