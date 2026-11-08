//! Talking with the assistant without holding a key: when the speaker has stopped talking. A plain energy detector over the microphone level
//! that voicewave::mic_level gives every ~33 ms (0 is a quiet room, 1 is loud): it learns the room's noise in the first moments, notices
//! when speech starts, and calls the end of an utterance after about a second of quiet.

/// A question asked within this long of the last one carries on the same conversation (she keeps what was said).
pub const CONTINUE_SECS: u64 = 180;
/// Quiet after speech that ends an utterance.
pub const END_SILENCE_MS: u32 = 1000;
/// Without any speech for this long, listening gives up (the conversation ends).
pub const NO_SPEECH_MS: u32 = 20_000;
/// The longest utterance.
pub const MAX_UTTERANCE_MS: u32 = 60_000;
const WARMUP_MS: u32 = 450;
const MIN_SPEECH_MS: u32 = 350;
const START_MS: u32 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    None,
    /// Speech began (for a screen cue; nothing needs doing).
    SpeechStarted,
    /// The person finished talking: send what was recorded.
    EndOfSpeech,
    /// Nobody said anything for a long time.
    GaveUp,
}

#[derive(Clone, Debug)]
pub struct Vad {
    floor_sum: f32,
    floor_n: u32,
    elapsed: u32,
    loud: u32,
    pub speaking: bool,
    speech_ms: u32,
    quiet: u32,
}

impl Default for Vad {
    fn default() -> Self {
        Vad::new()
    }
}

impl Vad {
    pub fn new() -> Vad {
        Vad { floor_sum: 0.0, floor_n: 0, elapsed: 0, loud: 0, speaking: false, speech_ms: 0, quiet: 0 }
    }

    fn floor(&self) -> f32 {
        if self.floor_n == 0 {
            0.0
        } else {
            (self.floor_sum / self.floor_n as f32).clamp(0.0, 0.6)
        }
    }

    /// Feed one measurement (`dt_ms` since the last one).
    pub fn feed(&mut self, level: f32, dt_ms: u32) -> Event {
        self.elapsed += dt_ms;
        if self.elapsed <= WARMUP_MS && !self.speaking {
            // the room's own noise (if the person is already talking in these moments the floor is high, and speech must be louder still)
            self.floor_sum += level;
            self.floor_n += 1;
            return Event::None;
        }
        let floor = self.floor();
        let speech_at = (floor + 0.17).max(0.25);
        let quiet_below = (floor + 0.08).max(0.18);
        if !self.speaking {
            if level > speech_at {
                self.loud += dt_ms;
                if self.loud >= START_MS {
                    self.speaking = true;
                    self.speech_ms = self.loud;
                    self.quiet = 0;
                    return Event::SpeechStarted;
                }
            } else {
                self.loud = 0;
            }
            if self.elapsed >= NO_SPEECH_MS {
                return Event::GaveUp;
            }
            return Event::None;
        }
        if level < quiet_below {
            self.quiet += dt_ms;
        } else {
            self.quiet = 0;
            self.speech_ms += dt_ms;
        }
        if (self.quiet >= END_SILENCE_MS && self.speech_ms >= MIN_SPEECH_MS) || self.elapsed >= MAX_UTTERANCE_MS {
            return Event::EndOfSpeech;
        }
        Event::None
    }
}

/// What a press of the talk key does. Tap to start, tap again to send: only the key going down counts, so it does not matter which
/// key of Super+S is let go first (Hyprland reports a release only while Super is still held, and a missed release used to leave the
/// recording running).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tap {
    /// Start recording.
    Start,
    /// Stop recording and send the question.
    Send,
    /// She is working on an answer or saying it: the press closes her (the answer is dropped, her voice stops). The next press
    /// asks again, and within CONTINUE_SECS it carries on the same conversation.
    Close,
}

pub fn tap(listening: bool, thinking: bool, speaking: bool) -> Tap {
    if listening {
        Tap::Send
    } else if thinking || speaking {
        Tap::Close
    } else {
        Tap::Start
    }
}

/// Whether a new question continues the conversation: asked within CONTINUE_SECS of the last one.
pub fn continues(since_last: Option<std::time::Duration>) -> bool {
    since_last.is_some_and(|d| d.as_secs() < CONTINUE_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(levels: &[(f32, u32)]) -> Vec<Event> {
        let mut v = Vad::new();
        let mut out = vec![];
        'all: for &(l, n) in levels {
            for _ in 0..n {
                let e = v.feed(l, 33);
                if e != Event::None {
                    out.push(e);
                }
                if matches!(e, Event::EndOfSpeech | Event::GaveUp) {
                    break 'all; // the caller stops listening at the first of these
                }
            }
        }
        out
    }

    #[test]
    fn a_sentence_followed_by_quiet_ends_the_utterance() {
        // 0.5 s of room noise, 2 s of speech, 1.5 s of quiet
        let ev = run(&[(0.1, 14), (0.6, 60), (0.1, 45)]);
        assert_eq!(ev.first(), Some(&Event::SpeechStarted));
        assert_eq!(ev.last(), Some(&Event::EndOfSpeech));
    }

    #[test]
    fn a_pause_inside_a_sentence_does_not_end_it() {
        let ev = run(&[(0.1, 14), (0.6, 30), (0.1, 20), (0.6, 30), (0.1, 45)]);
        // one end, and only after the second burst (the 0.66 s pause between the bursts is shorter than a second)
        assert_eq!(ev, vec![Event::SpeechStarted, Event::EndOfSpeech]);
    }

    #[test]
    fn a_click_is_not_speech() {
        let ev = run(&[(0.1, 14), (0.7, 2), (0.1, 650)]);
        assert!(!ev.contains(&Event::SpeechStarted), "{ev:?}");
        assert_eq!(ev.last(), Some(&Event::GaveUp));
    }

    #[test]
    fn a_noisy_room_raises_the_bar() {
        // a fan at 0.35: it is the floor, so 0.45 is not speech but 0.8 is
        let ev = run(&[(0.35, 14), (0.45, 60)]);
        assert!(!ev.contains(&Event::SpeechStarted));
        let ev = run(&[(0.35, 14), (0.8, 30), (0.35, 45)]);
        assert!(ev.contains(&Event::SpeechStarted) && ev.contains(&Event::EndOfSpeech), "{ev:?}");
    }

    #[test]
    fn silence_gives_up_after_twenty_seconds() {
        assert_eq!(run(&[(0.05, 400)]), vec![], "thirteen seconds of quiet: still listening (a pause to think is not the end)");
        assert_eq!(run(&[(0.05, 650)]), vec![Event::GaveUp]);
    }

    #[test]
    fn tap_to_start_tap_to_send() {
        assert_eq!(tap(false, false, false), Tap::Start);
        assert_eq!(tap(true, false, false), Tap::Send);
        assert_eq!(tap(false, true, false), Tap::Close, "a press while she works on an answer closes her");
        assert_eq!(tap(false, false, true), Tap::Close, "a press while she talks closes her, it does not start a question");
        assert_eq!(tap(true, false, true), Tap::Send, "her voice is stopped before recording, so listening wins");
        use std::time::Duration;
        assert!(continues(Some(Duration::from_secs(60))));
        assert!(!continues(Some(Duration::from_secs(CONTINUE_SECS))));
        assert!(!continues(None));
    }
}
