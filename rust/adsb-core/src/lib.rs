//! adsb-core: Pure decode + tracking library for Mode S / ADS-B.
//!
//! No async, no I/O — just algorithms. This crate is the shared core used by
//! both `adsb-feeder` (edge device) and `adsb-server` (web server + CLI).

pub mod config;
pub mod cpr;
pub mod crc;
pub mod decode;
pub mod frame;
pub mod icao;
pub mod tracker;
pub mod types;

// Re-export commonly used types at crate root
pub use decode::decode;
pub use frame::{parse_frame, parse_frame_uncached, IcaoCache, ModeFrame};
pub use tracker::{AircraftState, TrackEvent, Tracker};
pub use types::*;
