//! Placement query engine (EP-14) — the interactive payoff of the matrix:
//! given a patient's needs (real or hypothetical), which rooms are available,
//! and which is statistically the best?
//!
//! Two stages, both pure functions over the columnar [`RoomMatrix`] and the
//! EP-9/EP-11 statistics layers (handed in as plain slices so this module
//! stays sim-free):
//!
//! 1. **Hard filters** ([`hard_filter`]) on matrix masks: vacant +
//!    in-service, level-of-care band (the exact target, plus explicitly
//!    labeled adjacent-LOC overflow candidates *only* under the query's
//!    overflow toggle — never silently), and capability satisfaction
//!    (airborne isolation ⇒ negative-pressure room).
//! 2. **Scoring** ([`run`]) on the survivors: a lexicographic tier for LOC
//!    appropriateness — the owner's ruling: overflow to the appropriate
//!    level of care trumps service/unit placement conventions in a clinical
//!    crunch, so an exact-LOC room *always* outranks an overflow room — then
//!    a weighted sum of service-line match, unit occupancy headroom (lower
//!    occupancy and lower EP-9 pressure z), RN workload headroom (the EP-11
//!    composite of the covering RN), and telemetry fit (needing-and-capable
//!    ≻ needing-and-portable-monitor). Weights live in one visible table
//!    ([`PlacementWeights`]); every result carries its per-criterion
//!    breakdown, which sums to its score by construction.
//!
//! Transport distance is deliberately not a term (owner decision); EP-6 may
//! add it later. Querying is read-only — no RNG, no world mutation — so it
//! can never perturb sim determinism.

use crate::aggregate::UnitAggregate;
use crate::ids::RoomId;
use crate::matrix::{OccState, RoomMatrix, NO_UNIT};
use crate::model::UnitType;

/// Weights of the four scored criteria — one visible, owner-reviewable table
/// (the [`crate::workload::WorkloadWeights`] pattern). Descending in the
/// brief's priority order. LOC appropriateness is deliberately NOT a weight:
/// it is the lexicographic [`LocTier`], dominant over any weighted sum.
///
/// PROVENANCE: synthetic/inferred, like the workload weights — shaped so a
/// service-line match visibly outranks a marginally emptier unit, not
/// calibrated against real placement decisions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacementWeights {
    /// Room's unit belongs to the requested service line.
    pub service_line: f32,
    /// Unit occupancy headroom: lower live occupancy + lower pressure z.
    pub occupancy: f32,
    /// Covering RN's workload headroom (EP-11 composite, inverted).
    pub rn_headroom: f32,
    /// Telemetry-needing patient in a telemetry-capable room.
    pub telemetry: f32,
}

impl Default for PlacementWeights {
    fn default() -> Self {
        Self {
            service_line: 1.0,
            occupancy: 0.8,
            rn_headroom: 0.6,
            telemetry: 0.4,
        }
    }
}

impl PlacementWeights {
    /// Best attainable score for `q` — the score-bar scale: criteria the
    /// query doesn't engage (no line requested, no telemetry need) score 0
    /// for every candidate, so they don't inflate the denominator.
    pub fn max_for(&self, q: &PlacementQuery) -> f32 {
        self.occupancy
            + self.rn_headroom
            + if q.service_line.is_some() { self.service_line } else { 0.0 }
            + if q.telemetry { self.telemetry } else { 0.0 }
    }
}

/// Within the occupancy criterion, the share carried by live occupancy; the
/// remainder is the EP-9 pressure-z relief ("prefer lower occupancy AND
/// lower pressure z" — weighted evenly).
pub const OCC_NOW_SHARE: f32 = 0.5;

/// Pressure z is clamped to ±this span and mapped linearly onto 0..1 relief
/// (z ≤ −2 → 1.0, z = 0 → 0.5, z ≥ +2 → 0.0). Matches the dashboards' hot
/// threshold (`|z| ≥ 2`), so a hot unit bottoms out the relief term.
pub const OCC_Z_SPAN: f32 = 2.0;

