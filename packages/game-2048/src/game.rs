//! The rules of 2048 (as gabrielecirulli/2048 has them, by way of imranZERO/omarchy-2048): tiles slide as far as they can, two
//! that may merge become one once per move, a new tile (2 or 4, nine times in ten a 2) lands on a free cell after every move that
//! changed something. The game is won at the goal tile and can be played on; it is over when no move changes the board.

use serde::{Deserialize, Serialize};

pub const MIN_SIZE: usize = 3;
pub const MAX_SIZE: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash, PartialOrd, Ord)]
pub enum Mode {
    Classic,
    Fibonacci,
    Evil,
    Daily,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Classic, Mode::Fibonacci, Mode::Evil, Mode::Daily];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Classic => "Classic",
            Mode::Fibonacci => "Fibonacci",
            Mode::Evil => "Evil",
            Mode::Daily => "Daily",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Mode::Classic => "The original.",
            Mode::Fibonacci => "Tiles are 1, 2, 3, 5, 8, ... and neighbours in the sequence merge (3 + 5 = 8, 1 + 1 = 2).",
            Mode::Evil => "Each new tile lands where it leaves your best next move worst. The first two are random.",
            Mode::Daily => "Classic, but everyone gets the same game for the date and board size.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Right,
    Down,
    Left,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::Up, Dir::Right, Dir::Down, Dir::Left];

    fn vector(self) -> (i32, i32) {
        match self {
            Dir::Up => (0, -1),
            Dir::Right => (1, 0),
            Dir::Down => (0, 1),
            Dir::Left => (-1, 0),
        }
    }
}

/// The Fibonacci tiles: 1, 2, 3, 5, 8, ...
pub fn fib() -> Vec<u32> {
    let mut f = vec![1u32, 2];
    while *f.last().unwrap() < 100_000_000 {
        let n = f[f.len() - 1] + f[f.len() - 2];
        f.push(n);
    }
    f
}

/// The tile that wins: smaller boards cannot reach 2048, bigger ones reach it too easily.
pub fn goal_for(size: usize, mode: Mode) -> u32 {
    match (mode, size) {
        (Mode::Fibonacci, 3) => 144,
        (Mode::Fibonacci, 5) => 10946,
        (Mode::Fibonacci, 6) => 17711,
        (Mode::Fibonacci, _) => 2584,
        (_, 3) => 256,
        (_, 5) => 4096,
        (_, 6) => 8192,
        _ => 2048,
    }
}

/// How far up the scale a tile is (1 for the smallest, 11 at the goal of a 4x4 Classic board): for colours. Fibonacci's longer
/// sequence is scaled to reach 11 at its goal too.
pub fn level_of(value: u32, mode: Mode, size: usize) -> u32 {
    if mode == Mode::Fibonacci {
        let f = fib();
        let i = f.iter().position(|&v| v == value).unwrap_or(0) as f32;
        let g = f.iter().position(|&v| v == goal_for(size, mode)).unwrap_or(1) as f32;
        return 1 + (i * 10.0 / g).round() as u32;
    }
    (value.max(2) as f32).log2().round().max(1.0) as u32
}

pub fn can_merge(mode: Mode, a: u32, b: u32) -> bool {
    if mode != Mode::Fibonacci {
        return a == b;
    }
    if a == 1 && b == 1 {
        return true;
    }
    let f = fib();
    match (f.iter().position(|&v| v == a), f.iter().position(|&v| v == b)) {
        (Some(i), Some(j)) => i.abs_diff(j) == 1,
        _ => false,
    }
}

fn spawn_values(mode: Mode) -> [u32; 2] {
    if mode == Mode::Fibonacci { [1, 2] } else { [2, 4] }
}

/// A random number generator small enough to save with the game (mulberry32), so a Daily game is the same for everyone and a
/// resumed game carries on the same way.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rng(pub u32);

impl Rng {
    pub fn from_time() -> Rng {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        Rng((n ^ (n >> 32)) as u32 ^ std::process::id())
    }

