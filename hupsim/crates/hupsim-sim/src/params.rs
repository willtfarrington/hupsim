//! EP-10 sim-realism parameters — the one visible table.
//!
//! PROVENANCE: every number in this file is **synthetic/inferred**. Values
//! are shaped to publicly known patterns (cited inline where an anchor
//! exists: HUP is a ~1,000-bed quaternary academic center, ED volume on the
//! order of 60k visits/yr, average inpatient LOS ≈ 6 days, national ED
//! admit rates 15–30% with academic centers at the high end) but none of
//! them is measured HUP data. They exist so deviation-from-norm indicators
//! (EP-9 z-scores, EP-12/13 dashboards) demo against plausible diurnal
//! dynamics — treat them as demo defaults, not clinical truth.
//!
//! Everything is a compile-time constant on purpose (owner decision, EP-10):
//! no UI editing surface yet; tuning happens here, in one reviewable place.

use hupsim_core::ids::ServiceLineId;
use hupsim_core::model::UnitType;

// ---------------------------------------------------------------------------
// Time conventions
// ---------------------------------------------------------------------------

/// Sim epoch convention: `t = 0` is **Monday 00:00**. Purely synthetic — it
/// only drives the weekday/weekend factor below.
pub fn is_weekend(t_min: f64) -> bool {
    let day = (t_min / 1440.0).floor() as i64;
    matches!(day.rem_euclid(7), 5 | 6)
}

/// Sim hour-of-day (0..24) at a sim minute.
pub fn hour_of(t_min: f64) -> usize {
    ((t_min / 60.0).floor() as i64).rem_euclid(24) as usize
}

/// Arrival processes integrate λ(t) in sub-steps of at most this many sim
/// minutes, so coarse high-speed ticks still see the diurnal curve instead
/// of one flat rate per tick.
pub const SUBSTEP_MIN: f64 = 15.0;

/// λ(t) for one process: base rate × hour-of-day shape × weekend factor.
pub fn diurnal_rate_per_hr(
    base_per_hr: f64,
    shape: &[f64; 24],
    weekend_factor: f64,
    t_min: f64,
) -> f64 {
    let wk = if is_weekend(t_min) { weekend_factor } else { 1.0 };
    base_per_hr * shape[hour_of(t_min)] * wk
}

// ---------------------------------------------------------------------------
// Arrival intensity — inpatient (direct/scheduled + transfer-in) admissions
// ---------------------------------------------------------------------------

/// Hour-of-day multipliers, mean ≈ 1.0. Morning scheduled-admission bump
/// (post-op beds requested ~09:00–11:00), afternoon/evening ED-driven peak,
/// overnight trough. Shape inferred from the universal hospital admission
/// curve; not HUP-measured.
pub const INPATIENT_ARRIVAL_SHAPE: [f64; 24] = [
    0.35, 0.30, 0.25, 0.25, 0.30, 0.40, // 00–05 trough
    0.60, 0.90, 1.20, 1.50, 1.70, 1.60, // 06–11 scheduled bump
    1.40, 1.35, 1.50, 1.70, 1.80, 1.70, // 12–17 afternoon peak
    1.50, 1.30, 1.00, 0.80, 0.60, 0.45, // 18–23 wind-down
];

/// Whole-hospital direct inpatient admissions per hour at shape 1.0.
/// Sized so steady-state census (λ·LOS, plus ED-converted admissions below)
/// lands near ~85% occupancy of the 944 inpatient beds (MedSurg 728 + ICU
/// 144 + Stepdown 72 on the 1,115-room EP-23 roster) — the chronic-full
/// regime HUP actually operates in. Recalibrated for EP-21: the EP-23
/// service-line rulings raised the bed-weighted mean LOS to ~130 h (oncology
/// floor-12 block, cardiac/neuro at ~144 h), so the old 6.0 targeted >100%
/// and pegged the house. Sized against the *weekday* rate (Mon–Fri runs at
/// the full base; the weekend dip is a bonus breath, not part of the
/// budget): 3.5 + ~2.1/hr ED conversions ≈ 5.6/hr against ~144 h effective
/// mean LOS (bed-weighted table mean + the mid-day discharge-curve delay;
/// uniform-over-vacant placement lets the long-LOS lines run fullest) ⇒
/// ~805 census ≈ 85% of the 944 beds. Verified empirically by the
/// census-regime smoke test.
pub const DIURNAL_BASE_ADMITS_PER_HR: f64 = 3.5;

