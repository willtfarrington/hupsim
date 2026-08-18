//! hupsim-data — compiles the four OSINT data files into a runnable world:
//!
//! * `hup_topology.json` — the confidence-tagged property graph (buildings,
//!   edges, elevator cores, limitations)
//! * `units.json` — curated unit roster with room-generation specs, room
//!   capability rules, and target RN staffing ratios
//! * `service_lines.json` — service-line taxonomy + total unit → line mapping
//! * `layout.json` — 3D massing footprints
//!
//! Output: `hupsim_core::Hospital` + `CampusLayout`, plus deterministic demo
//! census seeding and scenario file I/O.

pub mod compile;
pub mod io;
#[cfg(feature = "tools")]
pub mod kml;
#[cfg(feature = "tools")]
pub mod layoutdoc;
#[cfg(feature = "tools")]
pub mod podium;
pub mod rawtopo;
pub mod seed;
pub mod service_lines;
pub mod unitsfile;

pub use compile::{compile, CompileError};
pub use io::{load_default, DataError, LoadedWorld};
pub use seed::seed_demo_census;
