# yggdrasil-sudoku

Sudoku as a Yggdrasil window (Rust, egui, yggdrasil-ui).

- **Classic:** Easy (36-40 clues), Medium (30-35), Hard (25-29), each with its best time (a game with hints does not count).
- **Survival challenge:** three lives. A wrong digit costs a life and is not kept; a cleared board gives two more lives and the
  next difficulty: Easy, Medium, Hard, Expert, then Extreme for as long as you last. The most boards cleared in one run is kept.
- Every puzzle has exactly one solution: clues are taken away in pairs mirrored through the centre, and only while a solver still
  finds a single answer. Puzzles are made on a thread of their own.
- Pencil notes (placing a digit clears it from the notes of its row, column and box), undo and redo, hints, a pause that hides
  the board, the row, column, box and same digit lit up, clashes in red, how many of each digit are left on the number pad.
- The game in progress and the stats are saved to `~/.local/share/yggdrasil/sudoku/` (`save.json`, `stats.json`). Nothing goes
  over the network.

| | |
|---|---|
| Click a cell / arrows or h j k l | select |
| 1-9, or a number on the pad | place (again: take it out) |
| 0, Backspace, Delete, right-click a cell | erase |
| N | notes on or off |
| Right-click a number on the pad | brush: every cell you click gets that number (Escape stops) |
| U or Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z | undo / redo |
| Shift+H | hint (H alone moves left) |
| P or Space | pause |

Rewritten in Rust from the ideas of [Morganczech/omarchy-sudoku-plugin](https://github.com/Morganczech/omarchy-sudoku-plugin) (MIT):
its modes, survival rules, clue ranges and controls. No code was copied. This crate is MPL-2.0, like the rest of Yggdrasil.
