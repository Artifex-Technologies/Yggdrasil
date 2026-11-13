//! The fingerprint reader (Settings > Users), through fprintd on the system bus, and fingerprint for sudo.
//!
//! fprintd and libfprint are an online component ("Fingerprint reader" in Agora), which the installer ticks when a reader
//! libfprint supports is plugged in. Enrolling is fprintd's own business (it asks for your password through the polkit agent).
//!
//! Fingerprint for sudo is off until you turn it on in Settings, and only the root helper changes PAM: `fingerprint sudo on` run
//! through pkexec (org.yggdrasil.fingerprint.sudo, an administrator's password every time) puts two lines at the top of
//! /etc/pam.d/sudo, a marker and `auth sufficient pam_fprintd.so`. "sufficient": a finger lets sudo in, and no finger (Ctrl+C,
//! the time running out, no reader) falls through to the password as before, so the line can never lock anyone out. `fingerprint
//! sudo off` takes exactly those two lines out again and nothing else. The lock screen, the login screen and the password boxes
//! do not use it (yet): see the plan in docs/DESKTOP.md, "Fingerprint reader".

use std::path::Path;

/// The marker line above ours (how the helper finds the line it wrote, and only that one).
pub const MARK: &str =
    "# Yggdrasil Settings: fingerprint for sudo (Settings > Users). Turn it off there, or remove this line and the next.";
/// The line itself.
pub const LINE: &str = "auth       sufficient   pam_fprintd.so";
/// fprintd's PAM module; without it the line would only slow sudo down.
pub const MODULE: &str = "/usr/lib/security/pam_fprintd.so";
/// The file changed.
pub const SUDO_PAM: &str = "/etc/pam.d/sudo";

/// Our two lines are there.
pub fn pam_has(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    lines.windows(2).any(|w| w[0] == MARK && w[1].trim() == LINE)
}

/// The file with our two lines before its first rule (after the `#%PAM-1.0` header and the comments). Refused for a file with
/// no `auth` rule: that is not a sudo file this helper understands.
pub fn pam_with(text: &str) -> Result<String, String> {
    if pam_has(text) {
        return Ok(text.to_string());
    }
    let lines: Vec<&str> = text.lines().collect();
    if !lines.iter().any(|l| l.split_whitespace().next() == Some("auth")) {
        return Err("this sudo PAM file has no auth rule; it was left as it is".into());
    }
    let at = lines.iter().position(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).unwrap_or(lines.len());
    let mut out: Vec<&str> = lines[..at].to_vec();
    out.push(MARK);
    out.push(LINE);
    out.extend_from_slice(&lines[at..]);
    Ok(out.join("\n") + "\n")
}

/// The file without our two lines (everything else as it was).
pub fn pam_without(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<&str> = vec![];
    let mut i = 0;
    while i < lines.len() {
        if lines[i] == MARK && lines.get(i + 1).is_some_and(|l| l.trim() == LINE) {
            i += 2;
            continue;
        }
        out.push(lines[i]);
        i += 1;
    }
    if out.is_empty() {
        String::new()
    } else {
        out.join("\n") + "\n"
    }
}

/// Turns fingerprint for sudo on or off in the PAM file at `path` (SUDO_PAM outside tests): written beside it and renamed over
/// it, so sudo never reads half a file; its permissions are kept. `module` is where pam_fprintd.so must be for "on".
pub fn set_sudo(path: &Path, on: bool, module: &Path) -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if on && !module.exists() {
        return Err("fprintd is not installed (Agora: Fingerprint reader)".into());
    }
    let new = if on { pam_with(&text)? } else { pam_without(&text) };
    if new == text {
        return Ok(false);
    }
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let tmp = path.with_file_name(format!(".{}.yggdrasil-new", path.file_name().and_then(|n| n.to_str()).unwrap_or("pam")));
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let written = f.write_all(new.as_bytes()).and_then(|_| f.sync_all());
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.to_string());
        }
    }
    let r = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(meta.permissions().mode() & 0o7777))
        .and_then(|_| std::fs::rename(&tmp, path));
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(true)
}

