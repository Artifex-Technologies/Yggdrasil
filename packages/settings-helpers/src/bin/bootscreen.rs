// bootscreen: Settings > Appearance > Boot screen (see src/bootscreen.rs)
fn main() {
    let argv: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    std::process::exit(settings_helpers::bootscreen::main(&argv));
}
