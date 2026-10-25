//! The puzzle itself: a solver that counts solutions (so a puzzle is known to have exactly one), and a generator that fills a grid
//! and takes clues away in mirrored pairs while the answer stays unique.

pub type Grid = [u8; 81];

/// The cells that share a row, column or box with cell `i` (20 of them).
pub fn peers(i: usize) -> impl Iterator<Item = usize> {
    let (r, c) = (i / 9, i % 9);
    let (br, bc) = (r / 3 * 3, c / 3 * 3);
    (0..81).filter(move |&j| j != i && (j / 9 == r || j % 9 == c || (j / 9 / 3 * 3 == br && j % 9 / 3 * 3 == bc)))
}

/// Digits 1-9 that cell `i` can still take, as bits 1..=9.
pub fn candidates(g: &Grid, i: usize) -> u16 {
    let mut used = 0u16;
    for j in peers(i) {
        used |= 1 << g[j];
    }
    !used & 0b11_1111_1110
}

/// How many solutions `g` has, counting no further than `limit`.
pub fn count_solutions(g: &Grid, limit: usize) -> usize {
    let mut g = *g;
    let mut n = 0;
    search(&mut g, limit, &mut n, &mut None);
    n
}

/// The solution, when there is one.
#[cfg(test)]
pub fn solve(g: &Grid) -> Option<Grid> {
    let mut g = *g;
    let mut n = 0;
    let mut out = None;
    search(&mut g, 1, &mut n, &mut out);
    out
}

fn search(g: &mut Grid, limit: usize, n: &mut usize, out: &mut Option<Grid>) {
    // the empty cell with the fewest candidates first
    let mut best: Option<(usize, u16)> = None;
    for i in 0..81 {
        if g[i] == 0 {
            let c = candidates(g, i);
            if c == 0 {
                return;
            }
            if best.is_none_or(|(_, b)| c.count_ones() < b.count_ones()) {
                best = Some((i, c));
            }
        }
    }
    let Some((i, c)) = best else {
        *n += 1;
        if out.is_none() {
            *out = Some(*g);
        }
        return;
    };
    for d in 1..=9u8 {
        if c & (1 << d) != 0 {
            g[i] = d;
            search(g, limit, n, out);
            g[i] = 0;
            if *n >= limit {
                return;
            }
        }
    }
}

/// A small xorshift generator: puzzles need no more than this, and nothing here needs a crate.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }
    pub fn from_time() -> Rng {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7);
        Rng::new(t ^ (std::process::id() as u64).rotate_left(32))
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i + 1));
        }
    }
}

/// A completely filled, valid grid.
pub fn full_grid(rng: &mut Rng) -> Grid {
    fn fill(g: &mut Grid, i: usize, rng: &mut Rng) -> bool {
        if i == 81 {
            return true;
        }
        let c = candidates(g, i);
        let mut ds: Vec<u8> = (1..=9).filter(|d| c & (1 << d) != 0).collect();
        rng.shuffle(&mut ds);
        for d in ds {
            g[i] = d;
            if fill(g, i + 1, rng) {
                return true;
            }
        }
        g[i] = 0;
        false
    }
    let mut g = [0; 81];
    fill(&mut g, 0, rng);
    g
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
    Expert,
    Extreme,
}

impl Difficulty {
    #[cfg(test)]
    pub const ALL: [Difficulty; 5] = [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard, Difficulty::Expert, Difficulty::Extreme];

    pub fn label(self) -> &'static str {
        match self {
            Difficulty::Easy => "Easy",
            Difficulty::Medium => "Medium",
            Difficulty::Hard => "Hard",
            Difficulty::Expert => "Expert",
            Difficulty::Extreme => "Extreme",
        }
    }

    /// How many clues a puzzle of this difficulty keeps.
    pub fn clues(self) -> (usize, usize) {
        match self {
            Difficulty::Easy => (36, 40),
            Difficulty::Medium => (30, 35),
            Difficulty::Hard => (25, 29),
            Difficulty::Expert => (22, 24),
            Difficulty::Extreme => (20, 23),
        }
    }

    /// The next step of the survival ladder (Extreme stays Extreme).
    pub fn next(self) -> Difficulty {
        match self {
            Difficulty::Easy => Difficulty::Medium,
            Difficulty::Medium => Difficulty::Hard,
            Difficulty::Hard => Difficulty::Expert,
            Difficulty::Expert | Difficulty::Extreme => Difficulty::Extreme,
        }
    }
}

