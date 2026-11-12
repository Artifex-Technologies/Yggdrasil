//! bootscreen: what shows while the computer starts and shuts down (Settings > Appearance > Boot screen), through Plymouth.
//! The Python script's behaviour (doc/bootscreen.txt is its help): `get` prints the JSON Settings reads, `set` (as root)
//! switches Plymouth on the first time (a mkinitcpio drop-in, NVIDIA's modules early, "splash" in GRUB's kernel options with
//! the old file kept) and picks the theme, then rebuilds the start-up image.
//!
//! "Your own picture" is decoded here (PNG, JPEG and WebP only, as the script let Pillow decode; never anything handed to
//! another program) instead of by Pillow, so its pixels can differ a little from what Pillow made (the scaling filter and
//! the PNG encoder are not Pillow's); its size is worked out by Pillow's thumbnail() rule.

use std::fs;
use std::path::Path;

use pycompat::json::{self, Map, Value};

use crate::py::{self, Crash, R};

pub const DOC: &str = include_str!("doc/bootscreen.txt");
pub const PICTURE_THEME: &str = "yggdrasil-picture";
pub const MESSAGES_THEME: &str = "details";

pub const DROPIN_TEXT: &str = r#"# Yggdrasil (Settings > Appearance > Boot screen): Plymouth shows the boot screen. Added after "kms" (or "udev").
if [[ " ${HOOKS[*]} " != *" plymouth "* ]]; then
  _ygg_hooks=()
  _ygg_done=0
  for _h in "${HOOKS[@]}"; do
    _ygg_hooks+=("$_h")
    if [[ $_ygg_done == 0 && ( $_h == kms || $_h == sd-vconsole ) ]]; then _ygg_hooks+=(plymouth); _ygg_done=1; fi
  done
  if [[ $_ygg_done == 0 ]]; then
    _ygg_hooks=()
    for _h in "${HOOKS[@]}"; do
      _ygg_hooks+=("$_h")
      if [[ $_ygg_done == 0 && ( $_h == udev || $_h == systemd ) ]]; then _ygg_hooks+=(plymouth); _ygg_done=1; fi
    done
  fi
  HOOKS=("${_ygg_hooks[@]}")
  unset _ygg_hooks _ygg_done _h
fi
"#;

pub const NVIDIA_DROPIN_TEXT: &str = r#"# Yggdrasil (Settings > Appearance > Boot screen): NVIDIA's driver starts inside the start-up image, so the boot screen is drawn
# through it at the screen's own resolution from the first moment (it loaded later and took the screen over mid-animation).
for _m in nvidia nvidia_modeset nvidia_uvm nvidia_drm; do
  [[ " ${MODULES[*]} " == *" $_m "* ]] || MODULES+=("$_m")
done
unset _m
"#;

pub const PICTURE_SCRIPT: &str = r#"# Yggdrasil: your own picture in the middle of the screen while the computer starts (Settings > Appearance > Boot screen).
Window.SetBackgroundTopColor(0, 0, 0);
Window.SetBackgroundBottomColor(0, 0, 0);
pic = Image("picture.png");
W = Window.GetWidth();
H = Window.GetHeight();
scale = 1;
if (pic.GetWidth() > W * 0.9) scale = W * 0.9 / pic.GetWidth();
if (pic.GetHeight() * scale > H * 0.9) scale = H * 0.9 / pic.GetHeight();
pic = pic.Scale(Math.Int(pic.GetWidth() * scale), Math.Int(pic.GetHeight() * scale));
s = Sprite(pic);
s.SetX(Math.Int((W - pic.GetWidth()) / 2));
s.SetY(Math.Int((H - pic.GetHeight()) / 2));
"#;

/// The paths, each from its environment variable as the script took them.
pub struct Paths {
    pub themes: String,
    pub integrations: String,
    pub dropin: String,
    pub nvidia_dropin: String,
    pub grub_default: String,
}

impl Paths {
    pub fn get() -> Paths {
        Paths {
            themes: py::env_or("BOOTSCREEN_THEMES", "/usr/share/plymouth/themes"),
            integrations: py::env_or("BOOTSCREEN_INTEGRATIONS", "/usr/share/yggdrasil/integrations"),
            dropin: py::env_or("BOOTSCREEN_DROPIN", "/etc/mkinitcpio.conf.d/yggdrasil-plymouth.conf"),
            nvidia_dropin: py::env_or("BOOTSCREEN_NVIDIA_DROPIN", "/etc/mkinitcpio.conf.d/yggdrasil-plymouth-nvidia.conf"),
            grub_default: py::env_or("BOOTSCREEN_GRUB", "/etc/default/grub"),
        }
    }
}

fn exists(p: &str) -> bool {
    Path::new(p).exists()
}

