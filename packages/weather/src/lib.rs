//! Yggdrasil weather: several independent forecasts combined into one answer with an
//! honest uncertainty, scored against measured temperatures over time.
//!
//! Rain nowcast: Open-Meteo's 15-minute precipitation for the next two hours (`nowcast`).
//!
//! Sources: Open-Meteo (free for non-commercial use, no key): GFS, ECMWF and ICON
//! models plus a 30-member ensemble; aviationweather.gov airport reports for measured
//! truth. No AccuWeather / Weather Company / Weather Channel: they need accounts or
//! paid keys, and nothing in their free offerings would add information the open
//! models do not already carry. A `Provider` trait would be the place to add them.

/// The system version (VERSION at the top of the Yggdrasil repository, the same as Labyrinth OS's), read when this is built.
pub const VERSION: &str = include_str!("../../../VERSION").trim_ascii();

/// What the program calls itself to the weather and place services: "yggdrasil-weather/1.0.0".
pub fn user_agent() -> String {
    format!("yggdrasil-weather/{VERSION}")
}

pub mod card;
pub mod config;
pub mod consensus;
pub mod locate;
pub mod nowcast;
pub mod observe;
pub mod openmeteo;
pub mod report;
pub mod timeutil;
pub mod verify;
