fn main() {
    power_helpers::sys::exit(power_helpers::powerlimit::main(std::env::args().collect()))
}
