fn main() {
    power_helpers::sys::exit(power_helpers::tune::main(std::env::args().collect()))
}