    /// FNV-1a of `text`: the seed of a Daily game.
    pub fn from_text(text: &str) -> Rng {
        let mut h: u32 = 0x811c9dc5;
        for b in text.bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(0x01000193);
        }
        Rng(h)
    }

    /// A number in [0, 1).
    pub fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6D2B79F5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4294967296.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        ((self.next() * n as f64) as usize).min(n.saturating_sub(1))
    }
}

/// One game: the board (column-major: `cells[x][y]`, 0 for empty), the score and where it stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Game {
    pub size: usize,
    pub mode: Mode,
    pub goal: u32,
    pub cells: Vec<Vec<u32>>,
    pub score: u32,
    pub moves: u32,
    pub over: bool,
    pub won: bool,
    /// Won, and the player chose to go on.
    pub keep_playing: bool,
    /// Daily only: the date it is for.
    #[serde(default)]
    pub day: String,
    pub rng: Rng,
}

/// What a move did, for the animation: where each tile of the old board went, the cells where two became one, the new tile.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Moved {
    pub moved: bool,
    /// (value, from, to) for every tile of the board before the move.
    pub slides: Vec<(u32, (usize, usize), (usize, usize))>,
    pub merged: Vec<(usize, usize)>,
    pub spawned: Option<(usize, usize)>,
    pub gained: u32,
    pub won: bool,
}

impl Game {
    /// A new game with its two opening tiles. `day` (YYYY-MM-DD) seeds a Daily game.
    pub fn new(size: usize, mode: Mode, day: &str) -> Game {
        let size = size.clamp(MIN_SIZE, MAX_SIZE);
        let rng = if mode == Mode::Daily { Rng::from_text(&format!("{day}/{size}")) } else { Rng::from_time() };
        let mut g = Game { size, mode, goal: goal_for(size, mode), cells: vec![vec![0; size]; size], score: 0, moves: 0, over: false, won: false, keep_playing: false, day: if mode == Mode::Daily { day.into() } else { String::new() }, rng };
        // the opening two are random even in Evil, so it is not lost from the start
        g.spawn(true);
        g.spawn(true);
        g
    }

    pub fn free(&self) -> Vec<(usize, usize)> {
        let mut v = vec![];
        for x in 0..self.size {
            for y in 0..self.size {
                if self.cells[x][y] == 0 {
                    v.push((x, y));
                }
            }
        }
        v
    }

    pub fn max_tile(&self) -> u32 {
        self.cells.iter().flatten().copied().max().unwrap_or(0)
    }

    /// Stopped: no move left, or won and not played on.
    pub fn terminated(&self) -> bool {
        self.over || (self.won && !self.keep_playing)
    }

    /// Drops a new tile on a free cell: random (nine times in ten the smaller value), or the cruellest one in Evil.
    fn spawn(&mut self, plain: bool) -> Option<(usize, usize)> {
        let free = self.free();
        if free.is_empty() {
            return None;
        }
        let (x, y, v) = if self.mode == Mode::Evil && !plain {
            self.evil_pick(&free)
        } else {
            let (x, y) = free[self.rng.below(free.len())];
            let vals = spawn_values(self.mode);
            (x, y, if self.rng.next() < 0.9 { vals[0] } else { vals[1] })
        };
        self.cells[x][y] = v;
        Some((x, y))
    }

    /// Evil: the cell and value that leave the player's best reply worst (by room to move and pairs ready to merge).
    fn evil_pick(&mut self, free: &[(usize, usize)]) -> (usize, usize, u32) {
        let mut worst = f32::INFINITY;
        let mut picks = vec![];
        let mut g = self.cells.clone();
        for &(x, y) in free {
            for v in spawn_values(self.mode) {
                g[x][y] = v;
                let mut best = -1.0f32;
                for d in Dir::ALL {
                    if let Some(after) = slide(&g, self.size, d, self.mode).map(|r| r.0) {
                        best = best.max(openness(&after, self.size, self.mode));
                    }
                }
                g[x][y] = 0;
                if best < worst {
                    worst = best;
                    picks.clear();
                }
                if best == worst {
                    picks.push((x, y, v));
                }
            }
        }
        picks[self.rng.below(picks.len())]
    }

