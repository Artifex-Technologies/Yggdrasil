//! Window places as in the Hyprland session (packages/session/hyprland/yggdrasil.lua): a program's window comes back where its
//! last one closed (the session's file, ~/.local/state/yggdrasil/window-places, read and written in its form), the screens set to
//! tile in Settings > Displays (~/.config/yggdrasil/tiling) tile the windows opened on them, and Steam's own maximize request is
//! ignored.
//!
//! Every test starts a compositor of its own in a private folder and has a time limit.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::*;

const RED: u32 = 0xffff0000;
const GREEN: u32 = 0xff00ff00;
const BLUE: u32 = 0xff0000ff;
const YELLOW: u32 = 0xffffff00;

// xdg_toplevel states
const MAXIMIZED: u32 = 1;

fn places_file(dir: &std::path::Path) -> PathBuf {
    dir.join("home/.local/state/yggdrasil/window-places")
}

/// Draw a window again at the size the compositor last gave it, until it is listed at that size.
fn settle(comp: &Compositor, c: &mut Client, w: &Window, title: &str, argb: u32) -> WinInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        c.roundtrip();
        let given = c.state.wins[w.index].size;
        let info = comp.windows().into_iter().find(|i| i.title == title).unwrap_or_else(|| panic!("{title} is listed"));
        if given.0 <= 0 || info.size == given {
            return info;
        }
        c.draw(w, given, argb);
        assert!(Instant::now() < deadline, "{title} did not settle at {given:?}: {info:?}");
    }
}

/// A window's number for the control socket's desktop commands, by its title.
fn id_of(comp: &Compositor, title: &str) -> String {
    comp.must("clients").iter().find(|l| l.split('\t').nth(2) == Some(title)).unwrap().split('\t').next().unwrap().to_string()
}

fn close(comp: &Compositor, c: &mut Client, w: Window, title: &str) {
    w.toplevel.destroy();
    w.xdg.destroy();
    w.surface.destroy();
    c.roundtrip();
    comp.wait(5, "the window gone", |comp| !comp.windows().iter().any(|i| i.title == title));
}

fn bar(c: &mut Client) -> LayerSurf {
    let b = c.layer_with(0, Layer::Top, "test-bar", Anchor::Top | Anchor::Left | Anchor::Right, (0, 32), 32, KeyboardInteractivity::None);
    c.draw_layer(&b, RED);
    b
}

#[test]
fn a_programs_window_comes_back_where_its_last_one_closed() {
    let _l = limit(60, "window places");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    let _bar = bar(&mut c);
    let w = c.window("test.editor", "Notes");
    c.draw(&w, (640, 480), BLUE);
    assert_eq!(comp.window("test.editor").unwrap().rect(), (480, 226, 640, 480), "the first time: centred in the free room");

    // put elsewhere at another size (as window zones or a drag do), then closed: kept, in the session's form
    comp.must(&format!("place {} 100 120 700 500", id_of(&comp, "Notes")));
    assert_eq!(settle(&comp, &mut c, &w, "Notes", BLUE).rect(), (100, 120, 700, 500));
    close(&comp, &mut c, w, "Notes");
    assert_eq!(std::fs::read_to_string(places_file(&comp.dir)).unwrap(), "test.editor\t100\t120\t700\t500\n");

    // its next window opens there, at that size
    let w = c.window("test.editor", "Notes");
    c.draw(&w, (640, 480), BLUE);
    assert_eq!(settle(&comp, &mut c, &w, "Notes", BLUE).rect(), (100, 120, 700, 500), "back where it was");

    // a second window of the program while one is open (a dialog, another document) is left as any new window: centred
    let find = c.window("test.editor", "Find");
    c.draw(&find, (300, 200), YELLOW);
    assert_eq!(comp.windows().into_iter().find(|i| i.title == "Find").unwrap().rect(), (650, 366, 300, 200));
    // ...and closing it keeps nothing: the program still has a window
    close(&comp, &mut c, find, "Find");
    assert_eq!(std::fs::read_to_string(places_file(&comp.dir)).unwrap(), "test.editor\t100\t120\t700\t500\n");

    // a maximized window's place is not kept
    comp.must(&format!("maximize {} on", id_of(&comp, "Notes")));
    settle(&comp, &mut c, &w, "Notes", BLUE);
    close(&comp, &mut c, w, "Notes");
    assert_eq!(std::fs::read_to_string(places_file(&comp.dir)).unwrap(), "test.editor\t100\t120\t700\t500\n");

    // nor one smaller than 120 x 80 (the session's limit)
    let tiny = c.window("test.tiny", "Tiny");
    c.draw(&tiny, (100, 60), YELLOW);
    close(&comp, &mut c, tiny, "Tiny");
    assert_eq!(std::fs::read_to_string(places_file(&comp.dir)).unwrap(), "test.editor\t100\t120\t700\t500\n");
}

