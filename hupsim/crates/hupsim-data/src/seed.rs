//! Deterministic demo census: fill the hospital to realistic occupancy with
//! synthetic patients so the app has something to show at first launch.
//!
//! EP-21: the snapshot is **mid-stream** — every seeded patient carries a
//! synthetic backdated admit time drawn from the same EP-10 LOS tables the
//! live flows use (`hupsim_sim::params`, all synthetic/inferred). Inpatients
//! draw a **length-biased** total LOS (stays in progress at a random instant
//! overrepresent long stays; for a log-normal that is the same σ with
//! median × e^(σ²)) and sit a uniform fraction of the way through it. ED
//! occupants backdate on the episode clock — hours, not days; boarders have
//! fully used a door-to-decision draw plus part of a boarding wait, so day-0
//! boarding waits read as a plausible few hours. Exits for this standing
//! census are scheduled by `hupsim_sim::backlog::BacklogDischarges`.

use hupsim_core::index::HospitalIndex;
use hupsim_core::model::{Hospital, RoomStatus, UnitType};
use hupsim_core::patient::{Acuity, Patient};
use hupsim_sim::params::{self, LosParams};
use hupsim_sim::sampling;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const SURNAMES: [&str; 24] = [
    "Alvarez", "Brooks", "Chen", "Diaz", "Evans", "Foster", "Garcia", "Huang", "Ibrahim",
    "Johnson", "Kim", "Lopez", "Murray", "Nguyen", "Okafor", "Patel", "Quinn", "Rivera",
    "Singh", "Thompson", "Ueda", "Vasquez", "Williams", "Zhao",
];

fn occupancy_target(ut: UnitType) -> f64 {
    match ut {
        UnitType::MedSurg => 0.87,
        UnitType::Icu => 0.85,
        UnitType::Stepdown => 0.85,
        UnitType::Ed => 0.55,
        UnitType::Obs => 0.5,
        UnitType::Psych => 0.9,
        UnitType::Detox => 0.8,
        UnitType::Research => 0.4,
        UnitType::LaborDelivery => 0.5,
        UnitType::Nursery => 0.7,
        UnitType::Other => 0.6,
    }
}

/// Mean instability by unit type — ICUs run sicker.
fn instability_base(ut: UnitType) -> f32 {
    match ut {
        UnitType::Icu => 0.55,
        UnitType::Stepdown => 0.35,
        UnitType::Ed => 0.4,
        UnitType::Nursery => 0.3,
        _ => 0.18,
    }
}

/// Synthetic elapsed stay for a patient found mid-stream (EP-21): a uniform
/// fraction of a length-biased total-LOS draw, in minutes.
fn elapsed_in_progress_min(rng: &mut ChaCha8Rng, los: &LosParams) -> f64 {
    let total_hr = sampling::lognormal_hours(rng, &los.length_biased());
    let u: f64 = rng.random_range(0.0..1.0);
    u * total_hr * 60.0
}

fn synth_patient(
    rng: &mut ChaCha8Rng,
    n: u64,
    ut: UnitType,
    service: &str,
    los: &LosParams,
    now_min: f64,
) -> Patient {
    let initial = (b'A' + rng.random_range(0..26u8)) as char;
    let surname = SURNAMES[rng.random_range(0..SURNAMES.len())];
    let base = instability_base(ut);
    // Beta-ish: average of two uniforms around the base, clamped.
    let u1: f32 = rng.random_range(0.0..1.0);
    let u2: f32 = rng.random_range(0.0..1.0);
    let instability = (base + (u1 + u2 - 1.0) * 0.35).clamp(0.01, 0.98);
    let boarding = ut == UnitType::Ed && rng.random_bool(0.6);
    // Backdated admit stamp (EP-21): episode-scale for ED bays, length-biased
    // service LOS everywhere else. All synthetic, like every other number in
    // params.rs.
    let elapsed_min = match ut {
        UnitType::Ed if boarding => {
            // The door-to-decision episode has fully elapsed (that's what
            // made them a boarder), plus a partial boarding wait.
            let decision_hr = sampling::lognormal_hours(rng, &params::ED_DECISION_TO_ADMIT);
            decision_hr * 60.0 + elapsed_in_progress_min(rng, &params::ED_BOARDING_WAIT)
        }
        UnitType::Ed => elapsed_in_progress_min(rng, &params::ED_TREAT_RELEASE_LOS),
        _ => elapsed_in_progress_min(rng, los),
    };
    // Ids are salted by the snapshot instant: a mid-run re-seed (after
    // "Clear census") would otherwise recreate the exact (room, id) pairs of
    // the t0 cohort, and still-pending guarded exits queued for the old
    // cohort would pass their `expect_patient` check against the new one.
    let id = if now_min == 0.0 {
        format!("seed.pt.{n}")
    } else {
        format!("seed.t{now_min:.0}.pt.{n}")
    };
    Patient {
        id: id.into(),
        alias: format!("{initial}. {surname}"),
        age: Some(rng.random_range(19..97)),
        service: Some(service.to_string()),
        admitted_at_min: Some(now_min - elapsed_min),
        acuity: Acuity {
            instability,
            trend_per_hr: rng.random_range(-0.015..0.015),
        },
        boarding,
        isolation: if rng.random_bool(0.07) {
            Some("Contact".into())
        } else {
            None
        },
        telemetry: instability > 0.3 || rng.random_bool(0.2),
        note: "synthetic seed patient".into(),
    }
}

