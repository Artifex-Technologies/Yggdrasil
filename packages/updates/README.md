# yggdrasil-updates

What the package repositories can update, asked without root and without touching pacman's own databases (a private copy of
the sync databases, refreshed with `fakeroot pacman -Sy --dbpath`, as checkupdates does). One checker for Agora's Updates and
Settings > General > Software Update, so both say the same. See the doc comment in `src/lib.rs`.
