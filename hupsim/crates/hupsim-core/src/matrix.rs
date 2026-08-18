//! `RoomMatrix` — a derived columnar (struct-of-arrays) view of every room,
//! for rapid compiled transforms: one dense row per room in `Hospital.rooms`
//! order, scalar columns instead of the nested `RoomStatus::Occupied` struct
//! with its heap `String`s and `String`-keyed joins.
//!
//! `Vec<Room>` stays the source of truth and the serialization format
//! (scenario schema v1 untouched); the matrix is rebuilt/refreshed keyed on
//! the app's world-version counter. [`RoomMatrix::refresh`] is the fast path:
//! when the room set is structurally unchanged (same ids, same unit
//! assignments) it rewrites only the status-derived columns; any structural
//! change falls back to a full [`RoomMatrix::build`].
//!
//! Transforms come in scalar form and — behind the optional `parallel`
//! feature — in rayon form (`par_*`). The parallel variants chunk rows at a
//! fixed size and merge partials in chunk order, so they are deterministic
//! run-to-run regardless of thread scheduling; float means may differ from
//! the scalar path only in the last ulp (different summation grouping).
//! 1,115 rooms don't need parallelism — the point is that the pattern holds
//! at health-system row counts (see the EP-9 benchmark at 50k rooms).

use crate::aggregate::{HospitalStats, UnitAggregate};
use crate::ids::RoomId;
use crate::index::HospitalIndex;
use crate::model::{Hospital, Room, RoomStatus, UnitType};
use crate::patient::StabilityTier;
use std::collections::HashMap;
use std::ops::Range;

/// Sentinel for a row whose unit is not in the index (should not happen with
/// compiler-produced data; tolerated so a malformed scenario can't panic).
pub const NO_UNIT: u32 = u32::MAX;
/// Sentinel for a unit whose building is unknown.
pub const NO_BUILDING: u16 = u16::MAX;
/// Sentinel for a unit with no resolved service line (pre-EP-8 saves).
pub const NO_SERVICE_LINE: u16 = u16::MAX;
/// Sentinel for "no admission timestamp" (vacant/OOS room, or a patient
/// present before t0). `NEG_INFINITY` rather than NaN so column equality
/// (the determinism guard) stays reflexive.
pub const NO_ADMIT_MIN: f32 = f32::NEG_INFINITY;

/// Room occupancy state as a dense byte (the matrix's unpacking of
/// [`RoomStatus`], minus the heap payloads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OccState {
    Vacant = 0,
    Occupied = 1,
    OutOfService = 2,
}

/// Dense columnar view: element `i` of every column describes row `i`, which
/// is `Hospital.rooms[i]`. Patient-derived columns (`instability`,
/// `trend_per_hr`, `telemetry_need`, `isolation`, `boarding`,
/// `admitted_at_min`) are only meaningful where `occupancy` is `Occupied`;
/// gate them with [`RoomMatrix::occupied_mask`] or the `occupancy` column.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoomMatrix {
    // Structural columns (rewritten only by `build`).
    /// Index into `Hospital.units` ([`NO_UNIT`] if unresolvable).
    pub unit_pos: Vec<u32>,
    pub unit_type: Vec<UnitType>,
    /// Index into `Hospital.service_lines` ([`NO_SERVICE_LINE`] if none).
    pub service_line: Vec<u16>,
    /// Index into `Hospital.buildings` ([`NO_BUILDING`] if unresolvable).
    pub building_pos: Vec<u16>,
    pub telemetry_capable: Vec<bool>,
    pub negative_pressure: Vec<bool>,
    // Status-derived columns (rewritten by `refresh`).
    pub occupancy: Vec<OccState>,
    pub boarding: Vec<bool>,
    /// Patient instability clamped to 0..=1 (as the aggregators consume it);
    /// 0.0 where unoccupied.
    pub instability: Vec<f32>,
    pub trend_per_hr: Vec<f32>,
    /// Patient is *on* telemetry (vs. the room being telemetry-capable).
    pub telemetry_need: Vec<bool>,
    pub isolation: Vec<bool>,
    /// Sim-clock admission minute; [`NO_ADMIT_MIN`] where absent.
    pub admitted_at_min: Vec<f32>,
    // Side tables: row ↔ RoomId.
    pub room_ids: Vec<RoomId>,
    row_by_id: HashMap<RoomId, u32>,
}

