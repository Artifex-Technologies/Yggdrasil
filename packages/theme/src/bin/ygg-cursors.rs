// ygg-cursors (see src/cursors/mod.rs)
fn main() {
    let mut args = std::env::args_os().map(|a| a.to_string_lossy().into_owned());
    let argv0 = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    std::process::exit(ygg_theme::cursors::main(&argv0, &rest));
}