/// Scheduled/elective admissions largely pause on weekends; emergent direct
/// admissions continue. Net factor inferred.
pub const WEEKEND_INPATIENT_FACTOR: f64 = 0.65;

/// Unit types that hold licensed inpatient beds the flow processes admit
/// into. ED bays, L&D, nursery, obs, research are excluded — they have their
/// own processes (ED) or are out of EP-10 scope.
pub const INPATIENT_UNIT_TYPES: [UnitType; 3] =
    [UnitType::MedSurg, UnitType::Icu, UnitType::Stepdown];

pub fn is_inpatient_unit(t: UnitType) -> bool {
    INPATIENT_UNIT_TYPES.contains(&t)
}

// ---------------------------------------------------------------------------
// Arrival intensity — ED
// ---------------------------------------------------------------------------

/// ED hour-of-day multipliers: late-morning ramp, sustained afternoon/evening
/// plateau, 03:00–05:00 trough. The classic ED arrival curve, synthetic.
pub const ED_ARRIVAL_SHAPE: [f64; 24] = [
    0.70, 0.55, 0.45, 0.40, 0.40, 0.45, // 00–05
    0.60, 0.85, 1.10, 1.35, 1.50, 1.60, // 06–11
    1.55, 1.50, 1.45, 1.40, 1.35, 1.35, // 12–17
    1.40, 1.35, 1.25, 1.10, 0.95, 0.80, // 18–23
];

/// ~61k ED visits/yr ≈ 7.0/hr average (Penn Medicine public figure, order of
/// magnitude only).
pub const ED_BASE_ARRIVALS_PER_HR: f64 = 7.0;

/// ED volume dips only slightly on weekends.
pub const WEEKEND_ED_FACTOR: f64 = 0.95;

/// Fraction of ED patients converting to inpatient admission. National rate
/// is 15–20%; quaternary academic centers run higher. Inferred at 0.30.
pub const ED_ADMIT_FRACTION: f64 = 0.30;

/// Level-of-care mix for ED-converted admissions (must sum to 1.0).
/// Academic-center shaped: most to med/surg, meaningful ICU share.
pub const ED_ADMIT_MIX: [(UnitType, f64); 3] = [
    (UnitType::MedSurg, 0.72),
    (UnitType::Stepdown, 0.13),
    (UnitType::Icu, 0.15),
];

// ---------------------------------------------------------------------------
// Length of stay — log-normal, per service line (EP-8 taxonomy)
// ---------------------------------------------------------------------------

/// Log-normal LOS: `LOS = median_hr · exp(sigma · Z)`, Z ~ N(0,1) from the
/// seeded RNG. Medians sit below the ~6-day HUP *mean* LOS because
/// log-normals are right-skewed — the long tail pulls the mean up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LosParams {
    pub median_hr: f64,
    /// σ of ln(LOS) — 0.5–0.7 gives the realistic 2×–3× spread.
    pub sigma: f64,
}

impl LosParams {
    /// The stay-length distribution seen when sampling stays *in progress* at
    /// a random instant (EP-21 census backdating): long stays are
    /// overrepresented in proportion to their length. For a log-normal the
    /// size-biased version is the same σ with median × e^(σ²).
    pub fn length_biased(self) -> Self {
        Self {
            median_hr: self.median_hr * (self.sigma * self.sigma).exp(),
            sigma: self.sigma,
        }
    }
}

