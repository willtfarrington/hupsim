//! EP-15 surge / what-if scenario tools: interventions, the headless A/B
//! runner, and the per-run metric series the compare view charts.
//!
//! Built on the engine's strongest tested guarantee — seeded deterministic
//! replay. Both variants clone the same world, seed identical engines, and
//! tick headlessly for the horizon; every difference between the runs is
//! attributable to the staged interventions. One run per arm is sufficient
//! for the A-vs-B delta, but note honestly: it is **one realization**, not an
//! expectation — EP-17's forecast fan is the place for uncertainty.
//!
//! Session flow (owner brief): staging an experiment never mutates the live
//! scenario. Variants run on clones; the live engine is only *read* (its
//! rolling norms seed both arms so z-scores judge against the pre-experiment
//! norm instead of warming from nothing). Results are held in memory, one
//! experiment at a time.
//!
//! Known, documented approximation: a fresh engine cannot inherit the live
//! run's scheduled discharges, so [`ExperimentSpec::backfill_discharges`]
//! schedules an exit for every patient in-house at experiment start. Since
//! EP-21 the backfill shares the standing-census residual-LOS mechanism
//! ([`sampling::residual_discharge_min`]): patients with a known admit time
//! get a total-LOS resample conditioned above their elapsed stay, so the
//! remaining fresh-LOS bias is confined to patients with *unknown* stamps
//! (hand-admitted pre-sim). Either way the draw order is room-index order
//! and both arms draw identically, so any bias cancels out of the A-vs-B
//! delta.

use crate::acuity::AcuityDrift;
use crate::diurnal::DiurnalFlow;
use crate::edflow::EdPressure;
use crate::engine::SimEngine;
use crate::events::SimEventKind;
use crate::params;
use crate::process::{SimCtx, SimProcess};
use crate::roster::RnRoster;
use crate::rolling::RollingStore;
use crate::sampling;
use crate::timeline::TimelineSample;
use hupsim_core::aggregate::{aggregate_unit, UnitAggregate};
use hupsim_core::ids::{NodeId, ServiceLineId, UnitId};
use hupsim_core::index::HospitalIndex;
use hupsim_core::model::{Hospital, UnitType};
use hupsim_core::ops;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Interventions
// ---------------------------------------------------------------------------

/// A small, reversible world/process modification staged before a run.
/// Applied to the variant's *clone* only — the live world never sees it.
#[derive(Debug, Clone, PartialEq)]
pub enum Intervention {
    /// Close a unit: vacant beds go out of service with a labeled reason;
    /// occupants are evacuated first-fit into vacant beds of the same level
    /// of care elsewhere (deterministic, no RNG). Anyone who cannot be
    /// placed stays put flagged `boarding` — waiting for an appropriate bed,
    /// never vanished — and a [`ClosureSweep`] process closes each remaining
    /// bed as it empties, so the closure holds for the whole run.
    CloseUnit { unit: UnitId, reason: String },
    /// Inject an arrival wave: `count` extra admissions needing `need`-level
    /// beds, paced evenly across a window (hours relative to experiment
    /// start). Arrivals that find no bed are retried every tick — demand
    /// backlogs, it does not silently vanish; the leftover is reported.
    ArrivalWave {
        start_hr: f64,
        duration_hr: f64,
        count: u32,
        need: UnitType,
    },
    /// Override a unit's patients-per-RN target for the run. Affects the
    /// EP-11 roster sizing and workload composite only — never bed physics.
    FlexRatio { unit: UnitId, ratio: f32 },
}

impl Intervention {
    /// One-line description printed with the compare charts.
    pub fn describe(&self, h: &Hospital) -> String {
        let unit_name = |id: &UnitId| {
            h.units
                .iter()
                .find(|u| &u.id == id)
                .map(|u| u.name.clone())
                .unwrap_or_else(|| id.to_string())
        };
        match self {
            Intervention::CloseUnit { unit, reason } => {
                format!("close {} ({reason})", unit_name(unit))
            }
            Intervention::ArrivalWave {
                start_hr,
                duration_hr,
                count,
                need,
            } => format!(
                "arrival wave: +{count} {} admissions over {duration_hr:.0} h (from t+{start_hr:.0} h)",
                need.label()
            ),
            Intervention::FlexRatio { unit, ratio } => {
                format!("flex {} to {ratio:.1} pts/RN", unit_name(unit))
            }
        }
    }
}

/// What closing a unit did to the world, for logs and the conservation test.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CloseOutcome {
    /// Beds taken out of service at staging time.
    pub closed: usize,
    /// Occupants moved to a same-level-of-care bed elsewhere.
    pub evacuated: usize,
    /// Occupants with nowhere to go: still in place, flagged boarding.
    pub stranded: usize,
}

