//! Programs that send what the protocols forbid or never expect: the compositor must survive each (a crash ends the whole
//! session) and keep serving the other programs. Each case is a client of its own, which first shows an ordinary window and
//! then sends the bad requests without waiting for an answer: a protocol error ending that client is a fine answer.

mod common;

use std::time::Duration;

use common::*;

const GREEN: u32 = 0xff00ff00;

/// The compositor is still there, and an ordinary program's window still listed.
fn still_serving(comp: &mut Compositor, what: &str) {
    std::thread::sleep(Duration::from_millis(300));
    assert!(comp.child.try_wait().unwrap().is_none(), "the compositor ended after {what}: {}", comp.log_tail(200));
    assert!(comp.window("test.bystander").is_some(), "after {what}: {:?}", comp.ctl("windows"));
}

#[test]
fn hostile_requests_do_not_bring_the_compositor_down() {
    let _l = limit(90, "hostile requests");
    let mut comp = Compositor::start(&["--size", "800x600"]);
    let mut b = comp.connect();
    let bystander = b.window("test.bystander", "Bystander");
    b.draw(&bystander, (300, 200), GREEN);
    b.roundtrip();

    // a region with a negative size, as input and opaque region (Smithay master guards against it)
    {
        let mut c = comp.connect();
        let w = c.window("test.region", "Region");
        c.draw(&w, (200, 100), GREEN);
        let r = c.compositor.create_region(&c.qh, ());
        r.add(0, 0, -100, -100);
        r.subtract(10, 10, -5, i32::MAX);
        r.add(i32::MAX, i32::MAX, i32::MAX, i32::MAX);
        w.surface.set_input_region(Some(&r));
        w.surface.set_opaque_region(Some(&r));
        w.surface.commit();
        let _ = c.conn.flush();
        std::thread::sleep(Duration::from_millis(200));
        still_serving(&mut comp, "the region requests");
        comp.must("pointer 100 100");
        comp.must("click");
    }
    still_serving(&mut comp, "a region with a negative size");

    // negative minimum and maximum sizes, and a minimum above the maximum
    for (min, max) in [((-5, -5), (-1, -1)), ((500, 400), (100, 80)), ((i32::MAX, i32::MAX), (0, 0))] {
        let mut c = comp.connect();
        let w = c.window("test.hints", "Hints");
        c.draw(&w, (200, 100), GREEN);
        w.toplevel.set_min_size(min.0, min.1);
        w.toplevel.set_max_size(max.0, max.1);
        w.surface.commit();
        let _ = c.conn.flush();
        std::thread::sleep(Duration::from_millis(200));
        let _ = comp.ctl("maximize-active");
        still_serving(&mut comp, &format!("size hints min {min:?} max {max:?}"));
    }

    // a window geometry of no size, a negative one, and one far outside its buffer
    for (x, y, w_, h_) in [(0, 0, 0, 0), (0, 0, -50, -50), (i32::MAX, i32::MAX, i32::MAX, i32::MAX), (-1000000, -1000000, 10, 10)] {
        let mut c = comp.connect();
        let w = c.window("test.geometry", "Geometry");
        c.draw(&w, (200, 100), GREEN);
        w.xdg.set_window_geometry(x, y, w_, h_);
        w.surface.commit();
        let _ = c.conn.flush();
        std::thread::sleep(Duration::from_millis(200));
        still_serving(&mut comp, &format!("window geometry {x},{y} {w_}x{h_}"));
    }

    // damage far outside the buffer, and a huge buffer scale
    {
        let mut c = comp.connect();
        let w = c.window("test.damage", "Damage");
        c.draw(&w, (200, 100), GREEN);
        let b2 = c.buffer(200, 100, GREEN);
        w.surface.attach(Some(&b2), 0, 0);
        w.surface.damage_buffer(i32::MAX - 1, i32::MAX - 1, i32::MAX, i32::MAX);
        w.surface.damage(-1000, -1000, i32::MAX, i32::MAX);
        w.surface.commit();
        let _ = c.conn.flush();
        std::thread::sleep(Duration::from_millis(200));
        still_serving(&mut comp, "damage outside the buffer");
    }
    // buffer scales the protocol forbids (zero, negative) and one far too big for the buffer
    for scale in [0, -1, 1 << 20] {
        let mut c = comp.connect();
        let w = c.window("test.scale", "Scale");
        c.draw(&w, (200, 100), GREEN);
        w.surface.set_buffer_scale(scale);
        let b2 = c.buffer(200, 100, GREEN);
        w.surface.attach(Some(&b2), 0, 0);
        w.surface.commit();
        let _ = c.conn.flush();
        std::thread::sleep(Duration::from_millis(200));
        still_serving(&mut comp, &format!("buffer scale {scale}"));
    }

    // a menu whose positioner has no size or a negative one (forbidden), anchored far outside the window
    for (pw, ph) in [(0, 0), (-10, -10)] {
        let mut c = comp.connect();
        let w = c.window("test.popup", "Popup");
        c.draw(&w, (200, 100), GREEN);
        let p = c.wm_base.create_positioner(&c.qh, ());
        p.set_size(pw, ph);
        p.set_anchor_rect(i32::MAX - 1, i32::MAX - 1, i32::MAX, i32::MAX);
        p.set_offset(i32::MIN, i32::MIN);
        let s = c.compositor.create_surface(&c.qh, ());
        let xs = c.wm_base.get_xdg_surface(&s, &c.qh, usize::MAX);
        let _popup = xs.get_popup(Some(&w.xdg), &p, &c.qh, ());
        s.commit();
        let _ = c.conn.flush();
        std::thread::sleep(Duration::from_millis(200));
        still_serving(&mut comp, &format!("a positioner of {pw}x{ph}"));
    }
    still_serving(&mut comp, "all of it");
}