impl RoomMatrix {
    pub fn len(&self) -> usize {
        self.unit_pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.unit_pos.is_empty()
    }

    /// Row index for a room id.
    pub fn row_of(&self, id: &RoomId) -> Option<u32> {
        self.row_by_id.get(id).copied()
    }

    /// Room id for a row index.
    pub fn room_id(&self, row: u32) -> &RoomId {
        &self.room_ids[row as usize]
    }

    /// Full construction from scratch: all columns, both side tables.
    pub fn build(h: &Hospital, idx: &HospitalIndex) -> Self {
        let n = h.rooms.len();
        let line_pos: HashMap<&crate::ids::ServiceLineId, u16> = h
            .service_lines
            .iter()
            .enumerate()
            .map(|(i, l)| (&l.id, i as u16))
            .collect();
        let mut m = Self {
            unit_pos: Vec::with_capacity(n),
            unit_type: Vec::with_capacity(n),
            service_line: Vec::with_capacity(n),
            building_pos: Vec::with_capacity(n),
            telemetry_capable: Vec::with_capacity(n),
            negative_pressure: Vec::with_capacity(n),
            occupancy: vec![OccState::Vacant; n],
            boarding: vec![false; n],
            instability: vec![0.0; n],
            trend_per_hr: vec![0.0; n],
            telemetry_need: vec![false; n],
            isolation: vec![false; n],
            admitted_at_min: vec![NO_ADMIT_MIN; n],
            room_ids: Vec::with_capacity(n),
            row_by_id: HashMap::with_capacity(n),
        };
        for (i, r) in h.rooms.iter().enumerate() {
            let (up, ut, sl, bp) = match idx.unit_pos.get(&r.unit) {
                Some(&up) => {
                    let u = &h.units[up];
                    (
                        up as u32,
                        u.unit_type,
                        u.service_line
                            .as_ref()
                            .and_then(|id| line_pos.get(id).copied())
                            .unwrap_or(NO_SERVICE_LINE),
                        idx.building_pos
                            .get(&u.building)
                            .map(|&b| b as u16)
                            .unwrap_or(NO_BUILDING),
                    )
                }
                None => (NO_UNIT, UnitType::Other, NO_SERVICE_LINE, NO_BUILDING),
            };
            m.unit_pos.push(up);
            m.unit_type.push(ut);
            m.service_line.push(sl);
            m.building_pos.push(bp);
            m.telemetry_capable.push(r.telemetry_capable);
            m.negative_pressure.push(r.negative_pressure);
            m.room_ids.push(r.id.clone());
            m.row_by_id.insert(r.id.clone(), i as u32);
            m.write_status_row(i, r);
        }
        m
    }

    /// Fast path: rewrite only the status-derived columns. Falls back to a
    /// full rebuild when the structure changed (room added/removed/reordered,
    /// or a room's unit assignment moved). Returns `true` when the fast path
    /// ran, `false` when it rebuilt. Static room attributes other than the
    /// unit assignment (capability flags, kind) are assumed immutable between
    /// structural edits — nothing in the app mutates them in place.
    pub fn refresh(&mut self, h: &Hospital, idx: &HospitalIndex) -> bool {
        let same_structure = h.rooms.len() == self.len()
            && h.rooms.iter().zip(&self.room_ids).all(|(r, id)| r.id == *id)
            && h.rooms.iter().zip(&self.unit_pos).all(|(r, &up)| {
                idx.unit_pos
                    .get(&r.unit)
                    .map(|&p| p as u32)
                    .unwrap_or(NO_UNIT)
                    == up
            });
        if !same_structure {
            *self = Self::build(h, idx);
            return false;
        }
        for (i, r) in h.rooms.iter().enumerate() {
            self.write_status_row(i, r);
        }
        true
    }

