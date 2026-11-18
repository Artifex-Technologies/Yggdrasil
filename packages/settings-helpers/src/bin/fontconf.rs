// fontconf: Settings > Appearance > Font, the login screen's font (see src/fontconf.rs)
fn main() {
    let argv: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    std::process::exit(settings_helpers::fontconf::main(&argv));
}