/// What Settings shows (`fingerprint json`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// fprintd answers on the system bus.
    pub installed: bool,
    /// The reader's name, when fprintd has one.
    pub reader: Option<String>,
    /// The fingers enrolled for this user ("right-index-finger").
    pub fingers: Vec<String>,
    /// Fingerprint for sudo is on.
    pub sudo: bool,
}

impl Status {
    pub fn json(&self) -> String {
        serde_json::json!({
            "installed": self.installed,
            "reader": self.reader,
            "fingers": self.fingers,
            "sudo": self.sudo,
        })
        .to_string()
    }
}

const BUS: &str = "net.reactivated.Fprint";

fn manager(conn: &zbus::blocking::Connection) -> zbus::Result<zbus::blocking::Proxy<'static>> {
    zbus::blocking::Proxy::new(conn, BUS, "/net/reactivated/Fprint/Manager", "net.reactivated.Fprint.Manager")
}

/// The default reader, when fprintd has one.
pub fn device(conn: &zbus::blocking::Connection) -> zbus::Result<zbus::blocking::Proxy<'static>> {
    let path: zbus::zvariant::OwnedObjectPath = manager(conn)?.call("GetDefaultDevice", &())?;
    zbus::blocking::Proxy::new(conn, BUS, path, "net.reactivated.Fprint.Device")
}

/// The name of a D-Bus error, when the call failed with one.
fn error_name(e: &zbus::Error) -> Option<String> {
    match e {
        zbus::Error::MethodError(name, _, _) => Some(name.to_string()),
        zbus::Error::FDO(f) => Some(format!("{f:?}")),
        _ => None,
    }
}

/// The reader and this user's fingers. `user` is the login; `sudo_pam` the file read for the switch.
pub fn status(conn: &zbus::blocking::Connection, user: &str, sudo_pam: &Path) -> Status {
    let sudo = std::fs::read_to_string(sudo_pam).map(|t| pam_has(&t)).unwrap_or(false);
    let dev = match device(conn) {
        Ok(d) => d,
        Err(e) => {
            // ServiceUnknown: fprintd is not installed; NoSuchDevice: installed, no reader it can drive
            let installed = error_name(&e).is_some_and(|n| n.contains("NoSuchDevice"));
            return Status { installed, sudo, ..Default::default() };
        }
    };
    let reader = dev.get_property::<String>("name").ok();
    let fingers: Vec<String> = dev.call("ListEnrolledFingers", &(user,)).unwrap_or_default();
    Status { installed: true, reader, fingers, sudo }
}

/// Enrols `finger` for this user: each of fprintd's steps is passed to `progress` ("enroll-stage-passed", "enroll-retry-scan",
/// ...); Ok when it finished with "enroll-completed".
pub fn enroll(conn: &zbus::blocking::Connection, finger: &str, mut progress: impl FnMut(&str)) -> Result<(), String> {
    let dev = device(conn).map_err(|e| e.to_string())?;
    let signals = dev.receive_signal("EnrollStatus").map_err(|e| e.to_string())?;
    dev.call_method("Claim", &("",)).map_err(|e| e.to_string())?;
    let result = (|| {
        dev.call_method("EnrollStart", &(finger,)).map_err(|e| e.to_string())?;
        let mut last = String::new();
        for m in signals {
            let Ok((result, done)) = m.body().deserialize::<(String, bool)>() else { continue };
            progress(&result);
            last = result;
            if done {
                break;
            }
        }
        let _ = dev.call_method("EnrollStop", &());
        if last == "enroll-completed" {
            Ok(())
        } else {
            Err(last)
        }
    })();
    let _ = dev.call_method("Release", &());
    result
}

/// Removes every finger this user enrolled.
pub fn delete_all(conn: &zbus::blocking::Connection) -> Result<(), String> {
    let dev = device(conn).map_err(|e| e.to_string())?;
    dev.call_method("Claim", &("",)).map_err(|e| e.to_string())?;
    let r = dev.call_method("DeleteEnrolledFingers2", &()).map(|_| ()).map_err(|e| e.to_string());
    let _ = dev.call_method("Release", &());
    r
}

