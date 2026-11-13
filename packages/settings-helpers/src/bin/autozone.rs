// autozone: Settings > Date & Time > "Set time zone automatically" (see src/autozone.rs)
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(settings_helpers::autozone::main(&args));
}
