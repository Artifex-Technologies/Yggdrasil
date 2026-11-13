// liki-icon (see src/icon.rs)
fn main() {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    std::process::exit(liki::icon::main(&argv));
}
