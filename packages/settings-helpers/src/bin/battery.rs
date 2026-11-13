// battery: the top bar's battery, its drop-down and charge limit (see src/battery.rs)
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(settings_helpers::battery::main(&args));
}
