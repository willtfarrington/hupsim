//! Rolling statistics: fixed-size ring buffers over the timeline's sampling
//! cadence, giving every tracked series a trailing norm (mean/σ) to judge
//! "is right now unusual" — the substrate for EP-12/EP-13 deviation
//! indicators and chart bands.
//!
//! Owner decisions (EP-9): the window is **96 samples × 15 sim-min =
//! trailing 24 h**, oldest sample replaced by newest; series are tracked per
//! unit AND per rollup group (service line, level of care, building, whole
//! hospital); during warm-up (window not yet full) `z()`/`band()` return
//! `None` rather than fake zeros.
//!
//! Feeding piggybacks on [`crate::timeline::Timeline::maybe_sample`] — the
//! engine records here exactly when the timeline samples, so there is no
//! second sampling clock to drift against.

use hupsim_core::aggregate::{aggregate_unit, UnitAggregate};
use hupsim_core::ids::{NodeId, ServiceLineId, UnitId};
use hupsim_core::index::HospitalIndex;
use hupsim_core::model::{Hospital, UnitType};
use std::collections::BTreeMap;

/// 96 × 15-min timeline samples = trailing 24 sim-hours.
pub const WINDOW_24H: usize = 96;

/// A fixed-capacity ring buffer of samples with summary statistics. Pushing
/// into a full window overwrites the oldest sample.
#[derive(Debug, Clone)]
pub struct RollingSeries {
    window: usize,
    buf: Vec<f32>,
    /// Overwrite position once the buffer is full (the oldest sample).
    next: usize,
}

impl RollingSeries {
    pub fn new(window: usize) -> Self {
        assert!(window > 0, "rolling window must be non-empty");
        Self {
            window,
            buf: Vec::with_capacity(window),
            next: 0,
        }
    }

    pub fn window(&self) -> usize {
        self.window
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Whether the window has filled — until then `z()`/`band()` are `None`.
    pub fn is_warm(&self) -> bool {
        self.buf.len() == self.window
    }

    pub fn push(&mut self, v: f32) {
        if self.buf.len() < self.window {
            self.buf.push(v);
        } else {
            // Filled in index order, so `next` walks the ring oldest-first.
            self.buf[self.next] = v;
            self.next = (self.next + 1) % self.window;
        }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.next = 0;
    }

    /// Samples oldest → newest, unwrapping the ring (chart-ready order).
    /// While filling, that is simply insertion order; once full, `next` is
    /// the oldest slot.
    pub fn iter_ordered(&self) -> impl Iterator<Item = f32> + '_ {
        let split = if self.buf.len() < self.window { 0 } else { self.next };
        self.buf[split..].iter().chain(self.buf[..split].iter()).copied()
    }

    /// Mean over whatever has been recorded so far (`None` when empty).
    pub fn mean(&self) -> Option<f32> {
        if self.buf.is_empty() {
            return None;
        }
        Some(self.buf.iter().sum::<f32>() / self.buf.len() as f32)
    }

    /// Population σ over whatever has been recorded so far.
    pub fn stddev(&self) -> Option<f32> {
        let mean = self.mean()?;
        let ssd: f32 = self.buf.iter().map(|v| (v - mean) * (v - mean)).sum();
        Some((ssd / self.buf.len() as f32).sqrt())
    }

    /// Standard score of `current` against the trailing window. `None` while
    /// warming up (window not full) and when the window is flat (σ ≈ 0 —
    /// there is no meaningful "how many sigmas" against a constant).
    pub fn z(&self, current: f32) -> Option<f32> {
        if !self.is_warm() {
            return None;
        }
        let (mean, sd) = (self.mean()?, self.stddev()?);
        if sd < 1e-6 {
            return None;
        }
        Some((current - mean) / sd)
    }

    /// Chart band `mean ± k·σ`. `None` while warming up, like `z()`.
    pub fn band(&self, k: f32) -> Option<(f32, f32)> {
        if !self.is_warm() {
            return None;
        }
        let (mean, sd) = (self.mean()?, self.stddev()?);
        Some((mean - k * sd, mean + k * sd))
    }
}

/// The four tracked metrics for one entity (unit or rollup group).
#[derive(Debug, Clone)]
pub struct MetricWindows {
    pub census: RollingSeries,
    pub occupancy_frac: RollingSeries,
    pub boarding: RollingSeries,
    /// Only advances when the entity has at least one occupied room — an
    /// empty unit contributes no instability sample, not a fake zero.
    pub mean_instability: RollingSeries,
}

impl MetricWindows {
    pub fn new(window: usize) -> Self {
        Self {
            census: RollingSeries::new(window),
            occupancy_frac: RollingSeries::new(window),
            boarding: RollingSeries::new(window),
            mean_instability: RollingSeries::new(window),
        }
    }

