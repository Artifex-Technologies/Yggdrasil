//! Settings > Power's root helpers in Rust, replacing the Python scripts at the same paths with the same interface
//! (commands, arguments, JSON, settings and state files, exit codes, log lines):
//!
//!   tune        /usr/lib/yggdrasil/tune        processor multipliers and power limits (MSRs, RAPL), graphics card clocks
//!                                              (NVML), with the crash guard; `tune watch` was the root service yggdrasil-tune
//!   casefan     /usr/lib/yggdrasil/casefan     Corsair Commander Pro case fans and pump; `casefan run` was yggdrasil-casefan
//!   gpufan      /usr/lib/yggdrasil/gpufan      NVIDIA fans through NVML; `gpufan run` was yggdrasil-gpufan
//!   powerlimit  /usr/lib/yggdrasil/powerlimit  each power mode's processor and graphics card limits; `powerlimit watch` was
//!                                              yggdrasil-powerlimit
//!
//! and the service that runs those four loops in one process, so libnvidia-ml is initialised once:
//!
//!   power       /usr/lib/yggdrasil/power       `power run` is yggdrasil-power; the four old units switch its parts on and
//!                                              off (`power on PART`, `power off PART`); see power.rs
//!
//! No async runtime and no crate but libc: each loop is one thread that wakes every 2 or 3 seconds. libnvidia-ml is opened
//! at run time. Every command also takes --dry-run (print each write instead of making it), and with it --root DIR (a fake
//! tree, no hardware), --steps N (N rounds of a service's loop on a virtual clock) and --real-card (the real card on a fake
//! tree, read only).
//!
//! The merged service catches a part's panic on its thread (power.rs), so the release build must unwind on panic (the
//! default; never `panic = "abort"`).

pub mod casefan;
pub mod gpufan;
pub mod json;
pub mod nvml;
pub mod power;
pub mod powerlimit;
pub mod py;
pub mod sys;
pub mod tune;

#[cfg(test)]
pub mod testutil;
