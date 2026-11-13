//! Signing in to Proton Drive with stand-ins for rclone, systemctl and mountpoint (on PATH only inside this test process): no
//! secret may reach a command line (every program can read /proc/PID/cmdline) or an environment, and rclone's config gets the
//! remote's section with every other section kept byte for byte. The real rclone is never run, and the config is a file in a
//! temporary folder (RCLONE_CONFIG, HOME and XDG_CONFIG_HOME all point there).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;

use minos_core::cloud;

/// PATH, HOME and the rest are the whole process's: one test at a time.
static ENV: Mutex<()> = Mutex::new(());

const PASSWORD: &str = "Pw correct-horse 7!";
const MAILBOX: &str = "Mb#secret;2 \"q\"";
const TWOFA: &str = "135790";

/// The stand-in's scrambling: "OBS" and the secret in base64 (URL alphabet), like rclone's own answer in shape.
fn scrambled(secret: &str) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::from("OBS");
    for c in secret.as_bytes().chunks(3) {
        let n = ((c[0] as u32) << 16) | ((*c.get(1).unwrap_or(&0) as u32) << 8) | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..=c.len() {
            out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

const RCLONE: &str = r#"#!/bin/bash
{ printf 'rclone'; for a in "$@"; do printf ' [%s]' "$a"; done; printf '\n'; } >> "$STANDIN_LOG/argv"
env >> "$STANDIN_LOG/env"
case "$1" in
  obscure)
    [ "$2" = "-" ] || { echo "the stand-in only reads secrets from stdin" >&2; exit 2; }
    s=$(cat)
    printf '%s\n' "$s" >> "$STANDIN_LOG/stdin"
    printf 'OBS%s\n' "$(printf '%s' "$s" | base64 -w0 | tr '+/' '-_' | tr -d '=')"
    ;;
  config)
    if [ "$2" = file ]; then
      if [ -f "$RCLONE_CONFIG" ]; then echo "Configuration file is stored at:"; else echo "Configuration file doesn't exist, but rclone will use this path:"; fi
      echo "$RCLONE_CONFIG"
    fi
    ;;
esac
exit 0
"#;

const OTHER: &str = r#"#!/bin/bash
{ printf '%s' "$(basename "$0")"; for a in "$@"; do printf ' [%s]' "$a"; done; printf '\n'; } >> "$STANDIN_LOG/argv"
env >> "$STANDIN_LOG/env"
exit 0
"#;

struct World {
    dir: PathBuf,
    conf: PathBuf,
}