    fn observe(&mut self, agg: &UnitAggregate) {
        self.census.push(agg.census as f32);
        self.occupancy_frac.push(agg.occupancy_frac());
        self.boarding.push(agg.boarding as f32);
        if let Some(m) = agg.mean_instability {
            self.mean_instability.push(m);
        }
    }
}

/// All rolling series, keyed by unit and by each rollup grouping. `BTreeMap`
/// (not `HashMap`) so iteration order is deterministic for any future
/// serialization or chart legend.
#[derive(Debug, Clone)]
pub struct RollingStore {
    window: usize,
    pub units: BTreeMap<UnitId, MetricWindows>,
    /// Indexed by [`UnitType::index`] (level of care).
    pub unit_types: Vec<MetricWindows>,
    pub service_lines: BTreeMap<ServiceLineId, MetricWindows>,
    pub buildings: BTreeMap<NodeId, MetricWindows>,
    pub hospital: MetricWindows,
}

impl Default for RollingStore {
    fn default() -> Self {
        Self::new(WINDOW_24H)
    }
}

impl RollingStore {
    pub fn new(window: usize) -> Self {
        Self {
            window,
            units: BTreeMap::new(),
            unit_types: UnitType::ALL.iter().map(|_| MetricWindows::new(window)).collect(),
            service_lines: BTreeMap::new(),
            buildings: BTreeMap::new(),
            hospital: MetricWindows::new(window),
        }
    }

    pub fn window(&self) -> usize {
        self.window
    }

    pub fn unit_type(&self, t: UnitType) -> &MetricWindows {
        &self.unit_types[t.index()]
    }

    /// Record one observation for every tracked series. Call exactly when
    /// the timeline samples (same cadence, no second clock). Read-only over
    /// the hospital — never perturbs sim state or the RNG.
    pub fn record(&mut self, h: &Hospital, idx: &HospitalIndex) {
        let window = self.window;
        let mut by_type: Vec<UnitAggregate> = vec![UnitAggregate::default(); UnitType::ALL.len()];
        let mut by_line: BTreeMap<&ServiceLineId, UnitAggregate> = BTreeMap::new();
        let mut by_building: BTreeMap<&NodeId, UnitAggregate> = BTreeMap::new();
        let mut hospital = UnitAggregate::default();

        for u in &h.units {
            let agg = aggregate_unit(h, idx, &u.id);
            self.units
                .entry(u.id.clone())
                .or_insert_with(|| MetricWindows::new(window))
                .observe(&agg);
            by_type[u.unit_type.index()].absorb(&agg);
            if let Some(line) = &u.service_line {
                by_line.entry(line).or_default().absorb(&agg);
            }
            by_building.entry(&u.building).or_default().absorb(&agg);
            hospital.absorb(&agg);
        }

        for (windows, agg) in self.unit_types.iter_mut().zip(&by_type) {
            windows.observe(agg);
        }
        for (line, agg) in by_line {
            self.service_lines
                .entry(line.clone())
                .or_insert_with(|| MetricWindows::new(window))
                .observe(&agg);
        }
        for (building, agg) in by_building {
            self.buildings
                .entry(building.clone())
                .or_insert_with(|| MetricWindows::new(window))
                .observe(&agg);
        }
        self.hospital.observe(&hospital);
    }

