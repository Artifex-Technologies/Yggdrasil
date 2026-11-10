// pkg-root: one narrow pacman command for Agora (see src/lib.rs)
fn main() {
    let argv: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    std::process::exit(pkg_root::main(&argv));
}