/// Covering-RN composite mapped onto 0..1 headroom: `1 − load / SCALE`,
/// clamped. 3.0 ≈ the workload bars' practical ceiling (an at-ratio census
/// with heavy acuity and overheads), so headroom hits 0 about where the
/// unit strip starts reading "heavy".
pub const RN_LOAD_SCALE: f32 = 3.0;

/// Neutral value for the pressure-relief and RN-headroom components when the
/// statistics layer makes no claim (norm warming up / flat, no roster) —
/// exactly the midpoint, so absence of evidence never beats real headroom.
pub const NEUTRAL: f32 = 0.5;

/// Levels of care a placement query can target: bedded environments a
/// patient is placed *into*. ED (placed out of, not into), research beds,
/// and "Other" spaces are not placement targets.
pub const PLACEMENT_TARGETS: [UnitType; 8] = [
    UnitType::MedSurg,
    UnitType::Icu,
    UnitType::Stepdown,
    UnitType::Obs,
    UnitType::Psych,
    UnitType::Detox,
    UnitType::LaborDelivery,
    UnitType::Nursery,
];

/// Explicitly labeled adjacent-LOC overflow band per target: the classic
/// crunch patterns (ICU overflow into stepdown; stepdown flexes up to ICU or
/// down to med/surg; med/surg to stepdown or obs; obs to med/surg). Psych,
/// detox, L&D, and nursery are clinically distinct environments — no
/// adjacency — and ED/research/other are not targets at all.
pub fn overflow_types(target: UnitType) -> &'static [UnitType] {
    use UnitType::*;
    match target {
        Icu => &[Stepdown],
        Stepdown => &[Icu, MedSurg],
        MedSurg => &[Stepdown, Obs],
        Obs => &[MedSurg],
        _ => &[],
    }
}

/// A staged placement question: the patient's needs, either copied from a
/// real patient ("find placement" on an occupied room) or set hypothetically
/// through the criteria chips.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementQuery {
    /// Required level of care.
    pub target: UnitType,
    /// Patient needs telemetry (scored, not filtered: a non-capable room is
    /// a labeled portable-monitor workaround, not a hidden candidate).
    pub telemetry: bool,
    /// Airborne isolation ⇒ negative-pressure room required (hard filter).
    pub airborne: bool,
    /// Requested service line, as an index into `Hospital.service_lines`
    /// (the matrix's `service_line` column encoding). `None` = any.
    pub service_line: Option<u16>,
    /// Current location when staged from a real patient: excluded from the
    /// candidates and the handoff turns into a transfer from here.
    pub from_room: Option<RoomId>,
    /// Adjacent-LOC overflow toggle — off by default; overflow candidates
    /// only ever appear explicitly labeled.
    pub overflow: bool,
}

impl Default for PlacementQuery {
    fn default() -> Self {
        Self {
            target: UnitType::MedSurg,
            telemetry: false,
            airborne: false,
            service_line: None,
            from_room: None,
            overflow: false,
        }
    }
}

/// LOC appropriateness tier — the lexicographically dominant key: any Exact
/// result outranks every Overflow result regardless of weighted score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LocTier {
    Exact,
    Overflow,
}

impl LocTier {
    pub fn label(self) -> &'static str {
        match self {
            LocTier::Exact => "exact LOC",
            LocTier::Overflow => "overflow",
        }
    }
}

/// How a candidate serves the query's telemetry need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeleFit {
    /// Query has no telemetry need — the criterion scores 0 for everyone.
    NotNeeded,
    Capable,
    /// Needing but not capable: placeable with a portable monitor, scored 0.
    PortableMonitor,
}

/// Per-criterion weighted scores; the components sum to the result's score
/// by construction (the same invariant as `WorkloadBreakdown`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScoreBreakdown {
    pub service_line: f32,
    pub occupancy: f32,
    pub rn_headroom: f32,
    pub telemetry: f32,
}

impl ScoreBreakdown {
    /// The headline score: by construction, the sum of the components.
    pub fn total(&self) -> f32 {
        self.service_line + self.occupancy + self.rn_headroom + self.telemetry
    }

    /// Components in the fixed display order of the score-bar segments.
    pub fn components(&self) -> [f32; 4] {
        [self.service_line, self.occupancy, self.rn_headroom, self.telemetry]
    }
}

