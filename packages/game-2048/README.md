# yggdrasil-2048

2048 as a Yggdrasil app (Rust, egui, yggdrasil-ui): slide the tiles, two that match merge, reach the goal tile and play on if
you like.

- **Moves:** arrow keys, WASD or hjkl, or drag across the board. `U` (or Ctrl+Z) undoes, up to 50 moves and also out of a game
  over. `N` starts a new game.
- **Animation:** tiles slide, merged ones pop and new ones grow. The window redraws only while something moves.
- **Modes:**

  | Mode | What changes |
  |---|---|
  | Classic | The original. The goal is 256, 2048, 4096 or 8192 on 3x3 to 6x6 boards |
  | Fibonacci | Tiles are 1, 2, 3, 5, 8, ... and neighbours in the sequence merge. The goal is 144, 2584, 10946 or 17711 |
  | Evil | Each new tile lands where it leaves your best next move worst. The first two are random |
  | Daily | Classic, but everyone gets the same game for the date (UTC) and board size; the best score is for today |

- **Boards:** sizes from 3x3 to 6x6. Each mode and size keeps its own game, best score and stats.
- **Stats:** games, wins, win rate, best tile and best score, for this mode and size or for all of them. A Fibonacci tile is
  left out of the overall best tile.
- **Tile colours:** shades of the system accent, or the original game's colours.
- **Saving:** the game, best scores and stats go to `~/.local/share/yggdrasil/2048/state.json`, written whole and renamed into
  place. It never uses the network.

The rules and timings follow [gabrielecirulli/2048](https://github.com/gabrielecirulli/2048). The modes, goals by board size
and the Evil rule follow [imranZERO/omarchy-2048](https://github.com/imranZERO/omarchy-2048). Both are MIT, and this is a new
program written from them.

    cargo build --release      # target/release/yggdrasil-2048
    cargo test                 # move and merge rules, Fibonacci, undo, game over, Daily, Evil

Installs: `/usr/bin/yggdrasil-2048`, `yggdrasil-2048.desktop` in `/usr/share/applications`; the icon `yggdrasil-2048` comes
from Liki.
