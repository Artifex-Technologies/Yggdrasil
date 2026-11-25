//! The Python script's tests (helpers/tests/test_bootscreen.py), carried over: GRUB's kernel options, the OS's own
//! animation, the mkinitcpio drop-ins (run in bash, as the script's test did), a picture decoded only from plain formats.

use super::*;
use crate::testutil::TempDir;

fn paths(d: &TempDir) -> Paths {
    Paths {
        themes: d.p("themes"),
        integrations: d.p("int"),
        dropin: d.p("dropin.conf"),
        nvidia_dropin: d.p("nvidia.conf"),
        grub_default: d.p("grub"),
    }
}

#[test]
fn test_splash_is_added_once_and_quotes_kept() {
    let g = "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX_DEFAULT='quiet loglevel=3'\nGRUB_CMDLINE_LINUX=\"\"\n";
    let once = grub_splash(g);
    assert!(once.contains("GRUB_CMDLINE_LINUX_DEFAULT='quiet loglevel=3 splash'"));
    assert!(once.contains("GRUB_CMDLINE_LINUX=\"\""), "the other line stays");
    assert_eq!(grub_splash(&once), once, "not twice");
    assert_eq!(with_splash("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet\""), "GRUB_CMDLINE_LINUX_DEFAULT=\"quiet splash\"");
}

#[test]
fn test_the_os_names_its_own_animation() {
    let d = TempDir::new();
    std::fs::create_dir_all(d.0.join("themes/labyrinth-rain")).unwrap();
    std::fs::create_dir_all(d.0.join("int")).unwrap();
    let p = paths(&d);
    assert_eq!(os_theme(&p).unwrap(), "spinner", "no branding: Plymouth's own");
    std::fs::write(d.0.join("int/x-branding.conf"), "kind = branding\nboot_theme = labyrinth-rain\n").unwrap();
    assert_eq!(os_theme(&p).unwrap(), "labyrinth-rain");
    std::fs::write(d.0.join("int/x-branding.conf"), "kind = branding\nboot_theme = not-installed\n").unwrap();
    assert_eq!(os_theme(&p).unwrap(), "spinner", "a theme that is not there");
}

#[test]
fn test_choices() {
    assert_eq!(choice_of("details"), "messages");
    assert_eq!(choice_of("yggdrasil-picture"), "picture");
    assert_eq!(choice_of("labyrinth-rain"), "animation");
}

