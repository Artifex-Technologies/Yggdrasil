//! One game of Sudoku: what is on the board, notes, undo and redo, hints, and the survival rules (three lives, a wrong digit
//! costs one, a cleared board gives two more and the next difficulty). Saved and loaded as JSON.

use crate::engine::{self, Difficulty};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Classic,
    Survival,
}

pub const START_LIVES: u32 = 3;
pub const BONUS_LIVES: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Snapshot {
    values: Vec<u8>,
    notes: Vec<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    pub mode: Mode,
    pub difficulty: Difficulty,
    /// The clues (0 = empty), the answer, and what is on the board now.
    pub puzzle: Vec<u8>,
    pub solution: Vec<u8>,
    pub values: Vec<u8>,
    /// Pencil notes: bits 1..=9.
    pub notes: Vec<u16>,
    pub lives: u32,
    /// Boards cleared in this survival run.
    pub cleared: u32,
    pub elapsed_ms: u64,
    pub hints: u32,
    pub mistakes: u32,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

/// What a move did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Placed,
    /// Survival: the digit was wrong and cost a life (it is not kept).
    LifeLost,
    /// Survival: that was the last life.
    OutOfLives,
    Solved,
}

impl Game {
    pub fn new(mode: Mode, difficulty: Difficulty, puzzle: engine::Grid, solution: engine::Grid) -> Game {
        Game {
            mode,
            difficulty,
            puzzle: puzzle.to_vec(),
            solution: solution.to_vec(),
            values: puzzle.to_vec(),
            notes: vec![0; 81],
            lives: START_LIVES,
            cleared: 0,
            elapsed_ms: 0,
            hints: 0,
            mistakes: 0,
            undo: vec![],
            redo: vec![],
        }
    }

    /// The next survival board: the lives carry over plus the bonus, one more board cleared, one step harder.
    pub fn next_survival(&self, puzzle: engine::Grid, solution: engine::Grid) -> Game {
        let mut g = Game::new(Mode::Survival, self.difficulty.next(), puzzle, solution);
        g.lives = self.lives + BONUS_LIVES;
        g.cleared = self.cleared + 1;
        g
    }

    /// Whether a loaded game is whole (81 cells everywhere).
    pub fn valid(&self) -> bool {
        [self.puzzle.len(), self.solution.len(), self.values.len(), self.notes.len()].iter().all(|&n| n == 81)
    }

    pub fn given(&self, i: usize) -> bool {
        self.puzzle[i] != 0
    }

    pub fn solved(&self) -> bool {
        self.values == self.solution
    }

    pub fn out_of_lives(&self) -> bool {
        self.mode == Mode::Survival && self.lives == 0
    }

    /// Cell `i` holds a digit that a peer holds too.
    pub fn conflict(&self, i: usize) -> bool {
        let v = self.values[i];
        v != 0 && engine::peers(i).any(|j| self.values[j] == v)
    }

    /// How many of digit `d` are still to be placed.
    pub fn remaining(&self, d: u8) -> usize {
        9usize.saturating_sub(self.values.iter().filter(|&&v| v == d).count())
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { values: self.values.clone(), notes: self.notes.clone() }
    }