fn isdir(p: &str) -> bool {
    Path::new(p).is_dir()
}

/// key = value lines of a text: (key, value) of each line with an "=", both stripped.
fn kv(text: &str) -> Vec<(String, String)> {
    pycompat::splitlines(text)
        .into_iter()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (pycompat::strip(k).to_string(), pycompat::strip(v).to_string())))
        .collect()
}

/// glob(DIR/*.conf), sorted: the names ending in .conf that do not start with a dot.
fn confs(dir: &str) -> Vec<String> {
    let mut out: Vec<String> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.ends_with(".conf") && !n.starts_with('.'))
            .map(|n| py::join(dir, &n))
            .collect(),
        Err(_) => vec![],
    };
    out.sort();
    out
}

/// The operating system's own animation: boot_theme in its branding file, else Plymouth's plain spinner.
pub fn os_theme(p: &Paths) -> R<String> {
    for f in confs(&p.integrations) {
        let Some(text) = read_plain(&f)? else { continue };
        let pairs = kv(&text);
        if !pairs.iter().any(|(k, v)| k == "kind" && v == "branding") {
            continue;
        }
        for (k, v) in &pairs {
            if k == "boot_theme" && !v.is_empty() && isdir(&py::join(&p.themes, v)) {
                return Ok(v.clone());
            }
        }
    }
    Ok("spinner".into())
}

/// A text file read whole, as open(...).read() in universal-newline mode (None: an OSError; Err: not UTF-8).
fn read_plain(path: &str) -> R<Option<String>> {
    py::read_text(path)
}

/// GRUB_CMDLINE_LINUX_DEFAULT=... with "splash" added (the same line when it is there already).
pub fn with_splash(line: &str) -> String {
    let (key, value) = match line.split_once('=') {
        Some((k, v)) => (k, v),
        None => (line, ""),
    };
    let first = value.chars().next();
    let quote = match first {
        Some(c @ ('\'' | '"')) => c.to_string(),
        _ => String::new(),
    };
    let inner = if !quote.is_empty() && value.ends_with(quote.as_str()) {
        // value[1:-1]
        let mut cs: Vec<char> = value.chars().collect();
        cs.remove(0);
        cs.pop();
        cs.into_iter().collect::<String>()
    } else {
        value.to_string()
    };
    let mut words: Vec<&str> = pycompat::split(&inner);
    if words.contains(&"splash") {
        return line.to_string();
    }
    words.push("splash");
    format!("{key}={quote}{}{quote}", words.join(" "))
}

/// GRUB's settings with "splash" in GRUB_CMDLINE_LINUX_DEFAULT.
pub fn grub_splash(text: &str) -> String {
    text.split('\n')
        .map(|l| if l.starts_with("GRUB_CMDLINE_LINUX_DEFAULT=") { with_splash(l) } else { l.to_string() })
        .collect::<Vec<_>>()
        .join("\n")
}

fn current_theme() -> R<String> {
    Ok(pycompat::strip(&py::run(&["plymouth-set-default-theme"], None)?.stdout).to_string())
}

pub fn choice_of(theme: &str) -> &'static str {
    match theme {
        MESSAGES_THEME => "messages",
        PICTURE_THEME => "picture",
        _ => "animation",
    }
}

fn read_must(path: &str) -> R<String> {
    match fs::read(path) {
        Ok(b) => String::from_utf8(b).map(|t| py::universal(&t)).map_err(|e| Crash(format!("UnicodeDecodeError: {path}: {e}"))),
        Err(e) => Err(Crash(py::oserror(&e, Some(path)))),
    }
}

fn cmdline_splash() -> R<bool> {
    Ok(pycompat::split(&read_must("/proc/cmdline")?).contains(&"splash"))
}

/// Plymouth is set up: in the start-up image, and "splash" in the kernel options (GRUB's settings; the running kernel only has
/// it from the next start).
pub fn ready(p: &Paths) -> R<bool> {
    let hooked = exists(&p.dropin) || {
        let conf = read_must("/etc/mkinitcpio.conf")?;
        let after = conf.splitn(2, "HOOKS=").last().unwrap_or("");
        after.split('\n').next().unwrap_or("").contains("plymouth")
    };
    let splash = match fs::read(&p.grub_default) {
        Ok(b) => {
            let grub = String::from_utf8(b)
                .map(|t| py::universal(&t))
                .map_err(|e| Crash(format!("UnicodeDecodeError: {}: {e}", p.grub_default)))?;
            pycompat::splitlines(&grub).iter().any(|l| {
                l.starts_with("GRUB_CMDLINE_LINUX_DEFAULT=") && {
                    let v = l.split_once('=').map(|x| x.1).unwrap_or("");
                    pycompat::split(v.trim_matches(|c| c == '\'' || c == '"' || c == ' ')).contains(&"splash")
                }
            })
        }
        Err(_) => cmdline_splash()?,
    };
    Ok(hooked && splash)
}