fn bash(script: &str) -> String {
    let out = std::process::Command::new("bash").args(["-c", script]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn test_the_nvidia_dropin_adds_its_modules_once() {
    for (mods, want) in
        [("", "nvidia nvidia_modeset nvidia_uvm nvidia_drm"), ("btrfs nvidia", "btrfs nvidia nvidia_modeset nvidia_uvm nvidia_drm")]
    {
        let script = format!("MODULES=({mods})\n{NVIDIA_DROPIN_TEXT}\n{NVIDIA_DROPIN_TEXT}\necho \"${{MODULES[*]}}\"\n");
        assert_eq!(bash(&script), want);
    }
}

#[test]
fn test_the_dropin_puts_plymouth_after_kms() {
    for (hooks, want) in [
        (
            "base udev autodetect microcode kms modconf block filesystems",
            "base udev autodetect microcode kms plymouth modconf block filesystems",
        ),
        ("base udev autodetect block filesystems", "base udev plymouth autodetect block filesystems"),
        ("base udev plymouth kms block", "base udev plymouth kms block"),
    ] {
        let script = format!("HOOKS=({hooks})\n{DROPIN_TEXT}\necho \"${{HOOKS[*]}}\"\n");
        assert_eq!(bash(&script), want);
    }
}

#[test]
fn test_a_picture_is_decoded_only_from_plain_formats() {
    let d = TempDir::new();
    let p = paths(&d);
    // an EPS named .png: Pillow would have handed it to Ghostscript, as root
    let eps = d.p("trap.png");
    std::fs::write(&eps, "%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 10 10\nshowpage\n").unwrap();
    let e = make_picture_theme(&p, &eps).unwrap_err();
    assert!(e.0.contains("UnidentifiedImageError"), "{e:?}");
    let png = d.p("ok.png");
    image::RgbImage::new(8, 8).save(&png).unwrap();
    make_picture_theme(&p, &png).unwrap();
    assert!(d.0.join("themes/yggdrasil-picture/picture.png").exists());
}

// ---- more

#[test]
fn other_formats_are_refused_whatever_their_name() {
    let d = TempDir::new();
    let p = paths(&d);
    for (name, bytes) in [
        ("a.jpg", b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".to_vec()),
        ("b.png", b"BM\x1e\x00\x00\x00\x00\x00\x00\x00\x1a\x00\x00\x00".to_vec()),
        ("c.webp", b"II*\x00\x08\x00\x00\x00".to_vec()),
        ("d.png", b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec()),
        ("e.png", vec![]),
    ] {
        let f = d.p(name);
        std::fs::write(&f, bytes).unwrap();
        assert!(make_picture_theme(&p, &f).is_err(), "{name}");
    }
    assert!(!d.0.join("themes/yggdrasil-picture/picture.png").exists());
}

#[test]
fn a_large_picture_is_made_screen_sized_as_pillow_did() {
    for ((w, h), want) in [
        ((7680, 4320), Some((3840, 2160))),
        ((5000, 1000), Some((3840, 768))),
        ((1000, 5000), Some((432, 2160))),
        ((3841, 2161), Some((3839, 2160))),
        ((3000, 3000), Some((2160, 2160))),
        ((4000, 2251), Some((3838, 2160))),
        ((100, 100), None),
        ((3840, 2160), None),
        ((3841, 10), Some((3840, 10))),
        ((10, 2161), Some((10, 2160))),
        ((12345, 6789), Some((3840, 2112))),
    ] {
        assert_eq!(thumbnail_size(w, h, 3840, 2160), want, "{w}x{h}");
    }
    let d = TempDir::new();
    let p = paths(&d);
    let png = d.p("wide.png");
    image::RgbImage::new(5000, 1000).save(&png).unwrap();
    make_picture_theme(&p, &png).unwrap();
    let out = image::open(d.0.join("themes/yggdrasil-picture/picture.png")).unwrap();
    assert_eq!((out.width(), out.height()), (3840, 768));
    assert!(matches!(out, image::DynamicImage::ImageRgba8(_)), "RGBA, as convert(\"RGBA\") wrote it");
    let theme = std::fs::read_to_string(d.0.join("themes/yggdrasil-picture/yggdrasil-picture.plymouth")).unwrap();
    assert!(theme.contains(&format!("ImageDir={}/themes/yggdrasil-picture\n", d.0.display())));
}

#[test]
fn grub_lines_as_the_script_wrote_them() {
    // a value with an opening quote and no closing one keeps it, as value[1:-1] was not taken
    assert_eq!(with_splash("GRUB_CMDLINE_LINUX_DEFAULT='quiet"), "GRUB_CMDLINE_LINUX_DEFAULT=''quiet splash'");
    assert_eq!(with_splash("GRUB_CMDLINE_LINUX_DEFAULT="), "GRUB_CMDLINE_LINUX_DEFAULT=splash");
    assert_eq!(with_splash("GRUB_CMDLINE_LINUX_DEFAULT='"), "GRUB_CMDLINE_LINUX_DEFAULT='splash'");
    assert_eq!(with_splash("GRUB_CMDLINE_LINUX_DEFAULT=\"a  b\tc\""), "GRUB_CMDLINE_LINUX_DEFAULT=\"a b c splash\"");
    assert_eq!(with_splash("GRUB_CMDLINE_LINUX_DEFAULT='splash quiet'"), "GRUB_CMDLINE_LINUX_DEFAULT='splash quiet'");
    assert_eq!(
        grub_splash("# GRUB_CMDLINE_LINUX_DEFAULT=x\n GRUB_CMDLINE_LINUX_DEFAULT=y"),
        "# GRUB_CMDLINE_LINUX_DEFAULT=x\n GRUB_CMDLINE_LINUX_DEFAULT=y"
    );
}

#[test]
fn only_a_branding_file_names_the_animation() {
    let d = TempDir::new();
    std::fs::create_dir_all(d.0.join("themes/rain")).unwrap();
    std::fs::create_dir_all(d.0.join("themes/snow")).unwrap();
    std::fs::create_dir_all(d.0.join("int/z.conf")).unwrap();
    let p = paths(&d);
    std::fs::write(d.0.join("int/a.conf"), "kind = assistant\nboot_theme = snow\n").unwrap();
    std::fs::write(d.0.join("int/.hidden.conf"), "kind = branding\nboot_theme = snow\n").unwrap();
    std::fs::write(d.0.join("int/b.txt"), "kind = branding\nboot_theme = snow\n").unwrap();
    assert_eq!(os_theme(&p).unwrap(), "spinner", "not branding, hidden, not .conf, a folder");
    // the first boot_theme that is installed, in the files' sorted order
    std::fs::write(d.0.join("int/m.conf"), "boot_theme =\nboot_theme = gone\nkind=branding\nboot_theme = rain\n").unwrap();
    std::fs::write(d.0.join("int/n.conf"), "kind = branding\nboot_theme = snow\n").unwrap();
    assert_eq!(os_theme(&p).unwrap(), "rain");
    // a file that is not UTF-8: the script stopped there
    std::fs::write(d.0.join("int/c.conf"), b"kind = branding\xff\n").unwrap();
    assert!(os_theme(&p).is_err());
}

/// YGG-73, run by hand (`cargo test --release --lib -- --ignored a_12_mp_photo_peak --nocapture`): the peak memory of
/// making the picture theme from a 12 MP photo (4000 x 3000 JPEG), from a peak reset just before it (/proc/self/clear_refs);
/// the bar is the Python's 129.8 MB for the whole program (docs/PORT_NUMBERS.md).
#[test]
#[ignore = "a measurement: run by hand, in release"]
fn a_12_mp_photo_peak() {
    let d = std::env::temp_dir().join(format!("bootscreen-peak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("themes")).unwrap();
    let photo = d.join("photo.jpg");
    {
        let img =
            image::RgbImage::from_fn(4000, 3000, |x, y| image::Rgb([(x * 7 + y) as u8, ((y * 5) ^ x) as u8, ((x / 9) * (y / 7)) as u8]));
        img.save_with_format(&photo, image::ImageFormat::Jpeg).unwrap();
    }
    let p = Paths {
        themes: d.join("themes").to_string_lossy().into_owned(),
        integrations: String::new(),
        dropin: String::new(),
        nvidia_dropin: String::new(),
        grub_default: String::new(),
    };
    let hwm = || std::fs::read_to_string("/proc/self/status").unwrap().lines().find(|l| l.starts_with("VmHWM")).unwrap().to_string();
    std::fs::write("/proc/self/clear_refs", "5").unwrap();
    let before = hwm();
    make_picture_theme(&p, &photo.to_string_lossy()).unwrap();
    let after = hwm();
    let out = image::open(d.join("themes/yggdrasil-picture/picture.png")).unwrap();
    eprintln!("PEAK before {before} | after {after} | picture {}x{} {:?}", out.width(), out.height(), out.color());
    let _ = std::fs::remove_dir_all(&d);
}

/// YGG-73: the row-at-a-time resize gives image's Catmull-Rom resize, to one level (float sums in another order), for an RGB
/// and an RGBA picture, shrinking both ways and one way only.
#[test]
fn the_shrink_matches_images_resize() {
    let rgb = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(301, 199, |x, y| {
        image::Rgb([(x * 13 + y * 3) as u8, (x ^ y) as u8, ((x * y) % 251) as u8])
    }));
    let rgba = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(120, 333, |x, y| {
        image::Rgba([(x * 5) as u8, (y * 7) as u8, ((x + y) * 3) as u8, ((x * y) % 256) as u8])
    }));
    for (img, w, h) in [(&rgb, 97, 64), (&rgb, 301, 50), (&rgba, 31, 87), (&rgba, 40, 333)] {
        let ours = shrink(img.clone(), w, h);
        let theirs = img.resize_exact(w, h, image::imageops::FilterType::CatmullRom).into_rgba8();
        assert_eq!(ours.dimensions(), theirs.dimensions());
        let worst = ours.as_raw().iter().zip(theirs.as_raw()).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 1, "{w}x{h}: off by {worst}");
    }
}
