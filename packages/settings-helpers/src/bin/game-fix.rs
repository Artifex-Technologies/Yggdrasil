// game-fix: Settings > Games > Game fixes (see src/gamefix.rs)
fn main() {
    let argv: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    std::process::exit(settings_helpers::gamefix::main(&argv));
}
