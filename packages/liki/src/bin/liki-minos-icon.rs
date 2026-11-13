// liki-minos-icon (see src/minos_icon.rs)
fn main() {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    std::process::exit(liki::minos_icon::main(&argv));
}