/// Fill vacant rooms to per-unit-type occupancy targets. Deterministic per
/// seed: same seed and `now_min` ⇒ identical census, stamps included.
/// `now_min` is the sim minute the snapshot is taken at (0 for app startup;
/// the toolbar's re-seed passes the current clock so backdated stamps stay
/// relative to "now").
pub fn seed_demo_census(h: &mut Hospital, idx: &HospitalIndex, seed: u64, now_min: f64) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut counter: u64 = 0;
    let units: Vec<_> = h
        .units
        .iter()
        .map(|u| (u.id.clone(), u.unit_type, u.service.clone(), u.service_line.clone()))
        .collect();
    for (uid, ut, service, line) in units {
        let target = occupancy_target(ut);
        let los = params::los_for(line.as_ref(), ut);
        for &ri in idx.room_indices(&uid) {
            let room = &mut h.rooms[ri];
            if !room.status.is_vacant() {
                continue;
            }
            // A sliver of beds are blocked for cleaning/maintenance.
            if rng.random_bool(0.02) {
                room.status = RoomStatus::OutOfService {
                    reason: "terminal clean".into(),
                };
                continue;
            }
            if rng.random_bool(target) {
                counter += 1;
                // synth_patient already draws the ~60% ED boarding mix —
                // an ED holds ED patients AND admitted boarders, not 100% boarders.
                let p = synth_patient(&mut rng, counter, ut, &service, &los, now_min);
                room.status = RoomStatus::Occupied { patient: p };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile;
    use crate::io::embedded;
    use hupsim_core::aggregate::hospital_stats;

    fn seeded(seed: u64, now_min: f64) -> (Hospital, HospitalIndex) {
        let (t, u, l, s) = embedded();
        let (mut h, _) = compile(t, u, l, s).unwrap();
        let idx = HospitalIndex::build(&h);
        seed_demo_census(&mut h, &idx, seed, now_min);
        (h, idx)
    }

    #[test]
    fn seeding_is_deterministic_and_plausible() {
        let (h1, _) = seeded(99, 0.0);
        let (h2, _) = seeded(99, 0.0);
        assert_eq!(h1, h2, "same seed must produce the same census, stamps included");

        let stats = hospital_stats(&h1);
        let frac = stats.census as f64 / stats.capacity as f64;
        assert!(
            (0.6..0.95).contains(&frac),
            "hospital-wide occupancy should look like a real Tuesday: {frac}"
        );
        assert!(stats.boarding > 0, "ED should hold boarders");
    }

    /// EP-21: every seeded patient is backdated — stamps are strictly before
    /// the snapshot instant, episode-scale in the ED, days-scale on the
    /// wards.
    #[test]
    fn stamps_are_backdated_with_sane_spreads() {
        let (h, idx) = seeded(99, 0.0);

        let mut ed_elapsed_hr: Vec<f64> = Vec::new();
        let mut ward_elapsed_hr: Vec<f64> = Vec::new();
        for room in &h.rooms {
            let Some(p) = room.status.patient() else {
                continue;
            };
            let admit = p
                .admitted_at_min
                .expect("every seeded patient carries a synthetic admit stamp");
            assert!(admit < 0.0, "a t0 snapshot must stamp strictly before t0: {admit}");
            let elapsed_hr = -admit / 60.0;
            let ut = h.units[idx.unit_pos[&room.unit]].unit_type;
            if ut == UnitType::Ed {
                ed_elapsed_hr.push(elapsed_hr);
            } else if params::is_inpatient_unit(ut) {
                ward_elapsed_hr.push(elapsed_hr);
            }
        }

        // ED stays are episode-scale: hours, not days.
        let ed_mean = ed_elapsed_hr.iter().sum::<f64>() / ed_elapsed_hr.len() as f64;
        assert!(
            ed_elapsed_hr.iter().all(|&e| e < 96.0),
            "ED elapsed must stay episode-scale"
        );
        assert!((1.0..24.0).contains(&ed_mean), "ED mean elapsed {ed_mean} h");

        // Ward stays are days-scale, length-biased: the mean elapsed sits
        // near half the size-biased mean LOS (≈ 60–90 h), far above the ED.
        let ward_mean = ward_elapsed_hr.iter().sum::<f64>() / ward_elapsed_hr.len() as f64;
        assert!(
            (24.0..168.0).contains(&ward_mean),
            "ward mean elapsed {ward_mean} h should read as days"
        );
        assert!(ward_mean > 3.0 * ed_mean, "wards must dwarf the ED episode scale");
        // The long tail exists (length bias oversamples long stays)…
        assert!(
            ward_elapsed_hr.iter().any(|&e| e > 150.0),
            "length-biased draws should produce week-plus stays"
        );
        // …and so do fresh admits (uniform elapsed fraction).
        assert!(
            ward_elapsed_hr.iter().any(|&e| e < 24.0),
            "uniform in-progress fractions should produce fresh stays"
        );
    }

    /// Boarder waits must read as a plausible few hours: longer than the
    /// treat-and-release crowd on average, bounded to the episode scale.
    #[test]
    fn seeded_boarders_wait_hours() {
        let (h, idx) = seeded(99, 0.0);
        let mut boarder_hr: Vec<f64> = Vec::new();
        let mut nonboarder_hr: Vec<f64> = Vec::new();
        for room in &h.rooms {
            let Some(p) = room.status.patient() else {
                continue;
            };
            if h.units[idx.unit_pos[&room.unit]].unit_type != UnitType::Ed {
                continue;
            }
            let elapsed_hr = -p.admitted_at_min.unwrap() / 60.0;
            if p.boarding {
                boarder_hr.push(elapsed_hr);
            } else {
                nonboarder_hr.push(elapsed_hr);
            }
        }
        assert!(!boarder_hr.is_empty() && !nonboarder_hr.is_empty());
        let bmean = boarder_hr.iter().sum::<f64>() / boarder_hr.len() as f64;
        let nmean = nonboarder_hr.iter().sum::<f64>() / nonboarder_hr.len() as f64;
        assert!(
            bmean > nmean,
            "boarders ({bmean:.1} h) have been in the ED longer than walk-ins ({nmean:.1} h)"
        );
        assert!((3.0..24.0).contains(&bmean), "boarder mean {bmean:.1} h reads as hours");
    }

    /// A mid-run re-seed (toolbar) stamps relative to the current clock, so
    /// elapsed stays honest instead of inflating by the sim time so far.
    #[test]
    fn reseed_mid_run_stamps_relative_to_now() {
        let now = 5000.0;
        let (h, _) = seeded(99, now);
        for room in &h.rooms {
            if let Some(p) = room.status.patient() {
                let admit = p.admitted_at_min.unwrap();
                assert!(admit < now, "stamps sit before the snapshot instant");
            }
        }
        // Same seed at a shifted clock: identical census, stamps shifted —
        // but the id namespace is salted, so still-pending guarded exits
        // queued for the t0 cohort can never fire on the re-seeded one.
        let (h0, _) = seeded(99, 0.0);
        for (r0, rn) in h0.rooms.iter().zip(&h.rooms) {
            match (r0.status.patient(), rn.status.patient()) {
                (Some(a), Some(b)) => {
                    assert_eq!(a.alias, b.alias);
                    assert_ne!(a.id, b.id, "re-seeded cohorts must not reuse patient ids");
                    let d = b.admitted_at_min.unwrap() - a.admitted_at_min.unwrap();
                    assert!((d - now).abs() < 1e-6, "stamps shift by exactly now_min");
                }
                (None, None) => {}
                _ => panic!("same seed must occupy the same rooms"),
            }
        }
    }
}
