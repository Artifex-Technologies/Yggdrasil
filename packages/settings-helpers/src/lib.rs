//! Settings' on-demand helpers in Rust, replacing the Python scripts at the same paths with the same interface (commands,
//! arguments, JSON, files, exit codes, messages):
//!
//!   account     /usr/lib/yggdrasil/account     Settings > Users, through pkexec: list, groups, add, admin, remove, name,
//!                                              password, login, sudo, group
//!   bootscreen  /usr/lib/yggdrasil/bootscreen  Settings > Appearance > Boot screen: get, and set through pkexec
//!   game-fix    /usr/lib/yggdrasil/game-fix    Settings > Games > Game fixes: list, apply, undo
//!   gpu-run     /usr/bin/yggdrasil-gpu-run     Settings > Games: a program on a chosen graphics card (a Steam launch option)
//!   battery     /usr/lib/yggdrasil/battery     The top bar's battery and its drop-down, low-battery warnings, the charge
//!                                              limit (set-limit and restore through pkexec / at boot)
//!   autozone    /usr/lib/yggdrasil/autozone    Settings > Date & Time: set the time zone from where the computer is, by its IP
//!                                              address (no Python script before it)
//!
//! Where a script would have stopped with a traceback (exit code 1), the Rust one stops too, with exit code 1 and a line on
//! standard error. The few places where one is stricter than its script are said in its module.

pub mod account;
pub mod autozone;
pub mod battery;
pub mod bootscreen;
pub mod gamefix;
pub mod gpurun;
pub mod py;

#[cfg(test)]
pub mod testutil;