/// Service-line LOS table (synthetic/inferred, keyed by the EP-8 service-line
/// ids in `service_lines.json`). Unmapped lines fall through to the
/// unit-type fallback below.
pub const LOS_BY_SERVICE_LINE: [(&str, LosParams); 9] = [
    ("line.medicine", LosParams { median_hr: 96.0, sigma: 0.55 }),
    ("line.cardiac", LosParams { median_hr: 120.0, sigma: 0.60 }),
    ("line.critical_care", LosParams { median_hr: 132.0, sigma: 0.70 }),
    ("line.surgery", LosParams { median_hr: 110.0, sigma: 0.60 }),
    ("line.oncology", LosParams { median_hr: 150.0, sigma: 0.65 }),
    ("line.neuro", LosParams { median_hr: 120.0, sigma: 0.60 }),
    ("line.womens_neonatal", LosParams { median_hr: 60.0, sigma: 0.50 }),
    ("line.behavioral_health", LosParams { median_hr: 168.0, sigma: 0.50 }),
    ("line.other", LosParams { median_hr: 96.0, sigma: 0.60 }),
];

/// Fallback when the unit has no resolved service line (pre-EP-8 saves,
/// synthetic test worlds): keyed by level of care.
pub fn los_fallback(unit_type: UnitType) -> LosParams {
    match unit_type {
        UnitType::Icu => LosParams { median_hr: 132.0, sigma: 0.70 },
        UnitType::Stepdown => LosParams { median_hr: 110.0, sigma: 0.60 },
        UnitType::Obs => LosParams { median_hr: 24.0, sigma: 0.40 },
        _ => LosParams { median_hr: 96.0, sigma: 0.55 },
    }
}

/// LOS parameters for an admission: service line first, unit type fallback.
pub fn los_for(line: Option<&ServiceLineId>, unit_type: UnitType) -> LosParams {
    if let Some(line) = line {
        for (id, p) in LOS_BY_SERVICE_LINE {
            if id == line.as_str() {
                return p;
            }
        }
    }
    los_fallback(unit_type)
}

/// Hour-of-day weights for *inpatient* discharge timing — the mid-day bulge
/// ("discharge by noon" is the lever EP-15/EP-17 pull). A sampled tentative
/// LOS is pushed forward onto this curve, never shortened.
pub const DISCHARGE_HOUR_WEIGHTS: [f64; 24] = [
    0.20, 0.10, 0.10, 0.10, 0.10, 0.20, // 00–05 (rare: deaths, AMA)
    0.40, 0.80, 1.50, 2.50, 4.00, 6.00, // 06–11 ramp
    7.00, 6.50, 5.50, 4.50, 3.50, 2.50, // 12–17 peak then taper
    1.80, 1.20, 0.80, 0.50, 0.35, 0.25, // 18–23
];

// ---------------------------------------------------------------------------
// ED episode timing
// ---------------------------------------------------------------------------

/// Door-to-discharge for treat-and-release ED visits (median ~3.5 h).
pub const ED_TREAT_RELEASE_LOS: LosParams = LosParams { median_hr: 3.5, sigma: 0.45 };

/// Door-to-admit-decision for converting patients (median ~4.5 h; boarding
/// clock starts when the decision fires and no bed exists).
pub const ED_DECISION_TO_ADMIT: LosParams = LosParams { median_hr: 4.5, sigma: 0.40 };

/// Boarding-wait scale for *seeded* ED boarders (EP-21 backdating): how long
/// a boarder has already waited past the admit decision when the snapshot is
/// taken. Synthetic/inferred — academic-center boarding waits run hours, not
/// minutes. Live boarders don't use this; their wait emerges from bed
/// availability.
pub const ED_BOARDING_WAIT: LosParams = LosParams { median_hr: 3.0, sigma: 0.60 };

// ---------------------------------------------------------------------------
// Acuity — initial draw and drift, per level of care
// ---------------------------------------------------------------------------

/// Mean-reversion anchor for instability, per unit type. Doubles as the
/// center of the initial-acuity draw for new admissions.
pub fn acuity_baseline(t: UnitType) -> f32 {
    match t {
        UnitType::Icu => 0.50,
        UnitType::Stepdown => 0.35,
        UnitType::Ed => 0.30,
        UnitType::LaborDelivery => 0.25,
        UnitType::Nursery => 0.30,
        UnitType::MedSurg => 0.18,
        _ => 0.20,
    }
}