/// Close a unit on a world: vacant → OOS, occupants evacuated first-fit to a
/// vacant bed of the same [`UnitType`] in another unit (lowest room index —
/// deterministic, no RNG), stranded occupants flagged boarding in place.
/// Conserves patients by construction: the only moves are `ops::transfer`.
pub fn apply_close_unit(
    h: &mut Hospital,
    idx: &HospitalIndex,
    unit: &UnitId,
    reason: &str,
) -> CloseOutcome {
    let mut out = CloseOutcome::default();
    let Some(&up) = idx.unit_pos.get(unit) else {
        return out;
    };
    let need = h.units[up].unit_type;
    let rooms: Vec<usize> = idx.room_indices(unit).to_vec();
    let oos_reason = format!("closed — {reason}");
    for ri in rooms {
        let id = h.rooms[ri].id.clone();
        if h.rooms[ri].status.is_vacant() {
            if ops::set_out_of_service(h, idx, &id, oos_reason.clone()).is_ok() {
                out.closed += 1;
            }
            continue;
        }
        if h.rooms[ri].status.patient().is_none() {
            continue; // already out of service — keep the original reason
        }
        // First-fit evacuation: lowest-index vacant bed of the same level of
        // care in a *different* unit.
        let dest = h.rooms.iter().position(|r| {
            r.unit != *unit
                && r.status.is_vacant()
                && idx
                    .unit_pos
                    .get(&r.unit)
                    .is_some_and(|&p| h.units[p].unit_type == need)
        });
        match dest {
            Some(di) => {
                let to = h.rooms[di].id.clone();
                if ops::transfer(h, idx, &id, &to).is_ok() {
                    out.evacuated += 1;
                    if ops::set_out_of_service(h, idx, &id, oos_reason.clone()).is_ok() {
                        out.closed += 1;
                    }
                } else if let Some(p) = h.rooms[ri].status.patient_mut() {
                    p.boarding = true;
                    out.stranded += 1;
                }
            }
            None => {
                if let Some(p) = h.rooms[ri].status.patient_mut() {
                    p.boarding = true;
                    out.stranded += 1;
                }
            }
        }
    }
    out
}

/// Experiment-only process holding a closure: any bed of a closed unit that
/// empties during the run goes out of service instead of back into play.
/// RNG-free and matrix-free, so it can run first without perturbing the
/// deterministic draw pattern of the flow processes behind it.
pub struct ClosureSweep {
    /// (unit, OOS reason) pairs, in staging order.
    pub units: Vec<(UnitId, String)>,
}

impl SimProcess for ClosureSweep {
    fn name(&self) -> &str {
        "Unit closure (EP-15 experiment)"
    }

    fn description(&self) -> &str {
        "Holds staged unit closures: beds that empty in a closed unit go out of service."
    }

    fn step(&mut self, ctx: &mut SimCtx) {
        for (unit, reason) in &self.units {
            for ri in ctx.index.room_indices(unit).to_vec() {
                if ctx.hospital.rooms[ri].status.is_vacant() {
                    let id = ctx.hospital.rooms[ri].id.clone();
                    if ops::set_out_of_service(ctx.hospital, ctx.index, &id, reason.clone())
                        .is_ok()
                    {
                        ctx.log(format!("closed bed as it emptied: {id}"));
                    }
                }
            }
        }
    }
}

/// Shared counters the runner reads back after the boxed process is consumed
/// by the engine. Single-threaded ticks — the atomics are only a cheap way
/// to share, not synchronization.
#[derive(Debug, Default)]
pub struct WaveCounters {
    pub injected: AtomicU32,
    /// Demand still unplaced at the end of the run (no silent caps).
    pub backlog: AtomicU32,
}

/// Experiment-only process injecting the staged arrival wave: extra
/// admissions paced evenly across the window, placed first-fit (lowest room
/// index — bed choice consumes no RNG) into vacant beds of the needed level
/// of care. Unplaced demand carries forward and is retried every tick, even
/// past the window's end.
pub struct ArrivalWave {
    pub start_min: f64,
    pub end_min: f64,
    pub total: u32,
    pub need: UnitType,
    injected: u32,
    counter: u64,
    pub stats: Arc<WaveCounters>,
}

impl ArrivalWave {
    pub fn new(start_min: f64, end_min: f64, total: u32, need: UnitType) -> Self {
        Self {
            start_min,
            end_min: end_min.max(start_min),
            total,
            need,
            injected: 0,
            counter: 0,
            stats: Arc::new(WaveCounters::default()),
        }
    }
}

impl SimProcess for ArrivalWave {
    fn name(&self) -> &str {
        "Arrival wave (EP-15 experiment)"
    }

    fn description(&self) -> &str {
        "Staged surge: N extra admissions of one level of care, paced across a window."
    }

    fn reset(&mut self) {
        self.injected = 0;
        self.counter = 0;
        self.stats.injected.store(0, Ordering::Relaxed);
        self.stats.backlog.store(0, Ordering::Relaxed);
    }

