// gpuunlock: the graphics card unlock (Settings > Power > Graphics card, Developer options; see src/gpuunlock.rs)
fn main() {
    let argv: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    std::process::exit(settings_helpers::gpuunlock::Gpu::system().main(&argv));
}