/// What fprintd's step names mean, for a person.
pub fn words(result: &str) -> &'static str {
    match result {
        "enroll-completed" => "Fingerprint added",
        "enroll-stage-passed" => "Good. Lift your finger and touch the reader again",
        "enroll-retry-scan" | "enroll-swipe-too-short" | "enroll-finger-not-centered" | "enroll-remove-and-retry" => {
            "Not quite: lift your finger and touch the reader again"
        }
        "enroll-data-full" => "There is no room for more fingers on this reader",
        "enroll-duplicate" => "That finger is already enrolled",
        "enroll-disconnected" => "The reader went away",
        _ => "The reader could not take the fingerprint",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arch's /etc/pam.d/sudo.
    const ARCH: &str = "#%PAM-1.0\nauth\t\tinclude\t\tsystem-auth\naccount\t\tinclude\t\tsystem-auth\nsession\t\tinclude\t\tsystem-auth\n";

    #[test]
    fn the_line_goes_first_and_comes_out_exactly() {
        let on = pam_with(ARCH).unwrap();
        let lines: Vec<&str> = on.lines().collect();
        assert_eq!(lines[..3], ["#%PAM-1.0", MARK, LINE], "after the header, before the first rule: a finger is asked first");
        assert!(pam_has(&on) && !pam_has(ARCH));
        assert_eq!(pam_with(&on).unwrap(), on, "on twice: one pair of lines");
        assert_eq!(pam_without(&on), ARCH, "off: the file as it was");
        assert_eq!(pam_without(ARCH), ARCH, "off when it was never on: unchanged");
        // somebody's own pam_fprintd line (without our marker) is theirs and stays
        let theirs = format!("#%PAM-1.0\n{LINE}\nauth include system-auth\n");
        assert_eq!(pam_without(&theirs), theirs);
        assert!(!pam_has(&theirs));
        // our marker with another line under it is not ours to remove
        let edited = format!("#%PAM-1.0\n{MARK}\nauth required pam_deny.so\nauth include system-auth\n");
        assert_eq!(pam_without(&edited), edited);
        assert!(pam_with("#%PAM-1.0\naccount include system-auth\n").is_err(), "no auth rule: not touched");
        assert!(pam_with("").is_err());
    }

    #[test]
    fn the_file_is_replaced_whole_and_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("ygg-fp-pam-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let pam = d.join("sudo");
        std::fs::write(&pam, ARCH).unwrap();
        std::fs::set_permissions(&pam, std::fs::Permissions::from_mode(0o644)).unwrap();
        let module = d.join("pam_fprintd.so");
        assert!(set_sudo(&pam, true, &module).unwrap_err().contains("Agora"), "no module: refused, file untouched");
        assert_eq!(std::fs::read_to_string(&pam).unwrap(), ARCH);
        std::fs::write(&module, "").unwrap();
        assert_eq!(set_sudo(&pam, true, &module), Ok(true));
        assert!(pam_has(&std::fs::read_to_string(&pam).unwrap()));
        assert_eq!(std::fs::metadata(&pam).unwrap().permissions().mode() & 0o777, 0o644);
        assert_eq!(set_sudo(&pam, true, &module), Ok(false), "already on: nothing written");
        assert_eq!(set_sudo(&pam, false, &module), Ok(true));
        assert_eq!(std::fs::read_to_string(&pam).unwrap(), ARCH);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 2, "no temporary file left behind");
        assert!(set_sudo(&d.join("missing"), false, &module).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn steps_are_said_in_words() {
        assert_eq!(words("enroll-completed"), "Fingerprint added");
        assert!(words("enroll-retry-scan").starts_with("Not quite"));
        assert!(words("something new").contains("could not"));
        let s =
            Status { installed: true, reader: Some("Synaptics Sensors".into()), fingers: vec!["right-index-finger".into()], sudo: false };
        let v: serde_json::Value = serde_json::from_str(&s.json()).unwrap();
        assert_eq!(
            (v["reader"].as_str(), v["fingers"][0].as_str(), v["sudo"].as_bool()),
            (Some("Synaptics Sensors"), Some("right-index-finger"), Some(false))
        );
    }
}