/// Spread of the initial-acuity draw around the baseline (see
/// `sampling::draw_initial_instability` for the skew construction).
pub fn acuity_init_spread(t: UnitType) -> f32 {
    match t {
        UnitType::Icu => 0.40,
        UnitType::Stepdown => 0.35,
        UnitType::Ed => 0.45,
        _ => 0.35,
    }
}

/// Volatility multiplier on the acuity random walk: ICU patients genuinely
/// wander more (device titration, active resuscitation); observation-level
/// patients barely move. Applied to `AcuityDrift::volatility_per_sqrt_hr`.
pub fn drift_volatility_factor(t: UnitType) -> f32 {
    match t {
        UnitType::Icu => 1.60,
        UnitType::Ed => 1.35,
        UnitType::Stepdown => 1.25,
        UnitType::MedSurg => 1.00,
        UnitType::LaborDelivery => 0.80,
        UnitType::Obs => 0.70,
        UnitType::Psych | UnitType::Detox => 0.60,
        UnitType::Nursery => 0.50,
        UnitType::Research => 0.40,
        UnitType::Other => 0.70,
    }
}

/// Fractional pull of instability toward the unit-type baseline, per hour.
/// Keeps long-stay patients from railing at 0 or 1 while leaving room for
/// excursions the z-scores can catch.
pub const ACUITY_REVERT_PER_HR: f32 = 0.02;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_average_near_unity() {
        for (name, shape) in [
            ("inpatient", &INPATIENT_ARRIVAL_SHAPE),
            ("ed", &ED_ARRIVAL_SHAPE),
        ] {
            let mean = shape.iter().sum::<f64>() / 24.0;
            assert!(
                (0.9..=1.1).contains(&mean),
                "{name} shape mean {mean} drifted from 1.0 — base rates would silently rescale"
            );
        }
    }

    #[test]
    fn ed_admit_mix_sums_to_one() {
        let s: f64 = ED_ADMIT_MIX.iter().map(|(_, p)| p).sum();
        assert!((s - 1.0).abs() < 1e-9);
    }

    #[test]
    fn weekend_epoch_convention() {
        // Day 0 = Monday ⇒ days 5,6 (Sat/Sun) are the weekend.
        assert!(!is_weekend(0.0));
        assert!(is_weekend(5.0 * 1440.0));
        assert!(is_weekend(6.9 * 1440.0));
        assert!(!is_weekend(7.0 * 1440.0));
    }

    /// EP-21 backdating stamps negative sim minutes — the time helpers must
    /// keep wrapping sanely before t0 (`rem_euclid`, not `%`).
    #[test]
    fn negative_minutes_wrap_sanely() {
        // 90 min before Monday 00:00 is Sunday 22:30.
        assert_eq!(hour_of(-90.0), 22);
        assert!(is_weekend(-90.0));
        // A full week back is Monday again.
        assert_eq!(hour_of(-7.0 * 1440.0), 0);
        assert!(!is_weekend(-7.0 * 1440.0));
        // λ(t) stays finite and positive on negative minutes.
        let r = diurnal_rate_per_hr(6.0, &INPATIENT_ARRIVAL_SHAPE, 0.65, -30.0);
        assert!(r.is_finite() && r > 0.0);
    }

    #[test]
    fn length_biased_lifts_the_median_and_keeps_sigma() {
        let p = LosParams { median_hr: 96.0, sigma: 0.55 };
        let lb = p.length_biased();
        assert_eq!(lb.sigma, p.sigma);
        let expect = 96.0 * (0.55f64 * 0.55).exp();
        assert!((lb.median_hr - expect).abs() < 1e-9);
        assert!(lb.median_hr > p.median_hr, "long stays must be overrepresented");
    }

    #[test]
    fn los_lookup_prefers_line_then_type() {
        let line: ServiceLineId = "line.oncology".into();
        assert_eq!(los_for(Some(&line), UnitType::MedSurg).median_hr, 150.0);
        let unknown: ServiceLineId = "line.does_not_exist".into();
        assert_eq!(
            los_for(Some(&unknown), UnitType::Icu),
            los_fallback(UnitType::Icu)
        );
        assert_eq!(los_for(None, UnitType::Obs).median_hr, 24.0);
    }
}
