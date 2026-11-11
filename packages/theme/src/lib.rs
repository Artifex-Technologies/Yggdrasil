//! ygg-theme in Rust: Yggdrasil's look for the programs Yggdrasil does not write (Qt, KDE and GTK programs) and for the mouse
//! pointer, replacing the Python script at the same path with the same commands, output and files (doc.txt is its help).
//!
//!   colors.rs  the palette, the KDE colour scheme, the GTK colours, the system GTK 3 and GTK 4 themes (GTK 4's own theme,
//!              read from the installed libgtk-4 at run time, with its colours replaced)
//!   files.rs   this user's files: appearance.conf, GTK's settings.ini and gtk.css, xsettingsd, the pointer, KDE's files
//!              only when a program built on KDE's libraries is installed
//!
//!   cursors/   ygg-cursors: the cursor theme, drawn as the Python script drew it with Pillow (the package runs it when it
//!              is built)

pub mod colors;
pub mod cursors;
pub mod files;

use colors::{gtk3_theme_css, gtk4_theme_css, gtk_colors, parse_accent, rgb, scheme, DEFAULT_ACCENT};

pub const DOC: &str = include_str!("doc.txt");

fn dispatch(argv: &[String]) -> files::R<i32> {
    use std::io::Write;
    if argv.is_empty() || argv[0] == "-h" || argv[0] == "--help" {
        println!("{DOC}");
        return Ok(0);
    }
    let cmd = argv[0].as_str();
    let dark_arg = || argv.get(1).map(String::as_str).unwrap_or("dark") != "light";
    match cmd {
        "gtk4-theme" => {
            print!("{}", gtk4_theme_css(dark_arg()));
            let _ = std::io::stdout().flush();
            Ok(0)
        }
        "gtk3-theme" => {
            print!("{}", gtk3_theme_css(dark_arg()));
            Ok(0)
        }
        "scheme" | "gtk" => {
            let dark = dark_arg();
            let acc = if argv.len() > 2 { parse_accent(&argv[2]) } else { Some(DEFAULT_ACCENT) };
            let acc = acc.unwrap_or(DEFAULT_ACCENT);
            println!("{}", if cmd == "scheme" { scheme(dark, acc) } else { gtk_colors(dark, acc) });
            Ok(0)
        }
        "cursor" => {
            if argv.len() < 2 {
                println!("{}", files::cursor_theme()?);
                return Ok(0);
            }
            let name = &argv[1];
            if !files::is_cursor_theme(name) {
                eprintln!("ygg-theme: no cursor theme called {} is installed", pycompat::repr(name));
                return Ok(1);
            }
            let (dark, acc) = files::current()?;
            // the default is not written down (write_appearance leaves it out), so a later change of default reaches this
            // user too
            files::write_appearance(dark, acc, Some(name))?;
            files::apply_cursor(name, true)?;
            println!("Pointer: {name}");
            Ok(0)
        }
        "apply" => {
            let (mut dark, mut acc) = files::current()?;
            let a = &argv[1..];
            let mut i = 0;
            while i < a.len() {
                if a[i] == "--dark" {
                    dark = true;
                } else if a[i] == "--light" {
                    dark = false;
                } else if a[i] == "--accent" && i + 1 < a.len() {
                    acc = parse_accent(&a[i + 1]).unwrap_or(acc);
                    i += 1;
                }
                i += 1;
            }
            files::apply(dark, acc)?;
            println!("Yggdrasil {}, accent {}", if dark { "dark" } else { "light" }, rgb(acc));
            Ok(0)
        }
        _ => {
            eprintln!("{DOC}");
            Ok(2)
        }
    }
}

/// The program: `argv` without its own name.
pub fn main(argv: &[String]) -> i32 {
    match dispatch(argv) {
        Ok(c) => c,
        Err(files::Crash(m)) => {
            eprintln!("ygg-theme: unexpected error (the Python script stopped here with a traceback): {m}");
            1
        }
    }
}

#[cfg(test)]
mod tests;