    fn remember(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo and redo move the board and the notes; lives lost stay lost.
    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.values = s.values;
            self.notes = s.notes;
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.values = s.values;
            self.notes = s.notes;
        }
    }

    /// Puts digit `d` in cell `i` (the same digit again takes it out). In survival a wrong digit costs a life and is not kept.
    pub fn place(&mut self, i: usize, d: u8) -> Outcome {
        if self.given(i) || !(1..=9).contains(&d) || self.out_of_lives() || self.solved() {
            return Outcome::Nothing;
        }
        if self.values[i] == d {
            self.remember();
            self.values[i] = 0;
            return Outcome::Placed;
        }
        if self.mode == Mode::Survival && self.solution[i] != d {
            self.mistakes += 1;
            self.lives = self.lives.saturating_sub(1);
            return if self.lives == 0 { Outcome::OutOfLives } else { Outcome::LifeLost };
        }
        self.remember();
        self.values[i] = d;
        self.notes[i] = 0;
        if self.solution[i] != d {
            self.mistakes += 1;
        } else {
            // the digit is settled: its peers can drop it from their notes
            for j in engine::peers(i) {
                self.notes[j] &= !(1 << d);
            }
        }
        if self.solved() { Outcome::Solved } else { Outcome::Placed }
    }

    /// Turns pencil note `d` in cell `i` on or off (only in an empty cell).
    pub fn toggle_note(&mut self, i: usize, d: u8) {
        if self.given(i) || self.values[i] != 0 || !(1..=9).contains(&d) || self.solved() {
            return;
        }
        self.remember();
        self.notes[i] ^= 1 << d;
    }

    /// Clears what the player put in cell `i`.
    pub fn erase(&mut self, i: usize) -> bool {
        if self.given(i) || (self.values[i] == 0 && self.notes[i] == 0) {
            return false;
        }
        self.remember();
        self.values[i] = 0;
        self.notes[i] = 0;
        true
    }

    /// Fills in the right digit: in cell `at` if it is open or wrong, else the first open cell. Returns the cell.
    pub fn hint(&mut self, at: Option<usize>) -> Option<(usize, Outcome)> {
        let open = |g: &Game, i: usize| !g.given(i) && g.values[i] != g.solution[i];
        let i = at.filter(|&i| open(self, i)).or_else(|| (0..81).find(|&i| open(self, i)))?;
        self.hints += 1;
        let d = self.solution[i];
        // a hint never costs a life
        self.remember();
        self.values[i] = d;
        self.notes[i] = 0;
        for j in engine::peers(i) {
            self.notes[j] &= !(1 << d);
        }
        Some((i, if self.solved() { Outcome::Solved } else { Outcome::Placed }))
    }
}

/// Best times and how many were played, kept between sessions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    /// Classic: fastest solve in seconds, by difficulty.
    pub best: std::collections::BTreeMap<Difficulty, u64>,
    pub solved: std::collections::BTreeMap<Difficulty, u32>,
    pub started: u32,
    /// Survival: the most boards cleared in one run.
    pub survival_best: u32,
    pub survival_runs: u32,
}

impl Stats {
    /// A classic board was solved in `secs` (without hints): true when it is a new best.
    pub fn record_solve(&mut self, d: Difficulty, secs: u64, hints: u32) -> bool {
        *self.solved.entry(d).or_insert(0) += 1;
        if hints > 0 {
            return false;
        }
        let best = self.best.entry(d).or_insert(u64::MAX);
        if secs < *best {
            *best = secs;
            true
        } else {
            false
        }
    }

