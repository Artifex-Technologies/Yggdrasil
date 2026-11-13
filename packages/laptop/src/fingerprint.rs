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

/// Whether the system around the reader can be trusted with it: Secure Boot on (what boots has been checked) and the system
/// disk encrypted (nothing on it can be changed or read with the laptop switched off). Until both, a finger cannot be enrolled
/// and sudo cannot take one (the owner's rule, 2026-11-13: a fingerprint is only as safe as the system that checks it); taking
/// fingers out and turning sudo off always work.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Trust {
    pub secure_boot: bool,
    pub encrypted: bool,
}

impl Trust {
    pub fn ok(self) -> bool {
        self.secure_boot && self.encrypted
    }

    /// Why a finger cannot be used yet, in a sentence; empty when it can.
    pub fn why_not(self) -> &'static str {
        match (self.secure_boot, self.encrypted) {
            (true, true) => "",
            (false, false) => "Off until Secure Boot is on and the system disk is encrypted",
            (false, true) => "Off until Secure Boot is on",
            (true, false) => "Off until the system disk is encrypted",
        }
    }

    pub fn here() -> Trust {
        Trust {
            secure_boot: std::fs::read(SECURE_BOOT_VAR).map(|b| secure_boot_on(&b)).unwrap_or(false),
            encrypted: root_encrypted(Path::new("/proc/self/mountinfo"), Path::new("/sys/class/block")),
        }
    }
}

/// The firmware's SecureBoot variable (UEFI global variables' GUID).
pub const SECURE_BOOT_VAR: &str = "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";

/// efivarfs gives 4 bytes of attributes, then the value: 1 is on.
pub fn secure_boot_on(var: &[u8]) -> bool {
    var.get(4) == Some(&1)
}

/// The block device the root file system is on (`/dev/nvme0n1p2`, `/dev/mapper/root`), from mountinfo's source field.
pub fn root_source(mountinfo: &str) -> Option<String> {
    mountinfo.lines().find_map(|l| {
        let (left, right) = l.split_once(" - ")?;
        (left.split_whitespace().nth(4)? == "/").then(|| right.split_whitespace().nth(1).map(str::to_string)).flatten()
    })
}

/// Whether block device `name` (as /sys/class/block lists it: nvme0n1p2, dm-0) is, or sits on, an encrypted (LUKS, dm-crypt)
/// device: a device-mapper device whose uuid starts with CRYPT-, here or below it (LVM on LUKS, and so on).
pub fn encrypted_in(sys_block: &Path, name: &str, depth: u8) -> bool {
    let dir = sys_block.join(name);
    if std::fs::read_to_string(dir.join("dm/uuid")).is_ok_and(|u| u.starts_with("CRYPT-")) {
        return true;
    }
    depth < 8
        && std::fs::read_dir(dir.join("slaves"))
            .map(|rd| rd.flatten().any(|e| encrypted_in(sys_block, &e.file_name().to_string_lossy(), depth + 1)))
            .unwrap_or(false)
}

fn root_encrypted(mountinfo: &Path, sys_block: &Path) -> bool {
    let Some(src) = std::fs::read_to_string(mountinfo).ok().and_then(|m| root_source(&m)) else { return false };
    // /dev/mapper/root is a link to /dev/dm-0; its last part is the name /sys/class/block uses
    let real = std::fs::canonicalize(&src).unwrap_or_else(|_| src.clone().into());
    let Some(name) = real.file_name().map(|n| n.to_string_lossy().into_owned()) else { return false };
    encrypted_in(sys_block, &name, 0)
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
    /// Secure Boot and disk encryption, which a finger waits for.
    pub trust: Trust,
}

impl Status {
    pub fn json(&self) -> String {
        serde_json::json!({
            "installed": self.installed,
            "reader": self.reader,
            "fingers": self.fingers,
            "sudo": self.sudo,
            "secure_boot": self.trust.secure_boot,
            "encrypted": self.trust.encrypted,
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
            return Status { installed, sudo, trust: Trust::here(), ..Default::default() };
        }
    };
    let reader = dev.get_property::<String>("name").ok();
    let fingers: Vec<String> = dev.call("ListEnrolledFingers", &(user,)).unwrap_or_default();
    Status { installed: true, reader, fingers, sudo, trust: Trust::here() }
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
    #[test]
    fn a_finger_waits_for_secure_boot_and_an_encrypted_disk() {
        assert!(secure_boot_on(&[6, 0, 0, 0, 1]) && !secure_boot_on(&[6, 0, 0, 0, 0]) && !secure_boot_on(&[]));
        let mi = "24 1 0:22 /@ / rw,relatime shared:1 - btrfs /dev/mapper/root rw\n25 24 0:23 / /boot rw - vfat /dev/nvme0n1p1 rw\n";
        assert_eq!(root_source(mi).as_deref(), Some("/dev/mapper/root"));
        // dm-1 (LVM) on dm-0 (LUKS) on nvme0n1p2; and a plain partition
        let root = std::env::temp_dir().join(format!("ygg-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dev, uuid, slave) in [
            ("dm-1", Some("LVM-abc"), Some("dm-0")),
            ("dm-0", Some("CRYPT-LUKS2-xyz"), Some("nvme0n1p2")),
            ("nvme0n1p2", None, None),
            ("sda1", None, None),
        ] {
            std::fs::create_dir_all(root.join(dev).join("slaves")).unwrap();
            if let Some(u) = uuid {
                std::fs::create_dir_all(root.join(dev).join("dm")).unwrap();
                std::fs::write(root.join(dev).join("dm/uuid"), u).unwrap();
            }
            if let Some(s) = slave {
                std::fs::create_dir_all(root.join(dev).join("slaves").join(s)).unwrap();
            }
        }
        assert!(encrypted_in(&root, "dm-1", 0), "LVM on LUKS");
        assert!(encrypted_in(&root, "dm-0", 0));
        assert!(!encrypted_in(&root, "sda1", 0), "a plain partition");
        std::fs::remove_dir_all(&root).unwrap();
        let t = Trust { secure_boot: false, encrypted: false };
        assert!(!t.ok() && t.why_not().contains("Secure Boot") && t.why_not().contains("encrypted"));
        assert!(Trust { secure_boot: true, encrypted: true }.ok());
    }

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
        let s = Status {
            installed: true,
            reader: Some("Synaptics Sensors".into()),
            fingers: vec!["right-index-finger".into()],
            sudo: false,
            trust: Trust::default(),
        };
        let v: serde_json::Value = serde_json::from_str(&s.json()).unwrap();
        assert_eq!(
            (v["reader"].as_str(), v["fingers"][0].as_str(), v["sudo"].as_bool()),
            (Some("Synaptics Sensors"), Some("right-index-finger"), Some(false))
        );
    }
}