    fn write_status_row(&mut self, i: usize, r: &Room) {
        match &r.status {
            RoomStatus::Vacant | RoomStatus::OutOfService { .. } => {
                self.occupancy[i] = if r.status.is_vacant() {
                    OccState::Vacant
                } else {
                    OccState::OutOfService
                };
                self.boarding[i] = false;
                self.instability[i] = 0.0;
                self.trend_per_hr[i] = 0.0;
                self.telemetry_need[i] = false;
                self.isolation[i] = false;
                self.admitted_at_min[i] = NO_ADMIT_MIN;
            }
            RoomStatus::Occupied { patient } => {
                self.occupancy[i] = OccState::Occupied;
                self.boarding[i] = patient.boarding;
                self.instability[i] = patient.acuity.instability.clamp(0.0, 1.0);
                self.trend_per_hr[i] = patient.acuity.trend_per_hr;
                self.telemetry_need[i] = patient.telemetry;
                self.isolation[i] = patient.isolation.is_some();
                self.admitted_at_min[i] =
                    patient.admitted_at_min.map(|m| m as f32).unwrap_or(NO_ADMIT_MIN);
            }
        }
    }

    // ------------------------------------------------------------------
    // Transforms: filter/aggregate over columns, never touching Vec<Room>.
    // ------------------------------------------------------------------

    /// Rows where a patient is present (gates the patient-derived columns).
    pub fn occupied_mask(&self) -> Vec<bool> {
        self.occupancy.iter().map(|&o| o == OccState::Occupied).collect()
    }

    /// Vacant rooms matching a level-of-care / capability requirement — the
    /// placement-engine primitive ("vacant tele-capable Stepdown beds").
    pub fn vacancy_mask(
        &self,
        unit_type: Option<UnitType>,
        need_telemetry: bool,
        need_negative_pressure: bool,
    ) -> Vec<bool> {
        (0..self.len())
            .map(|i| {
                self.occupancy[i] == OccState::Vacant
                    && unit_type.is_none_or(|t| self.unit_type[i] == t)
                    && (!need_telemetry || self.telemetry_capable[i])
                    && (!need_negative_pressure || self.negative_pressure[i])
            })
            .collect()
    }

    /// Count rows satisfying `pred`, bucketed by `unit_pos`. `pred` receives
    /// a row index and reads whatever columns it needs.
    pub fn count_by_unit(&self, n_units: usize, pred: impl Fn(usize) -> bool) -> Vec<u32> {
        let mut counts = vec![0u32; n_units];
        for i in 0..self.len() {
            let up = self.unit_pos[i];
            if up != NO_UNIT && pred(i) {
                if let Some(c) = counts.get_mut(up as usize) {
                    *c += 1;
                }
            }
        }
        counts
    }

    /// Occupied-room counts per stability tier, bucketed by unit position —
    /// the unit dashboard's tier histogram (EP-12). Index order matches
    /// [`StabilityTier`]'s declaration: Stable, Watcher, Unstable, Critical.
    pub fn tier_counts_by_unit(&self, n_units: usize) -> Vec<[u32; 4]> {
        let mut counts = vec![[0u32; 4]; n_units];
        for i in 0..self.len() {
            let up = self.unit_pos[i];
            if up != NO_UNIT && self.occupancy[i] == OccState::Occupied {
                if let Some(c) = counts.get_mut(up as usize) {
                    let tier = match StabilityTier::from_instability(self.instability[i]) {
                        StabilityTier::Stable => 0,
                        StabilityTier::Watcher => 1,
                        StabilityTier::Unstable => 2,
                        StabilityTier::Critical => 3,
                    };
                    c[tier] += 1;
                }
            }
        }
        counts
    }

