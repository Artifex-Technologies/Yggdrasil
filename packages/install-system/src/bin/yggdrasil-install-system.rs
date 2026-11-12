// yggdrasil-install-system: installs the operating system from its live medium (see src/lib.rs and docs/INSTALLER.md)
fn main() {
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let code = install_system::main_with(
        &args,
        &mut std::io::stdin().lock(),
        &install_system::Env::from_process(),
        install_system::install::Host::real(),
        &mut install_system::sys::Real,
    );
    std::process::exit(code);
}
