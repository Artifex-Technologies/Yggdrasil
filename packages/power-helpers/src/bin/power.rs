fn main() {
    power_helpers::sys::exit(power_helpers::power::main(std::env::args().collect()))
}
