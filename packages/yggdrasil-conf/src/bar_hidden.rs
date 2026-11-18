//! Top bar add-ons the user hid (Settings > Appearance > Top bar and dock, "Top bar add-ons"): their pills are not drawn and
//! their programs are not run. The add-on stays installed; switching it back on shows it again.
//!
//! The list: `~/.config/yggdrasil/bar-hidden`, one add-on id (its integration file's name without `.conf`) per line, `#` comments.
//! No file: nothing hidden.

/// The file's name in the config folder (the panel watches that folder and reads the list again when it changes).
pub const FILE: &str = "bar-hidden";

/// The ids in the file: lines without comments, blanks and spaces.
pub fn parse(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
        .filter(|l| !l.is_empty() && !l.contains(char::is_whitespace))
        .collect()
}

/// The hidden add-ons now (one small file read; none when it is missing).
pub fn load() -> Vec<String> {
    std::fs::read_to_string(crate::dirs::config().join(FILE)).map(|t| parse(&t)).unwrap_or_default()
}

/// Shows (`hidden` false) or hides one add-on. Written beside the list and renamed, so the panel never reads half a file.
pub fn set(id: &str, hidden: bool) -> Result<(), String> {
    let mut list = load();
    list.retain(|x| x != id);
    if hidden {
        list.push(id.to_string());
    }
    let dir = crate::dirs::config();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(FILE);
    let tmp = dir.join(format!(".{FILE}.tmp"));
    let mut text = String::from("# Top bar add-ons that are hidden (Settings > Appearance > Top bar and dock), one id per line\n");
    for x in &list {
        text += x;
        text.push('\n');
    }
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hidden_list() {
        assert_eq!(parse("# hidden\nagents\n  weather  # mine\n\nnot an id\n"), ["agents", "weather"]);
        assert!(parse("").is_empty());
    }
}