    /// Drop all recorded history (sim reset), keeping the window size.
    pub fn clear(&mut self) {
        *self = Self::new(self.window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hupsim_core::model::*;
    use hupsim_core::patient::{Acuity, Patient};
    use hupsim_core::provenance::Provenance;

    #[test]
    fn ring_replaces_oldest() {
        let mut s = RollingSeries::new(4);
        for v in [1.0, 2.0, 3.0, 4.0, 5.0, 6.0] {
            s.push(v);
        }
        // Window holds 3,4,5,6 — the two oldest were overwritten.
        assert_eq!(s.len(), 4);
        assert!((s.mean().unwrap() - 4.5).abs() < 1e-6);
    }

    #[test]
    fn iter_ordered_unwraps_the_ring() {
        let mut s = RollingSeries::new(4);
        // Partial window: insertion order.
        for v in [1.0, 2.0, 3.0] {
            s.push(v);
        }
        assert_eq!(s.iter_ordered().collect::<Vec<_>>(), vec![1.0, 2.0, 3.0]);
        // Wrapped: oldest-first regardless of the overwrite position.
        for v in [4.0, 5.0, 6.0] {
            s.push(v);
        }
        assert_eq!(s.iter_ordered().collect::<Vec<_>>(), vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn warmup_returns_none_z_not_fake_zeros() {
        let mut s = RollingSeries::new(4);
        s.push(1.0);
        s.push(2.0);
        s.push(3.0);
        assert!(!s.is_warm());
        assert_eq!(s.z(10.0), None, "warm-up must not fabricate a z-score");
        assert_eq!(s.band(2.0), None);
        // mean/σ over the partial window are still available for display.
        assert!(s.mean().is_some());
        s.push(4.0);
        assert!(s.is_warm());
        let z = s.z(4.5).unwrap();
        // mean 2.5, σ = sqrt(1.25) ≈ 1.118 → z(4.5) ≈ 1.789
        assert!((z - 1.7889).abs() < 1e-3);
    }

    #[test]
    fn flat_window_has_no_z() {
        let mut s = RollingSeries::new(3);
        for _ in 0..3 {
            s.push(7.0);
        }
        assert!(s.is_warm());
        assert_eq!(s.z(7.0), None, "σ=0 cannot standardize");
        // The band still exists — it is just degenerate.
        let (lo, hi) = s.band(2.0).unwrap();
        assert!((lo - 7.0).abs() < 1e-6 && (hi - 7.0).abs() < 1e-6);
    }

    #[test]
    fn band_is_mean_plus_minus_k_sigma() {
        let mut s = RollingSeries::new(2);
        s.push(1.0);
        s.push(3.0);
        let (lo, hi) = s.band(1.0).unwrap();
        // mean 2, σ 1
        assert!((lo - 1.0).abs() < 1e-6);
        assert!((hi - 3.0).abs() < 1e-6);
    }

    fn tiny_hospital() -> (Hospital, HospitalIndex) {
        let h = Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![],
            units: vec![
                Unit {
                    id: "u1".into(),
                    name: "U1".into(),
                    service: "GenMed".into(),
                    service_line: Some("line.medicine".into()),
                    unit_type: UnitType::MedSurg,
                    building: "b1".into(),
                    level: FloorLevel::Numbered(1),
                    elevator_core: None,
                    target_rn_ratio: None,
                    provenance: Provenance::default(),
                    note: None,
                },
                Unit {
                    id: "u2".into(),
                    name: "U2".into(),
                    service: "CCU".into(),
                    service_line: Some("line.critical".into()),
                    unit_type: UnitType::Icu,
                    building: "b1".into(),
                    level: FloorLevel::Numbered(2),
                    elevator_core: None,
                    target_rn_ratio: None,
                    provenance: Provenance::default(),
                    note: None,
                },
            ],
            rooms: vec![
                Room {
                    id: "r1".into(),
                    number: "1".into(),
                    unit: "u1".into(),
                    kind: RoomKind::Inpatient,
                    telemetry_capable: false,
                    negative_pressure: false,
                    status: RoomStatus::Occupied {
                        patient: Patient {
                            id: "p1".into(),
                            alias: "P1".into(),
                            age: None,
                            service: None,
                            admitted_at_min: None,
                            acuity: Acuity {
                                instability: 0.4,
                                trend_per_hr: 0.0,
                            },
                            boarding: false,
                            isolation: None,
                            telemetry: false,
                            note: String::new(),
                        },
                    },
                    provenance: Provenance::default(),
                },
                Room {
                    id: "r2".into(),
                    number: "2".into(),
                    unit: "u2".into(),
                    kind: RoomKind::IcuBay,
                    telemetry_capable: true,
                    negative_pressure: false,
                    status: RoomStatus::Vacant,
                    provenance: Provenance::default(),
                },
            ],
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![],
        };
        let idx = HospitalIndex::build(&h);
        (h, idx)
    }

    #[test]
    fn record_tracks_units_and_groups() {
        let (h, idx) = tiny_hospital();
        let mut store = RollingStore::new(4);
        for _ in 0..4 {
            store.record(&h, &idx);
        }
        // Per unit.
        let u1 = store.units.get(&"u1".into()).unwrap();
        assert!(u1.census.is_warm());
        assert!((u1.census.mean().unwrap() - 1.0).abs() < 1e-6);
        assert!((u1.mean_instability.mean().unwrap() - 0.4).abs() < 1e-6);
        // The empty ICU advances census but never instability.
        let u2 = store.units.get(&"u2".into()).unwrap();
        assert!((u2.census.mean().unwrap() - 0.0).abs() < 1e-6);
        assert!(u2.mean_instability.is_empty());
        // Per level of care, service line, building, hospital.
        assert!((store.unit_type(UnitType::MedSurg).census.mean().unwrap() - 1.0).abs() < 1e-6);
        assert!(store
            .service_lines
            .get(&"line.medicine".into())
            .is_some_and(|w| w.census.is_warm()));
        assert!(store.buildings.get(&"b1".into()).is_some_and(|w| w.census.is_warm()));
        assert!((store.hospital.census.mean().unwrap() - 1.0).abs() < 1e-6);
        assert!((store.hospital.occupancy_frac.mean().unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn clear_drops_history_keeps_window() {
        let (h, idx) = tiny_hospital();
        let mut store = RollingStore::new(4);
        store.record(&h, &idx);
        store.clear();
        assert_eq!(store.window(), 4);
        assert!(store.units.is_empty());
        assert!(store.hospital.census.is_empty());
    }
}