/// One ranked candidate, carrying the raw facts its reasons line is built
/// from (unit occupancy, pressure z, covering RN, telemetry fit) alongside
/// the tier and breakdown — the UI never recomputes scoring inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementResult {
    /// Matrix row (= `Hospital.rooms` index).
    pub row: u32,
    pub room: RoomId,
    /// Index into `Hospital.units`.
    pub unit_pos: u32,
    /// The candidate's own level of care (labels overflow rows: "Stepdown").
    pub unit_type: UnitType,
    pub tier: LocTier,
    pub breakdown: ScoreBreakdown,
    /// `None` when the query requested no line.
    pub service_match: Option<bool>,
    /// The unit's live occupancy fraction (census / staffed).
    pub occupancy_frac: f32,
    /// The unit's occupancy z vs its trailing-24 h norm (`None` = no claim).
    pub occ_z: Option<f32>,
    /// Covering RN (0-based index, EP-11 composite scalar); `None` = no
    /// roster claim (unstaffed type, empty unit).
    pub covering_rn: Option<(u16, f32)>,
    pub tele: TeleFit,
}

/// The statistics context scoring reads — plain slices so hupsim-core stays
/// free of sim types. All are derived, read-only state.
pub struct PlacementContext<'a> {
    pub matrix: &'a RoomMatrix,
    /// Per-unit aggregates, indexed like `Hospital.units` (occupancy source).
    pub unit_aggs: &'a [UnitAggregate],
    /// Per-unit occupancy z vs the trailing-24 h norm (EP-9); `None` while
    /// the norm warms up or when it is flat.
    pub unit_occ_z: &'a [Option<f32>],
    /// Covering RN (index, composite) per room row, row-parallel to the
    /// matrix (`RnRoster::covering_rn_loads` sim-side); `None` = no claim.
    pub covering_rn: &'a [Option<(u16, f32)>],
}

/// Which LOC tier a candidate row occupies for `q`, `None` if outside the
/// band entirely.
fn loc_tier(q: &PlacementQuery, t: UnitType) -> Option<LocTier> {
    if t == q.target {
        Some(LocTier::Exact)
    } else if q.overflow && overflow_types(q.target).contains(&t) {
        Some(LocTier::Overflow)
    } else {
        None
    }
}

/// Hard filters as a matrix mask: vacant + in-service, inside the LOC band,
/// capability-satisfying, and not the patient's own room.
pub fn hard_filter(m: &RoomMatrix, q: &PlacementQuery) -> Vec<bool> {
    let from_row = q.from_room.as_ref().and_then(|r| m.row_of(r));
    (0..m.len())
        .map(|i| {
            m.occupancy[i] == OccState::Vacant
                && m.unit_pos[i] != NO_UNIT
                && loc_tier(q, m.unit_type[i]).is_some()
                && (!q.airborne || m.negative_pressure[i])
                && from_row != Some(i as u32)
        })
        .collect()
}