    /// Plays a move. Nothing happens when the game is over (or won and not played on) or the move changes nothing.
    pub fn play(&mut self, dir: Dir) -> Moved {
        let mut out = Moved::default();
        if self.terminated() {
            return out;
        }
        let Some((cells, slides, merged, gained)) = slide(&self.cells, self.size, dir, self.mode) else { return out };
        self.cells = cells;
        self.score += gained;
        self.moves += 1;
        out.moved = true;
        out.slides = slides;
        out.gained = gained;
        if !self.won && merged.iter().any(|&(x, y)| self.cells[x][y] >= self.goal) {
            self.won = true;
            out.won = true;
        }
        out.merged = merged;
        out.spawned = self.spawn(false);
        if !self.moves_available() {
            self.over = true;
        }
        out
    }

    pub fn moves_available(&self) -> bool {
        !self.free().is_empty() || Dir::ALL.iter().any(|&d| slide(&self.cells, self.size, d, self.mode).is_some())
    }
}

/// The last moves, to take back (at most 50).
#[derive(Clone, Debug, Default)]
pub struct Undo {
    stack: Vec<Game>,
}

impl Undo {
    pub const DEPTH: usize = 50;

    /// Remembers the game as it was before a move.
    pub fn push(&mut self, before: Game) {
        self.stack.push(before);
        if self.stack.len() > Self::DEPTH {
            self.stack.remove(0);
        }
    }

    pub fn pop(&mut self) -> Option<Game> {
        self.stack.pop()
    }