fn get(p: &Paths) -> R<i32> {
    let theme = current_theme()?;
    let mut m = Map::new();
    m.insert("ready".into(), ready(p)?.into());
    m.insert("grub".into(), exists(&p.grub_default).into());
    m.insert("theme".into(), theme.clone().into());
    m.insert("choice".into(), choice_of(&theme).into());
    m.insert("animation".into(), os_theme(p)?.into());
    m.insert("splash_now".into(), cmdline_splash()?.into());
    println!("{}", json::dumps(&Value::Object(m)));
    Ok(0)
}

/// Pillow's thumbnail() size for a picture of `w` x `h` in a box of `bw` x `bh`: None when it fits already (it is kept as it
/// is), else the size that keeps its shape, each side rounded to whichever of floor and ceiling keeps the shape better.
pub fn thumbnail_size(w: u32, h: u32, bw: u32, bh: u32) -> Option<(u32, u32)> {
    if bw >= w && bh >= h {
        return None;
    }
    let aspect = w as f64 / h as f64;
    let (x, y) = (bw as f64, bh as f64);
    // min(floor, ceil, key=...): the floor when the two keep the shape equally well
    let round_aspect = |n: f64, key: &dyn Fn(f64) -> f64| -> u32 {
        let (f, c) = (n.floor(), n.ceil());
        let pick = if key(c) < key(f) { c } else { f };
        (pick.max(1.0)) as u32
    };
    if x / y >= aspect {
        let nx = round_aspect(y * aspect, &|n| (aspect - n / y).abs());
        Some((nx, bh))
    } else {
        let ny = round_aspect(x / aspect, &|n| if n == 0.0 { 0.0 } else { (aspect - x / n).abs() });
        Some((bw, ny))
    }
}

/// Pillow refused a picture of more than twice Image.MAX_IMAGE_PIXELS (64 000 000) pixels before decoding it (and only
/// warned above once that).
pub const MAX_PIXELS: u64 = 2 * 64_000_000;

/// Your own picture as the Plymouth theme yggdrasil-picture: picture.png (at most 3840 x 2160, RGBA), the script and the
/// theme file. Only PNG, JPEG and WebP are decoded.
pub fn make_picture_theme(p: &Paths, src: &str) -> R<()> {
    let d = py::join(&p.themes, PICTURE_THEME);
    fs::create_dir_all(&d).map_err(|e| Crash(py::oserror(&e, Some(&d))))?;
    let unidentified = || Crash(format!("PIL.UnidentifiedImageError: cannot identify image file {}", pycompat::repr(src)));
    let reader =
        image::ImageReader::open(src).map_err(|e| Crash(py::oserror(&e, Some(src))))?.with_guessed_format().map_err(|_| unidentified())?;
    match reader.format() {
        Some(image::ImageFormat::Png) | Some(image::ImageFormat::Jpeg) | Some(image::ImageFormat::WebP) => {}
        _ => return Err(unidentified()),
    }
    let (w, h) = image::ImageReader::open(src)
        .and_then(|r| r.with_guessed_format())
        .map_err(|_| unidentified())?
        .into_dimensions()
        .map_err(|_| unidentified())?;
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(Crash(format!(
            "PIL.Image.DecompressionBombError: Image size ({} pixels) exceeds limit of {MAX_PIXELS} pixels, could be decompression bomb DOS attack.",
            w as u64 * h as u64
        )));
    }
    let img = reader.decode().map_err(|e| Crash(format!("OSError: {e}")))?;
    let img = match thumbnail_size(img.width(), img.height(), 3840, 2160) {
        Some((nw, nh)) => img.resize_exact(nw, nh, image::imageops::FilterType::CatmullRom),
        None => img,
    };
    let out = py::join(&d, "picture.png");
    img.to_rgba8().save_with_format(&out, image::ImageFormat::Png).map_err(|e| Crash(format!("OSError: {e}")))?;
    let script = py::join(&d, &format!("{PICTURE_THEME}.script"));
    fs::write(&script, PICTURE_SCRIPT).map_err(|e| Crash(py::oserror(&e, Some(&script))))?;
    let theme = py::join(&d, &format!("{PICTURE_THEME}.plymouth"));
    let text = format!(
        "[Plymouth Theme]\nName=Your picture\nDescription=Your own picture while the computer starts\nModuleName=script\n\n[script]\nImageDir={d}\nScriptFile={d}/{PICTURE_THEME}.script\n"
    );
    fs::write(&theme, text).map_err(|e| Crash(py::oserror(&e, Some(&theme))))?;
    Ok(())
}