    /// Per-unit aggregates (indexed by unit position), matching
    /// [`crate::aggregate::aggregate_unit`] exactly: rows are visited in
    /// `Hospital.rooms` order, which preserves each unit's room order.
    pub fn unit_aggregates(&self, n_units: usize) -> Vec<UnitAggregate> {
        self.accumulate(0..self.len(), n_units, |m, i| m.unit_pos[i] as usize)
            .into_iter()
            .map(AggAcc::finalize)
            .collect()
    }

    /// Rollup by level of care, in [`UnitType::ALL`] order.
    pub fn rollup_by_unit_type(&self) -> Vec<(UnitType, UnitAggregate)> {
        let accs = self.accumulate(0..self.len(), UnitType::ALL.len(), |m, i| {
            m.unit_type[i].index()
        });
        UnitType::ALL
            .into_iter()
            .zip(accs.into_iter().map(AggAcc::finalize))
            .collect()
    }

    /// Rollup by service line, indexed like `Hospital.service_lines`. Rows
    /// with no resolved line (pre-EP-8 saves) are skipped.
    pub fn rollup_by_service_line(&self, n_lines: usize) -> Vec<UnitAggregate> {
        self.accumulate(0..self.len(), n_lines, |m, i| m.service_line[i] as usize)
            .into_iter()
            .map(AggAcc::finalize)
            .collect()
    }

    /// Rollup by building, indexed like `Hospital.buildings`.
    pub fn rollup_by_building(&self, n_buildings: usize) -> Vec<UnitAggregate> {
        self.accumulate(0..self.len(), n_buildings, |m, i| {
            m.building_pos[i] as usize
        })
        .into_iter()
        .map(AggAcc::finalize)
        .collect()
    }

    /// Whole-hospital headline numbers, matching
    /// [`crate::aggregate::hospital_stats`].
    pub fn hospital_stats(&self) -> HospitalStats {
        self.stats_over(0..self.len())
    }

    /// Shared bucketed accumulation: one pass over `range`, `key` maps a row
    /// to its bucket (out-of-range keys — the sentinels — are skipped).
    fn accumulate(
        &self,
        range: Range<usize>,
        n_buckets: usize,
        key: impl Fn(&Self, usize) -> usize,
    ) -> Vec<AggAcc> {
        let mut accs = vec![AggAcc::default(); n_buckets];
        for i in range {
            let k = key(self, i);
            if let Some(acc) = accs.get_mut(k) {
                acc.add_row(self, i);
            }
        }
        accs
    }

    fn stats_over(&self, range: Range<usize>) -> HospitalStats {
        let mut s = HospitalStats::default();
        for i in range {
            s.capacity += 1;
            match self.occupancy[i] {
                OccState::Vacant => {}
                OccState::OutOfService => s.out_of_service += 1,
                OccState::Occupied => {
                    s.census += 1;
                    if self.boarding[i] {
                        s.boarding += 1;
                    }
                    match StabilityTier::from_instability(self.instability[i]) {
                        StabilityTier::Critical => s.critical += 1,
                        StabilityTier::Unstable => s.unstable += 1,
                        _ => {}
                    }
                }
            }
        }
        s
    }
}

/// Mean/population-σ over `values[i]` where `mask[i]` — the z-score input for
/// "how does this room/unit compare to its peers right now". `None` when the
/// mask selects nothing.
pub fn masked_mean_std(values: &[f32], mask: &[bool]) -> Option<(f32, f32)> {
    assert_eq!(values.len(), mask.len(), "column/mask length mismatch");
    let mut n = 0u32;
    let mut sum = 0.0f32;
    for (v, &m) in values.iter().zip(mask) {
        if m {
            n += 1;
            sum += v;
        }
    }
    if n == 0 {
        return None;
    }
    let mean = sum / n as f32;
    let mut ssd = 0.0f32;
    for (v, &m) in values.iter().zip(mask) {
        if m {
            ssd += (v - mean) * (v - mean);
        }
    }
    Some((mean, (ssd / n as f32).sqrt()))
}