    pub fn clear(&mut self) {
        self.stack.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}

type SlideResult = (Vec<Vec<u32>>, Vec<(u32, (usize, usize), (usize, usize))>, Vec<(usize, usize)>, u32);

/// Slides a board: the new board, where each tile went, the cells where two merged, the points. None if nothing moved.
pub fn slide(g: &[Vec<u32>], size: usize, dir: Dir, mode: Mode) -> Option<SlideResult> {
    let (vx, vy) = dir.vector();
    let mut out: Vec<Vec<u32>> = g.to_vec();
    let mut merged = vec![vec![false; size]; size];
    let mut slides = vec![];
    let mut merges = vec![];
    let mut gained = 0;
    let mut moved = false;
    // the tiles nearest the wall they move toward go first
    let order = |i: usize, toward: i32| if toward == 1 { size - 1 - i } else { i };
    for i in 0..size {
        for j in 0..size {
            let (cx, cy) = (order(i, vx), order(j, vy));
            let v = out[cx][cy];
            if v == 0 {
                continue;
            }
            let (mut fx, mut fy) = (cx as i32, cy as i32);
            let (mut nx, mut ny) = (fx + vx, fy + vy);
            let inside = |x: i32, y: i32| x >= 0 && y >= 0 && (x as usize) < size && (y as usize) < size;
            while inside(nx, ny) && out[nx as usize][ny as usize] == 0 {
                fx = nx;
                fy = ny;
                nx += vx;
                ny += vy;
            }
            if inside(nx, ny) && !merged[nx as usize][ny as usize] && can_merge(mode, v, out[nx as usize][ny as usize]) {
                let (tx, ty) = (nx as usize, ny as usize);
                out[tx][ty] += v;
                gained += out[tx][ty];
                merged[tx][ty] = true;
                out[cx][cy] = 0;
                slides.push((v, (cx, cy), (tx, ty)));
                merges.push((tx, ty));
                moved = true;
            } else {
                let (tx, ty) = (fx as usize, fy as usize);
                if (tx, ty) != (cx, cy) {
                    out[tx][ty] = v;
                    out[cx][cy] = 0;
                    moved = true;
                }
                slides.push((v, (cx, cy), (tx, ty)));
            }
        }
    }
    moved.then_some((out, slides, merges, gained))
}

/// How good a position is for the player: room to move and pairs ready to merge.
fn openness(g: &[Vec<u32>], size: usize, mode: Mode) -> f32 {
    let mut s = 0.0;
    for x in 0..size {
        for y in 0..size {
            if g[x][y] == 0 {
                s += 1.0;
                continue;
            }
            if x + 1 < size && g[x + 1][y] != 0 && can_merge(mode, g[x][y], g[x + 1][y]) {
                s += 0.5;
            }
            if y + 1 < size && g[x][y + 1] != 0 && can_merge(mode, g[x][y], g[x][y + 1]) {
                s += 0.5;
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A board from rows (as seen on screen: rows[y][x]).
    fn board(rows: &[&[u32]]) -> Game {
        let size = rows.len();
        let mut g = Game::new(size, Mode::Classic, "");
        for x in 0..size {
            for y in 0..size {
                g.cells[x][y] = rows[y][x];
            }
        }
        g.over = false;
        g
    }

    fn rows(g: &[Vec<u32>]) -> Vec<Vec<u32>> {
        let n = g.len();
        (0..n).map(|y| (0..n).map(|x| g[x][y]).collect()).collect()
    }

    #[test]
    fn tiles_slide_and_merge_once_in_every_direction() {
        let g = board(&[&[2, 2, 2, 2], &[0, 4, 0, 4], &[0, 0, 0, 0], &[8, 0, 0, 8]]);
        let (left, _, merges, pts) = slide(&g.cells, 4, Dir::Left, Mode::Classic).unwrap();
        assert_eq!(rows(&left), vec![vec![4, 4, 0, 0], vec![8, 0, 0, 0], vec![0, 0, 0, 0], vec![16, 0, 0, 0]]);
        assert_eq!((merges.len(), pts), (4, 4 + 4 + 8 + 16));
        let (right, ..) = slide(&g.cells, 4, Dir::Right, Mode::Classic).unwrap();
        assert_eq!(rows(&right), vec![vec![0, 0, 4, 4], vec![0, 0, 0, 8], vec![0, 0, 0, 0], vec![0, 0, 0, 16]]);
        let (up, ..) = slide(&g.cells, 4, Dir::Up, Mode::Classic).unwrap();
        assert_eq!(rows(&up), vec![vec![2, 2, 2, 2], vec![8, 4, 0, 4], vec![0, 0, 0, 8], vec![0, 0, 0, 0]]);
        let (down, ..) = slide(&g.cells, 4, Dir::Down, Mode::Classic).unwrap();
        assert_eq!(rows(&down), vec![vec![0, 0, 0, 0], vec![0, 0, 0, 2], vec![2, 2, 0, 4], vec![8, 4, 2, 8]]);
    }

    #[test]
    fn three_in_a_row_merge_the_two_at_the_wall() {
        let g = board(&[&[2, 2, 2], &[0, 0, 0], &[0, 0, 0]]);
        let (left, ..) = slide(&g.cells, 3, Dir::Left, Mode::Classic).unwrap();
        assert_eq!(rows(&left)[0], vec![4, 2, 0]);
        let (right, ..) = slide(&g.cells, 3, Dir::Right, Mode::Classic).unwrap();
        assert_eq!(rows(&right)[0], vec![0, 2, 4]);
        // a merged tile does not merge again in the same move
        let g = board(&[&[4, 2, 2], &[0, 0, 0], &[0, 0, 0]]);
        assert_eq!(rows(&slide(&g.cells, 3, Dir::Left, Mode::Classic).unwrap().0)[0], vec![4, 4, 0]);
    }

    #[test]
    fn a_move_that_changes_nothing_is_no_move() {
        let mut g = board(&[&[2, 4, 8], &[0, 0, 0], &[0, 0, 0]]);
        assert!(slide(&g.cells, 3, Dir::Left, Mode::Classic).is_none());
        assert!(slide(&g.cells, 3, Dir::Up, Mode::Classic).is_none());
        let before = g.clone();
        assert!(!g.play(Dir::Left).moved);
        assert_eq!(g, before, "nothing spawns after a move that did nothing");
        let m = g.play(Dir::Down);
        assert!(m.moved && m.spawned.is_some());
        assert_eq!(g.moves, 1);
    }

    #[test]
    fn fibonacci_neighbours_merge() {
        assert!(can_merge(Mode::Fibonacci, 3, 5) && can_merge(Mode::Fibonacci, 5, 3) && can_merge(Mode::Fibonacci, 1, 1) && can_merge(Mode::Fibonacci, 1, 2));
        assert!(!can_merge(Mode::Fibonacci, 3, 3) && !can_merge(Mode::Fibonacci, 2, 5));
        assert!(!can_merge(Mode::Classic, 3, 5));
        let mut g = board(&[&[3, 5, 0], &[1, 1, 0], &[2, 2, 0]]);
        g.mode = Mode::Fibonacci;
        let (left, _, _, pts) = slide(&g.cells, 3, Dir::Left, Mode::Fibonacci).unwrap();
        assert_eq!(rows(&left), vec![vec![8, 0, 0], vec![2, 0, 0], vec![2, 2, 0]]);
        assert_eq!(pts, 10);
        assert_eq!(goal_for(4, Mode::Fibonacci), 2584);
    }

    #[test]
    fn the_game_ends_when_no_move_is_left() {
        let full = board(&[&[2, 4, 2], &[4, 2, 4], &[2, 4, 2]]);
        assert!(!full.moves_available());
        let mergeable = board(&[&[2, 4, 2], &[4, 2, 4], &[2, 4, 4]]);
        assert!(mergeable.moves_available());
        let mut g = board(&[&[2, 4, 2], &[4, 2, 4], &[8, 16, 0]]);
        g.play(Dir::Right);
        // after the move the spawned tile fills the board; whether it is over depends on that tile, so just check it agrees
        assert_eq!(g.over, !g.moves_available());
    }

    #[test]
    fn winning_stops_until_playing_on() {
        let mut g = board(&[&[1024, 1024, 0, 0], &[0, 0, 0, 0], &[0, 0, 0, 0], &[0, 0, 0, 0]]);
        assert!(g.play(Dir::Left).won && g.won && g.terminated());
        assert!(!g.play(Dir::Right).moved);
        g.keep_playing = true;
        assert!(g.play(Dir::Right).moved);
    }

    #[test]
    fn daily_games_are_the_same_for_the_same_day_and_size() {
        let a = Game::new(4, Mode::Daily, "2026-10-25");
        let b = Game::new(4, Mode::Daily, "2026-10-25");
        assert_eq!(a.cells, b.cells);
        let mut a2 = a.clone();
        let mut b2 = b.clone();
        for d in [Dir::Left, Dir::Up, Dir::Right, Dir::Down, Dir::Left] {
            a2.play(d);
            b2.play(d);
        }
        assert_eq!(a2, b2);
    }

    #[test]
    fn undo_takes_moves_back_even_out_of_a_game_over() {
        let mut g = board(&[&[2, 2, 0], &[0, 0, 0], &[0, 0, 0]]);
        let mut undo = Undo::default();
        let start = g.clone();
        undo.push(g.clone());
        assert!(g.play(Dir::Left).moved);
        let after_one = g.clone();
        undo.push(g.clone());
        g.play(Dir::Up);
        g.over = true;
        g = undo.pop().unwrap();
        assert_eq!(g, after_one);
        g = undo.pop().unwrap();
        assert_eq!(g, start);
        assert!(undo.pop().is_none());
        for _ in 0..60 {
            undo.push(start.clone());
        }
        let mut n = 0;
        while undo.pop().is_some() {
            n += 1;
        }
        assert_eq!(n, Undo::DEPTH);
    }

    #[test]
    fn evil_spawns_somewhere_free() {
        let mut g = Game::new(4, Mode::Evil, "");
        for d in [Dir::Left, Dir::Up, Dir::Right, Dir::Down, Dir::Left, Dir::Up] {
            let before = g.cells.iter().flatten().filter(|&&v| v != 0).count();
            let m = g.play(d);
            if m.moved {
                let (x, y) = m.spawned.unwrap();
                assert!(g.cells[x][y] == 2 || g.cells[x][y] == 4);
                assert!(g.cells.iter().flatten().filter(|&&v| v != 0).count() <= before + 1);
            }
        }
    }
}
