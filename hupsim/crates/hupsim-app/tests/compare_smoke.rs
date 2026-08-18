//! EP-15 headless acceptance smoke: the real compiled hospital, a 24 h
//! warm-up under the EP-10 stack (filling the rolling norms exactly like the
//! live app), then the brief's two acceptance runs executed the way the
//! compare tab's Run button does — `run_experiment` on a clone with the live
//! norms:
//!
//! 1. Close an ICU for 48 h against baseline — boarding rises in the
//!    intervention line with a stated (positive) peak delta.
//! 2. Flex a med-surg unit's RN ratio — workload relief with census lines
//!    unchanged.

use hupsim_core::index::HospitalIndex;
use hupsim_core::model::UnitType;
use hupsim_sim::experiment::{run_experiment, summarize, ExperimentSpec, Intervention, SeriesKey};
use hupsim_sim::prelude::*;

/// The live-app stack exactly as `main.rs` builds it: compiled hospital,
/// seeded demo census (the chronic-full regime the owner's Run would start
/// from), EP-10 processes on, 24 sim-hours ticked at the sampling cadence to
/// fill the rolling norms.
fn warmed_world() -> (hupsim_core::model::Hospital, HospitalIndex, SimEngine) {
    let loaded = hupsim_data::load_default().expect("embedded data compiles");
    let mut h = loaded.hospital;
    let idx = HospitalIndex::build(&h);
    hupsim_data::seed_demo_census(&mut h, &idx, 0xC0FFEE, 0.0);
    let mut eng = SimEngine::new(0xC0FFEE);
    // SimDrive parity: standing-census exits first (EP-21), then the EP-10
    // stack — the warm-up runs in the breathing regime, not a saturating one.
    eng.register(Box::new(BacklogDischarges::default()), true);
    eng.register(Box::new(AcuityDrift::default()), true);
    eng.register(Box::new(DiurnalFlow::default()), true);
    eng.register(Box::new(EdPressure::default()), true);
    for _ in 0..96 {
        eng.tick(&mut h, &idx, 15.0);
    }
    (h, idx, eng)
}

#[test]
fn closing_an_icu_raises_boarding_with_a_stated_peak_delta() {
    let (h, _idx, eng) = warmed_world();

    // The unit the owner's acceptance run closes: the biggest general ICU.
    let icu = h
        .units
        .iter()
        .filter(|u| u.unit_type == UnitType::Icu)
        .max_by_key(|u| _idx.room_indices(&u.id).len())
        .expect("the compiled hospital has ICU units")
        .id
        .clone();

    let mut spec = ExperimentSpec::new(0xEB15, eng.clock.now_min, 48.0);
    spec.interventions.push(Intervention::CloseUnit {
        unit: icu.clone(),
        reason: "surge drill".into(),
    });
    let res = run_experiment(&h, Some(&eng.rolling), &spec);

    // The compare tab's headline: peak boarding, baseline vs intervention,
    // with a stated delta — and the delta must be positive.
    let base = summarize(&res.baseline.series.hospital, spec.sample_every_min);
    let var = summarize(&res.variant.series.hospital, spec.sample_every_min);
    let delta = var.peak_boarding - base.peak_boarding;
    assert!(
        delta > 0.0,
        "closing {icu} must raise peak boarding: baseline {} vs intervention {} (Δ{delta:+})",
        base.peak_boarding,
        var.peak_boarding
    );
    // The boarding line itself rises above the baseline line somewhere.
    let a = &res.baseline.series.hospital.boarding;
    let b = &res.variant.series.hospital.boarding;
    assert!(
        a.iter().zip(b).any(|(x, y)| y > x),
        "the intervention boarding line must rise above baseline"
    );

    // The closure holds: nobody is admitted into the closed unit for the
    // whole horizon (its census can only fall), and at no sample is the
    // closed unit fuller than its baseline self.
    let closed = &res.variant.series.units[&icu].census;
    assert!(
        closed.windows(2).all(|w| w[1] <= w[0]),
        "closed-unit census must never rise"
    );
    let open = &res.baseline.series.units[&icu].census;
    assert!(
        open.iter().zip(closed).all(|(o, c)| o >= c),
        "the closed unit can never be fuller than its baseline self"
    );
    // And the live world was never mutated by the run (session-flow rule):
    // no room of the closed unit carries the experiment's closure marker in
    // `h`. (Plain OOS can legitimately exist — the seeded census blocks a
    // sliver of beds for terminal cleans.)
    assert!(
        _idx.room_indices(&icu).iter().all(|&ri| {
            !matches!(
                &h.rooms[ri].status,
                hupsim_core::model::RoomStatus::OutOfService { reason } if reason.starts_with("closed")
            )
        }),
        "running an experiment must not close beds in the live scenario"
    );
}

#[test]
fn flexing_med_surg_ratio_relieves_workload_with_unchanged_census() {
    let (h, idx, eng) = warmed_world();

    // The busiest med-surg unit at warm-up end.
    let ms = h
        .units
        .iter()
        .filter(|u| u.unit_type == UnitType::MedSurg)
        .max_by_key(|u| {
            idx.room_indices(&u.id)
                .iter()
                .filter(|&&ri| h.rooms[ri].status.patient().is_some())
                .count()
        })
        .expect("the compiled hospital has med-surg units")
        .id
        .clone();

    // Flex to 2.0, not 2.5: since EP-23 the busiest med-surg is the 36-bed
    // 12 Campus oncology unit at ratio 4, and against ⌈census/ratio⌉ RN
    // allocation a 2.5 norm makes a 3-patient corridor run read as 1.2 on
    // the census term — the peak RN's composite can RISE even as everyone's
    // patient counts fall (allocation granularity, not physics). A 2.0 norm
    // divides the runs cleanly and the relief signal is unambiguous.
    let mut spec = ExperimentSpec::new(0xEB15, eng.clock.now_min, 48.0);
    spec.interventions.push(Intervention::FlexRatio {
        unit: ms.clone(),
        ratio: 2.0,
    });
    let res = run_experiment(&h, Some(&eng.rolling), &spec);

    // Census lines unchanged — staffing is not physics. Hospital-wide and
    // for the flexed unit itself.
    assert_eq!(
        res.baseline.series.hospital.census, res.variant.series.hospital.census,
        "ratio flex must not move the hospital census line"
    );
    assert_eq!(
        res.baseline.series.units[&ms].census, res.variant.series.units[&ms].census,
        "ratio flex must not move the unit census line"
    );
    assert_eq!(res.baseline.series.hospital.boarding, res.variant.series.hospital.boarding);

    // Workload relief on the flexed unit: the peak per-RN composite drops.
    let key = SeriesKey::Unit(ms.clone());
    let base = summarize(
        res.baseline.series.series(&key).unwrap(),
        spec.sample_every_min,
    );
    let var = summarize(
        res.variant.series.series(&key).unwrap(),
        spec.sample_every_min,
    );
    let (b, v) = (
        base.peak_workload.expect("staffed unit has workload"),
        var.peak_workload.expect("staffed unit has workload"),
    );
    assert!(
        v < b,
        "richer staffing on {ms} must relieve peak per-RN workload: {b:.2} → {v:.2}"
    );
}
