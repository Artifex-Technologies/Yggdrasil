//! Settings > Power's root helpers in Rust, replacing the Python scripts at the same paths with the same interface
//! (commands, arguments, JSON, settings and state files, exit codes, log lines):
//!
//!   tune     /usr/lib/yggdrasil/tune     processor multipliers and power limits (MSRs, RAPL), graphics card clocks (NVML),
//!                                        with the crash guard; `tune watch` is the root service yggdrasil-tune
//!   casefan  /usr/lib/yggdrasil/casefan  Corsair Commander Pro case fans and pump; `casefan run` is yggdrasil-casefan
//!   gpufan   /usr/lib/yggdrasil/gpufan   NVIDIA fans through NVML; `gpufan run` is yggdrasil-gpufan
//!
//! No async runtime and no crate but libc: each service is one thread that wakes every 2 seconds. libnvidia-ml is opened at
//! run time. Every command also takes --dry-run (print each write instead of making it), and with it --root DIR (a fake
//! tree, no hardware) and --steps N (N rounds of a service's loop on a virtual clock).

pub mod casefan;
pub mod gpufan;
pub mod json;
pub mod nvml;
pub mod powerlimit;
pub mod py;
pub mod sys;
pub mod tune;

#[cfg(test)]
pub mod testutil;
