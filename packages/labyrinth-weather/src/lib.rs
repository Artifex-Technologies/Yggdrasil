//! Labyrinth weather: several independent forecasts combined into one answer with an
//! honest uncertainty, scored against measured temperatures over time.
//!
//! Sources: Open-Meteo (free for non-commercial use, no key): GFS, ECMWF and ICON
//! models plus a 30-member ensemble; aviationweather.gov airport reports for measured
//! truth. No AccuWeather / Weather Company / Weather Channel: they need accounts or
//! paid keys, and nothing in their free offerings would add information the open
//! models do not already carry. A `Provider` trait would be the place to add them.

pub mod config;
pub mod consensus;
pub mod observe;
pub mod openmeteo;
pub mod report;
pub mod timeutil;
pub mod verify;
