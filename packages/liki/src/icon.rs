//! liki-icon: gives apps different icons, for instance dark macOS icons downloaded from macosicons.com. The icons go to
//! ~/.local/share/icons/Liki, the theme Yggdrasil uses everywhere.
//!
//! As the shell script it replaced (same arguments, output, files, exit codes): an .icns is turned into its biggest PNG by
//! this crate's own reader (the script ran liki-icns into a temporary file), each name's old .svg and .png are removed before
//! the new one is copied in, and the user's copy of the theme gets the package's index.theme and a fresh icon cache
//! (gtk-update-icon-cache, when installed). Not 1:1: under --import, a picture that could not be copied (a folder named like
//! a picture, a full disk) no longer counts as imported.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const USAGE: &str = "\
Give apps different icons, for instance dark macOS icons downloaded from macosicons.com.
  liki-icon APP FILE            one icon: APP is the app's icon name (the Icon= line of its
                                     .desktop file), FILE is a .png, .svg or macOS .icns
  liki-icon --import FOLDER     every .icns/.png/.svg in FOLDER whose file name says which
                                     app it is (Finder_..., Spotify_..., Steam_...); lists the rest
  liki-icon --reset APP         back to the theme's icon
Icons go to ~/.local/share/icons/Liki, the theme Yggdrasil uses everywhere, so the dock,
Minos, Settings and the launcher all pick them up (restart an app to see its new icon).
";

/// The package's description of the theme, copied into the user's copy.
const INDEX: &str = "/usr/share/icons/Liki/index.theme";

pub fn main(args: &[OsString]) -> i32 {
    let theme = data_dir().join("icons/Liki");
    let dir = theme.join("apps/scalable");
    let arg = |i: usize| args.get(i).map(|a| a.as_bytes()).unwrap_or_default();
    match arg(0) {
        b"--reset" => {
            if arg(1).is_empty() {
                return 1;
            }
            remove_icon(&dir, arg(1));
            if let Err(code) = refresh(&theme) {
                return code;
            }
            println!("reset {}", String::from_utf8_lossy(arg(1)));
            0
        }
        b"--import" => {
            let folder = Path::new(std::ffi::OsStr::from_bytes(arg(1)));
            if arg(1).is_empty() || !folder.is_dir() {
                eprintln!("give a folder");
                return 1;
            }
            let mut done = 0;
            for f in pictures(arg(1)) {
                let base = String::from_utf8_lossy(Path::new(&f).file_name().map(|n| n.as_bytes()).unwrap_or_default()).into_owned();
                match names_for(&base) {
                    Some(names) => {
                        if install(&dir, Path::new(&f), names).is_ok() {
                            println!("  {base} -> {}", names[0]);
                            done += 1;
                        }
                    }
                    None => println!("  (no app known for {base}: liki-icon APP '{}')", f.to_string_lossy()),
                }
            }
            if let Err(code) = refresh(&theme) {
                return code;
            }
            println!("{done} icons imported");
            0
        }
        b"" => {
            print!("{USAGE}");
            1
        }
        _ => {
            let file = Path::new(std::ffi::OsStr::from_bytes(arg(1)));
            if args.len() != 2 || !readable(file) {
                eprintln!("usage: liki-icon APP FILE");
                return 1;
            }
            let name = String::from_utf8_lossy(arg(0)).into_owned();
            if install(&dir, file, &[&name]).is_err() {
                return 1;
            }
            if let Err(code) = refresh(&theme) {
                return code;
            }
            println!("{name} now uses {}", file.to_string_lossy());
            0
        }
    }
}

/// ${XDG_DATA_HOME:-$HOME/.local/share}
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

pub fn xdg(name: &str, rest: &str) -> PathBuf {
    match std::env::var_os(name) {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
    }
}

/// [[ -r FILE ]]
fn readable(p: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
    // SAFETY: a NUL-terminated path that lives across the call
    !p.as_os_str().is_empty() && unsafe { libc::access(c.as_ptr(), libc::R_OK) } == 0
}

