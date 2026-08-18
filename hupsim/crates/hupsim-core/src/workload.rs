//! RN workload composite (EP-11) — pure functions shared by the sim-side
//! roster, the unit dashboard (EP-12), and placement scoring (EP-14). Lives
//! beside the matrix in core so UI and scoring consume one definition.
//!
//! The composite for one RN is a weighted sum of four components:
//! (a) census vs the unit's patients-per-RN ratio (1.0 = exactly at ratio),
//! (b) summed patient instability, (c) per-patient isolation and telemetry
//! overheads, and (d) trailing-window churn (admits/discharges/transfers
//! touching that RN's rooms). The breakdown's components sum exactly to the
//! scalar — the UI shows both.
//!
//! PROVENANCE: like `hupsim-sim/src/params.rs`, every weight here is
//! synthetic/inferred — shaped to make an isolation+telemetry patient
//! visibly costlier than a stable walkie-talkie, not measured acuity-tool
//! output. One visible table, tuned in one reviewable place.

use crate::model::{Unit, UnitType};

/// Floor for a patients-per-RN ratio: guards the census term against a
/// zero/garbage ratio in hand-edited data.
pub const MIN_RN_RATIO: f32 = 0.25;

/// Weights of the four composite components.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkloadWeights {
    /// Census-vs-ratio term: contributes `census / ratio`, so an RN carrying
    /// exactly the unit's target ratio scores 1.0 here.
    pub census: f32,
    /// Per unit of summed patient instability (0..1 each).
    pub instability: f32,
    /// Flat overhead per isolation patient (donning/doffing, cohorting).
    pub isolation: f32,
    /// Flat overhead per telemetry patient (alarm burden, strip review).
    pub telemetry: f32,
    /// Per trailing-window churn event (admit/discharge/transfer touching
    /// the RN's rooms; boarding patients count like any other occupant).
    pub churn: f32,
}

impl Default for WorkloadWeights {
    fn default() -> Self {
        Self {
            census: 1.0,
            instability: 0.9,
            isolation: 0.20,
            telemetry: 0.12,
            churn: 0.15,
        }
    }
}

/// The per-patient facts the composite consumes (a thin slice of
/// [`crate::patient::Patient`] / the matrix's patient columns).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RnLoadInput {
    pub instability: f32,
    pub isolation: bool,
    pub telemetry: bool,
}

/// Marginal load one occupied room adds to its RN: the room's share of the
/// census term plus its patient's instability and flag overheads. The
/// roster's balance pass moves rooms between RNs by exactly this quantity,
/// so `compose` over an RN's rooms equals the sum of their `room_load`s
/// plus the churn term.
pub fn room_load(w: &WorkloadWeights, ratio: f32, p: &RnLoadInput) -> f32 {
    w.census / ratio.max(MIN_RN_RATIO)
        + w.instability * p.instability.clamp(0.0, 1.0)
        + if p.isolation { w.isolation } else { 0.0 }
        + if p.telemetry { w.telemetry } else { 0.0 }
}

/// One RN's workload, already weighted: the components sum to the scalar.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WorkloadBreakdown {
    /// `w.census × census / ratio` — 1.0 means exactly at ratio.
    pub census_vs_ratio: f32,
    /// `w.instability × Σ instability`.
    pub instability: f32,
    /// Isolation + telemetry overheads.
    pub overhead: f32,
    /// `w.churn × trailing churn events`.
    pub churn: f32,
}

impl WorkloadBreakdown {
    /// The headline scalar: by construction, the sum of the components.
    pub fn scalar(&self) -> f32 {
        self.census_vs_ratio + self.instability + self.overhead + self.churn
    }
}

/// Compose one RN's workload from their assigned patients and trailing
/// churn count (a decayed event counter, sim-side).
pub fn compose(
    w: &WorkloadWeights,
    ratio: f32,
    patients: &[RnLoadInput],
    churn_events: f32,
) -> WorkloadBreakdown {
    let mut b = WorkloadBreakdown {
        census_vs_ratio: w.census * patients.len() as f32 / ratio.max(MIN_RN_RATIO),
        churn: w.churn * churn_events,
        ..Default::default()
    };
    for p in patients {
        b.instability += w.instability * p.instability.clamp(0.0, 1.0);
        b.overhead += if p.isolation { w.isolation } else { 0.0 };
        b.overhead += if p.telemetry { w.telemetry } else { 0.0 };
    }
    b
}

