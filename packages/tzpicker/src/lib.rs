//! The time zone picker of Settings > Date & time and the installer: a world map in the Yggdrasil look (the zone bands of Natural
//! Earth, the chosen one in the highlight colour) beside a short list of friendly zones grouped by UTC offset. See README.md.

pub mod catalog;
pub mod draw;
mod picker;
pub mod world;

pub use catalog::Catalog;
pub use picker::{local_time, Options, Picker};