/// sys.exit(message): the message on standard error, exit code 1.
fn exit_msg(msg: &str) -> R<i32> {
    eprintln!("{msg}");
    Ok(1)
}

fn makedirs_for(path: &str) -> R<()> {
    let d = py::dirname(path);
    fs::create_dir_all(&d).map_err(|e| Crash(py::oserror(&e, Some(&d))))
}

fn write_file(path: &str, text: &str) -> R<()> {
    fs::write(path, text).map_err(|e| Crash(py::oserror(&e, Some(path))))
}

/// shutil.copy2: the bytes, the permissions and the times.
pub fn copy2(src: &str, dst: &str) -> R<()> {
    fs::copy(src, dst).map_err(|e| Crash(py::oserror(&e, Some(src))))?;
    let m = fs::metadata(src).map_err(|e| Crash(py::oserror(&e, Some(src))))?;
    use std::os::unix::fs::MetadataExt;
    let times =
        [libc::timespec { tv_sec: m.atime(), tv_nsec: m.atime_nsec() }, libc::timespec { tv_sec: m.mtime(), tv_nsec: m.mtime_nsec() }];
    if let Ok(c) = std::ffi::CString::new(dst) {
        unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) };
    }
    Ok(())
}

/// (r.stderr or r.stdout).strip()[-n:]
fn tail_of(r: &py::Output, n: usize) -> String {
    let t = if r.stderr.is_empty() { &r.stdout } else { &r.stderr };
    py::last_chars(pycompat::strip(t), n)
}

fn set(p: &Paths, what: &str, arg: Option<&str>) -> R<i32> {
    if unsafe { libc::geteuid() } != 0 {
        return exit_msg("bootscreen set needs root (Settings runs it with pkexec)");
    }
    // the script worked out the OS's own animation whatever was asked for
    let os = os_theme(p)?;
    let theme = match what {
        "animation" => os,
        "messages" => MESSAGES_THEME.to_string(),
        "picture" => PICTURE_THEME.to_string(),
        _ => return exit_msg("set animation | messages | picture FILE"),
    };
    if what == "picture" {
        match arg {
            Some(a) if !a.is_empty() && Path::new(a).is_file() => make_picture_theme(p, a)?,
            _ => return exit_msg("the picture was not found"),
        }
    }
    // NVIDIA: its driver in the start-up image too (the boot screen is drawn through it from the start)
    if isdir("/sys/module/nvidia") && !exists(&p.nvidia_dropin) {
        makedirs_for(&p.nvidia_dropin)?;
        write_file(&p.nvidia_dropin, NVIDIA_DROPIN_TEXT)?;
    }
    // Plymouth in the start-up image
    if !exists(&p.dropin) {
        makedirs_for(&p.dropin)?;
        write_file(&p.dropin, DROPIN_TEXT)?;
    }
    // "splash" on the kernel's command line (GRUB)
    let g = &p.grub_default;
    if exists(g) {
        let old = read_must(g)?;
        let new = grub_splash(&old);
        if new != old {
            let kept = format!("{g}.before-bootscreen");
            if !exists(&kept) {
                copy2(g, &kept)?;
            }
            write_file(g, &new)?;
            let r = py::run(&["grub-mkconfig", "-o", "/boot/grub/grub.cfg"], None)?;
            if r.code != 0 {
                copy2(&kept, g)?;
                return exit_msg(&format!(
                    "GRUB could not be updated, nothing was changed: {}",
                    py::last_chars(pycompat::strip(&r.stderr), 300)
                ));
            }
        }
    }
    let r = py::run(&["plymouth-set-default-theme", theme.as_str()], None)?;
    if r.code != 0 {
        return exit_msg(&format!("Plymouth did not take the theme: {}", tail_of(&r, 300)));
    }
    let r = py::run(&["mkinitcpio", "-P"], None)?;
    if r.code != 0 {
        return exit_msg(&format!("the start-up image could not be rebuilt: {}", tail_of(&r, 400)));
    }
    println!("boot screen: {what} ({theme}); shows from the next start");
    Ok(0)
}

fn dispatch(argv: &[String]) -> R<i32> {
    let a = &argv[1.min(argv.len())..];
    let p = Paths::get();
    match a.first().map(String::as_str) {
        Some("get") => get(&p),
        Some("set") if a.len() >= 2 => set(&p, &a[1], a.get(2).map(String::as_str)),
        _ => exit_msg(DOC),
    }
}

pub fn main(argv: &[String]) -> i32 {
    match dispatch(argv) {
        Ok(c) => c,
        Err(c) => py::crashed("bootscreen", &c),
    }
}

#[cfg(test)]
mod tests;
