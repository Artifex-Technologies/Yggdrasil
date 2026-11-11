//! ygg-cursors as a program on a scratch folder: the files and names programs ask for, the X cursor format, and every
//! cursor's bytes and the preview's pixels against what Python's ygg-cursors (Pillow 13.0) wrote. The run has no fc-match
//! on its PATH, so the help cursor's "?" is in Pillow's own default font, which needs nothing from the system but
//! FreeType; the cursors' pixels do not depend on anything else.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Python's ygg-cursors (python/ygg-cursors with Pillow 13.0.0, no fc-match): FNV-1a 64 of each cursor file
const GOLDEN: [(&str, u64); 22] = [
    ("alias", 0xf2f293ef7cb04ef5),
    ("cell", 0xac35df786996b6f0),
    ("context-menu", 0x3411e594ad306eba),
    ("copy", 0x26d4b8969da2ef00),
    ("crosshair", 0xc267d635cb5d3f63),
    ("ew-resize", 0x21f1523b5178ab11),
    ("grab", 0x46f5682a3087a569),
    ("grabbing", 0xcef5ad4ce80c0b50),
    ("help", 0x4afe20e3ba66ee3a),
    ("left_ptr", 0x7e2b5cfa1fda5795),
    ("move", 0x8680ebf325035c92),
    ("nesw-resize", 0x56ea667d3b48de66),
    ("not-allowed", 0x2f8bebc9c7a39c23),
    ("ns-resize", 0xead8e3d183fd2c04),
    ("nwse-resize", 0x2ae49c759d2809ee),
    ("pointer", 0x7545c4fa2b15ed37),
    ("progress", 0x42c239b6b5dcf0b8),
    ("text", 0x669e53e76c81dfed),
    ("vertical-text", 0x0809e49fff9c5da2),
    ("wait", 0xe814099fb3320bec),
    ("zoom-in", 0x6e377313e3929a00),
    ("zoom-out", 0x24eb60f2864e9db6),
];
/// the preview sheet's decoded RGBA pixels (768 x 288)
const GOLDEN_PREVIEW: u64 = 0x0f92c22e0548e5ae;

/// FreeType's version, read from the library itself: the help cursor's "?" (and the preview, which shows it) are FreeType's
/// own rendering, so their hashes hold for the FreeType they were made with (2.15.0)
fn freetype_version() -> Option<(i32, i32, i32)> {
    unsafe {
        let h = libc::dlopen(
            c"libfreetype.so.6".as_ptr(),
            libc::RTLD_NOW | libc::RTLD_LOCAL,
        );
        if h.is_null() {
            return None;
        }
        let init = libc::dlsym(h, c"FT_Init_FreeType".as_ptr());
        let version = libc::dlsym(h, c"FT_Library_Version".as_ptr());
        if init.is_null() || version.is_null() {
            return None;
        }
        let init: unsafe extern "C" fn(*mut *mut libc::c_void) -> libc::c_int =
            std::mem::transmute(init);
        let version: unsafe extern "C" fn(
            *mut libc::c_void,
            *mut libc::c_int,
            *mut libc::c_int,
            *mut libc::c_int,
        ) = std::mem::transmute(version);
        let mut lib = std::ptr::null_mut();
        if init(&mut lib) != 0 {
            return None;
        }
        let (mut a, mut b, mut c) = (0, 0, 0);
        version(lib, &mut a, &mut b, &mut c);
        Some((a, b, c))
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn draw(name: &str, preview: bool) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ygg-cursors-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let empty = d.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_ygg-cursors"));
    c.arg(d.join("out")).env("PATH", &empty).env("HOME", &d);
    if preview {
        c.arg("--preview");
    }
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        format!(
            "96 cursor names, 22 shapes, in {}\n",
            d.join("out/cursors").display()
        )
    );
    d.join("out")
}

