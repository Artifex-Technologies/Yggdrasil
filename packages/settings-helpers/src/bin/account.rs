// account: Settings > Users (see src/account.rs)
fn main() {
    let argv: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    std::process::exit(settings_helpers::account::main(&argv));
}
