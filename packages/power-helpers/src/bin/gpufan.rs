fn main() {
    power_helpers::sys::exit(power_helpers::gpufan::main(std::env::args().collect()))
}