/// Intermediate accumulator: keeps the instability *sum* so partial results
/// merge exactly (means only recombine census-weighted).
#[derive(Debug, Clone, Default)]
struct AggAcc {
    capacity: u32,
    census: u32,
    out_of_service: u32,
    boarding: u32,
    sum_instability: f32,
    worst_instability: Option<f32>,
}

impl AggAcc {
    fn add_row(&mut self, m: &RoomMatrix, i: usize) {
        self.capacity += 1;
        match m.occupancy[i] {
            OccState::Vacant => {}
            OccState::OutOfService => self.out_of_service += 1,
            OccState::Occupied => {
                self.census += 1;
                if m.boarding[i] {
                    self.boarding += 1;
                }
                let x = m.instability[i];
                self.sum_instability += x;
                self.worst_instability =
                    Some(self.worst_instability.map_or(x, |w: f32| w.max(x)));
            }
        }
    }

    #[cfg(feature = "parallel")]
    fn merge(&mut self, o: &AggAcc) {
        self.capacity += o.capacity;
        self.census += o.census;
        self.out_of_service += o.out_of_service;
        self.boarding += o.boarding;
        self.sum_instability += o.sum_instability;
        self.worst_instability = match (self.worst_instability, o.worst_instability) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
    }

    fn finalize(self) -> UnitAggregate {
        UnitAggregate {
            capacity: self.capacity as usize,
            census: self.census as usize,
            out_of_service: self.out_of_service as usize,
            boarding: self.boarding as usize,
            mean_instability: (self.census > 0)
                .then(|| self.sum_instability / self.census as f32),
            worst_instability: self.worst_instability,
        }
    }
}

/// Rayon-parallel variants of the transforms. Rows are split into fixed-size
/// chunks; partials merge in chunk order, so results are deterministic
/// run-to-run (float means can differ from the scalar path in the last ulp).
#[cfg(feature = "parallel")]
mod par {
    use super::*;
    use rayon::prelude::*;

    /// Fixed chunk size: deterministic partial boundaries regardless of
    /// worker count, coarse enough that per-chunk overhead stays trivial.
    const CHUNK: usize = 4096;

    fn chunk_ranges(n: usize) -> Vec<Range<usize>> {
        (0..n).step_by(CHUNK).map(|s| s..(s + CHUNK).min(n)).collect()
    }

    impl RoomMatrix {
        pub fn par_unit_aggregates(&self, n_units: usize) -> Vec<UnitAggregate> {
            self.par_accumulate(n_units, |m, i| m.unit_pos[i] as usize)
        }

        pub fn par_rollup_by_unit_type(&self) -> Vec<(UnitType, UnitAggregate)> {
            let aggs = self.par_accumulate(UnitType::ALL.len(), |m, i| m.unit_type[i].index());
            UnitType::ALL.into_iter().zip(aggs).collect()
        }

        pub fn par_rollup_by_service_line(&self, n_lines: usize) -> Vec<UnitAggregate> {
            self.par_accumulate(n_lines, |m, i| m.service_line[i] as usize)
        }

        pub fn par_hospital_stats(&self) -> HospitalStats {
            chunk_ranges(self.len())
                .into_par_iter()
                .map(|r| self.stats_over(r))
                .collect::<Vec<_>>()
                .into_iter()
                .fold(HospitalStats::default(), |mut a, b| {
                    a.capacity += b.capacity;
                    a.census += b.census;
                    a.out_of_service += b.out_of_service;
                    a.boarding += b.boarding;
                    a.critical += b.critical;
                    a.unstable += b.unstable;
                    a
                })
        }

