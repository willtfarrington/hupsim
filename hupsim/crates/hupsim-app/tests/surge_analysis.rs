//! EP-18 case-study analysis: the EP-15 surge experiments run headlessly and
//! printed for `docs/analyses/`
//! (`cargo test -p hupsim-app --test surge_analysis -- --nocapture`).
//!
//! Same warmed world as `compare_smoke` (the live-app stack at seed 0xC0FFEE,
//! 24 h of rolling-norm warm-up), then three A/B experiments at seed 0xEB15:
//!
//! 1. Close the largest ICU for 48 h — where does the displaced demand pool?
//! 2. Inject a +40 med-surg arrival wave over 6 h — what does a mass-casualty
//!    afternoon do to occupancy and boarding?
//! 3. Flex the busiest med-surg unit's RN ratio — which lever relieves
//!    workload, and what it deliberately cannot do (bed physics).
//!
//! The printed numbers are one deterministic realization per arm (the EP-15
//! design), not expectations — the study text carries that caveat.

use hupsim_core::index::HospitalIndex;
use hupsim_core::model::UnitType;
use hupsim_sim::experiment::{
    run_experiment, summarize, ExperimentSpec, Intervention, MetricSeries, SeriesSummary,
};
use hupsim_sim::prelude::*;

/// The live-app stack exactly as `main.rs` builds it (compare_smoke parity):
/// compiled hospital, seeded demo census, EP-10 processes on, 24 sim-hours
/// ticked at the sampling cadence to fill the rolling norms.
fn warmed_world() -> (hupsim_core::model::Hospital, HospitalIndex, SimEngine) {
    let loaded = hupsim_data::load_default().expect("embedded data compiles");
    let mut h = loaded.hospital;
    let idx = HospitalIndex::build(&h);
    hupsim_data::seed_demo_census(&mut h, &idx, 0xC0FFEE, 0.0);
    let mut eng = SimEngine::new(0xC0FFEE);
    eng.register(Box::new(BacklogDischarges::default()), true);
    eng.register(Box::new(AcuityDrift::default()), true);
    eng.register(Box::new(DiurnalFlow::default()), true);
    eng.register(Box::new(EdPressure::default()), true);
    for _ in 0..96 {
        eng.tick(&mut h, &idx, 15.0);
    }
    (h, idx, eng)
}

fn fmt_opt(v: Option<f32>) -> String {
    v.map_or_else(|| "  n/a".into(), |v| format!("{v:5.2}"))
}

fn print_summary(label: &str, base: &SeriesSummary, var: &SeriesSummary) {
    println!("{label} (baseline → variant, Δ):");
    println!(
        "  peak census      {:6.0} → {:6.0}  ({:+.0})",
        base.peak_census,
        var.peak_census,
        var.peak_census - base.peak_census
    );
    println!(
        "  peak boarding    {:6.0} → {:6.0}  ({:+.0})",
        base.peak_boarding,
        var.peak_boarding,
        var.peak_boarding - base.peak_boarding
    );
    println!(
        "  hours > 95% occ  {:6.1} → {:6.1}  ({:+.1})",
        base.hours_over_95,
        var.hours_over_95,
        var.hours_over_95 - base.hours_over_95
    );
    println!(
        "  peak workload    {:>6} → {:>6}",
        fmt_opt(base.peak_workload),
        fmt_opt(var.peak_workload)
    );
}

/// (peak boarding, hours after experiment start it occurred).
fn peak_boarding_at(s: &MetricSeries, t: &[f64], start_min: f64) -> (f32, f64) {
    s.boarding
        .iter()
        .zip(t)
        .fold((0.0f32, 0.0f64), |(bm, tm), (&b, &at)| {
            if b > bm {
                (b, (at - start_min) / 60.0)
            } else {
                (bm, tm)
            }
        })
}

