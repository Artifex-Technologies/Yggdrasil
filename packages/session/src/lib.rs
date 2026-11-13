//! The session's small programs, each a bin with the name, arguments, output, files, calls and exit codes of the shell script it
//! replaced (packages/session/bin until 2026-11-12). What they share is in `run`.

pub mod hypr_logkeep;
pub mod hypr_plugin;
pub mod run;
pub mod screen_reader;
pub mod zoom;
