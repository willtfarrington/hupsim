//! Deterministic sampling helpers for the flow processes: log-normal draws
//! (Box–Muller over the seeded ChaCha8 — no extra dependency), weighted
//! hour-of-day picks, discharge-time shaping, and the synthetic-patient
//! factory shared by [`crate::diurnal`] and [`crate::edflow`].

use crate::params::{self, LosParams};
use hupsim_core::model::UnitType;
use hupsim_core::patient::{Acuity, Patient};
use rand::Rng;
use rand_chacha::ChaCha8Rng;

/// Synthetic surnames for patient aliases — never real PHI.
pub(crate) const SURNAMES: [&str; 24] = [
    "Alvarez", "Brooks", "Chen", "Diaz", "Evans", "Foster", "Garcia", "Huang", "Ibrahim",
    "Johnson", "Kim", "Lopez", "Murray", "Nguyen", "Okafor", "Patel", "Quinn", "Rivera",
    "Singh", "Thompson", "Ueda", "Vasquez", "Williams", "Zhao",
];

/// Standard normal via Box–Muller (two uniform draws, always exactly two —
/// keep the RNG-consumption pattern fixed for determinism reasoning).
pub fn standard_normal(rng: &mut ChaCha8Rng) -> f64 {
    let u1: f64 = rng.random_range(f64::MIN_POSITIVE..1.0);
    let u2: f64 = rng.random_range(0.0..1.0);
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Log-normal duration in hours: `median · exp(σ·Z)`.
pub fn lognormal_hours(rng: &mut ChaCha8Rng, p: &LosParams) -> f64 {
    p.median_hr * (p.sigma * standard_normal(rng)).exp()
}

/// Pick an hour 0..24 with probability proportional to `weights`.
pub fn weighted_hour(rng: &mut ChaCha8Rng, weights: &[f64; 24]) -> usize {
    let total: f64 = weights.iter().sum();
    let mut x: f64 = rng.random_range(0.0..total);
    for (h, w) in weights.iter().enumerate() {
        if x < *w {
            return h;
        }
        x -= w;
    }
    23
}

/// Push a tentative discharge instant onto the mid-day discharge curve:
/// sample an hour from [`params::DISCHARGE_HOUR_WEIGHTS`], land on that hour
/// of the tentative day (or the next day if that slot already passed).
/// Monotone: never earlier than `tentative_min`, at most ~24 h later — LOS
/// scale is preserved, census gets its "discharge by noon" dip.
pub fn shape_discharge_min(rng: &mut ChaCha8Rng, tentative_min: f64) -> f64 {
    let h = weighted_hour(rng, &params::DISCHARGE_HOUR_WEIGHTS) as f64;
    let day_start = (tentative_min / 1440.0).floor() * 1440.0;
    let t = day_start + h * 60.0 + rng.random_range(0.0..60.0);
    if t < tentative_min {
        t + 1440.0
    } else {
        t
    }
}

/// Bounded retries for the conditioned total-LOS resample below. Bounded so
/// RNG consumption per patient stays small and the deep tail (elapsed far
/// beyond the distribution) degrades to the caller's prompt-discharge
/// fallback instead of spinning.
pub const RESIDUAL_LOS_RETRIES: usize = 8;

/// Total-LOS resample conditioned above an elapsed stay (EP-21 standing-
/// census exits): redraw the service log-normal until it exceeds
/// `elapsed_hr`. `None` when `RESIDUAL_LOS_RETRIES` draws can't clear the
/// bound — the stay is deep in the tail and the caller should fall back to a
/// prompt, curve-shaped discharge.
pub fn residual_total_los_hours(
    rng: &mut ChaCha8Rng,
    p: &LosParams,
    elapsed_hr: f64,
) -> Option<f64> {
    for _ in 0..RESIDUAL_LOS_RETRIES {
        let total = lognormal_hours(rng, p);
        if total > elapsed_hr {
            return Some(total);
        }
    }
    None
}

/// Discharge instant for a patient already mid-stay at `now_min`: residual
/// total-LOS conditioned above the elapsed stay, shaped onto the mid-day
/// discharge curve; falls back to a prompt shaped discharge (within ~a day)
/// when the conditional resample can't clear the elapsed bound. Always
/// ≥ `now_min`.
pub fn residual_discharge_min(
    rng: &mut ChaCha8Rng,
    p: &LosParams,
    admit_min: f64,
    now_min: f64,
) -> f64 {
    let elapsed_hr = ((now_min - admit_min) / 60.0).max(0.0);
    match residual_total_los_hours(rng, p, elapsed_hr) {
        Some(total_hr) => shape_discharge_min(rng, admit_min + total_hr * 60.0),
        None => shape_discharge_min(rng, now_min),
    }
}

/// Initial instability for a new admission at a given level of care:
/// baseline-centered with a quadratic skew toward the stable side (most
/// arrivals are compensated; a tail is not).
pub fn draw_initial_instability(rng: &mut ChaCha8Rng, t: UnitType) -> f32 {
    let u: f32 = rng.random_range(0.0..1.0);
    let baseline = params::acuity_baseline(t);
    let spread = params::acuity_init_spread(t);
    (baseline + spread * (u * u * 2.0 - 0.5)).clamp(0.02, 0.95)
}

/// Build a synthetic patient. `id_prefix` namespaces the counter per process
/// ("adm", "ed") so ids never collide across processes.
pub fn synth_patient(
    rng: &mut ChaCha8Rng,
    id_prefix: &str,
    n: u64,
    now_min: f64,
    instability: f32,
    service: Option<String>,
    note: &str,
) -> Patient {
    let initial = (b'A' + rng.random_range(0..26u8)) as char;
    let surname = SURNAMES[rng.random_range(0..SURNAMES.len())];
    Patient {
        id: format!("{id_prefix}.pt.{n}").into(),
        alias: format!("{initial}. {surname}"),
        age: Some(rng.random_range(19..97)),
        service,
        admitted_at_min: Some(now_min),
        acuity: Acuity {
            instability,
            trend_per_hr: rng.random_range(-0.02..0.02),
        },
        boarding: false,
        isolation: None,
        telemetry: instability > 0.35,
        note: note.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn lognormal_median_lands_near_parameter() {
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        let p = LosParams {
            median_hr: 96.0,
            sigma: 0.55,
        };
        let mut xs: Vec<f64> = (0..2000).map(|_| lognormal_hours(&mut rng, &p)).collect();
        xs.sort_by(f64::total_cmp);
        let median = xs[xs.len() / 2];
        assert!(
            (median - 96.0).abs() < 96.0 * 0.15,
            "sample median {median} too far from 96"
        );
        assert!(xs.iter().all(|&x| x > 0.0));
    }

    #[test]
    fn discharge_shaping_is_monotone_and_midday_peaked() {
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        let mut by_hour = [0u32; 24];
        for i in 0..2000 {
            let tentative = (i as f64) * 61.3; // sweep across days and hours
            let shaped = shape_discharge_min(&mut rng, tentative);
            assert!(shaped >= tentative, "shaping must never shorten a stay");
            assert!(shaped - tentative <= 1440.0 + 60.0, "at most ~one day later");
            by_hour[params::hour_of(shaped)] += 1;
        }
        let peak = (0..24).max_by_key(|&h| by_hour[h]).unwrap();
        assert!(
            (10..=15).contains(&peak),
            "discharge peak hour {peak} not mid-day: {by_hour:?}"
        );
        let overnight: u32 = (0..6).map(|h| by_hour[h]).sum();
        let midday: u32 = (10..16).map(|h| by_hour[h]).sum();
        assert!(midday > overnight * 5);
    }

    #[test]
    fn residual_resample_conditions_above_elapsed() {
        let mut rng = ChaCha8Rng::seed_from_u64(21);
        let p = LosParams { median_hr: 96.0, sigma: 0.55 };
        // Typical elapsed: the resample must clear the bound when it returns.
        for elapsed in [0.0, 24.0, 96.0, 150.0] {
            for _ in 0..200 {
                if let Some(total) = residual_total_los_hours(&mut rng, &p, elapsed) {
                    assert!(total > elapsed, "conditioned draw {total} ≤ elapsed {elapsed}");
                }
            }
        }
        // Deep tail: e^(6σ) ≈ 27× the median is beyond 8 retries essentially
        // always — the bounded resample must give up, not spin.
        let deep = 96.0 * (6.0f64 * 0.55).exp();
        let fails = (0..200)
            .filter(|_| residual_total_los_hours(&mut rng, &p, deep).is_none())
            .count();
        assert!(fails >= 199, "deep-tail elapsed should exhaust the retries: {fails}/200");
    }

    #[test]
    fn residual_discharge_never_lands_in_the_past() {
        let mut rng = ChaCha8Rng::seed_from_u64(22);
        let p = LosParams { median_hr: 96.0, sigma: 0.55 };
        let now = 15.0;
        for i in 0..500 {
            // Sweep elapsed from fresh to well past the median (backdated
            // stamps are negative sim minutes).
            let admit = now - (i as f64) * 37.0;
            let at = residual_discharge_min(&mut rng, &p, admit, now);
            assert!(at >= now, "discharge at {at} predates now {now} (admit {admit})");
        }
        // Beyond-the-tail elapsed (≈27× the median): the bounded resample
        // gives up and the fallback keeps the discharge prompt — landed on
        // the shaped curve within ~a day of now.
        let deep_admit = now - 96.0 * (6.0f64 * 0.55).exp() * 60.0;
        for _ in 0..100 {
            let at = residual_discharge_min(&mut rng, &p, deep_admit, now);
            assert!(at >= now);
            assert!(at <= now + 2.0 * 1440.0, "deep-tail fallback should be prompt: {at}");
        }
    }

    #[test]
    fn initial_instability_respects_level_of_care() {
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let mean = |rng: &mut ChaCha8Rng, t: UnitType| -> f32 {
            (0..500).map(|_| draw_initial_instability(rng, t)).sum::<f32>() / 500.0
        };
        let icu = mean(&mut rng, UnitType::Icu);
        let ms = mean(&mut rng, UnitType::MedSurg);
        assert!(
            icu > ms + 0.15,
            "ICU arrivals ({icu}) should be sicker than med/surg ({ms})"
        );
    }
}