#[test]
fn surge_case_study_analysis() {
    let (h, idx, eng) = warmed_world();
    let start = eng.clock.now_min;

    // ------------------------------------------------------------------
    // Experiment 1 — close the largest ICU for 48 h (compare_smoke's unit
    // selection, duplicated here — keep the two aligned when either moves).
    // ------------------------------------------------------------------
    let icu = h
        .units
        .iter()
        .filter(|u| u.unit_type == UnitType::Icu)
        .max_by_key(|u| idx.room_indices(&u.id).len())
        .expect("the compiled hospital has ICU units")
        .id
        .clone();

    let mut spec = ExperimentSpec::new(0xEB15, start, 48.0);
    spec.interventions.push(Intervention::CloseUnit {
        unit: icu.clone(),
        reason: "surge drill".into(),
    });
    println!("== Experiment 1: {} ==", spec.describe(&h));
    let res = run_experiment(&h, Some(&eng.rolling), &spec);
    let o = &res.variant.close_outcomes[0];
    println!(
        "staging outcome: {} beds closed, {} occupants evacuated, {} stranded in place",
        o.closed, o.evacuated, o.stranded
    );
    let base = summarize(&res.baseline.series.hospital, spec.sample_every_min);
    let var = summarize(&res.variant.series.hospital, spec.sample_every_min);
    print_summary("hospital", &base, &var);
    assert!(
        var.peak_boarding > base.peak_boarding,
        "closing {icu} must raise peak boarding"
    );

    println!("peak boarding by level of care (baseline → variant, peak hour in variant):");
    for t in UnitType::ALL {
        let b = &res.baseline.series.unit_types[t.index()];
        let v = &res.variant.series.unit_types[t.index()];
        let (bp, _) = peak_boarding_at(b, &res.baseline.series.t_min, start);
        let (vp, vh) = peak_boarding_at(v, &res.variant.series.t_min, start);
        if bp > 0.0 || vp > 0.0 {
            println!(
                "  {:12}  {:5.0} → {:5.0}  (t+{:.0} h)",
                t.label(),
                bp,
                vp,
                vh
            );
        }
    }
    println!("hours > 95% occupancy by level of care (baseline → variant):");
    for t in UnitType::ALL {
        let b = summarize(
            &res.baseline.series.unit_types[t.index()],
            spec.sample_every_min,
        );
        let v = summarize(
            &res.variant.series.unit_types[t.index()],
            spec.sample_every_min,
        );
        if b.hours_over_95 > 0.0 || v.hours_over_95 > 0.0 {
            println!(
                "  {:12}  {:5.1} → {:5.1}",
                t.label(),
                b.hours_over_95,
                v.hours_over_95
            );
        }
    }

    // ------------------------------------------------------------------
    // Experiment 2 — +40 med-surg arrivals over 6 h, starting t+2 h (the
    // same shock size the EP-17 calibration surge test injects, so the two
    // studies cross-reference one scenario).
    // ------------------------------------------------------------------
    let mut spec2 = ExperimentSpec::new(0xEB15, start, 48.0);
    spec2.interventions.push(Intervention::ArrivalWave {
        start_hr: 2.0,
        duration_hr: 6.0,
        count: 40,
        need: UnitType::MedSurg,
    });
    println!("\n== Experiment 2: {} ==", spec2.describe(&h));
    let res2 = run_experiment(&h, Some(&eng.rolling), &spec2);
    println!(
        "wave outcome: {} admissions placed, {} still unplaced at run end",
        res2.variant.wave_injected, res2.variant.wave_backlog
    );
    assert_eq!(
        res2.variant.wave_injected + res2.variant.wave_backlog,
        40,
        "wave demand is conserved: placed + backlog = staged"
    );
    let base2 = summarize(&res2.baseline.series.hospital, spec2.sample_every_min);
    let var2 = summarize(&res2.variant.series.hospital, spec2.sample_every_min);
    print_summary("hospital", &base2, &var2);
    let bm = summarize(
        &res2.baseline.series.unit_types[UnitType::MedSurg.index()],
        spec2.sample_every_min,
    );
    let vm = summarize(
        &res2.variant.series.unit_types[UnitType::MedSurg.index()],
        spec2.sample_every_min,
    );
    print_summary("med-surg only", &bm, &vm);

    // ------------------------------------------------------------------
    // Experiment 3 — flex the busiest med-surg unit's RN ratio to 2.0
    // (compare_smoke's relief lever; ratio choice explained there).
    // ------------------------------------------------------------------
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
    let mut spec3 = ExperimentSpec::new(0xEB15, start, 48.0);
    spec3.interventions.push(Intervention::FlexRatio {
        unit: ms.clone(),
        ratio: 2.0,
    });
    println!("\n== Experiment 3: {} ==", spec3.describe(&h));
    let res3 = run_experiment(&h, Some(&eng.rolling), &spec3);
    let key = hupsim_sim::experiment::SeriesKey::Unit(ms.clone());
    let b3 = summarize(
        res3.baseline.series.series(&key).unwrap(),
        spec3.sample_every_min,
    );
    let v3 = summarize(
        res3.variant.series.series(&key).unwrap(),
        spec3.sample_every_min,
    );
    print_summary("flexed unit", &b3, &v3);
    assert_eq!(
        res3.baseline.series.hospital.census, res3.variant.series.hospital.census,
        "staffing is not physics: the census line must not move"
    );
    println!("census/boarding lines: identical in both arms (staffing is not physics)");
    assert!(
        v3.peak_workload.unwrap() < b3.peak_workload.unwrap(),
        "richer staffing must relieve peak per-RN workload"
    );
}