        pub fn par_count_by_unit(
            &self,
            n_units: usize,
            pred: impl Fn(usize) -> bool + Sync,
        ) -> Vec<u32> {
            let partials: Vec<Vec<u32>> = chunk_ranges(self.len())
                .into_par_iter()
                .map(|range| {
                    let mut counts = vec![0u32; n_units];
                    for i in range {
                        let up = self.unit_pos[i];
                        if up != NO_UNIT && pred(i) {
                            if let Some(c) = counts.get_mut(up as usize) {
                                *c += 1;
                            }
                        }
                    }
                    counts
                })
                .collect();
            let mut counts = vec![0u32; n_units];
            for p in partials {
                for (a, b) in counts.iter_mut().zip(p) {
                    *a += b;
                }
            }
            counts
        }

        fn par_accumulate(
            &self,
            n_buckets: usize,
            key: impl Fn(&Self, usize) -> usize + Sync,
        ) -> Vec<UnitAggregate> {
            let partials: Vec<Vec<AggAcc>> = chunk_ranges(self.len())
                .into_par_iter()
                .map(|range| self.accumulate(range, n_buckets, &key))
                .collect();
            let mut accs = vec![AggAcc::default(); n_buckets];
            for p in partials {
                for (a, b) in accs.iter_mut().zip(&p) {
                    a.merge(b);
                }
            }
            accs.into_iter().map(AggAcc::finalize).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{aggregate_unit, hospital_stats};
    use crate::model::*;
    use crate::patient::{Acuity, Patient};
    use crate::provenance::Provenance;

    fn patient(instability: f32, boarding: bool) -> Patient {
        Patient {
            id: "p".into(),
            alias: "P".into(),
            age: None,
            service: None,
            admitted_at_min: Some(120.0),
            acuity: Acuity {
                instability,
                trend_per_hr: 0.01,
            },
            boarding,
            isolation: None,
            telemetry: true,
            note: String::new(),
        }
    }

    fn hospital() -> Hospital {
        let unit = |id: &str, building: &str, ut: UnitType, line: Option<&str>| Unit {
            id: id.into(),
            name: id.to_uppercase(),
            service: "S".into(),
            service_line: line.map(|l| l.into()),
            unit_type: ut,
            building: building.into(),
            level: FloorLevel::Numbered(1),
            elevator_core: None,
            target_rn_ratio: None,
            provenance: Provenance::default(),
            note: None,
        };
        let building = |id: &str| Building {
            id: id.into(),
            name: id.to_uppercase(),
            short_name: id.to_uppercase(),
            kind: BuildingKind::Wing,
            built: None,
            aliases: vec![],
            floors: vec![],
            has_inpatient_beds: true,
            provenance: Provenance::default(),
            note: None,
        };
        let room = |id: &str, unit: &str, tele: bool, status: RoomStatus| Room {
            id: id.into(),
            number: id.into(),
            unit: unit.into(),
            kind: RoomKind::Inpatient,
            telemetry_capable: tele,
            negative_pressure: false,
            status,
            provenance: Provenance::default(),
        };
        Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![building("b1"), building("b2")],
            units: vec![
                unit("u1", "b1", UnitType::MedSurg, Some("line.medicine")),
                unit("u2", "b2", UnitType::Icu, Some("line.critical")),
            ],
            rooms: vec![
                room(
                    "r1",
                    "u1",
                    true,
                    RoomStatus::Occupied {
                        patient: patient(0.2, false),
                    },
                ),
                room("r2", "u1", false, RoomStatus::Vacant),
                room(
                    "r3",
                    "u1",
                    true,
                    RoomStatus::OutOfService {
                        reason: "clean".into(),
                    },
                ),
                room(
                    "r4",
                    "u2",
                    true,
                    RoomStatus::Occupied {
                        patient: patient(0.9, true),
                    },
                ),
                room("r5", "u2", true, RoomStatus::Vacant),
            ],
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![
                ServiceLine {
                    id: "line.medicine".into(),
                    name: "Medicine".into(),
                    description: None,
                },
                ServiceLine {
                    id: "line.critical".into(),
                    name: "Critical Care".into(),
                    description: None,
                },
            ],
        }
    }

