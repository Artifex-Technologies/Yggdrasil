//! What the laptop milestone adds that the headless backend can show: the touchscreen (touch.rs), driven from the control socket
//! (`touch-down`, `touch-move`, `touch-up`) as libinput drives it on the hardware backend. The rest of the laptop's parts (the
//! lid, the default scale, waking from a suspend, a touchpad's settings, copies from the GPU) need the hardware; their rules
//! have unit tests (laptop.rs, input.rs, readback.rs) and the hardware backend is checked by hand (tests/laptop-tty.sh).

mod common;

use common::*;

const BLUE: u32 = 0xff0000ff;

#[test]
fn a_finger_on_the_screen_reaches_the_window_under_it_as_touch() {
    let _l = limit(60, "touch");
    let comp = Compositor::start(&["--size", "800x600"]);
    let mut c = comp.connect();
    let (_k, _p) = c.input();
    assert_eq!(c.state.seat_caps & 4, 0, "no touchscreen yet: the seat has no touch");
    let w = c.window("test.w", "W");
    c.draw(&w, (400, 300), BLUE);
    let other = c.window("test.other", "Other");
    c.draw(&other, (200, 150), BLUE);
    let info = comp.window("test.w").unwrap();
    let (x, y) = (info.at.0 + 30, info.at.1 + 40);

    // the first touch gives the seat its touch; the program then takes it
    comp.must(&format!("touch-down 0 {x} {y}"));
    comp.must("touch-up 0");
    c.until("the seat has touch", |s| s.seat_caps & 4 != 0);
    let _t = c.touch();

    // a finger down on the window: at its place on the window's surface, and the window gets the keyboard
    comp.must(&format!("touch-down 1 {x} {y}"));
    c.until("down", |s| s.touches.iter().any(|t| t.0 == "down"));
    let down = *c.state.touches.iter().find(|t| t.0 == "down").unwrap();
    assert_eq!((down.1, down.2, down.3), (1, 30.0, 40.0), "{:?}", c.state.touches);
    assert!(comp.focus().contains("test.w"), "{}", comp.focus());
    // it moves and lifts; each change ends with a frame
    comp.must(&format!("touch-move 1 {} {}", x + 15, y + 5));
    comp.must("touch-up 1");
    c.until("up", |s| s.touches.iter().any(|t| t.0 == "up"));
    let kinds: Vec<&str> = c.state.touches.iter().map(|t| t.0).collect();
    assert_eq!(kinds, ["down", "frame", "motion", "frame", "up", "frame"]);
    assert_eq!(c.state.touches[2].2, 45.0);
    assert!(comp.ctl("touch-down 99 1 1").is_err(), "a slot from 0 to 15");
}