/// "$2"/*.icns "$2"/*.png "$2"/*.svg as bash expanded them with nocaseglob: three lists, each in name order (byte order, as in
/// the C locale), names starting with a dot left out; each path written as FOLDER/NAME, the folder as given.
fn pictures(folder: &[u8]) -> Vec<OsString> {
    let names: Vec<Vec<u8>> = std::fs::read_dir(std::ffi::OsStr::from_bytes(folder))
        .map(|d| d.flatten().map(|e| e.file_name().into_vec()).filter(|n| !n.starts_with(b".")).collect())
        .unwrap_or_default();
    let mut out = vec![];
    for ext in [&b".icns"[..], b".png", b".svg"] {
        let mut v: Vec<&Vec<u8>> = names.iter().filter(|n| n.to_ascii_lowercase().ends_with(ext)).collect();
        v.sort();
        out.extend(v.into_iter().map(|n| OsString::from_vec([folder, b"/", n].concat())));
    }
    out
}

/// The icon names an app uses, from the start of a picture's file name (Finder_..., Spotify_...).
pub fn names_for(file: &str) -> Option<&'static [&'static str]> {
    let f = file.to_lowercase();
    const TABLE: &[(&[&str], &[&str])] = &[
        (&["finder", "minos"], &["system-file-manager", "file-manager", "org.kde.dolphin"]),
        (&["settings"], &["preferences-system", "yggdrasil-settings", "systemsettings"]),
        (&["terminal"], &["utilities-terminal"]),
        (&["ghostty"], &["com.mitchellh.ghostty", "ghostty"]),
        (&["spotify"], &["spotify-launcher", "spotify-client", "spotify"]),
        (&["steam"], &["steam"]),
        (&["obsidian"], &["obsidian", "md.obsidian.Obsidian"]),
        (&["brave"], &["brave-browser", "brave"]),
        (&["firefox"], &["firefox"]),
        (&["vivaldi"], &["vivaldi-stable", "vivaldi"]),
        (&["tor_browser"], &["torbrowser", "tor-browser"]),
        (&["discord"], &["discord"]),
        (&["signal"], &["signal-desktop"]),
        (&["vs_code"], &["code", "visual-studio-code", "vscode"]),
        (&["github"], &["github-desktop"]),
        (&["claude"], &["claude", "claude-desktop"]),
        (&["chatgpt"], &["chatgpt"]),
        (&["vlc"], &["vlc"]),
        (&["blender"], &["blender"]),
        (&["freecad"], &["org.freecad.FreeCAD", "freecad"]),
        (&["sketchup"], &["sketchup"]),
        (&["protonvpn"], &["protonvpn", "proton-vpn"]),
        (&["logi"], &["logi-options-plus", "logiops", "solaar", "logitech-options"]),
        (&["minecraft_launcher"], &["minecraft-launcher", "minecraft"]),
        (&["duckstation"], &["duckstation", "org.duckstation.DuckStation"]),
        (&["retroarch"], &["retroarch", "com.libretro.RetroArch"]),
        (&["macos_bin"], &["user-trash-full"]),
        (&["this_pc", "this-pc"], &["computer"]),
    ];
    TABLE.iter().find(|(starts, _)| starts.iter().any(|s| f.starts_with(s))).map(|(_, names)| *names)
}

/// rm -f DIR/NAME.svg DIR/NAME.png
fn remove_icon(dir: &Path, name: &[u8]) {
    for ext in [&b".svg"[..], b".png"] {
        let _ = std::fs::remove_file(dir.join(std::ffi::OsStr::from_bytes(&[name, ext].concat())));
    }
}

