//! Version-keyed aggregate cache (EP-9 item 4): one recompute per
//! `AppWorld.version` bump instead of per egui frame.
//!
//! Before this, the navigator recomputed ~10 building rollups every frame and
//! the status bar re-scanned all 1,115 rooms (`ui.rs`), and `apply_colors`
//! re-aggregated per slab per dirty frame. Now every consumer reads this
//! resource; a version bump invalidates once, and the recompute itself runs
//! on the columnar [`RoomMatrix`] (refresh fast path + matrix transforms),
//! not on `Vec<Room>` scans.
//!
//! This cache deliberately owns its **own** matrix rather than reading
//! `SimDrive.engine.matrix`: the engine's copy refreshes only during ticks,
//! so it is stale while the clock is paused and during editor mutations. The
//! cache keys on `AppWorld.version` and reads the `Hospital` directly. It is
//! derived, read-only state — it never mutates sim state or touches the RNG.

use crate::world::AppWorld;
use bevy::prelude::*;
use hupsim_core::aggregate::{HospitalStats, UnitAggregate};
use hupsim_core::ids::NodeId;
use hupsim_core::matrix::RoomMatrix;
use hupsim_core::model::{FloorLevel, UnitType};
use std::collections::HashMap;

#[derive(Resource, Default)]
pub struct AggCache {
    /// `AppWorld.version` the cached results were computed for (0 = never —
    /// real versions start at 1, so the first consumer always computes).
    version: u64,
    /// The app-side columnar view; rebuilt/refreshed here, nowhere else.
    pub matrix: RoomMatrix,
    /// Per-unit aggregates, indexed like `Hospital.units`.
    pub units: Vec<UnitAggregate>,
    /// Per-unit occupied-room counts by stability tier (Stable, Watcher,
    /// Unstable, Critical), indexed like `Hospital.units` — the unit
    /// dashboard's histogram input (EP-12).
    pub unit_tiers: Vec<[u32; 4]>,
    /// Whole-building rollups, indexed like `Hospital.buildings`.
    pub buildings: Vec<UnitAggregate>,
    /// Per-(building, floor) rollups for slab tints, tooltips, inspectors.
    floors: HashMap<NodeId, HashMap<FloorLevel, UnitAggregate>>,
    /// Level-of-care rollup in [`UnitType::ALL`] order (EP-13 grouping 1).
    pub by_unit_type: Vec<(UnitType, UnitAggregate)>,
    /// Service-line rollup indexed like `Hospital.service_lines` (grouping 2).
    pub by_service_line: Vec<UnitAggregate>,
    /// Status-bar headline numbers.
    pub stats: HospitalStats,
    /// EP-17 census forecast fan for the *live* world (never experiment
    /// arms — scope fence), recomputed when the world version or the sim
    /// instant moves. `None` only before the first consumer frame.
    pub fan: Option<hupsim_sim::forecast::CensusFan>,
    /// (version, now_min bits) the fan was computed for.
    fan_key: (u64, u64),
}

impl AggCache {
    /// Recompute everything iff `AppWorld.version` moved. Cheap to call from
    /// every consumer system; the matrix refresh takes the status-column fast
    /// path whenever the room set is structurally unchanged.
    pub fn refresh_if_stale(&mut self, world: &AppWorld) {
        if self.version == world.version {
            return;
        }
        let h = world.hospital();
        self.matrix.refresh(h, &world.index);
        self.units = self.matrix.unit_aggregates(h.units.len());
        self.unit_tiers = self.matrix.tier_counts_by_unit(h.units.len());
        self.buildings = self.matrix.rollup_by_building(h.buildings.len());
        self.by_unit_type = self.matrix.rollup_by_unit_type();
        self.by_service_line = self.matrix.rollup_by_service_line(h.service_lines.len());
        self.stats = self.matrix.hospital_stats();
        self.floors.clear();
        for (ui, u) in h.units.iter().enumerate() {
            if let Some(agg) = self.units.get(ui) {
                self.floors
                    .entry(u.building.clone())
                    .or_default()
                    .entry(u.level.clone())
                    .or_default()
                    .absorb(agg);
            }
        }
        self.version = world.version;
    }

    /// Recompute the census fan iff the world version or sim instant moved.
    /// Reads the `Hospital` directly (like everything here) — the forecast
    /// is pure and RNG-free, so this cannot perturb determinism.
    pub fn refresh_fan(&mut self, world: &AppWorld, now_min: f64) {
        let key = (world.version, now_min.to_bits());
        if self.fan_key == key && self.fan.is_some() {
            return;
        }
        self.fan_key = key;
        self.fan = Some(hupsim_sim::forecast::forecast_census(
            world.hospital(),
            &world.index,
            now_min,
            &hupsim_sim::forecast::ForecastParams::default(),
        ));
    }

    /// Aggregate for the unit at `unit_pos` (`HospitalIndex::unit_pos`).
    pub fn unit_at(&self, unit_pos: usize) -> UnitAggregate {
        self.units.get(unit_pos).cloned().unwrap_or_default()
    }

    /// Stability-tier counts for the unit at `unit_pos` (Stable, Watcher,
    /// Unstable, Critical).
    pub fn unit_tiers_at(&self, unit_pos: usize) -> [u32; 4] {
        self.unit_tiers.get(unit_pos).copied().unwrap_or_default()
    }

    /// Whole-building aggregate (default = zero capacity for bedless wings).
    pub fn building(&self, world: &AppWorld, id: &NodeId) -> UnitAggregate {
        world
            .index
            .building_pos
            .get(id)
            .and_then(|&bp| self.buildings.get(bp))
            .cloned()
            .unwrap_or_default()
    }

    /// Aggregate for one floor of one building.
    pub fn floor(&self, id: &NodeId, level: &FloorLevel) -> UnitAggregate {
        self.floors
            .get(id)
            .and_then(|m| m.get(level))
            .cloned()
            .unwrap_or_default()
    }
}
