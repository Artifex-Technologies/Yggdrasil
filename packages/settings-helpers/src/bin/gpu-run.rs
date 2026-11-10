// yggdrasil-gpu-run: a program on a chosen graphics card (see src/gpurun.rs)
fn main() {
    // the arguments as the system gave them: the program started gets them byte for byte
    let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    std::process::exit(settings_helpers::gpurun::main(&argv));
}
