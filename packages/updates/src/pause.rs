//! "Pause updates": for a week, nothing looks for updates by itself (Settings' Software Update when it opens, Agora at its start
//! and every half hour while open). Asking by hand still works, and pacman in a terminal is never affected. Kept per user in
//! `$XDG_STATE_HOME/yggdrasil/updates-paused-until` (seconds since 1970); a time in the past means not paused.

use std::path::PathBuf;

/// One week, the pause the button gives.
pub const WEEK: i64 = 7 * 86_400;

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("yggdrasil/updates-paused-until"))
}

/// The end of the pause from the file's text, when it is still ahead of `now`.
pub fn parse(text: &str, now: i64) -> Option<i64> {
    text.trim().parse::<i64>().ok().filter(|t| *t > now)
}

/// Until when automatic checks are paused (None: they are not).
pub fn until(now: i64) -> Option<i64> {
    path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| parse(&t, now))
}

/// Pause for `secs` from `now`.
pub fn pause(now: i64, secs: i64) -> Result<(), String> {
    let p = path().ok_or("no home folder")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, format!("{}\n", now + secs)).map_err(|e| e.to_string())
}

/// Checks run by themselves again.
pub fn resume() -> Result<(), String> {
    match path().map(std::fs::remove_file) {
        Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

/// Seconds since 1970, for `until` and `pause`.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pause_holds_only_until_its_end() {
        assert_eq!(parse("2000\n", 1000), Some(2000));
        assert_eq!(parse("1000", 1000), None, "over");
        assert_eq!(parse("soon", 1000), None);
        assert_eq!(parse("", 1000), None);
    }
}