    #[test]
    fn build_twice_is_identical() {
        // The EP-9 determinism guard: construction is a pure function of the
        // hospital, column for column.
        let h = hospital();
        let idx = HospitalIndex::build(&h);
        assert_eq!(RoomMatrix::build(&h, &idx), RoomMatrix::build(&h, &idx));
    }

    #[test]
    fn columns_reflect_rooms() {
        let h = hospital();
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        assert_eq!(m.len(), 5);
        assert_eq!(m.unit_pos, vec![0, 0, 0, 1, 1]);
        assert_eq!(m.building_pos, vec![0, 0, 0, 1, 1]);
        assert_eq!(m.service_line, vec![0, 0, 0, 1, 1]);
        assert_eq!(
            m.occupancy,
            vec![
                OccState::Occupied,
                OccState::Vacant,
                OccState::OutOfService,
                OccState::Occupied,
                OccState::Vacant
            ]
        );
        assert_eq!(m.boarding, vec![false, false, false, true, false]);
        assert_eq!(m.telemetry_need, vec![true, false, false, true, false]);
        assert_eq!(m.admitted_at_min[0], 120.0);
        assert_eq!(m.admitted_at_min[1], NO_ADMIT_MIN);
        assert_eq!(m.row_of(&"r4".into()), Some(3));
        assert_eq!(m.room_id(3), &RoomId::from("r4"));
    }

    #[test]
    fn refresh_fast_path_matches_fresh_build() {
        let mut h = hospital();
        let idx = HospitalIndex::build(&h);
        let mut m = RoomMatrix::build(&h, &idx);
        // Status-only mutation: discharge r1, admit into r5.
        h.rooms[0].status = RoomStatus::Vacant;
        h.rooms[4].status = RoomStatus::Occupied {
            patient: patient(0.5, false),
        };
        assert!(m.refresh(&h, &idx), "id-stable edit must take the fast path");
        assert_eq!(m, RoomMatrix::build(&h, &idx));
    }

    #[test]
    fn refresh_rebuilds_on_structural_change() {
        let mut h = hospital();
        let idx = HospitalIndex::build(&h);
        let mut m = RoomMatrix::build(&h, &idx);
        h.rooms.push(Room {
            id: "r6".into(),
            number: "r6".into(),
            unit: "u2".into(),
            kind: RoomKind::Inpatient,
            telemetry_capable: false,
            negative_pressure: false,
            status: RoomStatus::Vacant,
            provenance: Provenance::default(),
        });
        let idx = HospitalIndex::build(&h);
        assert!(!m.refresh(&h, &idx), "new room must force a rebuild");
        assert_eq!(m, RoomMatrix::build(&h, &idx));
    }

    #[test]
    fn unit_aggregates_match_room_scan() {
        let h = hospital();
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        let aggs = m.unit_aggregates(h.units.len());
        for (i, u) in h.units.iter().enumerate() {
            assert_eq!(aggs[i], aggregate_unit(&h, &idx, &u.id), "unit {}", u.id);
        }
    }

    #[test]
    fn stats_match_room_scan() {
        let h = hospital();
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        assert_eq!(m.hospital_stats(), hospital_stats(&h));
    }

    #[test]
    fn rollups_partition_the_hospital() {
        let h = hospital();
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        let by_type = m.rollup_by_unit_type();
        assert_eq!(by_type[UnitType::MedSurg.index()].1.capacity, 3);
        assert_eq!(by_type[UnitType::Icu.index()].1.census, 1);
        assert_eq!(
            by_type.iter().map(|(_, a)| a.capacity).sum::<usize>(),
            m.len()
        );
        let by_line = m.rollup_by_service_line(h.service_lines.len());
        assert_eq!(by_line[0].capacity, 3);
        assert_eq!(by_line[1].boarding, 1);
        let by_bldg = m.rollup_by_building(h.buildings.len());
        assert_eq!(by_bldg[0].capacity, 3);
        assert_eq!(by_bldg[1].capacity, 2);
    }