    fn step(&mut self, ctx: &mut SimCtx) {
        if ctx.now_min < self.start_min || self.injected >= self.total {
            return;
        }
        // Even pacing: cumulative target tracks the elapsed window fraction.
        let frac = if self.end_min > self.start_min {
            ((ctx.now_min - self.start_min) / (self.end_min - self.start_min)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let target = (self.total as f64 * frac).round() as u32;
        while self.injected < target {
            if !self.admit_one(ctx) {
                break; // no bed this tick — backlog retries next tick
            }
        }
        self.stats.injected.store(self.injected, Ordering::Relaxed);
        self.stats
            .backlog
            .store(self.total - self.injected, Ordering::Relaxed);
    }
}

impl ArrivalWave {
    /// First-fit placement of one wave admission. RNG is consumed only on a
    /// successful admission (fixed pattern per admit, like the EP-10 flows).
    fn admit_one(&mut self, ctx: &mut SimCtx) -> bool {
        use hupsim_core::matrix::NO_UNIT;
        let m = ctx.matrix;
        let row = (0..m.len()).find(|&i| {
            m.unit_type[i] == self.need
                && m.unit_pos[i] != NO_UNIT
                && ctx.hospital.rooms[i].status.is_vacant()
        });
        let Some(row) = row else {
            return false;
        };
        let (service, line) = {
            let u = &ctx.hospital.units[m.unit_pos[row] as usize];
            (u.service.clone(), u.service_line.clone())
        };
        let room_id = m.room_id(row as u32).clone();

        self.counter += 1;
        let instability = sampling::draw_initial_instability(ctx.rng, self.need);
        let patient = sampling::synth_patient(
            ctx.rng,
            "wave",
            self.counter,
            ctx.now_min,
            instability,
            Some(service),
            "EP-15 arrival wave",
        );
        let pid = patient.id.clone();
        if ops::admit(ctx.hospital, ctx.index, &room_id, patient).is_err() {
            return false;
        }
        let los_hr = sampling::lognormal_hours(ctx.rng, &params::los_for(line.as_ref(), self.need));
        let dc_at = sampling::shape_discharge_min(ctx.rng, ctx.now_min + los_hr * 60.0);
        ctx.queue.schedule(
            dc_at,
            SimEventKind::Discharge {
                room: room_id.clone(),
                expect_patient: Some(pid),
            },
        );
        self.injected += 1;
        ctx.log(format!("wave admission → {room_id}"));
        true
    }
}

// ---------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------

/// Which of the EP-10 realism processes run in the experiment — mirrored
/// from the live toolbar toggles so the variants model the same world the
/// user is watching.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcessToggles {
    pub diurnal: bool,
    pub ed: bool,
    pub drift: bool,
}

impl Default for ProcessToggles {
    fn default() -> Self {
        Self {
            diurnal: true,
            ed: true,
            drift: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentSpec {
    /// Both arms seed identical engines with this — the determinism anchor.
    pub seed: u64,
    /// Sim minute the experiment starts at (keeps the diurnal phase honest).
    pub start_min: f64,
    pub horizon_min: f64,
    /// Tick length = sampling cadence, so every tick records one sample.
    pub sample_every_min: f64,
    pub toggles: ProcessToggles,
    /// Schedule an exit for everyone in-house at start — residual-LOS where
    /// the admit time is known, fresh-LOS otherwise (see the module docs for
    /// the honesty note). Identical in both arms.
    pub backfill_discharges: bool,
    pub interventions: Vec<Intervention>,
}

impl ExperimentSpec {
    pub fn new(seed: u64, start_min: f64, horizon_hr: f64) -> Self {
        Self {
            seed,
            start_min,
            horizon_min: horizon_hr * 60.0,
            sample_every_min: 15.0,
            toggles: ProcessToggles::default(),
            backfill_discharges: true,
            interventions: Vec::new(),
        }
    }

    /// All staged interventions, one line each, for the chart caption.
    pub fn describe(&self, h: &Hospital) -> String {
        if self.interventions.is_empty() {
            return "no intervention (A/A)".into();
        }
        self.interventions
            .iter()
            .map(|iv| iv.describe(h))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

// ---------------------------------------------------------------------------
// Collected series
// ---------------------------------------------------------------------------

/// One entity's per-sample metrics across a run. `occ_z` and `workload_max`
/// are `Option` per the EP-9 warm-up honesty rule: no norm window, no z; no
/// staffed RNs, no workload — never a fake zero.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MetricSeries {
    pub census: Vec<f32>,
    pub boarding: Vec<f32>,
    /// Census / staffed beds (the EP-12/13 occupancy definition).
    pub occupancy: Vec<f32>,
    /// Occupancy standard score vs the run's rolling norm (seeded from the
    /// live norms when provided).
    pub occ_z: Vec<Option<f32>>,
    /// Peak per-RN workload composite over the entity's staffed units.
    pub workload_max: Vec<Option<f32>>,
}

impl MetricSeries {
    fn push(
        &mut self,
        agg: &UnitAggregate,
        windows: Option<&crate::rolling::MetricWindows>,
        workload_max: Option<f32>,
    ) {
        let occ = agg.occupancy_frac();
        self.census.push(agg.census as f32);
        self.boarding.push(agg.boarding as f32);
        self.occupancy.push(occ);
        self.occ_z
            .push(windows.and_then(|w| w.occupancy_frac.z(occ)));
        self.workload_max.push(workload_max);
    }
}

/// Which entity a compare chart shows — the EP-9/EP-13 groupings plus
/// single units.
#[derive(Debug, Clone, PartialEq)]
pub enum SeriesKey {
    Hospital,
    UnitType(UnitType),
    ServiceLine(ServiceLineId),
    Building(NodeId),
    Unit(UnitId),
}

/// All series one variant collected, keyed like [`RollingStore`] (the EP-9
/// groups: units, levels of care, service lines, buildings, whole hospital).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunSeries {
    pub t_min: Vec<f64>,
    pub hospital: MetricSeries,
    /// Indexed by [`UnitType::index`].
    pub unit_types: Vec<MetricSeries>,
    pub service_lines: BTreeMap<ServiceLineId, MetricSeries>,
    pub buildings: BTreeMap<NodeId, MetricSeries>,
    pub units: BTreeMap<UnitId, MetricSeries>,
}

impl RunSeries {
    fn new() -> Self {
        Self {
            unit_types: UnitType::ALL.iter().map(|_| MetricSeries::default()).collect(),
            ..Default::default()
        }
    }

    pub fn series(&self, key: &SeriesKey) -> Option<&MetricSeries> {
        match key {
            SeriesKey::Hospital => Some(&self.hospital),
            SeriesKey::UnitType(t) => self.unit_types.get(t.index()),
            SeriesKey::ServiceLine(id) => self.service_lines.get(id),
            SeriesKey::Building(id) => self.buildings.get(id),
            SeriesKey::Unit(id) => self.units.get(id),
        }
    }

    /// Record one sample for every tracked entity. Mirrors
    /// [`RollingStore::record`]'s aggregation so groups mean the same thing
    /// everywhere; z-scores read the engine's rolling store as of this
    /// sample; workload peaks read the EP-11 roster.
    fn record(
        &mut self,
        now_min: f64,
        h: &Hospital,
        idx: &HospitalIndex,
        rolling: &RollingStore,
        roster: &RnRoster,
    ) {
        self.t_min.push(now_min);

        let mut by_type: Vec<UnitAggregate> = vec![UnitAggregate::default(); UnitType::ALL.len()];
        let mut by_line: BTreeMap<&ServiceLineId, UnitAggregate> = BTreeMap::new();
        let mut by_building: BTreeMap<&NodeId, UnitAggregate> = BTreeMap::new();
        let mut hospital = UnitAggregate::default();
        let mut wl_type: Vec<Option<f32>> = vec![None; UnitType::ALL.len()];
        let mut wl_line: BTreeMap<&ServiceLineId, f32> = BTreeMap::new();
        let mut wl_building: BTreeMap<&NodeId, f32> = BTreeMap::new();
        let mut wl_hospital: Option<f32> = None;

        let fold = |slot: &mut Option<f32>, v: f32| {
            *slot = Some(slot.map_or(v, |m| m.max(v)));
        };

        for u in &h.units {
            let agg = aggregate_unit(h, idx, &u.id);
            let wl = roster
                .unit(&u.id)
                .and_then(|ur| ur.workloads.iter().map(|w| w.scalar()).fold(None, |m: Option<f32>, v| {
                    Some(m.map_or(v, |x| x.max(v)))
                }));
            self.units
                .entry(u.id.clone())
                .or_default()
                .push(&agg, rolling.units.get(&u.id), wl);

            by_type[u.unit_type.index()].absorb(&agg);
            if let Some(v) = wl {
                fold(&mut wl_type[u.unit_type.index()], v);
                fold(&mut wl_hospital, v);
            }
            if let Some(line) = &u.service_line {
                by_line.entry(line).or_default().absorb(&agg);
                if let Some(v) = wl {
                    wl_line
                        .entry(line)
                        .and_modify(|m| *m = m.max(v))
                        .or_insert(v);
                }
            }
            by_building.entry(&u.building).or_default().absorb(&agg);
            if let Some(v) = wl {
                wl_building
                    .entry(&u.building)
                    .and_modify(|m| *m = m.max(v))
                    .or_insert(v);
            }
            hospital.absorb(&agg);
        }

        for (i, agg) in by_type.iter().enumerate() {
            self.unit_types[i].push(agg, Some(&rolling.unit_types[i]), wl_type[i]);
        }
        for (line, agg) in by_line {
            self.service_lines.entry(line.clone()).or_default().push(
                &agg,
                rolling.service_lines.get(line),
                wl_line.get(line).copied(),
            );
        }
        for (b, agg) in by_building {
            self.buildings.entry(b.clone()).or_default().push(
                &agg,
                rolling.buildings.get(b),
                wl_building.get(b).copied(),
            );
        }
        self.hospital
            .push(&hospital, Some(&rolling.hospital), wl_hospital);
    }
}

/// Headline numbers for one series over the whole run — the compare view's
/// summary-delta ingredients.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SeriesSummary {
    pub peak_census: f32,
    pub peak_boarding: f32,
    /// Sim hours spent above 95% occupancy.
    pub hours_over_95: f32,
    pub peak_workload: Option<f32>,
}

pub fn summarize(s: &MetricSeries, sample_every_min: f64) -> SeriesSummary {
    let peak = |v: &[f32]| v.iter().copied().fold(0.0f32, f32::max);
    SeriesSummary {
        peak_census: peak(&s.census),
        peak_boarding: peak(&s.boarding),
        hours_over_95: s.occupancy.iter().filter(|&&o| o > 0.95).count() as f32
            * (sample_every_min / 60.0) as f32,
        peak_workload: s
            .workload_max
            .iter()
            .flatten()
            .copied()
            .fold(None, |m: Option<f32>, v| Some(m.map_or(v, |x| x.max(v)))),
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// One arm's complete output.
#[derive(Debug)]
pub struct VariantRun {
    pub series: RunSeries,
    /// The arm's own bounded timeline (brief item 2).
    pub timeline: Vec<TimelineSample>,
    /// End-of-run world — tests assert conservation and physics-untouched
    /// invariants against it.
    pub hospital: Hospital,
    /// What closures did at staging time (empty for the baseline arm).
    pub close_outcomes: Vec<CloseOutcome>,
    /// Per staged wave: admissions placed / still unplaced at run end.
    pub wave_injected: u32,
    pub wave_backlog: u32,
}

#[derive(Debug)]
pub struct ExperimentResult {
    pub spec: ExperimentSpec,
    pub baseline: VariantRun,
    pub variant: VariantRun,
}

/// Run baseline and intervention arms from a clone of `h0` and return both
/// runs. `norms` (the live engine's rolling store) pre-warms both arms'
/// z-score windows; pass `None` to start cold. Read-only over the inputs.
pub fn run_experiment(
    h0: &Hospital,
    norms: Option<&RollingStore>,
    spec: &ExperimentSpec,
) -> ExperimentResult {
    ExperimentResult {
        spec: spec.clone(),
        baseline: run_variant(h0, norms, spec, false),
        variant: run_variant(h0, norms, spec, true),
    }
}

fn run_variant(
    h0: &Hospital,
    norms: Option<&RollingStore>,
    spec: &ExperimentSpec,
    with_interventions: bool,
) -> VariantRun {
    let mut h = h0.clone();
    let idx = HospitalIndex::build(&h);
    let mut eng = SimEngine::new(spec.seed);
    eng.clock.now_min = spec.start_min;
    eng.timeline.sample_every_min = spec.sample_every_min;
    if let Some(n) = norms {
        eng.rolling = n.clone();
    }

    // Stage interventions on the clone (variant arm only), before anything
    // is scheduled or registered.
    let mut close_outcomes = Vec::new();
    let mut closed: Vec<(UnitId, String)> = Vec::new();
    let mut waves: Vec<ArrivalWave> = Vec::new();
    if with_interventions {
        for iv in &spec.interventions {
            match iv {
                Intervention::CloseUnit { unit, reason } => {
                    close_outcomes.push(apply_close_unit(&mut h, &idx, unit, reason));
                    closed.push((unit.clone(), format!("closed — {reason}")));
                }
                Intervention::ArrivalWave {
                    start_hr,
                    duration_hr,
                    count,
                    need,
                } => {
                    let start = spec.start_min + start_hr * 60.0;
                    waves.push(ArrivalWave::new(
                        start,
                        start + duration_hr * 60.0,
                        *count,
                        *need,
                    ));
                }
                Intervention::FlexRatio { unit, ratio } => {
                    if let Some(&up) = idx.unit_pos.get(unit) {
                        h.units[up].target_rn_ratio = Some(*ratio);
                    }
                }
            }
        }
    }

    // Registration order: the closure sweep first (RNG-free — closes beds
    // vacated by this tick's events before the flow processes can refill
    // them), then the standard EP-10 roster in the live app's order, then
    // the waves (their extra RNG draws land after the shared prefix).
    if !closed.is_empty() {
        eng.register(Box::new(ClosureSweep { units: closed }), true);
    }
    eng.register(Box::new(AcuityDrift::default()), spec.toggles.drift);
    eng.register(Box::new(DiurnalFlow::default()), spec.toggles.diurnal);
    eng.register(Box::new(EdPressure::default()), spec.toggles.ed);
    let wave_stats: Vec<Arc<WaveCounters>> = waves.iter().map(|w| w.stats.clone()).collect();
    for w in waves {
        eng.register(Box::new(w), true);
    }

    if spec.backfill_discharges {
        backfill_discharges(&h, &idx, &mut eng, spec.start_min);
    }

    // Prime roster + record the staged starting state as sample 0.
    let SimEngine {
        roster,
        log,
        timeline,
        rolling,
        ..
    } = &mut eng;
    roster.maintain(&h, &idx, spec.start_min, log);
    if timeline.maybe_sample(spec.start_min, &h, &idx) {
        rolling.record(&h, &idx);
    }
    let mut series = RunSeries::new();
    series.record(spec.start_min, &h, &idx, &eng.rolling, &eng.roster);

    // The headless loop: tick length = sampling cadence, so every tick
    // samples exactly once and the arms' series stay index-aligned.
    let ticks = (spec.horizon_min / spec.sample_every_min).round() as usize;
    for _ in 0..ticks {
        eng.tick(&mut h, &idx, spec.sample_every_min);
        series.record(eng.clock.now_min, &h, &idx, &eng.rolling, &eng.roster);
    }

    VariantRun {
        series,
        timeline: eng.timeline.samples().cloned().collect(),
        hospital: h,
        close_outcomes,
        wave_injected: wave_stats
            .iter()
            .map(|s| s.injected.load(Ordering::Relaxed))
            .sum(),
        wave_backlog: wave_stats
            .iter()
            .map(|s| s.backlog.load(Ordering::Relaxed))
            .sum(),
    }
}

/// Schedule a guarded discharge for every patient in-house at experiment
/// start: ED occupants on the ED episode clock, everyone else on their
/// service line's LOS pushed onto the mid-day discharge curve. Patients with
/// a known admit time exit at residual LOS (the EP-21 standing-census
/// mechanism — total-LOS resample conditioned above the elapsed stay);
/// unknown stamps keep the fresh-LOS draw. Room-index order keeps the RNG
/// draw pattern identical across arms (until an intervention changes who is
/// where — which is the point).
fn backfill_discharges(h: &Hospital, idx: &HospitalIndex, eng: &mut SimEngine, t0: f64) {
    for room in &h.rooms {
        let Some(p) = room.status.patient() else {
            continue;
        };
        let Some(&up) = idx.unit_pos.get(&room.unit) else {
            continue;
        };
        let u = &h.units[up];
        // A stamp in the sim future can't happen from the flows; treat it as
        // unknown so elapsed never goes negative.
        let known_admit = p.admitted_at_min.filter(|&m| m <= t0);
        let at = if u.unit_type == UnitType::Ed {
            match known_admit {
                Some(admit) => {
                    let elapsed_hr = (t0 - admit) / 60.0;
                    match sampling::residual_total_los_hours(
                        &mut eng.rng,
                        &params::ED_TREAT_RELEASE_LOS,
                        elapsed_hr,
                    ) {
                        // ED episodes stay on the episode clock — no
                        // discharge-curve shaping, exactly like the live flow.
                        Some(total_hr) => admit + total_hr * 60.0,
                        // Deep in the tail: walk out on a fresh episode-scale
                        // draw from now.
                        None => {
                            t0 + sampling::lognormal_hours(
                                &mut eng.rng,
                                &params::ED_TREAT_RELEASE_LOS,
                            ) * 60.0
                        }
                    }
                }
                None => {
                    t0 + sampling::lognormal_hours(&mut eng.rng, &params::ED_TREAT_RELEASE_LOS)
                        * 60.0
                }
            }
        } else {
            let los = params::los_for(u.service_line.as_ref(), u.unit_type);
            match known_admit {
                Some(admit) => sampling::residual_discharge_min(&mut eng.rng, &los, admit, t0),
                None => {
                    let hrs = sampling::lognormal_hours(&mut eng.rng, &los);
                    sampling::shape_discharge_min(&mut eng.rng, t0 + hrs * 60.0)
                }
            }
        };
        eng.queue.schedule(
            at,
            SimEventKind::Discharge {
                room: room.id.clone(),
                expect_patient: Some(p.id.clone()),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testworld::world;
    use hupsim_core::model::RoomStatus;
    use hupsim_core::patient::{Acuity, Patient};

    fn pt(name: &str, instability: f32) -> Patient {
        Patient {
            id: name.into(),
            alias: name.into(),
            age: Some(60),
            service: None,
            admitted_at_min: Some(0.0),
            acuity: Acuity {
                instability,
                trend_per_hr: 0.0,
            },
            boarding: false,
            isolation: None,
            telemetry: false,
            note: String::new(),
        }
    }

    fn patient_count(h: &Hospital) -> usize {
        h.rooms.iter().filter(|r| r.status.patient().is_some()).count()
    }

    /// The acceptance cornerstone: same seed, no intervention — the arms
    /// must be *identical* in every collected artifact.
    #[test]
    fn aa_null_diff_is_exact() {
        let (mut h, idx) = world(&[
            ("ed", UnitType::Ed, 8),
            ("ms", UnitType::MedSurg, 20),
            ("icu", UnitType::Icu, 4),
            ("sd", UnitType::Stepdown, 4),
        ]);
        // Some starting census so the discharge backfill has work to do.
        for i in 0..6 {
            let id = format!("ms.r{i}").into();
            ops::admit(&mut h, &idx, &id, pt(&format!("p{i}"), 0.3)).unwrap();
        }
        let spec = ExperimentSpec::new(42, 300.0, 24.0);
        let res = run_experiment(&h, None, &spec);
        assert_eq!(res.baseline.series, res.variant.series, "A/A series must be identical");
        assert_eq!(
            res.baseline.timeline, res.variant.timeline,
            "A/A timelines must be identical"
        );
        assert_eq!(
            res.baseline.hospital, res.variant.hospital,
            "A/A end-of-run worlds must be identical"
        );
        // Sanity on the collected shape: one sample per tick plus t0.
        assert_eq!(res.baseline.series.t_min.len(), 96 + 1);
        assert_eq!(res.baseline.timeline.len(), 96 + 1);
        // And the input world was never touched (session-flow guarantee).
        assert_eq!(patient_count(&h), 6);
    }

    /// EP-21 behavioral pins for the residual-LOS backfill — the A/A test is
    /// blind to the backfill's *distribution* (any draw identical in both
    /// arms passes), so pin the two guards directly:
    /// 1. Deep-tail elapsed (≈27× the median) exhausts the conditioned
    ///    resample and falls back to a prompt shaped discharge — everyone
    ///    exits within the first two days. A fresh-LOS regression would keep
    ///    most of them (median 96 h, unconditioned) well past that.
    /// 2. A stamp in the sim future (clock rewound) is treated as unknown —
    ///    fresh LOS from t0 — never anchored at the future stamp.
    #[test]
    fn backfill_uses_residual_los_and_guards_future_stamps() {
        let quiet = |seed: u64, horizon_hr: f64| {
            let mut spec = ExperimentSpec::new(seed, 0.0, horizon_hr);
            spec.toggles = ProcessToggles {
                diurnal: false,
                ed: false,
                drift: false,
            };
            spec
        };

        // 1 — deep tail: four patients ~27× the med/surg median deep.
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 8)]);
        let deep = -(96.0 * (6.0f64 * 0.55).exp() * 60.0);
        for i in 0..4 {
            let id = format!("ms.r{i}").into();
            let mut p = pt(&format!("deep{i}"), 0.3);
            p.admitted_at_min = Some(deep);
            ops::admit(&mut h, &idx, &id, p).unwrap();
        }
        let res = run_experiment(&h, None, &quiet(17, 48.0));
        assert_eq!(
            patient_count(&res.baseline.hospital),
            0,
            "deep-tail standing patients must exit promptly on the residual fallback"
        );

        // 2 — future stamp: one patient stamped 30 days ahead of t0.
        let (mut h2, idx2) = world(&[("ms", UnitType::MedSurg, 2)]);
        let mut p = pt("rewound", 0.3);
        p.admitted_at_min = Some(30.0 * 1440.0);
        ops::admit(&mut h2, &idx2, &"ms.r0".into(), p).unwrap();
        let res2 = run_experiment(&h2, None, &quiet(17, 480.0));
        assert_eq!(
            patient_count(&res2.baseline.hospital),
            0,
            "a future-stamped patient exits on a fresh LOS from t0, not from the stamp"
        );
    }

    #[test]
    fn close_unit_conserves_patients_and_shifts_census() {
        let (mut h, idx) = world(&[
            ("icu_a", UnitType::Icu, 4),
            ("icu_b", UnitType::Icu, 3),
            ("ms", UnitType::MedSurg, 4),
        ]);
        // icu_a: 3 occupied, 1 vacant. icu_b: 2 vacant, 1 occupied.
        for i in 0..3 {
            let id = format!("icu_a.r{i}").into();
            ops::admit(&mut h, &idx, &id, pt(&format!("a{i}"), 0.6)).unwrap();
        }
        ops::admit(&mut h, &idx, &"icu_b.r0".into(), pt("b0", 0.5)).unwrap();
        let before = patient_count(&h);

        let out = apply_close_unit(&mut h, &idx, &"icu_a".into(), "surge drill");
        assert_eq!(patient_count(&h), before, "closure must conserve patients");
        // Two evacuees fit icu_b's two vacant beds; the third has no ICU bed
        // anywhere (med/surg doesn't count) and strands in place, boarding.
        assert_eq!(out.evacuated, 2);
        assert_eq!(out.stranded, 1);
        // Vacant bed + both vacated beds are out of service.
        assert_eq!(out.closed, 3);
        let icu_a_rooms: Vec<_> = idx.room_indices(&"icu_a".into()).to_vec();
        let oos = icu_a_rooms
            .iter()
            .filter(|&&ri| matches!(h.rooms[ri].status, RoomStatus::OutOfService { .. }))
            .count();
        assert_eq!(oos, 3);
        let stranded: Vec<_> = icu_a_rooms
            .iter()
            .filter_map(|&ri| h.rooms[ri].status.patient())
            .collect();
        assert_eq!(stranded.len(), 1);
        assert!(stranded[0].boarding, "a stranded occupant is flagged boarding");
        // icu_b absorbed the shift.
        let b = aggregate_unit(&h, &idx, &"icu_b".into());
        assert_eq!(b.census, 3);
    }

    /// Closure held over a run: beds that empty in the closed unit go OOS,
    /// and the constrained variant boards more than the baseline.
    #[test]
    fn closure_raises_boarding_and_holds_through_the_run() {
        let (mut h, idx) = world(&[
            ("ed", UnitType::Ed, 10),
            ("ms", UnitType::MedSurg, 12),
            ("icu", UnitType::Icu, 3),
            ("sd", UnitType::Stepdown, 3),
        ]);
        // The ICU starts occupied so the sweep has beds to close mid-run
        // (backfilled discharges will empty them).
        for i in 0..2 {
            let id = format!("icu.r{i}").into();
            ops::admit(&mut h, &idx, &id, pt(&format!("i{i}"), 0.7)).unwrap();
        }
        let mut spec = ExperimentSpec::new(9, 0.0, 48.0);
        spec.interventions.push(Intervention::CloseUnit {
            unit: "icu".into(),
            reason: "flood damage".into(),
        });
        let res = run_experiment(&h, None, &spec);

        let base = summarize(&res.baseline.series.hospital, spec.sample_every_min);
        let var = summarize(&res.variant.series.hospital, spec.sample_every_min);
        assert!(
            var.peak_boarding > base.peak_boarding,
            "closing the only ICU must raise peak boarding: baseline {} vs variant {}",
            base.peak_boarding,
            var.peak_boarding
        );
        // No same-type bed exists for evacuation — both occupants stranded.
        assert_eq!(res.variant.close_outcomes[0].stranded, 2);
        // The closure holds: nobody is ever admitted into the closed unit
        // (census only falls as stranded occupants leave) and no closed bed
        // returns to play (Vacant) — it is either OOS or still occupied.
        let icu_census = &res.variant.series.units[&"icu".into()].census;
        assert!(
            icu_census.windows(2).all(|w| w[1] <= w[0]),
            "closed-unit census must never rise: {icu_census:?}"
        );
        assert!(
            idx.room_indices(&"icu".into()).iter().all(|&ri| {
                !res.variant.hospital.rooms[ri].status.is_vacant()
            }),
            "no closed bed may sit vacant-and-admittable at run end"
        );
        // The baseline arm was never closed.
        assert!(res.baseline.close_outcomes.is_empty());
    }

    /// The sweep directly: a bed of a closed unit that empties mid-run goes
    /// out of service on the next tick instead of back into circulation.
    #[test]
    fn closure_sweep_closes_beds_as_they_empty() {
        let (mut h, idx) = world(&[("icu", UnitType::Icu, 2), ("ms", UnitType::MedSurg, 2)]);
        ops::admit(&mut h, &idx, &"icu.r0".into(), pt("a", 0.5)).unwrap();
        let out = apply_close_unit(&mut h, &idx, &"icu".into(), "drill");
        assert_eq!((out.closed, out.stranded), (1, 1), "r1 closes, r0's occupant strands");

        let mut eng = SimEngine::new(1);
        eng.register(
            Box::new(ClosureSweep {
                units: vec![("icu".into(), "closed — drill".into())],
            }),
            true,
        );
        eng.tick(&mut h, &idx, 15.0);
        assert!(h.rooms[0].status.patient().is_some(), "occupied bed stays put");

        ops::discharge(&mut h, &idx, &"icu.r0".into()).unwrap();
        eng.tick(&mut h, &idx, 15.0);
        assert!(
            matches!(h.rooms[0].status, RoomStatus::OutOfService { .. }),
            "an emptied closed bed must go out of service, not back into play"
        );
    }

    /// Ratio flex is staffing-only: bed physics identical, workload differs.
    #[test]
    fn ratio_flex_changes_workload_series_only() {
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 14)]);
        for i in 0..12 {
            let id = format!("ms.r{i}").into();
            ops::admit(&mut h, &idx, &id, pt(&format!("p{i}"), 0.5)).unwrap();
        }
        // Pin the baseline ratio so the relief direction is unambiguous
        // (12 patients: 2 RNs × 6 pts at 6.0 → 4 RNs × 3 pts at 3.0).
        for u in &mut h.units {
            u.target_rn_ratio = Some(6.0);
        }
        let mut spec = ExperimentSpec::new(5, 0.0, 24.0);
        // Physics quiet + no backfill: census stays flat, so any series
        // difference could only come from the intervention.
        spec.toggles = ProcessToggles {
            diurnal: false,
            ed: false,
            drift: false,
        };
        spec.backfill_discharges = false;
        spec.interventions.push(Intervention::FlexRatio {
            unit: "ms".into(),
            ratio: 3.0,
        });
        let res = run_experiment(&h, None, &spec);

        let b = &res.baseline.series.hospital;
        let v = &res.variant.series.hospital;
        assert_eq!(b.census, v.census, "ratio flex must not move census");
        assert_eq!(b.boarding, v.boarding);
        assert_eq!(b.occupancy, v.occupancy);
        assert_ne!(
            b.workload_max, v.workload_max,
            "ratio flex must move the workload series"
        );
        let bs = summarize(b, spec.sample_every_min);
        let vs = summarize(v, spec.sample_every_min);
        assert!(
            vs.peak_workload.unwrap() < bs.peak_workload.unwrap(),
            "richer staffing must relieve peak per-RN workload ({:?} → {:?})",
            bs.peak_workload,
            vs.peak_workload
        );
        // Rooms identical at run end — only the unit's ratio field differs.
        assert_eq!(res.baseline.hospital.rooms, res.variant.hospital.rooms);
        assert_ne!(res.baseline.hospital.units, res.variant.hospital.units);
    }

    #[test]
    fn arrival_wave_places_all_or_reports_backlog() {
        // Plenty of room: all 8 place, census ends +8 vs baseline.
        let (h, _idx) = world(&[("ms", UnitType::MedSurg, 20)]);
        let mut spec = ExperimentSpec::new(3, 0.0, 6.0);
        spec.toggles = ProcessToggles {
            diurnal: false,
            ed: false,
            drift: false,
        };
        spec.interventions.push(Intervention::ArrivalWave {
            start_hr: 0.0,
            duration_hr: 2.0,
            count: 8,
            need: UnitType::MedSurg,
        });
        let res = run_experiment(&h, None, &spec);
        assert_eq!(res.variant.wave_injected, 8);
        assert_eq!(res.variant.wave_backlog, 0);
        let b_end = *res.baseline.series.hospital.census.last().unwrap();
        let v_end = *res.variant.series.hospital.census.last().unwrap();
        assert_eq!(v_end - b_end, 8.0);
        // Baseline never sees the wave.
        assert_eq!(res.baseline.wave_injected, 0);

        // Constrained: only 4 beds — the leftover is reported, not dropped.
        let (h2, _) = world(&[("ms", UnitType::MedSurg, 4)]);
        let res2 = run_experiment(&h2, None, &spec);
        assert_eq!(res2.variant.wave_injected, 4);
        assert_eq!(res2.variant.wave_backlog, 4);
    }

    /// Pre-warmed norms give day-one z-scores; without them the z lane obeys
    /// the warm-up honesty rule (None until the window fills).
    #[test]
    fn live_norms_prewarm_the_z_lane() {
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 10)]);
        for i in 0..5 {
            let id = format!("ms.r{i}").into();
            ops::admit(&mut h, &idx, &id, pt(&format!("p{i}"), 0.3)).unwrap();
        }
        // A warm live store: 96 samples of the current occupancy.
        let mut norms = RollingStore::default();
        for _ in 0..96 {
            norms.record(&h, &idx);
        }
        let mut spec = ExperimentSpec::new(1, 0.0, 6.0);
        spec.toggles = ProcessToggles {
            diurnal: false,
            ed: false,
            drift: false,
        };
        spec.backfill_discharges = false;

        let cold = run_experiment(&h, None, &spec);
        assert!(
            cold.baseline.series.hospital.occ_z.iter().all(Option::is_none),
            "cold norms must not fabricate z-scores inside a 6 h run"
        );
        // Warm norms are flat here (σ≈0 → still no z) — perturb them so a z
        // exists: vary the recorded occupancy by toggling one admission.
        let mut varied = RollingStore::default();
        for i in 0..96 {
            if i % 2 == 0 {
                let _ = ops::discharge(&mut h, &idx, &"ms.r0".into());
            } else {
                let _ = ops::admit(&mut h, &idx, &"ms.r0".into(), pt("p0", 0.3));
            }
            varied.record(&h, &idx);
        }
        let _ = ops::admit(&mut h, &idx, &"ms.r0".into(), pt("p0", 0.3));
        let warm = run_experiment(&h, Some(&varied), &spec);
        assert!(
            warm.baseline.series.hospital.occ_z[0].is_some(),
            "live norms must give sample-0 z-scores"
        );
    }
}
