//! /usr/lib/yggdrasil/spell-global approve|deny WORD...: an administrator's choice for the computer's shared word list (Settings >
//! Keyboard > Spelling and languages runs it through pkexec; the polkit action names this path). Only these two commands; the
//! words are checked again by yggdrasil-spell, which this process becomes (`yggdrasil-spell global approve|deny WORD...`), as the
//! shell script of the same name did.

use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if !matches!(args.first().and_then(|a| a.to_str()), Some("approve" | "deny")) {
        eprintln!("usage: spell-global approve|deny WORD...");
        std::process::exit(2);
    }
    let e = Command::new("/usr/bin/yggdrasil-spell").arg("global").args(&args).exec();
    if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!("spell-global: /usr/bin/yggdrasil-spell: not found");
        std::process::exit(127)
    }
    eprintln!("spell-global: /usr/bin/yggdrasil-spell: {e}");
    std::process::exit(126)
}
