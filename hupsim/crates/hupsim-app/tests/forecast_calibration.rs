//! EP-17 calibration replay — the honesty ledger behind the census fan.
//!
//! Replays the real compiled hospital from the seeded Tuesday (seed 42, the
//! census-regime smoke setup) for 8 sim-days, issues a 24 h forecast every
//! sim-hour through day 7, and scores median absolute error, bias, and 10–90
//! band coverage against realized census at 2/6/12/24 h horizons — split
//! into the day-1 settle-in (seeded-snapshot transient) and the settled
//! regime (days 2–7). A second replay injects an unforecastable surge and
//! *asserts the fan misses it* — the documented failure mode must stay true.
//!
//! `cargo test -p hupsim-app --test forecast_calibration -- --nocapture`
//! prints the markdown tables committed in `hupsim/CALIBRATION.md`; the
//! headline coverage number is pinned as
//! `hupsim_sim::forecast::FAN_COVERAGE_10_90_PCT` (quoted wherever the fan
//! is shown). If the model or the params change, regenerate both.

use hupsim_core::index::HospitalIndex;
use hupsim_core::patient::{Acuity, Patient};
use hupsim_sim::events::SimEventKind;
use hupsim_sim::forecast::{forecast_census, CensusFan, ForecastParams};
use hupsim_sim::prelude::*;

const TICK_MIN: f64 = 15.0;
const HORIZONS_MIN: [f64; 4] = [120.0, 360.0, 720.0, 1440.0];

fn engine_with_regime_processes(seed: u64) -> SimEngine {
    let mut eng = SimEngine::new(seed);
    eng.register(Box::new(BacklogDischarges::default()), true);
    eng.register(Box::new(AcuityDrift::default()), true);
    eng.register(Box::new(DiurnalFlow::default()), true);
    eng.register(Box::new(EdPressure::default()), true);
    eng
}

fn census_of(h: &hupsim_core::model::Hospital) -> u32 {
    h.rooms
        .iter()
        .filter(|r| r.status.patient().is_some())
        .count() as u32
}

/// Realized census at sim-minute `t` from the per-tick record (ticks end at
/// 15, 30, …).
fn realized(census_by_tick: &[u32], t_min: f64) -> u32 {
    let i = (t_min / TICK_MIN).round() as usize - 1;
    census_by_tick[i]
}

#[derive(Default)]
struct Score {
    n: usize,
    hits: usize,
    abs_err: f64,
    bias: f64,
}

impl Score {
    fn add(&mut self, fan: &CensusFan, k: usize, real: u32) {
        self.n += 1;
        let real = real as f64;
        if (fan.q10[k] as f64) <= real && real <= (fan.q90[k] as f64) {
            self.hits += 1;
        }
        self.abs_err += (fan.median[k] as f64 - real).abs();
        self.bias += fan.median[k] as f64 - real;
    }

    fn coverage_pct(&self) -> f64 {
        100.0 * self.hits as f64 / self.n.max(1) as f64
    }

    fn mae(&self) -> f64 {
        self.abs_err / self.n.max(1) as f64
    }

    fn mean_bias(&self) -> f64 {
        self.bias / self.n.max(1) as f64
    }
}

#[test]
fn regime_calibration_table() {
    let loaded = hupsim_data::load_default().expect("embedded data compiles");
    let mut h = loaded.hospital;
    let idx = HospitalIndex::build(&h);
    hupsim_data::seed_demo_census(&mut h, &idx, 42, 0.0);
    let mut eng = engine_with_regime_processes(42);

    let fp = ForecastParams {
        horizon_min: 1440.0,
        ..ForecastParams::default()
    };

    // 8 sim-days; forecasts issued hourly through day 7 so every issue can
    // be scored a full 24 h ahead.
    let mut census_by_tick: Vec<u32> = Vec::new();
    let mut issues: Vec<CensusFan> = Vec::new();
    for tick in 0..(8 * 96) {
        eng.tick(&mut h, &idx, TICK_MIN);
        census_by_tick.push(census_of(&h));
        let t = (tick + 1) as f64 * TICK_MIN;
        // Hourly, and only while a +24 h scoring target still exists in-run.
        if t % 60.0 == 0.0 && t + 1440.0 <= 8.0 * 1440.0 {
            issues.push(forecast_census(&h, &idx, t, &fp));
        }
    }

    // Score, split into the day-1 settle-in and the settled regime.
    let mut day1 = [Score::default(), Score::default(), Score::default(), Score::default()];
    let mut regime = [Score::default(), Score::default(), Score::default(), Score::default()];
    for fan in &issues {
        for (hi, h_min) in HORIZONS_MIN.iter().enumerate() {
            let k = (h_min / fan.step_min).round() as usize;
            let real = realized(&census_by_tick, fan.issued_at_min + h_min);
            let seg = if fan.issued_at_min < 1440.0 {
                &mut day1
            } else {
                &mut regime
            };
            seg[hi].add(fan, k, real);
        }
    }

    println!("\n### Calibration — seeded start, seed 42, 8 sim-days, hourly 24 h forecasts\n");
    println!("| segment | horizon | n | 10–90 coverage | MAE (beds) | bias (beds) |");
    println!("|---|---|---|---|---|---|");
    for (name, seg) in [("day 1 (settle-in)", &day1), ("days 2–7 (regime)", &regime)] {
        for (hi, h_min) in HORIZONS_MIN.iter().enumerate() {
            let s = &seg[hi];
            println!(
                "| {name} | {:>2.0} h | {} | {:.0}% | {:.1} | {:+.1} |",
                h_min / 60.0,
                s.n,
                s.coverage_pct(),
                s.mae(),
                s.mean_bias(),
            );
        }
    }
    println!(
        "\nheadline (regime, 12 h): coverage {:.0}% — pinned as FAN_COVERAGE_10_90_PCT\n",
        regime[2].coverage_pct()
    );

    // Regression guards, deliberately loose: the point is the committed
    // table, not brittle numbers. Nominal band coverage is 80%; Poisson
    // arrival variance errs conservative (see forecast.rs), so measured
    // coverage may sit above nominal — but must not collapse.
    for (hi, h_min) in HORIZONS_MIN.iter().enumerate() {
        let s = &regime[hi];
        assert!(
            s.coverage_pct() >= 65.0,
            "regime {}h coverage collapsed: {:.0}%",
            h_min / 60.0,
            s.coverage_pct()
        );
    }
    assert!(
        regime[1].mae() < 12.0,
        "regime 6 h MAE {:.1} beds looks broken",
        regime[1].mae()
    );
    assert!(
        regime[3].mean_bias().abs() < 20.0,
        "regime 24 h bias {:+.1} beds looks broken",
        regime[3].mean_bias()
    );
    // The pinned headline number must stay in step with the measurement.
    let pinned = hupsim_sim::forecast::FAN_COVERAGE_10_90_PCT as f64;
    assert!(
        (regime[2].coverage_pct() - pinned).abs() <= 3.0,
        "measured regime 12 h coverage {:.0}% drifted from the pinned {pinned:.0}% — \
         re-run with --nocapture, update FAN_COVERAGE_10_90_PCT and CALIBRATION.md",
        regime[2].coverage_pct()
    );
}