/// Puts FILE in the theme under each name (an .icns as its biggest PNG); errors said on standard error as the script's tools
/// said them.
fn install(dir: &Path, file: &Path, names: &[&str]) -> Result<(), ()> {
    let raw = file.as_os_str().as_bytes();
    let ext = String::from_utf8_lossy(raw.rsplit(|&b| b == b'.').next().unwrap_or(raw)).to_lowercase();
    let (data, ext) = if ext == "icns" {
        let read = std::fs::read(file).map_err(|e| eprintln!("liki-icns: {}: {e}", file.display()))?;
        let png = crate::icns::biggest_png(&read).map_err(|e| eprintln!("liki-icns: {e}"))?.to_vec();
        (Some(png), "png".to_string())
    } else {
        (None, ext)
    };
    if ext != "png" && ext != "svg" {
        eprintln!("use an .svg, .png or .icns file: {}", file.display());
        return Err(());
    }
    std::fs::create_dir_all(dir).map_err(|e| eprintln!("mkdir: cannot create directory '{}': {}", dir.display(), why(&e)))?;
    for name in names {
        remove_icon(dir, name.as_bytes());
        let to = dir.join(format!("{name}.{ext}"));
        match &data {
            // as the script's copy of liki-icns' temporary file (mktemp's 0600)
            Some(png) => write_new(&to, png, 0o600).map_err(|e| eprintln!("liki-icns: {}: {e}", to.display()))?,
            None => cp(file, &to)?,
        }
    }
    Ok(())
}

/// `cp FROM TO`: the bytes, and for a new file the source's permissions (less the umask), as cp gives them.
pub(crate) fn cp(from: &Path, to: &Path) -> Result<(), ()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(from).map_err(|e| eprintln!("cp: cannot stat '{}': {}", from.display(), why(&e)))?;
    if meta.is_dir() {
        eprintln!("cp: -r not specified; omitting directory '{}'", from.display());
        return Err(());
    }
    let data = std::fs::read(from).map_err(|e| eprintln!("cp: cannot open '{}' for reading: {}", from.display(), why(&e)))?;
    write_new(to, &data, meta.permissions().mode() & 0o7777)
        .map_err(|e| eprintln!("cp: cannot create regular file '{}': {}", to.display(), why(&e)))
}

/// Writes a file as the shell's tools create one: an existing file keeps its permissions, a new one gets MODE less the umask.
pub fn write_new(to: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(to)?;
    f.write_all(data)
}

/// An error as coreutils word it ("No such file or directory"), without Rust's "(os error N)".
pub fn why(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// The user's copy of the theme gets the package's index.theme when it has none, then a fresh icon cache. Err(exit code) when
/// index.theme could not be copied (the script stopped there under set -e).
fn refresh(theme: &Path) -> Result<(), i32> {
    if !theme.join("index.theme").exists() {
        std::fs::create_dir_all(theme).map_err(|e| {
            eprintln!("mkdir: cannot create directory '{}': {}", theme.display(), why(&e));
            1
        })?;
        cp(Path::new(INDEX), &theme.join("index.theme")).map_err(|()| 1)?;
    }
    update_icon_cache(theme);
    Ok(())
}

/// gtk-update-icon-cache -qf DIR, quietly, when it is installed.
pub fn update_icon_cache(dir: &Path) {
    let _ = Command::new("gtk-update-icon-cache").arg("-qf").arg(dir).stderr(Stdio::null()).status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_from_file_names() {
        assert_eq!(names_for("Finder_dark.icns").unwrap()[0], "system-file-manager");
        assert_eq!(names_for("MINOS.png").unwrap()[0], "system-file-manager");
        assert_eq!(names_for("This-PC.svg"), Some(&["computer"][..]));
        assert_eq!(names_for("tor_browser_x.png").unwrap(), ["torbrowser", "tor-browser"]);
        assert_eq!(names_for("torch.png"), None);
        assert_eq!(names_for("Logitech.png").unwrap()[0], "logi-options-plus");
    }

    #[test]
    fn pictures_in_glob_order() {
        let d = std::env::temp_dir().join(format!("liki-icon-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for n in ["b.PNG", "a.png", ".hidden.png", "c.svg", "Z.icns", "x.txt", ".png", "png"] {
            std::fs::write(d.join(n), b"").unwrap();
        }
        let p = d.to_string_lossy().into_owned();
        let got: Vec<String> = pictures(p.as_bytes()).iter().map(|f| f.to_string_lossy().into_owned()).collect();
        assert_eq!(got, [format!("{p}/Z.icns"), format!("{p}/a.png"), format!("{p}/b.PNG"), format!("{p}/c.svg")]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
