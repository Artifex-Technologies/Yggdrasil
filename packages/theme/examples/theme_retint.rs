//! For parity/compare.py: GTK 4's own theme (standard input) retinted for dark or light and an accent, as retint_css does.
//!
//!   theme_retint dark|light R,G,B < Default-dark.css
use std::io::Read;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut css = String::new();
    std::io::stdin().read_to_string(&mut css).unwrap();
    let accent = ygg_theme::colors::parse_accent(&a[1]).unwrap();
    print!("{}", ygg_theme::colors::retint_css(&css, a[0] != "light", accent));
}