    #[test]
    fn vacancy_mask_honors_type_and_capability() {
        let h = hospital();
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        assert_eq!(
            m.vacancy_mask(None, false, false),
            vec![false, true, false, false, true]
        );
        // r2 is vacant but not telemetry-capable.
        assert_eq!(
            m.vacancy_mask(None, true, false),
            vec![false, false, false, false, true]
        );
        assert_eq!(
            m.vacancy_mask(Some(UnitType::Icu), false, false),
            vec![false, false, false, false, true]
        );
        assert_eq!(m.vacancy_mask(None, false, true), vec![false; 5]);
    }

    #[test]
    fn tier_counts_by_unit_match_room_scan() {
        let h = hospital();
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        let counts = m.tier_counts_by_unit(h.units.len());
        // u1: one occupied at 0.2 (Stable); u2: one occupied at 0.9 (Critical).
        assert_eq!(counts[0], [1, 0, 0, 0]);
        assert_eq!(counts[1], [0, 0, 0, 1]);
        // Tier totals equal each unit's census.
        let aggs = m.unit_aggregates(h.units.len());
        for (c, a) in counts.iter().zip(&aggs) {
            assert_eq!(c.iter().sum::<u32>() as usize, a.census);
        }
    }

    #[test]
    fn count_by_unit_buckets_rows() {
        let h = hospital();
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        let vacancies = m.count_by_unit(h.units.len(), |i| m.occupancy[i] == OccState::Vacant);
        assert_eq!(vacancies, vec![1, 1]);
        let tele_need = m.count_by_unit(h.units.len(), |i| m.telemetry_need[i]);
        assert_eq!(tele_need, vec![1, 1]);
    }

    #[test]
    fn masked_stats() {
        let vals = [0.2, 0.9, 0.4];
        let mask = [true, true, false];
        let (mean, sd) = masked_mean_std(&vals, &mask).unwrap();
        assert!((mean - 0.55).abs() < 1e-6);
        assert!((sd - 0.35).abs() < 1e-6);
        assert_eq!(masked_mean_std(&vals, &[false; 3]), None);
    }

    #[test]
    fn unknown_unit_gets_sentinels_without_panic() {
        let mut h = hospital();
        h.rooms.push(Room {
            id: "orphan".into(),
            number: "orphan".into(),
            unit: "nope".into(),
            kind: RoomKind::Other,
            telemetry_capable: false,
            negative_pressure: false,
            status: RoomStatus::Vacant,
            provenance: Provenance::default(),
        });
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        let row = m.row_of(&"orphan".into()).unwrap() as usize;
        assert_eq!(m.unit_pos[row], NO_UNIT);
        assert_eq!(m.building_pos[row], NO_BUILDING);
        // Excluded from unit buckets, still counted in hospital stats.
        let aggs = m.unit_aggregates(h.units.len());
        assert_eq!(aggs.iter().map(|a| a.capacity).sum::<usize>(), 5);
        assert_eq!(m.hospital_stats().capacity, 6);
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn parallel_matches_scalar() {
        let h = hospital();
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        assert_eq!(m.par_unit_aggregates(h.units.len()), m.unit_aggregates(h.units.len()));
        assert_eq!(m.par_hospital_stats(), m.hospital_stats());
        assert_eq!(m.par_rollup_by_unit_type(), m.rollup_by_unit_type());
        assert_eq!(
            m.par_rollup_by_service_line(h.service_lines.len()),
            m.rollup_by_service_line(h.service_lines.len())
        );
        let pred = |i: usize| m.occupancy[i] == OccState::Vacant;
        assert_eq!(
            m.par_count_by_unit(h.units.len(), pred),
            m.count_by_unit(h.units.len(), pred)
        );
    }
}
