//! yggdrasil-autocorrect: correcting as you type, in every app that takes part in Wayland's text input (GTK, Qt, Firefox, Chromium;
//! Electron apps with --enable-wayland-ime). It is an input method (zwp_input_method_v2) that never takes the keyboard: it reads
//! the text around the cursor that the app shares, and when a word has just been finished it replaces it only when the fix is
//! all but certain (the rule is in yggdrasil_spell::autocorrect). Backspace straight after a correction puts the word back, and
//! that word is not corrected again until the input method starts again.
//!
//! It runs only while spell.conf says `autocorrect = yes` (Settings > Keyboard > Spelling and languages starts and stops the user
//! service yggdrasil-autocorrect). It leaves passwords, PINs, numbers, addresses, links, names, terminals and anything an app
//! marks as hidden or sensitive alone, and it changes nothing an app did not type itself (a paste, a cursor jump).

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::{zwp_input_method_manager_v2, zwp_input_method_v2};
use yggdrasil_spell::autocorrect::{fix, undone, Fix};
use yggdrasil_spell::conf::SpellConf;
use yggdrasil_spell::Speller;

/// text-input-v3 content purposes the input method may correct in: normal and alpha (not names, numbers, passwords, addresses ...).
const PURPOSE_NORMAL: u32 = 0;
const PURPOSE_ALPHA: u32 = 1;
/// text-input-v3 content hints that keep it out: hidden text and sensitive data.
const HINT_HIDDEN: u32 = 0x40;
const HINT_SENSITIVE: u32 = 0x80;

#[derive(Default)]
struct Pending {
    active: Option<bool>,
    text: Option<(String, u32)>,
    cause: u32,
    purpose: u32,
    hint: u32,
}

struct State {
    manager: Option<zwp_input_method_manager_v2::ZwpInputMethodManagerV2>,
    seat: Option<wl_seat::WlSeat>,
    im: Option<zwp_input_method_v2::ZwpInputMethodV2>,
    pending: Pending,
    active: bool,
    purpose: u32,
    hint: u32,
    /// the text before the cursor at the last `done`, to tell one typed character from a paste or a jump
    before: Option<String>,
    serial: u32,
    /// The dictionary work runs on a thread of its own, word by word while it is still being typed, so the answer is ready the
    /// moment the word ends. Finding suggestions takes long enough that, done only then, the next letters typed would arrive
    /// before the correction and it would delete the wrong text (found 2026-11-05: "adn thier" became "and ttheir").
    lookups: Sender<Lookup>,
    /// word -> (right?, suggestions best first); a word not in here yet is never corrected
    verdicts: Arc<Mutex<HashMap<String, (bool, Vec<String>)>>>,
    asked: HashSet<String>,
    loaded: Option<SystemTime>,
    /// the last correction and the text it was made in (for Backspace to undo it), and the words put back
    last: Option<(Fix, String)>,
    kept: HashSet<String>,
    unavailable: bool,
}

enum Lookup {
    Word(String),
    /// the word lists or languages changed: load the speller again and forget the answers
    Reload,
}

/// The dictionary thread: answers words into `verdicts`.
fn start_lookups(verdicts: Arc<Mutex<HashMap<String, (bool, Vec<String>)>>>) -> Sender<Lookup> {
    let (tx, rx) = channel::<Lookup>();
    std::thread::spawn(move || {
        let mut speller = Speller::load();
        while let Ok(first) = rx.recv() {
            // only the newest word matters (the earlier ones were the same word, shorter): skip to it, but never skip a reload
            let mut job = first;
            while let Ok(next) = rx.try_recv() {
                if matches!(job, Lookup::Reload) {
                    speller = Speller::load();
                    verdicts.lock().map(|mut v| v.clear()).ok();
                }
                job = next;
            }
            match job {
                Lookup::Reload => {
                    speller = Speller::load();
                    verdicts.lock().map(|mut v| v.clear()).ok();
                }
                Lookup::Word(w) => {
                    let right = speller.check(&w);
                    let suggestions = if right { vec![] } else { speller.suggest(&w) };
                    if let Ok(mut v) = verdicts.lock() {
                        v.insert(w, (right, suggestions));
                    }
                }
            }
        }
    });
    tx
}

/// The word that ends at the end of `before` (letters and apostrophes), if any.
fn word_at_end(before: &str) -> Option<&str> {
    let start = before.char_indices().rev().take_while(|(_, c)| c.is_alphabetic() || *c == '\'').last().map(|(i, _)| i)?;
    Some(&before[start..])
}

/// When any of the lists or settings last changed (the speller is loaded again after a change).
fn lists_changed() -> Option<SystemTime> {
    let words = yggdrasil_conf::dirs::data().join("spell");
    [SpellConf::path(), words.join("words.txt"), std::path::PathBuf::from(yggdrasil_spell::words::GLOBAL)]
        .iter()
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .max()
}

/// YGGDRASIL_AUTOCORRECT_DEBUG=1 prints what the input method hears and does (to the journal, for the user's own eyes).
fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("YGGDRASIL_AUTOCORRECT_DEBUG").is_some())
}