    pub fn record_survival(&mut self, cleared: u32) -> bool {
        self.survival_runs += 1;
        if cleared > self.survival_best {
            self.survival_best = cleared;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Rng;

    fn game(mode: Mode) -> Game {
        let (p, s) = engine::generate(Difficulty::Easy, &mut Rng::new(3));
        Game::new(mode, Difficulty::Easy, p, s)
    }

    fn open(g: &Game) -> usize {
        (0..81).find(|&i| !g.given(i)).unwrap()
    }

    fn wrong(g: &Game, i: usize) -> u8 {
        (1..=9).find(|&d| d != g.solution[i]).unwrap()
    }

    #[test]
    fn a_right_digit_clears_it_from_the_peers_notes() {
        let mut g = game(Mode::Classic);
        let i = open(&g);
        let d = g.solution[i];
        let peer = engine::peers(i).find(|&j| !g.given(j)).unwrap();
        g.toggle_note(peer, d);
        g.toggle_note(peer, if d == 9 { 1 } else { d + 1 });
        assert_eq!(g.notes[peer].count_ones(), 2);
        assert_eq!(g.place(i, d), Outcome::Placed);
        assert_eq!(g.notes[peer] & (1 << d), 0, "the placed digit is gone from the peer's notes");
        assert_eq!(g.notes[peer].count_ones(), 1, "the other note stays");
    }

    #[test]
    fn undo_and_redo_walk_the_moves() {
        let mut g = game(Mode::Classic);
        let i = open(&g);
        let before = g.values.clone();
        g.place(i, g.solution[i]);
        let after = g.values.clone();
        g.undo();
        assert_eq!(g.values, before);
        assert!(g.can_redo());
        g.redo();
        assert_eq!(g.values, after);
        g.undo();
        g.toggle_note(i, 5);
        assert!(!g.can_redo(), "a new move drops the redo list");
        assert!(!g.erase(0) || !g.given(0));
    }

    #[test]
    fn givens_cannot_be_changed() {
        let mut g = game(Mode::Classic);
        let i = (0..81).find(|&i| g.given(i)).unwrap();
        assert_eq!(g.place(i, wrong(&g, i)), Outcome::Nothing);
        assert!(!g.erase(i));
    }

    #[test]
    fn survival_lives() {
        let mut g = game(Mode::Survival);
        let i = open(&g);
        assert_eq!(g.place(i, wrong(&g, i)), Outcome::LifeLost);
        assert_eq!(g.lives, 2);
        assert_eq!(g.values[i], 0, "a wrong digit is not kept");
        g.undo();
        assert_eq!(g.lives, 2, "undo gives no life back");
        assert_eq!(g.place(i, wrong(&g, i)), Outcome::LifeLost);
        assert_eq!(g.place(i, wrong(&g, i)), Outcome::OutOfLives);
        assert!(g.out_of_lives());
        assert_eq!(g.place(i, g.solution[i]), Outcome::Nothing, "no moves after the last life");
        // a cleared board: +2 lives and the next difficulty
        let mut g = game(Mode::Survival);
        let i = open(&g);
        g.place(i, wrong(&g, i));
        let (p, s) = engine::generate(Difficulty::Medium, &mut Rng::new(9));
        let n = g.next_survival(p, s);
        assert_eq!((n.lives, n.cleared, n.difficulty), (2 + BONUS_LIVES, 1, Difficulty::Medium));
        assert_eq!(Difficulty::Extreme.next(), Difficulty::Extreme);
    }

    #[test]
    fn hints_fill_the_board_to_solved() {
        let mut g = game(Mode::Classic);
        let mut last = Outcome::Nothing;
        while let Some((_, o)) = g.hint(None) {
            last = o;
        }
        assert_eq!(last, Outcome::Solved);
        assert!(g.solved());
    }

    #[test]
    fn classic_keeps_a_wrong_digit_and_marks_a_clash() {
        let mut g = game(Mode::Classic);
        let i = open(&g);
        // a digit some peer already has
        let d = engine::peers(i).map(|j| g.values[j]).find(|&v| v != 0).unwrap();
        assert_eq!(g.place(i, d), Outcome::Placed);
        assert!(g.conflict(i));
        assert_eq!(g.mistakes, 1);
    }

    #[test]
    fn best_times() {
        let mut s = Stats::default();
        assert!(s.record_solve(Difficulty::Easy, 300, 0));
        assert!(!s.record_solve(Difficulty::Easy, 400, 0));
        assert!(!s.record_solve(Difficulty::Easy, 100, 1), "a solve with hints is not a best time");
        assert_eq!(s.best[&Difficulty::Easy], 300);
        assert_eq!(s.solved[&Difficulty::Easy], 3);
    }

    #[test]
    fn a_game_round_trips_as_json() {
        let g = game(Mode::Survival);
        let back: Game = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert!(back.valid());
        assert_eq!(back.values, g.values);
    }
}
