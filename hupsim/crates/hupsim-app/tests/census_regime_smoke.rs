//! EP-21 acceptance, headless: a seeded start must *breathe*, not saturate.
//!
//! Before EP-21 the seeded census was immortal (no admit stamps ⇒ no
//! scheduled exits) and the inpatient house pegged the 100% rail within a
//! day. With backdated stamps + `BacklogDischarges` residual exits + the
//! recalibrated arrival base (params.rs), a multi-day run from the seeded
//! Tuesday must hold the intended ~85% chronic-full regime: off the rail,
//! visibly waving intra-day.

use hupsim_core::index::HospitalIndex;
use hupsim_core::matrix::RoomMatrix;
use hupsim_sim::params;
use hupsim_sim::prelude::*;

#[test]
fn seeded_start_breathes_off_the_full_rail() {
    let loaded = hupsim_data::load_default().expect("embedded data compiles");
    let mut h = loaded.hospital;
    let idx = HospitalIndex::build(&h);
    hupsim_data::seed_demo_census(&mut h, &idx, 42, 0.0);

    let mut eng = SimEngine::new(42);
    eng.register(Box::new(BacklogDischarges::default()), true);
    eng.register(Box::new(AcuityDrift::default()), true);
    eng.register(Box::new(DiurnalFlow::default()), true);
    eng.register(Box::new(EdPressure::default()), true);

    // Inpatient occupancy (census / staffed) per 15-min tick over 8 sim days
    // — a full week plus the Monday after, so the weekend dip and the
    // weekday refill are both in frame.
    let mut occ_by_tick: Vec<(f64, f64)> = Vec::new();
    let mut m = RoomMatrix::default();
    for _ in 0..(8 * 96) {
        eng.tick(&mut h, &idx, 15.0);
        m.refresh(&h, &idx);
        let (mut census, mut staffed) = (0usize, 0usize);
        for (t, agg) in m.rollup_by_unit_type() {
            if params::is_inpatient_unit(t) {
                census += agg.census;
                staffed += agg.capacity - agg.out_of_service;
            }
        }
        occ_by_tick.push((eng.clock.now_min, census as f64 / staffed as f64));
    }

    // Off the rail, chronically full: every sample from day 2 on sits in the
    // breathing band. (Day 1 is the settle-in from the seeded snapshot.)
    let settled: Vec<&(f64, f64)> =
        occ_by_tick.iter().filter(|(t, _)| *t >= 1440.0).collect();
    let peak = settled.iter().map(|(_, o)| *o).fold(0.0, f64::max);
    let floor = settled.iter().map(|(_, o)| *o).fold(1.0, f64::min);
    let mean = settled.iter().map(|(_, o)| *o).sum::<f64>() / settled.len() as f64;
    assert!(
        peak < 0.985,
        "inpatient occupancy must stay off the 100% rail, peaked at {peak:.3}"
    );
    assert!(
        (0.75..0.92).contains(&mean),
        "settled mean occupancy {mean:.3} should read chronic-full (~85%)"
    );
    assert!(floor > 0.65, "the house must not drain either: floor {floor:.3}");

    // The weekend breathes: Saturday–Sunday (sim days 5–6, Monday epoch)
    // must dip visibly below the weekday mean.
    let weekend_min = occ_by_tick
        .iter()
        .filter(|(t, _)| (5.0 * 1440.0..7.0 * 1440.0).contains(t))
        .map(|(_, o)| *o)
        .fold(1.0, f64::min);
    assert!(
        weekend_min < mean - 0.03,
        "the weekend should dip below the weekday regime: min {weekend_min:.3} vs mean {mean:.3}"
    );

    // Breathing: each settled day shows a visible intra-day swing.
    for day in 1..8 {
        let day_vals: Vec<f64> = occ_by_tick
            .iter()
            .filter(|(t, _)| (*t / 1440.0).floor() as usize == day)
            .map(|(_, o)| *o)
            .collect();
        let (dmin, dmax) = (
            day_vals.iter().copied().fold(1.0, f64::min),
            day_vals.iter().copied().fold(0.0, f64::max),
        );
        assert!(
            dmax - dmin >= 0.015,
            "day {day} should breathe visibly: swing {:.4}",
            dmax - dmin
        );
    }
}