#[test]
fn places_kept_on_hyprland_are_used_kept_clear_of_the_bar_and_skipped_off_screen() {
    let _l = limit(60, "the session's places file");
    // as the Hyprland session left it
    let comp = Compositor::start_with(&["--size", "1280x720"], |dir| {
        let p = places_file(dir);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "test.away\t5000\t5000\t600\t400\ntest.under\t200\t10\t600\t400\n").unwrap();
    });
    let mut c = comp.connect();
    let _bar = bar(&mut c);
    let under = c.window("test.under", "Under");
    c.draw(&under, (300, 200), BLUE);
    assert_eq!(settle(&comp, &mut c, &under, "Under", BLUE).rect(), (200, 32, 600, 400), "where it was, moved out from under the bar");
    let away = c.window("test.away", "Away");
    c.draw(&away, (600, 400), YELLOW);
    assert_eq!(settle(&comp, &mut c, &away, "Away", YELLOW).rect(), (340, 176, 600, 400), "that spot is on no screen: centred");
    // (both windows are the same program here, one process: the first to close is not kept, as in the session)
    close(&comp, &mut c, away, "Away");
    close(&comp, &mut c, under, "Under");
    assert_eq!(
        std::fs::read_to_string(places_file(&comp.dir)).unwrap(),
        "test.away\t5000\t5000\t600\t400\ntest.under\t200\t32\t600\t400\n",
        "the last one kept again, in the session's form"
    );
}

#[test]
fn a_kept_place_too_big_for_the_screen_is_made_to_fit_and_dialogs_are_not_kept() {
    let _l = limit(60, "places that no longer fit, dialogs");
    // kept at scale 1 on a bigger screen (VS Code's 1208x806), and a dialog's place kept by the session before 2026-11-13
    let comp = Compositor::start_with(&["--size", "1280x720"], |dir| {
        let p = places_file(dir);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "test.code\t50\t20\t1208\t806\ntest.dialog\t10\t10\t500\t300\n").unwrap();
    });
    let mut c = comp.connect();
    let _bar = bar(&mut c);
    // the free room is 0,32 1280x688: the kept size is made to fit it less the gap of 8, centred (keep-clear.lua's open)
    let main = c.window("test.code", "Main");
    c.draw(&main, (640, 480), BLUE);
    assert_eq!(settle(&comp, &mut c, &main, "Main", BLUE).rect(), (36, 40, 1208, 672));
    // a dialog of the same process with another class: no kept place for it, centred as any new window
    let dialog = c.window("test.dialog", "Trust");
    c.draw(&dialog, (300, 200), YELLOW);
    assert_eq!(settle(&comp, &mut c, &dialog, "Trust", YELLOW).rect(), (490, 276, 300, 200));
    // ...and closing it keeps nothing: its program still has a window
    close(&comp, &mut c, dialog, "Trust");
    let file = || std::fs::read_to_string(places_file(&comp.dir)).unwrap();
    assert_eq!(file(), "test.code\t50\t20\t1208\t806\ntest.dialog\t10\t10\t500\t300\n");
    // a window that opens bigger than the screen gets the free room less the gap
    let wide = c.window("test.code", "Wide");
    c.draw(&wide, (1500, 900), BLUE);
    assert_eq!(settle(&comp, &mut c, &wide, "Wide", BLUE).rect(), (8, 40, 1264, 672));
    close(&comp, &mut c, wide, "Wide");
    // the program's last window closes: kept at the size it has now
    close(&comp, &mut c, main, "Main");
    assert_eq!(file(), "test.code\t36\t40\t1208\t672\ntest.dialog\t10\t10\t500\t300\n");
}