/// The documented failure mode, kept true by test: an injected surge the
/// generator's tables know nothing about must escape the band — the fan is
/// a baseline, not an oracle, and its calibration notes say exactly this.
#[test]
fn surge_injection_escapes_the_band() {
    let loaded = hupsim_data::load_default().expect("embedded data compiles");
    let mut h = loaded.hospital;
    let idx = HospitalIndex::build(&h);
    hupsim_data::seed_demo_census(&mut h, &idx, 42, 0.0);
    let mut eng = engine_with_regime_processes(42);

    // Settle two days, then issue the fan at day 3, 08:00.
    let issue_at = 2.0 * 1440.0 + 8.0 * 60.0;
    while eng.clock.now_min < issue_at {
        eng.tick(&mut h, &idx, TICK_MIN);
    }
    let fan = forecast_census(
        &h,
        &idx,
        eng.clock.now_min,
        &ForecastParams {
            horizon_min: 1440.0,
            ..ForecastParams::default()
        },
    );

    // 12:00: a 40-admission surge (bus crash, regional diversion) lands over
    // one hour — pure event-queue injection, invisible to the λ tables.
    let surge_at = 2.0 * 1440.0 + 12.0 * 60.0;
    while eng.clock.now_min < surge_at {
        eng.tick(&mut h, &idx, TICK_MIN);
    }
    let vacant: Vec<_> = h
        .rooms
        .iter()
        .filter(|r| {
            r.status.is_vacant()
                && idx
                    .unit_pos
                    .get(&r.unit)
                    .is_some_and(|&up| {
                        hupsim_sim::params::is_inpatient_unit(h.units[up].unit_type)
                    })
        })
        .map(|r| r.id.clone())
        .take(40)
        .collect();
    assert!(vacant.len() >= 30, "need vacancy headroom for the surge");
    for (i, room) in vacant.iter().enumerate() {
        eng.queue.schedule(
            eng.clock.now_min + 1.0 + (i as f64 % 60.0),
            SimEventKind::Admission {
                room: room.clone(),
                patient: Box::new(Patient {
                    id: format!("surge.{i}").into(),
                    alias: format!("Surge {i}"),
                    age: Some(45),
                    service: Some("mass casualty".into()),
                    admitted_at_min: Some(eng.clock.now_min),
                    acuity: Acuity {
                        instability: 0.5,
                        trend_per_hr: 0.0,
                    },
                    boarding: false,
                    isolation: None,
                    telemetry: true,
                    note: "calibration surge injection".into(),
                }),
            },
        );
    }

    // Track realized census out to the fan's 24 h horizon.
    let mut breach = None;
    while eng.clock.now_min < issue_at + 1440.0 {
        eng.tick(&mut h, &idx, TICK_MIN);
        let t = eng.clock.now_min;
        let k = ((t - issue_at) / fan.step_min).round() as usize;
        if k < fan.q90.len() {
            let real = census_of(&h) as f32;
            if real > fan.q90[k] && breach.is_none() {
                breach = Some((t - issue_at, real, fan.q90[k]));
            }
        }
    }
    let (h_min, real, q90) = breach.expect(
        "a 40-admission surge must escape the 10–90 band — if it stops doing so, \
         the CALIBRATION.md failure notes are stale",
    );
    println!(
        "\nsurge escape: realized {real:.0} beds crossed q90 {q90:.0} at +{:.1} h after issue\n",
        h_min / 60.0
    );
    // Sanity: the pre-surge portion of the fan was not already breached at
    // the same magnitude — the escape is the surge's doing.
    assert!(h_min / 60.0 > 3.9, "breach should follow the 12:00 surge, not precede it");
}