impl World {
    fn new(name: &str) -> World {
        let dir = std::env::temp_dir().join(format!("minos-proton-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let bin = dir.join("bin");
        let log = dir.join("log");
        let home = dir.join("home");
        for d in [&bin, &log, &home.join(".config/systemd/user")] {
            fs::create_dir_all(d).unwrap();
        }
        // the mount's unit, so starting it gets as far as (the stand-in) systemctl
        fs::write(home.join(".config/systemd/user/proton-drive.service"), "[Service]\n").unwrap();
        for (prog, body) in [("rclone", RCLONE), ("systemctl", OTHER), ("mountpoint", OTHER)] {
            let p = bin.join(prog);
            fs::write(&p, body).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let conf = home.join(".config/rclone/rclone.conf");
        // edition 2021: set_var is safe; the lock above keeps the tests from changing it under each other
        std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("RCLONE_CONFIG", &conf);
        std::env::set_var("STANDIN_LOG", &log);
        World { dir, conf }
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.dir.join("log").join(name)).unwrap_or_default()
    }

    /// No secret, plain or scrambled, in any program's command line or environment.
    fn assert_no_secret_leaked(&self, secrets: &[&str]) {
        let (argv, env) = (self.log("argv"), self.log("env"));
        assert!(!argv.is_empty());
        for s in secrets {
            for form in [s.to_string(), scrambled(s)] {
                assert!(!argv.contains(&form), "{form:?} was on a command line:\n{argv}");
                assert!(!env.contains(&form), "{form:?} was in an environment");
            }
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_first_sign_in_keeps_every_secret_off_the_command_line() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let w = World::new("first");
    let other = "# my remotes\n[backup]\ntype = sftp\nhost = example.invalid\n";
    fs::create_dir_all(w.conf.parent().unwrap()).unwrap();
    fs::write(&w.conf, other).unwrap();

    cloud::connect_proton(" me@example.invalid ", PASSWORD, MAILBOX, TWOFA).unwrap();

    w.assert_no_secret_leaked(&[PASSWORD, MAILBOX, TWOFA]);
    // the passwords went to rclone obscure on its standard input
    assert_eq!(w.log("stdin"), format!("{PASSWORD}\n{MAILBOX}\n"));
    let conf = fs::read_to_string(&w.conf).unwrap();
    assert_eq!(
        conf,
        format!(
            "{other}\n[proton]\ntype = protondrive\nusername = me@example.invalid\npassword = {}\nmailbox_password = {}\n2fa = {TWOFA}\n\n",
            scrambled(PASSWORD),
            scrambled(MAILBOX)
        )
    );
    assert_eq!(fs::metadata(&w.conf).unwrap().permissions().mode() & 0o777, 0o600);
    // the mount was started, and the one-time code was taken out again afterwards (an empty value: nothing secret on that line)
    let argv = w.log("argv");
    assert!(argv.contains("systemctl [--user] [enable] [--now] [proton-drive]"), "{argv}");
    assert!(argv.contains("rclone [config] [update] [proton] [2fa] []"), "{argv}");
    assert!(!argv.contains("[create]"), "{argv}");
}

#[test]
fn a_renewed_sign_in_drops_the_old_session_and_keeps_the_rest() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let w = World::new("renew");
    let before = "[a]\ntype = local\n\n[proton]\ntype = protondrive\nusername = me@example.invalid\npassword = T0xE\n\
                  client_uid = u\nclient_access_token = a\nclient_refresh_token = r\nclient_salted_key_pass = s\n\n[z]\ntype = local\n";
    fs::create_dir_all(w.conf.parent().unwrap()).unwrap();
    fs::write(&w.conf, before).unwrap();

    // no mailbox password and no two-factor code this time
    cloud::save_proton_signin("me@example.invalid", PASSWORD, "", "").unwrap();

    w.assert_no_secret_leaked(&[PASSWORD]);
    assert_eq!(
        fs::read_to_string(&w.conf).unwrap(),
        format!(
            "[a]\ntype = local\n\n[proton]\ntype = protondrive\nusername = me@example.invalid\npassword = {}\n\n[z]\ntype = local\n",
            scrambled(PASSWORD)
        )
    );
}

#[test]
fn a_user_name_that_would_break_the_config_is_refused_before_anything_runs() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let w = World::new("refused");
    let e = cloud::save_proton_signin("me\nclient_uid = x", PASSWORD, "", "").unwrap_err();
    assert!(e.contains("user name"), "{e}");
    let e = cloud::save_proton_signin("me@example.invalid", PASSWORD, "", "12 34").unwrap_err();
    assert!(e.contains("two-factor"), "{e}");
    assert_eq!(w.log("argv"), "");
    assert!(!w.conf.exists());
}

#[test]
fn an_encrypted_config_is_left_alone() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let w = World::new("encrypted");
    let enc = "# Encrypted rclone configuration File\n\nRCLONE_ENCRYPT_V0:\nAAAA\n";
    fs::create_dir_all(w.conf.parent().unwrap()).unwrap();
    fs::write(&w.conf, enc).unwrap();
    let e = cloud::save_proton_signin("me@example.invalid", PASSWORD, "", TWOFA).unwrap_err();
    assert!(e.contains("encrypted"), "{e}");
    assert_eq!(fs::read_to_string(&w.conf).unwrap(), enc);
    // the password was never handed to rclone at all
    assert_eq!(w.log("stdin"), "");
}