/// The tiling file as Settings > Displays writes it (yggdrasil_conf::tiling::set_outputs).
fn tiling_file(names: &[&str]) -> String {
    let mut t = String::from("# Screens that tile (Settings > Displays). One per line.\n");
    for n in names {
        t.push_str(n);
        t.push('\n');
    }
    t
}

#[test]
fn windows_opened_on_a_screen_set_to_tile_tile_there() {
    let _l = limit(60, "tiling screens");
    let comp = Compositor::start_with(&["--outputs", "2", "--size", "1280x720"], |dir| {
        std::fs::write(dir.join("home/.config/yggdrasil/tiling"), tiling_file(&["HEADLESS-2"])).unwrap();
    });
    let mut c = comp.connect();

    // on the first screen windows float
    comp.must("pointer 300 300");
    let f = c.window("test.float", "Float");
    c.draw(&f, (400, 300), BLUE);
    let info = comp.window("test.float").unwrap();
    assert_eq!(info.rect(), (440, 210, 400, 300));
    assert!(info.has("floating"));

    // on the second they tile: the first takes the left half, the next two share the right, with the session's gaps (8 at the
    // screen's edges, 4 + 4 between) and the border inside each box
    comp.must("pointer 1900 300");
    let mut tiles = vec![];
    for (title, colour) in [("One", BLUE), ("Two", YELLOW), ("Three", GREEN)] {
        let w = c.window("test.tile", title);
        c.draw(&w, (400, 300), colour);
        tiles.push((w, title, colour));
    }
    let want = [(1289, 9, 626, 702), (1925, 9, 626, 346), (1925, 365, 626, 346)];
    for ((w, title, colour), r) in tiles.iter().zip(want) {
        let info = settle(&comp, &mut c, w, title, *colour);
        assert_eq!(info.rect(), r, "{title}");
        assert!(info.has("tiled"), "{title}: {info:?}");
    }
    snapshot!(comp.screenshot("HEADLESS-2"), "tiling-screen");

    // one closes: the others take its room
    let (two, _, _) = tiles.remove(1);
    close(&comp, &mut c, two, "Two");
    for ((w, title, colour), r) in tiles.iter().zip([(1289, 9, 626, 702), (1925, 9, 626, 702)]) {
        assert_eq!(settle(&comp, &mut c, w, title, *colour).rect(), r, "{title}");
    }

    // the screen no longer tiles (Settings writes the file and reloads): windows opening there now float, those tiled stay so
    // (as on Hyprland, where the rule is applied when a window opens)
    std::fs::write(comp.dir.join("home/.config/yggdrasil/tiling"), tiling_file(&[])).unwrap();
    comp.must("reload");
    let n = c.window("test.new", "New");
    c.draw(&n, (400, 300), BLUE);
    let info = comp.window("test.new").unwrap();
    assert_eq!(info.rect(), (1720, 210, 400, 300));
    assert!(info.has("floating"));
    assert!(comp.windows().iter().filter(|i| i.app_id == "test.tile").all(|i| i.has("tiled")));
}

#[test]
fn steams_own_maximize_request_is_ignored() {
    let _l = limit(60, "Steam's maximize");
    let comp = Compositor::start(&["--size", "1280x720"]);
    let mut c = comp.connect();
    let steam = c.window("steam", "Steam");
    c.draw(&steam, (800, 600), BLUE);
    steam.toplevel.set_maximized();
    c.roundtrip();
    std::thread::sleep(Duration::from_millis(200));
    c.roundtrip();
    assert!(!comp.window("steam").unwrap().has("maximized"), "Steam asked; it is not done");
    assert!(!c.state.wins[steam.index].states.contains(&MAXIMIZED));
    // Super+Up still maximizes it
    comp.must("key super+up");
    let info = settle(&comp, &mut c, &steam, "Steam", BLUE);
    assert!(info.has("maximized"));
    // another program's own request is done
    let other = c.window("test.other", "Other");
    c.draw(&other, (400, 300), YELLOW);
    other.toplevel.set_maximized();
    let info = settle(&comp, &mut c, &other, "Other", YELLOW);
    assert!(info.has("maximized"), "{info:?}");
}