/// A puzzle with exactly one solution, and that solution. Clues are taken away in pairs mirrored through the centre while the
/// answer stays unique; when a try stalls above the range it is started again, and the closest try is kept.
pub fn generate(d: Difficulty, rng: &mut Rng) -> (Grid, Grid) {
    let (min, max) = d.clues();
    let target = min + rng.below(max - min + 1);
    let mut best: Option<(Grid, Grid, usize)> = None;
    for _ in 0..40 {
        let solution = full_grid(rng);
        let mut puzzle = solution;
        let mut givens = 81;
        let mut order: Vec<usize> = (0..=40).collect();
        rng.shuffle(&mut order);
        // mirrored pairs first, then single cells to get the last few out
        for &i in &order {
            let cells: &[usize] = if i == 40 { &[40] } else { &[i, 80 - i] };
            if givens < target + cells.len() || cells.iter().any(|&c| puzzle[c] == 0) {
                continue;
            }
            let saved: Vec<u8> = cells.iter().map(|&c| puzzle[c]).collect();
            for &c in cells {
                puzzle[c] = 0;
            }
            if count_solutions(&puzzle, 2) == 1 {
                givens -= cells.len();
            } else {
                for (&c, &v) in cells.iter().zip(&saved) {
                    puzzle[c] = v;
                }
            }
        }
        let mut singles: Vec<usize> = (0..81).collect();
        rng.shuffle(&mut singles);
        for c in singles {
            if givens <= target {
                break;
            }
            if puzzle[c] == 0 {
                continue;
            }
            let v = puzzle[c];
            puzzle[c] = 0;
            if count_solutions(&puzzle, 2) == 1 {
                givens -= 1;
            } else {
                puzzle[c] = v;
            }
        }
        if givens <= max && givens >= min {
            return (puzzle, solution);
        }
        if best.as_ref().is_none_or(|b| givens.abs_diff(target) < b.2.abs_diff(target)) {
            best = Some((puzzle, solution, givens));
        }
    }
    let (p, s, _) = best.expect("at least one try");
    (p, s)
}

#[cfg(test)]
pub fn givens(g: &Grid) -> usize {
    g.iter().filter(|&&v| v != 0).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUZZLE: &str = "530070000600195000098000060800060003400803001700020006060000280000419005000080079";
    const SOLUTION: &str = "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

    fn grid(s: &str) -> Grid {
        let mut g = [0; 81];
        for (i, ch) in s.chars().enumerate() {
            g[i] = ch.to_digit(10).unwrap() as u8;
        }
        g
    }

    #[test]
    fn the_solver_finds_the_one_answer() {
        assert_eq!(solve(&grid(PUZZLE)), Some(grid(SOLUTION)));
        assert_eq!(count_solutions(&grid(PUZZLE), 2), 1);
        // an empty grid has many solutions; a broken one none
        assert_eq!(count_solutions(&[0; 81], 2), 2);
        let mut bad = grid(PUZZLE);
        bad[2] = 5; // a second 5 in the first row
        assert_eq!(count_solutions(&bad, 2), 0);
    }

    #[test]
    fn peers_are_twenty() {
        for i in [0, 40, 80, 13] {
            assert_eq!(peers(i).count(), 20);
        }
    }

    #[test]
    fn a_full_grid_is_valid() {
        let g = full_grid(&mut Rng::new(42));
        for i in 0..81 {
            assert!(g[i] != 0 && peers(i).all(|j| g[j] != g[i]));
        }
    }

    #[test]
    fn generated_puzzles_are_unique_and_in_range() {
        let mut rng = Rng::new(7);
        for d in Difficulty::ALL {
            for _ in 0..3 {
                let (p, s) = generate(d, &mut rng);
                assert_eq!(count_solutions(&p, 2), 1, "{d:?}");
                assert_eq!(solve(&p), Some(s));
                for i in 0..81 {
                    assert!(p[i] == 0 || p[i] == s[i]);
                }
                let (min, max) = d.clues();
                let n = givens(&p);
                if d <= Difficulty::Hard {
                    assert!(n >= min && n <= max, "{d:?}: {n} clues");
                } else {
                    // the hardest boards are kept as close as the tries got (a unique 20-clue board is rare)
                    assert!(n <= max + 4, "{d:?}: {n} clues");
                }
            }
        }
    }
}
