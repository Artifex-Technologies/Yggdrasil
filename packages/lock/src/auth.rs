//! Password checking through PAM, the system's own login machinery (the same one `login` and `sudo` use), so passwords, fingerprint
//! policies and lockout rules stay where the administrator set them. No password is ever stored or compared here.

use std::ffi::{c_char, c_int, c_void, CString};

pub trait Authenticator {
    /// Ok(true) = right password, Ok(false) = wrong, Err = PAM itself failed.
    fn check(&self, user: &str, password: &str) -> Result<bool, String>;
}

// ---- the small part of libpam that is needed
#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}
#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}
#[repr(C)]
struct PamConv {
    conv: extern "C" fn(c_int, *mut *const PamMessage, *mut *mut PamResponse, *mut c_void) -> c_int,
    appdata_ptr: *mut c_void,
}

#[link(name = "pam")]
extern "C" {
    fn pam_start(service: *const c_char, user: *const c_char, conv: *const PamConv, handle: *mut *mut c_void) -> c_int;
    fn pam_authenticate(handle: *mut c_void, flags: c_int) -> c_int;
    fn pam_acct_mgmt(handle: *mut c_void, flags: c_int) -> c_int;
    fn pam_end(handle: *mut c_void, status: c_int) -> c_int;
}

const PAM_SUCCESS: c_int = 0;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_CONV_ERR: c_int = 19;

extern "C" fn conversation(n: c_int, msgs: *mut *const PamMessage, resp: *mut *mut PamResponse, appdata: *mut c_void) -> c_int {
    unsafe {
        let replies = libc::calloc(n as usize, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
        if replies.is_null() {
            return PAM_CONV_ERR;
        }
        for i in 0..n as isize {
            let m = &**msgs.offset(i);
            if m.msg_style == PAM_PROMPT_ECHO_OFF || m.msg_style == PAM_PROMPT_ECHO_ON {
                // the password goes to every prompt (PAM asks once for it)
                (*replies.offset(i)).resp = libc::strdup(appdata as *const c_char);
            }
        }
        *resp = replies;
    }
    PAM_SUCCESS
}

pub struct Pam {
    pub service: String,
}

impl Authenticator for Pam {
    fn check(&self, user: &str, password: &str) -> Result<bool, String> {
        let service = CString::new(self.service.as_str()).map_err(|_| "bad service name")?;
        let user_c = CString::new(user).map_err(|_| "bad user name")?;
        let pw = CString::new(password).map_err(|_| "password contains a NUL")?;
        let pw_ptr = pw.as_ptr() as *mut c_void;
        let conv = PamConv { conv: conversation, appdata_ptr: pw_ptr };
        let mut handle: *mut c_void = std::ptr::null_mut();
        unsafe {
            let rc = pam_start(service.as_ptr(), user_c.as_ptr(), &conv, &mut handle);
            if rc != PAM_SUCCESS {
                return Err(format!("pam_start failed ({rc})"));
            }
            let mut rc = pam_authenticate(handle, 0);
            if rc == PAM_SUCCESS {
                rc = pam_acct_mgmt(handle, 0); // an expired or locked account must not unlock the screen
            }
            pam_end(handle, rc);
            // wipe our copy of the password
            let mut bytes = pw.into_bytes();
            zeroize::Zeroize::zeroize(&mut bytes);
            // fail closed: anything but a clean success keeps the screen locked
            Ok(rc == PAM_SUCCESS)
        }
    }
}

/// A stand-in for tests in a throw-away compositor: the password is in the environment. Compiled only with the `test-auth` feature.
#[cfg(feature = "test-auth")]
pub struct Fake;

#[cfg(feature = "test-auth")]
impl Authenticator for Fake {
    fn check(&self, _user: &str, password: &str) -> Result<bool, String> {
        Ok(std::env::var("LABYRINTH_LOCK_TEST_PASSWORD").map(|p| p == password).unwrap_or(false))
    }
}

/// Asks the distress password helper when the real password was wrong. Only used when the distress password has been set up
/// (its settings file exists). Unlocks only for a "silent login"; for anything else (a decoy session, or a distress password typed
/// too few times so far) the screen simply stays locked, exactly as for any wrong password.
pub struct WithDistress<A: Authenticator> {
    pub inner: A,
}

impl<A: Authenticator> Authenticator for WithDistress<A> {
    fn check(&self, user: &str, password: &str) -> Result<bool, String> {
        match self.inner.check(user, password) {
            Ok(false) if std::path::Path::new("/etc/labyrinth/distress.json").exists() => Ok(distress(password)),
            other => other,
        }
    }
}

fn distress(password: &str) -> bool {
    use std::io::Write;
    let Ok(mut child) = std::process::Command::new("pkexec").arg("/usr/bin/labyrinth-distress").arg("check").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn() else { return false };
    let body = format!("{{\"password\":{}}}", json_string(password));
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(body.as_bytes());
    }
    let Ok(out) = child.wait_with_output() else { return false };
    let t = String::from_utf8_lossy(&out.stdout);
    t.contains("\"result\":\"distress\"") && t.contains("\"unlock\":true")
}

fn json_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
mod distress_tests {
    #[test]
    fn json_escapes() {
        assert_eq!(super::json_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
