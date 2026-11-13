//! The login screen in cage with three made-up screens (wlroots' headless outputs, side by side): the sign-in is on the main
//! display only and every other screen is plain black. Without a choice the main display is the top-left screen; with one (a
//! system copy naming HEADLESS-2) it is the middle one; a copy that cannot be trusted is ignored.
//!
//! Needs cage, grim and a font; it skips without them unless YGGDRASIL_TEST_CAGE_REQUIRED is set (CI's headless job).
//! YGGDRASIL_TEST_SHOTS=DIR keeps the pictures there.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SCREENS: usize = 3;

fn have(tool: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {tool}")]).stdout(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// A picture from grim as PPM (P6): width, height and RGB bytes.
fn ppm(bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let mut fields = vec![];
    let mut i = 0;
    while fields.len() < 4 {
        while bytes.get(i)?.is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while !bytes.get(i)?.is_ascii_whitespace() {
            i += 1;
        }
        fields.push(std::str::from_utf8(&bytes[start..i]).ok()?.to_string());
    }
    let (w, h): (usize, usize) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    let data = bytes.get(i + 1..i + 1 + w * h * 3)?.to_vec();
    (fields[0] == "P6").then_some((w, h, data))
}

/// How many pixels of each screen (equal widths, side by side) are not black.
fn lit_per_screen(w: usize, h: usize, rgb: &[u8]) -> Vec<usize> {
    let sw = w / SCREENS;
    (0..SCREENS)
        .map(|s| (0..h).flat_map(|y| (s * sw..(s + 1) * sw).map(move |x| (y * w + x) * 3)).filter(|&p| rgb[p..p + 3] != [0, 0, 0]).count())
        .collect()
}

/// Runs the greeter in cage with `display` as its system copy until the sign-in is drawn; the screens' lit pixel counts.
fn run(tmp: &Path, name: &str, display: &Path) -> Vec<usize> {
    let runtime = tmp.join(format!("run-{name}"));
    std::fs::create_dir_all(&runtime).unwrap();
    Command::new("chmod").args(["700"]).arg(&runtime).status().unwrap();
    let conf = tmp.join("greeter.conf");
    std::fs::write(&conf, "design = split\n").unwrap();
    let mut cage = Command::new("cage")
        .args(["--", env!("CARGO_BIN_EXE_yggdrasil-greeter")])
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("WLR_BACKENDS", "headless")
        .env("WLR_HEADLESS_OUTPUTS", SCREENS.to_string())
        .env("WLR_RENDERER", "pixman")
        .env("WLR_LIBINPUT_NO_DEVICES", "1")
        .env("GREETER_CONF", &conf)
        .env("YGGDRASIL_GREETER_USERS", "alex")
        .env("YGGDRASIL_GREETER_DISPLAY", display)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cage");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = vec![];
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        let Ok(out) = Command::new("grim")
            .args(["-t", "ppm", "-"])
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("WAYLAND_DISPLAY", "wayland-0")
            .stderr(Stdio::null())
            .output()
        else {
            continue;
        };
        let Some((w, h, rgb)) = ppm(&out.stdout) else { continue };
        assert_eq!(w % SCREENS, 0, "{SCREENS} screens of one width side by side: {w}x{h}");
        last = lit_per_screen(w, h, &rgb);
        // drawn: one screen has the sign-in (the clock changes nothing in which screens are lit)
        if last.iter().any(|&n| n > w * h / SCREENS / 10) {
            if let Some(dir) = std::env::var_os("YGGDRASIL_TEST_SHOTS") {
                let _ = Command::new("grim")
                    .arg(PathBuf::from(dir).join(format!("greeter-{name}.png")))
                    .env("XDG_RUNTIME_DIR", &runtime)
                    .env("WAYLAND_DISPLAY", "wayland-0")
                    .status();
            }
            break;
        }
    }
    let _ = cage.kill();
    let _ = cage.wait();
    last
}

#[test]
fn the_sign_in_is_on_the_main_display_and_the_others_are_black() {
    if !(have("cage") && have("grim") && have("fc-match")) {
        assert!(std::env::var_os("YGGDRASIL_TEST_CAGE_REQUIRED").is_none(), "cage, grim and fontconfig are required");
        eprintln!("skipped: cage, grim or fontconfig missing");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("ygg-greeter-screens-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let only = |lit: &[usize], main: usize| lit.iter().enumerate().all(|(i, &n)| if i == main { n > 0 } else { n == 0 });

    // no choice anywhere: the top-left screen
    let lit = run(&tmp, "no-choice", &tmp.join("missing.conf"));
    assert!(only(&lit, 0), "lit pixels per screen: {lit:?}");

    // the latest sign-in chose the middle screen
    let chosen = tmp.join("display.conf");
    std::fs::write(&chosen, "# made-up\nmain_make = \nmain_model = \nmain_connector = HEADLESS-2\n").unwrap();
    Command::new("chmod").args(["644"]).arg(&chosen).status().unwrap();
    let lit = run(&tmp, "chosen-middle", &chosen);
    assert!(only(&lit, 1), "lit pixels per screen: {lit:?}");

    // a copy anyone may write is not trusted: the top-left screen again
    Command::new("chmod").args(["666"]).arg(&chosen).status().unwrap();
    let lit = run(&tmp, "untrusted", &chosen);
    assert!(only(&lit, 0), "lit pixels per screen: {lit:?}");
    let _ = std::fs::remove_dir_all(&tmp);
}