/// Patients-per-RN fallback for units that predate the EP-8 staffing data
/// (old saves, synthetic test worlds): inferred professional norms by level
/// of care, mirroring the units.json `staffing` provenance.
pub fn fallback_rn_ratio(t: UnitType) -> f32 {
    match t {
        UnitType::Icu => 2.0,
        UnitType::Stepdown => 3.0,
        UnitType::Ed => 4.0,
        UnitType::LaborDelivery => 2.0,
        UnitType::Nursery => 3.0,
        UnitType::Psych | UnitType::Detox => 6.0,
        UnitType::MedSurg | UnitType::Obs | UnitType::Research | UnitType::Other => 5.0,
    }
}

/// Effective patients-per-RN for a unit: the EP-8 `target_rn_ratio` when
/// present and sane, else the level-of-care fallback.
pub fn effective_rn_ratio(u: &Unit) -> f32 {
    u.target_rn_ratio
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or_else(|| fallback_rn_ratio(u.unit_type))
        .max(MIN_RN_RATIO)
}

/// Unit types that carry an RN roster (EP-11 scope: inpatient + ED + other
/// bedside-nursed spaces). Research beds and "Other" spaces are not staffed
/// by the model.
pub fn is_rn_staffed(t: UnitType) -> bool {
    !matches!(t, UnitType::Research | UnitType::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakdown_components_sum_to_scalar_and_match_room_loads() {
        let w = WorkloadWeights::default();
        let patients = [
            RnLoadInput { instability: 0.7, isolation: true, telemetry: true },
            RnLoadInput { instability: 0.1, isolation: false, telemetry: false },
            RnLoadInput { instability: 0.4, isolation: false, telemetry: true },
        ];
        let churn = 3.5;
        let b = compose(&w, 4.0, &patients, churn);
        // Components sum to the scalar (the acceptance invariant)...
        let sum = b.census_vs_ratio + b.instability + b.overhead + b.churn;
        assert!((b.scalar() - sum).abs() < 1e-6);
        // ...and the composite equals Σ room_load + churn term, which is
        // what lets the balance pass move rooms by their marginal load.
        let via_rooms: f32 = patients.iter().map(|p| room_load(&w, 4.0, p)).sum::<f32>()
            + w.churn * churn;
        assert!((b.scalar() - via_rooms).abs() < 1e-5);
    }

    #[test]
    fn isolation_telemetry_patient_costs_visibly_more_than_stable_walkie() {
        let w = WorkloadWeights::default();
        let walkie = RnLoadInput { instability: 0.1, isolation: false, telemetry: false };
        let heavy = RnLoadInput { instability: 0.6, isolation: true, telemetry: true };
        let (lo, hi) = (room_load(&w, 5.0, &walkie), room_load(&w, 5.0, &heavy));
        assert!(
            hi > 2.0 * lo,
            "iso+tele patient ({hi:.2}) should visibly outweigh a stable walkie-talkie ({lo:.2})"
        );
    }

    #[test]
    fn effective_ratio_prefers_data_then_type_fallback() {
        use crate::model::FloorLevel;
        use crate::provenance::Provenance;
        let mut u = Unit {
            id: "u".into(),
            name: "U".into(),
            service: "S".into(),
            service_line: None,
            unit_type: UnitType::Icu,
            building: "b".into(),
            level: FloorLevel::Numbered(1),
            elevator_core: None,
            target_rn_ratio: Some(1.0),
            provenance: Provenance::default(),
            note: None,
        };
        assert_eq!(effective_rn_ratio(&u), 1.0);
        u.target_rn_ratio = None;
        assert_eq!(effective_rn_ratio(&u), fallback_rn_ratio(UnitType::Icu));
        u.target_rn_ratio = Some(0.0); // garbage → fallback, floored
        assert_eq!(effective_rn_ratio(&u), fallback_rn_ratio(UnitType::Icu));
    }

    #[test]
    fn staffed_types_exclude_research_and_other() {
        assert!(is_rn_staffed(UnitType::MedSurg));
        assert!(is_rn_staffed(UnitType::Ed));
        assert!(!is_rn_staffed(UnitType::Research));
        assert!(!is_rn_staffed(UnitType::Other));
    }
}