/// Filter, score, and rank. Ordering is total and deterministic: LOC tier
/// first (lexicographic — exact always above overflow), then weighted score
/// descending, then matrix row ascending (stable ties).
pub fn run(
    ctx: &PlacementContext,
    q: &PlacementQuery,
    w: &PlacementWeights,
) -> Vec<PlacementResult> {
    let m = ctx.matrix;
    assert_eq!(
        ctx.unit_aggs.len(),
        ctx.unit_occ_z.len(),
        "unit_aggs / unit_occ_z must both index Hospital.units"
    );
    assert_eq!(
        ctx.covering_rn.len(),
        m.len(),
        "covering_rn must be row-parallel to the matrix"
    );

    let mask = hard_filter(m, q);
    let mut results: Vec<PlacementResult> = mask
        .iter()
        .enumerate()
        .filter(|(_, &pass)| pass)
        .filter_map(|(i, _)| {
            let up = m.unit_pos[i] as usize;
            // NO_UNIT rows are already masked out; a unit_pos beyond the
            // aggregate slice means the context is stale — skip, don't panic.
            let agg = ctx.unit_aggs.get(up)?;
            let tier = loc_tier(q, m.unit_type[i]).expect("masked rows are in the LOC band");

            let service_match = q.service_line.map(|line| m.service_line[i] == line);
            let service_score = match service_match {
                Some(true) => w.service_line,
                _ => 0.0,
            };

            let occ = agg.occupancy_frac();
            let occ_z = ctx.unit_occ_z[up];
            let relief = match occ_z {
                Some(z) => (OCC_Z_SPAN - z.clamp(-OCC_Z_SPAN, OCC_Z_SPAN)) / (2.0 * OCC_Z_SPAN),
                None => NEUTRAL,
            };
            let occ_score = w.occupancy
                * (OCC_NOW_SHARE * (1.0 - occ).clamp(0.0, 1.0) + (1.0 - OCC_NOW_SHARE) * relief);

            let covering_rn = ctx.covering_rn[i];
            let headroom = covering_rn
                .map(|(_, load)| (1.0 - load / RN_LOAD_SCALE).clamp(0.0, 1.0))
                .unwrap_or(NEUTRAL);
            let rn_score = w.rn_headroom * headroom;

            let (tele, tele_score) = if !q.telemetry {
                (TeleFit::NotNeeded, 0.0)
            } else if m.telemetry_capable[i] {
                (TeleFit::Capable, w.telemetry)
            } else {
                (TeleFit::PortableMonitor, 0.0)
            };

            Some(PlacementResult {
                row: i as u32,
                room: m.room_id(i as u32).clone(),
                unit_pos: up as u32,
                unit_type: m.unit_type[i],
                tier,
                breakdown: ScoreBreakdown {
                    service_line: service_score,
                    occupancy: occ_score,
                    rn_headroom: rn_score,
                    telemetry: tele_score,
                },
                service_match,
                occupancy_frac: occ,
                occ_z,
                covering_rn,
                tele,
            })
        })
        .collect();

    results.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then(b.breakdown.total().total_cmp(&a.breakdown.total()))
            .then(a.row.cmp(&b.row))
    });
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::HospitalIndex;
    use crate::model::*;
    use crate::patient::{Acuity, Patient};
    use crate::provenance::Provenance;

    fn patient(telemetry: bool) -> Patient {
        Patient {
            id: "p".into(),
            alias: "P".into(),
            age: None,
            service: None,
            admitted_at_min: None,
            acuity: Acuity {
                instability: 0.4,
                trend_per_hr: 0.0,
            },
            boarding: false,
            isolation: None,
            telemetry,
            note: String::new(),
        }
    }

    fn unit(id: &str, ut: UnitType, line: Option<&str>) -> Unit {
        Unit {
            id: id.into(),
            name: id.to_uppercase(),
            service: "S".into(),
            service_line: line.map(|l| l.into()),
            unit_type: ut,
            building: "b1".into(),
            level: FloorLevel::Numbered(1),
            elevator_core: None,
            target_rn_ratio: None,
            provenance: Provenance::default(),
            note: None,
        }
    }

    fn room(id: &str, unit: &str, tele: bool, np: bool, status: RoomStatus) -> Room {
        Room {
            id: id.into(),
            number: id.into(),
            unit: unit.into(),
            kind: RoomKind::Inpatient,
            telemetry_capable: tele,
            negative_pressure: np,
            status,
            provenance: Provenance::default(),
        }
    }

    fn building(id: &str) -> Building {
        Building {
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
        }
    }

    fn line(id: &str) -> ServiceLine {
        ServiceLine {
            id: id.into(),
            name: id.into(),
            description: None,
        }
    }

    /// Crafted world: one ICU (critical line), one stepdown + one med/surg
    /// (medicine line), one ED. Room roster per test via the `rooms` arg.
    fn hospital(rooms: Vec<Room>) -> Hospital {
        Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![building("b1")],
            units: vec![
                unit("icu", UnitType::Icu, Some("line.critical")),
                unit("pcu", UnitType::Stepdown, Some("line.medicine")),
                unit("ms", UnitType::MedSurg, Some("line.medicine")),
                unit("ed", UnitType::Ed, Some("line.emergency")),
            ],
            rooms,
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![
                line("line.critical"),
                line("line.medicine"),
                line("line.emergency"),
            ],
        }
    }

    /// Matrix + neutral context (no norms, no roster) over `h`. The returned
    /// aggregates reflect the crafted census.
    fn neutral_ctx(h: &Hospital) -> (RoomMatrix, Vec<UnitAggregate>, Vec<Option<f32>>) {
        let idx = HospitalIndex::build(h);
        let m = RoomMatrix::build(h, &idx);
        let aggs = m.unit_aggregates(h.units.len());
        let zs = vec![None; h.units.len()];
        (m, aggs, zs)
    }

    fn icu_query() -> PlacementQuery {
        PlacementQuery {
            target: UnitType::Icu,
            ..Default::default()
        }
    }

    #[test]
    fn hard_filter_keeps_only_vacant_in_service_target_rooms() {
        let h = hospital(vec![
            room("i1", "icu", true, false, RoomStatus::Vacant),
            room("i2", "icu", true, false, RoomStatus::Occupied { patient: patient(false) }),
            room("i3", "icu", true, false, RoomStatus::OutOfService { reason: "clean".into() }),
            room("s1", "pcu", true, false, RoomStatus::Vacant),
            room("m1", "ms", true, false, RoomStatus::Vacant),
        ]);
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        assert_eq!(
            hard_filter(&m, &icu_query()),
            vec![true, false, false, false, false],
            "occupied, OOS, and off-target rooms must all drop"
        );
    }

    #[test]
    fn airborne_isolation_excludes_non_negative_pressure() {
        let h = hospital(vec![
            room("i1", "icu", true, true, RoomStatus::Vacant),
            room("i2", "icu", true, false, RoomStatus::Vacant),
        ]);
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        let q = PlacementQuery {
            airborne: true,
            ..icu_query()
        };
        assert_eq!(hard_filter(&m, &q), vec![true, false]);
        // Without the isolation need, both qualify.
        assert_eq!(hard_filter(&m, &icu_query()), vec![true, true]);
    }

    #[test]
    fn overflow_toggle_admits_adjacent_loc_only_when_on() {
        let h = hospital(vec![
            room("i1", "icu", true, false, RoomStatus::Vacant),
            room("s1", "pcu", true, false, RoomStatus::Vacant),
            room("m1", "ms", true, false, RoomStatus::Vacant),
            room("e1", "ed", true, false, RoomStatus::Vacant),
        ]);
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        // Off: exact target only.
        assert_eq!(hard_filter(&m, &icu_query()), vec![true, false, false, false]);
        // On: stepdown joins for an ICU target; med/surg and ED never do.
        let q = PlacementQuery {
            overflow: true,
            ..icu_query()
        };
        assert_eq!(hard_filter(&m, &q), vec![true, true, false, false]);
        // And the results label the band, never silently.
        let (m, aggs, zs) = neutral_ctx(&h);
        let cov = vec![None; m.len()];
        let ctx = PlacementContext {
            matrix: &m,
            unit_aggs: &aggs,
            unit_occ_z: &zs,
            covering_rn: &cov,
        };
        let results = run(&ctx, &q, &PlacementWeights::default());
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .any(|r| r.tier == LocTier::Overflow && r.unit_type == UnitType::Stepdown));
    }

    #[test]
    fn ed_and_distinct_environments_have_no_overflow_band() {
        assert!(overflow_types(UnitType::Ed).is_empty());
        assert!(overflow_types(UnitType::Psych).is_empty());
        assert!(overflow_types(UnitType::LaborDelivery).is_empty());
        assert!(overflow_types(UnitType::Nursery).is_empty());
        // The bands that exist are the documented crunch patterns.
        assert_eq!(overflow_types(UnitType::Icu), &[UnitType::Stepdown]);
        assert!(overflow_types(UnitType::Stepdown).contains(&UnitType::Icu));
    }

    #[test]
    fn loc_tier_strictly_dominates_the_weighted_score() {
        // The exact-LOC room is made maximally unattractive: its unit is
        // nearly full and hot (z +2), its covering RN is buried, no service
        // match, telemetry only by portable monitor. The overflow room gets
        // everything: empty unit, cold z, fresh RN, line match, tele capable.
        let h = hospital(vec![
            room("i1", "icu", false, false, RoomStatus::Vacant),
            room("i2", "icu", false, false, RoomStatus::Occupied { patient: patient(true) }),
            room("i3", "icu", false, false, RoomStatus::Occupied { patient: patient(true) }),
            room("s1", "pcu", true, false, RoomStatus::Vacant),
        ]);
        let (m, aggs, _) = neutral_ctx(&h);
        let zs = vec![Some(2.0), Some(-2.0), None, None];
        let cov: Vec<Option<(u16, f32)>> = vec![
            Some((0, 4.0)), // buried RN covering the ICU bed
            None,
            None,
            Some((0, 0.3)), // fresh RN covering the stepdown bed
        ];
        let ctx = PlacementContext {
            matrix: &m,
            unit_aggs: &aggs,
            unit_occ_z: &zs,
            covering_rn: &cov,
        };
        let q = PlacementQuery {
            telemetry: true,
            service_line: Some(1), // line.medicine — matches the stepdown unit
            overflow: true,
            ..icu_query()
        };
        let results = run(&ctx, &q, &PlacementWeights::default());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].tier, LocTier::Exact, "exact LOC must lead");
        assert_eq!(results[0].room, RoomId::from("i1"));
        assert!(
            results[1].breakdown.total() > results[0].breakdown.total(),
            "the overflow room must have the better weighted score for this test to bite"
        );
    }

    #[test]
    fn breakdown_components_sum_to_score_and_stay_within_weights() {
        let h = hospital(vec![
            room("i1", "icu", true, true, RoomStatus::Vacant),
            room("i2", "icu", false, false, RoomStatus::Vacant),
            room("s1", "pcu", true, false, RoomStatus::Vacant),
            room("m1", "ms", false, false, RoomStatus::Occupied { patient: patient(true) }),
        ]);
        let (m, aggs, _) = neutral_ctx(&h);
        let zs = vec![Some(0.7), Some(-1.2), None, None];
        let cov: Vec<Option<(u16, f32)>> =
            vec![Some((1, 1.4)), Some((0, 2.2)), None, Some((0, 1.0))];
        let ctx = PlacementContext {
            matrix: &m,
            unit_aggs: &aggs,
            unit_occ_z: &zs,
            covering_rn: &cov,
        };
        let w = PlacementWeights::default();
        let q = PlacementQuery {
            telemetry: true,
            service_line: Some(0),
            overflow: true,
            ..icu_query()
        };
        let results = run(&ctx, &q, &w);
        assert!(!results.is_empty());
        for r in &results {
            let sum: f32 = r.breakdown.components().iter().sum();
            assert!((r.breakdown.total() - sum).abs() < 1e-6, "breakdown must sum to the score");
            assert!(r.breakdown.service_line >= 0.0 && r.breakdown.service_line <= w.service_line);
            assert!(r.breakdown.occupancy >= 0.0 && r.breakdown.occupancy <= w.occupancy);
            assert!(r.breakdown.rn_headroom >= 0.0 && r.breakdown.rn_headroom <= w.rn_headroom);
            assert!(r.breakdown.telemetry >= 0.0 && r.breakdown.telemetry <= w.telemetry);
            assert!(r.breakdown.total() <= w.max_for(&q) + 1e-6);
        }
        // Criteria the query doesn't engage score zero, not a hidden bonus.
        let plain = run(&ctx, &icu_query(), &w);
        for r in &plain {
            assert_eq!(r.breakdown.service_line, 0.0);
            assert_eq!(r.breakdown.telemetry, 0.0);
            assert_eq!(r.tele, TeleFit::NotNeeded);
            assert_eq!(r.service_match, None);
        }
    }

    #[test]
    fn scoring_prefers_headroom_on_every_axis() {
        // Two vacant ICU beds; ICU at 50% occupancy. Axis under test varies,
        // everything else held equal.
        let base = || {
            hospital(vec![
                room("i1", "icu", true, false, RoomStatus::Vacant),
                room("i2", "icu", true, false, RoomStatus::Vacant),
                room("i3", "icu", true, false, RoomStatus::Occupied { patient: patient(false) }),
                room("i4", "icu", true, false, RoomStatus::Occupied { patient: patient(false) }),
            ])
        };
        let w = PlacementWeights::default();

        // RN headroom: the lighter covering RN wins.
        let h = base();
        let (m, aggs, zs) = neutral_ctx(&h);
        let cov: Vec<Option<(u16, f32)>> =
            vec![Some((0, 2.5)), Some((1, 0.8)), None, None];
        let ctx = PlacementContext { matrix: &m, unit_aggs: &aggs, unit_occ_z: &zs, covering_rn: &cov };
        let results = run(&ctx, &icu_query(), &w);
        assert_eq!(results[0].room, RoomId::from("i2"), "lighter RN must rank first");

        // Telemetry fit: capable beats portable-monitor when needed.
        let h = hospital(vec![
            room("i1", "icu", false, false, RoomStatus::Vacant),
            room("i2", "icu", true, false, RoomStatus::Vacant),
        ]);
        let (m, aggs, zs) = neutral_ctx(&h);
        let cov = vec![None; m.len()];
        let ctx = PlacementContext { matrix: &m, unit_aggs: &aggs, unit_occ_z: &zs, covering_rn: &cov };
        let q = PlacementQuery { telemetry: true, ..icu_query() };
        let results = run(&ctx, &q, &w);
        assert_eq!(results[0].room, RoomId::from("i2"));
        assert_eq!(results[0].tele, TeleFit::Capable);
        assert_eq!(results[1].tele, TeleFit::PortableMonitor);

        // Pressure z: with occupancy equal, the colder unit wins. Stepdown
        // overflow band gives us a second unit at equal occupancy.
        let h = hospital(vec![
            room("i1", "icu", true, false, RoomStatus::Vacant),
            room("s1", "pcu", true, false, RoomStatus::Vacant),
        ]);
        let (m, aggs, _) = neutral_ctx(&h);
        let zs = vec![Some(1.5), Some(-1.5), None, None];
        let cov = vec![None; m.len()];
        let ctx = PlacementContext { matrix: &m, unit_aggs: &aggs, unit_occ_z: &zs, covering_rn: &cov };
        let q = PlacementQuery { target: UnitType::Stepdown, overflow: true, ..Default::default() };
        let results = run(&ctx, &q, &w);
        // Exact stepdown leads regardless; its relief term must beat what
        // the same room would score under the hot z.
        assert_eq!(results[0].room, RoomId::from("s1"));
        let hot_zs = vec![Some(1.5), Some(1.5), None, None];
        let hot_ctx = PlacementContext { matrix: &m, unit_aggs: &aggs, unit_occ_z: &hot_zs, covering_rn: &cov };
        let hot = run(&hot_ctx, &q, &w);
        assert!(
            results[0].breakdown.occupancy > hot[0].breakdown.occupancy,
            "a colder unit must score more occupancy headroom"
        );
    }

    #[test]
    fn from_room_is_excluded_from_candidates() {
        let h = hospital(vec![
            room("i1", "icu", true, false, RoomStatus::Vacant),
            room("i2", "icu", true, false, RoomStatus::Vacant),
        ]);
        let idx = HospitalIndex::build(&h);
        let m = RoomMatrix::build(&h, &idx);
        let q = PlacementQuery {
            from_room: Some("i1".into()),
            ..icu_query()
        };
        assert_eq!(hard_filter(&m, &q), vec![false, true]);
    }

    #[test]
    fn ranking_is_deterministic_with_stable_ties() {
        let h = hospital(vec![
            room("i1", "icu", true, false, RoomStatus::Vacant),
            room("i2", "icu", true, false, RoomStatus::Vacant),
            room("i3", "icu", true, false, RoomStatus::Vacant),
        ]);
        let (m, aggs, zs) = neutral_ctx(&h);
        let cov = vec![None; m.len()];
        let ctx = PlacementContext { matrix: &m, unit_aggs: &aggs, unit_occ_z: &zs, covering_rn: &cov };
        let w = PlacementWeights::default();
        let a = run(&ctx, &icu_query(), &w);
        let b = run(&ctx, &icu_query(), &w);
        assert_eq!(a, b, "same world must produce the identical ranking");
        // Identical rooms tie on score → matrix row order, i1 i2 i3.
        assert_eq!(
            a.iter().map(|r| r.room.as_str()).collect::<Vec<_>>(),
            vec!["i1", "i2", "i3"]
        );
    }
}