impl State {
    fn on_done(&mut self) {
        self.serial = self.serial.wrapping_add(1);
        let p = std::mem::take(&mut self.pending);
        if let Some(a) = p.active {
            self.active = a;
            self.before = None;
            self.last = None;
            if a {
                // a new text field: the settings and word lists may have changed since
                if !SpellConf::load().autocorrect {
                    std::process::exit(0);
                }
                let t = lists_changed();
                if t != self.loaded {
                    let _ = self.lookups.send(Lookup::Reload);
                    self.asked.clear();
                    self.loaded = t;
                }
            }
        }
        self.purpose = p.purpose;
        self.hint = p.hint;
        if debug() {
            eprintln!(
                "done #{}: active={} purpose={} hint={:#x} cause={} text={:?}",
                self.serial, self.active, self.purpose, self.hint, p.cause, p.text
            );
        }
        let Some((text, cursor)) = p.text else { return };
        if !self.active {
            return;
        }
        let cursor = (cursor as usize).min(text.len());
        let Some(before) = text.get(..cursor).map(str::to_string) else { return };
        let prev = self.before.replace(before.clone());
        // (text_change_cause is no guide: GTK reports plain typing as the input method's too. A correction of ours is never taken
        // for typing, because `before` is forgotten after it, so the next update is not "one character typed".)
        if !matches!(self.purpose, PURPOSE_NORMAL | PURPOSE_ALPHA) || self.hint & (HINT_HIDDEN | HINT_SENSITIVE) != 0 {
            return;
        }
        // the word being typed: have the dictionary thread look at it now
        if let Some(w) = word_at_end(&before).filter(|w| w.chars().count() >= 3) {
            if self.asked.len() > 5000 {
                self.asked.clear();
            }
            if self.asked.insert(w.to_string()) {
                let _ = self.lookups.send(Lookup::Word(w.to_string()));
            }
        }
        let Some(im) = self.im.clone() else { return };
        // Backspace straight after a correction: put the word back as typed
        if let Some((f, at)) = self.last.take() {
            if undone(&before, &f, &at) {
                im.delete_surrounding_text(f.fixed.len() as u32, 0);
                im.commit_string(f.typed.clone());
                im.commit(self.serial);
                self.kept.insert(f.typed);
                self.before = None;
                return;
            }
            // the correction itself arriving back, which apps report in steps (the word deleted, then the new one typed): keep
            // waiting for a Backspace while the text is on the way to what it made
            let head = &at[..at.len() - f.delete];
            let made = format!("{head}{}", f.insert);
            if before.len() >= head.len() && made.starts_with(before.as_str()) && before != format!("{head}{}", f.fixed) {
                self.last = Some((f, at));
                return;
            }
        }
        // only right after one character was typed at the end of what came before (not a paste, not a jump)
        let typed_one =
            prev.as_deref().is_some_and(|p| before.len() > p.len() && before.starts_with(p) && before[p.len()..].chars().count() == 1);
        if !typed_one {
            return;
        }
        // only from answers that are ready: a word not looked at yet counts as right (left alone)
        let verdicts = self.verdicts.lock().map(|v| v.clone()).unwrap_or_default();
        let check = |w: &str| verdicts.get(w).map(|v| v.0).unwrap_or(true);
        let suggest = |w: &str| verdicts.get(w).map(|v| v.1.clone()).unwrap_or_default();
        if let Some(f) = fix(&before, check, suggest, &self.kept) {
            if debug() {
                eprintln!("correcting {:?} -> {:?}", f.typed, f.fixed);
            }
            im.delete_surrounding_text(f.delete as u32, 0);
            im.commit_string(f.insert.clone());
            im.commit(self.serial);
            self.last = Some((f, before));
            self.before = None;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "zwp_input_method_manager_v2" => state.manager = Some(registry.bind(name, 1, qh, ())),
                "wl_seat" if state.seat.is_none() => state.seat = Some(registry.bind(name, version.min(5), qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<zwp_input_method_manager_v2::ZwpInputMethodManagerV2, ()> for State {
    fn event(
        _: &mut Self,
        _: &zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        _: zwp_input_method_manager_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_input_method_v2::ZwpInputMethodV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwp_input_method_v2::ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_input_method_v2::Event as E;
        match event {
            E::Activate => {
                state.pending = Pending { active: Some(true), ..Default::default() };
            }
            E::Deactivate => state.pending.active = Some(false),
            E::SurroundingText { text, cursor, anchor: _ } => state.pending.text = Some((text, cursor)),
            E::TextChangeCause { cause } => state.pending.cause = u32::from(cause),
            E::ContentType { hint, purpose } => {
                state.pending.hint = u32::from(hint);
                state.pending.purpose = u32::from(purpose);
            }
            E::Done => state.on_done(),
            // another input method has the seat (fcitx5, ibus ...): step aside
            E::Unavailable => state.unavailable = true,
            _ => {}
        }
    }
}

fn main() {
    if !SpellConf::load().autocorrect {
        eprintln!("yggdrasil-autocorrect: off (spell.conf: autocorrect = yes switches it on)");
        return;
    }
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yggdrasil-autocorrect: no Wayland display: {e}");
            std::process::exit(1);
        }
    };
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let verdicts = Arc::new(Mutex::new(HashMap::new()));
    let mut state = State {
        manager: None,
        seat: None,
        im: None,
        pending: Pending::default(),
        active: false,
        purpose: 0,
        hint: 0,
        before: None,
        serial: 0,
        lookups: start_lookups(verdicts.clone()),
        verdicts,
        asked: HashSet::new(),
        loaded: lists_changed(),
        last: None,
        kept: HashSet::new(),
        unavailable: false,
    };
    if queue.roundtrip(&mut state).is_err() {
        std::process::exit(1);
    }
    let (Some(manager), Some(seat)) = (state.manager.clone(), state.seat.clone()) else {
        // this desktop has no input-method support: nothing to do, and not a failure worth restarting for
        eprintln!("yggdrasil-autocorrect: the compositor offers no input method protocol");
        return;
    };
    state.im = Some(manager.get_input_method(&seat, &qh, ()));
    loop {
        if queue.blocking_dispatch(&mut state).is_err() {
            std::process::exit(1);
        }
        if state.unavailable {
            eprintln!("yggdrasil-autocorrect: another input method is in use; stepping aside");
            return;
        }
    }
}
