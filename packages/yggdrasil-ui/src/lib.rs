//! The shared look of Yggdrasil's own apps, so Minos, Settings and whatever comes next match:
//! the palette and egui theme (`theme`), the system font (`fonts`) and the self-drawn title bar with traffic lights
//! (`chrome`), and one window per app that its own command toggles (`single`). Also what every app that shows names or files
//! shares for privacy: the look-alike check of names (`lookalike`, the same file as Heimdall's) and the privacy pack's switches
//! (`privacy_prefs`). And drag and drop with other programs on Wayland (`dnd`), which winit lacks.

pub mod chrome;
pub mod dnd;
pub mod fonts;
pub mod lookalike;
pub mod privacy_prefs;
pub mod single;
pub mod sysicon;
pub mod theme;
pub mod touchmove;