fn chunks(path: &Path) -> (std::collections::BTreeSet<u32>, Vec<[u32; 9]>) {
    let data = std::fs::read(path).unwrap();
    let u = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    assert_eq!(&data[..4], b"Xcur");
    assert_eq!((u(4), u(8)), (16, 0x10000));
    let n = u(12) as usize;
    let mut sizes = std::collections::BTreeSet::new();
    let mut heads = vec![];
    for i in 0..n {
        let (t, sub, pos) = (u(16 + 12 * i), u(20 + 12 * i), u(24 + 12 * i) as usize);
        assert_eq!(t, 0xFFFD0002);
        let head: [u32; 9] = std::array::from_fn(|k| u(pos + 4 * k));
        assert_eq!(head[..4], [36, 0xFFFD0002, sub, 1]);
        assert_eq!((head[4], head[5]), (sub, sub));
        assert!(head[6] < head[4] && head[7] < head[5]);
        assert!(pos + 36 + (head[4] * head[5] * 4) as usize <= data.len());
        sizes.insert(sub);
        heads.push(head);
    }
    (sizes, heads)
}

#[test]
fn the_theme_is_the_one_pillow_drew() {
    let out = draw("golden", true);
    let same_freetype = freetype_version() == Some((2, 15, 0));
    if !same_freetype {
        eprintln!(
            "FreeType {:?}, not 2.15.0: the help cursor and the preview are not compared",
            freetype_version()
        );
    }
    for (name, want) in GOLDEN {
        if name == "help" && !same_freetype {
            continue;
        }
        let got = fnv(&std::fs::read(out.join("cursors").join(name)).unwrap());
        assert_eq!(
            got, want,
            "{name}: not the cursor Python's ygg-cursors drew"
        );
    }
    // the preview: a PNG of our own (filter 0 rows), its pixels Pillow's
    if !same_freetype {
        return;
    }
    let png = std::fs::read(out.join("preview.png")).unwrap();
    let mut z = vec![];
    let mut i = 8;
    while i + 8 <= png.len() {
        let len = u32::from_be_bytes(png[i..i + 4].try_into().unwrap()) as usize;
        if &png[i + 4..i + 8] == b"IDAT" {
            z.extend_from_slice(&png[i + 8..i + 8 + len]);
        }
        i += 12 + len;
    }
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&z).unwrap();
    let pixels: Vec<u8> = raw
        .chunks(768 * 4 + 1)
        .flat_map(|row| row[1..].to_vec())
        .collect();
    assert_eq!(pixels.len(), 768 * 288 * 4);
    assert_eq!(fnv(&pixels), GOLDEN_PREVIEW);
}

#[test]
fn the_names_programs_ask_for_exist_and_are_well_formed() {
    let out = draw("names", false);
    let cur = out.join("cursors");
    for n in [
        "left_ptr",
        "default",
        "text",
        "pointer",
        "hand2",
        "wait",
        "progress",
        "crosshair",
        "not-allowed",
        "grabbing",
        "ew-resize",
        "ns-resize",
        "nwse-resize",
        "nesw-resize",
        "move",
        "help",
        "copy",
        "context-menu",
        "col-resize",
        "row-resize",
        "zoom-in",
        "all-scroll",
        "sb_h_double_arrow",
        "top_left_corner",
    ] {
        assert!(std::fs::symlink_metadata(cur.join(n)).is_ok(), "{n}");
    }
    assert_eq!(
        std::fs::read_link(cur.join("hand2")).unwrap(),
        PathBuf::from("pointer")
    );
    let (sizes, _) = chunks(&cur.join("left_ptr"));
    assert_eq!(
        sizes.into_iter().collect::<Vec<_>>(),
        vec![24, 32, 48, 64, 96]
    );
    // the busy cursor is animated: 12 frames in each of the 5 sizes, 80 ms each
    let (_, heads) = chunks(&cur.join("wait"));
    assert_eq!(heads.len(), 60);
    assert!(heads.iter().all(|h| h[8] == 80));
    assert!(std::fs::read_to_string(out.join("index.theme"))
        .unwrap()
        .contains("Name=Yggdrasil"));
    assert_eq!(std::fs::read_dir(&cur).unwrap().count(), 96);
}

#[test]
fn without_a_folder_it_prints_how_to_use_it() {
    let o = Command::new(env!("CARGO_BIN_EXE_ygg-cursors"))
        .arg("--preview")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stdout)
        .starts_with("ygg-cursors: draws Yggdrasil's mouse cursors"));
}
