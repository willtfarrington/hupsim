//! `BacklogDischarges` — one-shot standing-census exit scheduler (EP-21).
//!
//! A seeded (or freshly loaded) world holds patients whose discharges were
//! never put on the event queue: `seed_demo_census` stamps them with
//! backdated admit times but schedules nothing, and scenario load drops the
//! ephemeral queue entirely. Without exits the standing census is immortal
//! and the house saturates to the 100% rail within a day — an artifact of
//! the missing schedule, not a modeling choice.
//!
//! On its first step this process schedules a guarded discharge
//! (`expect_patient`) for every occupied **inpatient** bed whose patient has
//! a known admit time, at residual LOS: the service line's total-LOS
//! log-normal resampled conditioned above the elapsed stay
//! ([`sampling::residual_discharge_min`]), shaped onto the mid-day discharge
//! curve, with a prompt shaped fallback when the elapsed stay is deep in the
//! tail. Then it disarms until the next engine reset. Non-inpatient units
//! (ED, psych, obs, …) are deliberately left alone: they have no refill
//! process, so draining them would empty the demo world instead of starting
//! it mid-stream.

use crate::events::SimEventKind;
use crate::params;
use crate::process::{SimCtx, SimProcess};
use crate::sampling;

#[derive(Debug, Default)]
pub struct BacklogDischarges {
    fired: bool,
}

impl SimProcess for BacklogDischarges {
    fn name(&self) -> &str {
        "Standing-census discharges"
    }

    fn description(&self) -> &str {
        "One-shot: schedules a residual-LOS discharge for every occupied inpatient bed with a known admit time, so a seeded or loaded census exits on the same synthetic LOS tables that admitted it (EP-21)."
    }

    fn reset(&mut self) {
        self.fired = false;
    }

    fn step(&mut self, ctx: &mut SimCtx) {
        if self.fired {
            return;
        }
        self.fired = true;
        let mut scheduled = 0usize;
        for i in 0..ctx.hospital.rooms.len() {
            let room = &ctx.hospital.rooms[i];
            let Some(p) = room.status.patient() else {
                continue;
            };
            let Some(admit_min) = p.admitted_at_min else {
                continue; // hand-admitted "pre-sim" patients keep the labeled rule
            };
            // A clock rewind (toolbar reset keeps the census, zeroes the
            // clock) leaves live-admitted stamps in the sim *future* —
            // anchor those at `now` so they read as fresh admissions
            // instead of freezing the bed until the stamp comes round
            // again (mirrors the experiment backfill's future-stamp guard).
            let admit_min = admit_min.min(ctx.now_min);
            let Some(&up) = ctx.index.unit_pos.get(&room.unit) else {
                continue;
            };
            let unit = &ctx.hospital.units[up];
            if !params::is_inpatient_unit(unit.unit_type) {
                continue;
            }
            let los = params::los_for(unit.service_line.as_ref(), unit.unit_type);
            let at = sampling::residual_discharge_min(ctx.rng, &los, admit_min, ctx.now_min);
            ctx.queue.schedule(
                at,
                SimEventKind::Discharge {
                    room: room.id.clone(),
                    expect_patient: Some(p.id.clone()),
                },
            );
            scheduled += 1;
        }
        if scheduled > 0 {
            // Wording must not collide with the report's log-corroboration
            // prefixes ("admission", "discharge ", "ED decision to admit").
            ctx.log(format!(
                "standing census: scheduled {scheduled} residual-LOS exits"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::SimEngine;
    use crate::testworld::world;
    use hupsim_core::model::UnitType;
    use hupsim_core::ops;
    use hupsim_core::patient::{Acuity, Patient};

    fn pt(name: &str, admitted: Option<f64>) -> Patient {
        Patient {
            id: name.into(),
            alias: name.into(),
            age: Some(50),
            service: None,
            admitted_at_min: admitted,
            acuity: Acuity {
                instability: 0.2,
                trend_per_hr: 0.0,
            },
            boarding: false,
            isolation: None,
            telemetry: false,
            note: String::new(),
        }
    }

    fn seeded_world() -> (
        hupsim_core::model::Hospital,
        hupsim_core::index::HospitalIndex,
    ) {
        let (mut h, idx) = world(&[
            ("ms", UnitType::MedSurg, 6),
            ("icu", UnitType::Icu, 2),
            ("ed", UnitType::Ed, 3),
        ]);
        // Backdated inpatients (negative stamps), one unknown-stamp legacy
        // patient, one live-stamped patient, and a backdated ED occupant.
        ops::admit(&mut h, &idx, &"ms.r0".into(), pt("a", Some(-3000.0))).unwrap();
        ops::admit(&mut h, &idx, &"ms.r1".into(), pt("b", Some(-90.0))).unwrap();
        ops::admit(&mut h, &idx, &"ms.r2".into(), pt("legacy", None)).unwrap();
        ops::admit(&mut h, &idx, &"icu.r0".into(), pt("c", Some(-8000.0))).unwrap();
        ops::admit(&mut h, &idx, &"ed.r0".into(), pt("edp", Some(-120.0))).unwrap();
        (h, idx)
    }

    #[test]
    fn schedules_residual_exits_once_for_known_inpatients_only() {
        let (mut h, idx) = seeded_world();
        let mut eng = SimEngine::new(7);
        eng.register(Box::new(BacklogDischarges::default()), true);

        eng.tick(&mut h, &idx, 15.0);
        // a, b, c — not the unknown-stamp legacy patient, not the ED bay.
        assert_eq!(eng.queue.len(), 3, "one exit per known-stamp inpatient");
        assert!(eng
            .log
            .iter()
            .any(|l| l.message == "standing census: scheduled 3 residual-LOS exits"));
        assert!(eng.queue.peek_time().unwrap() >= eng.clock.now_min);

        // One-shot: later ticks add nothing.
        let before = eng.queue.len();
        eng.tick(&mut h, &idx, 15.0);
        assert!(eng.queue.len() <= before, "the process must disarm after firing");
    }

    #[test]
    fn exits_actually_fire() {
        let (mut h, idx) = seeded_world();
        let mut eng = SimEngine::new(7);
        eng.register(Box::new(BacklogDischarges::default()), true);
        eng.tick(&mut h, &idx, 15.0);

        // Run far past every plausible residual LOS (60 sim days ≫ the
        // 3σ tail of the widest table entry).
        for _ in 0..(60 * 96) {
            eng.tick(&mut h, &idx, 15.0);
        }
        for r in ["ms.r0", "ms.r1", "icu.r0"] {
            let ri = idx.room_pos[&r.into()];
            assert!(
                h.rooms[ri].status.is_vacant(),
                "{r} should have discharged on its residual LOS"
            );
        }
        // Untouched: unknown-stamp legacy patient and the ED occupant.
        assert!(h.rooms[idx.room_pos[&"ms.r2".into()]].status.patient().is_some());
        assert!(h.rooms[idx.room_pos[&"ed.r0".into()]].status.patient().is_some());
    }

    #[test]
    fn reset_rearms_the_one_shot() {
        let (mut h, idx) = seeded_world();
        let mut eng = SimEngine::new(7);
        eng.register(Box::new(BacklogDischarges::default()), true);
        eng.tick(&mut h, &idx, 15.0);
        assert_eq!(eng.queue.len(), 3);

        eng.reset(7);
        assert_eq!(eng.queue.len(), 0, "reset drops the queue");
        let (mut h2, idx2) = seeded_world();
        eng.tick(&mut h2, &idx2, 15.0);
        assert_eq!(eng.queue.len(), 3, "a reset engine must re-schedule the backlog");
    }

    /// A toolbar reset keeps the census but rewinds the clock, leaving
    /// live-admitted stamps in the sim future. Those must be anchored at
    /// `now` — exit on a fresh LOS from today — not at the future stamp
    /// plus a full LOS (which would freeze the bed for the prior run's
    /// whole duration).
    #[test]
    fn future_stamps_anchor_at_now() {
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 2)]);
        let future = 30.0 * 1440.0; // stamped 30 sim days ahead of the clock
        ops::admit(&mut h, &idx, &"ms.r0".into(), pt("rewound", Some(future))).unwrap();
        let mut eng = SimEngine::new(11);
        eng.register(Box::new(BacklogDischarges::default()), true);
        eng.tick(&mut h, &idx, 15.0);
        let exits = eng.queue.pop_due(f64::INFINITY);
        assert_eq!(exits.len(), 1);
        assert!(
            exits[0].at_min < future,
            "a future-stamped patient must exit on a fresh LOS from now, \
             not from the stamp: scheduled {} vs stamp {}",
            exits[0].at_min,
            future
        );
        assert!(exits[0].at_min >= eng.clock.now_min);
    }

    #[test]
    fn same_seed_schedules_identically() {
        // Compare the scheduled exits themselves: (time, event) pairs after
        // the one-shot fires.
        let schedule = |seed: u64| -> Vec<(f64, SimEventKind)> {
            let (mut h, idx) = seeded_world();
            let mut eng = SimEngine::new(seed);
            eng.register(Box::new(BacklogDischarges::default()), true);
            eng.tick(&mut h, &idx, 15.0);
            eng.queue
                .pop_due(f64::INFINITY)
                .into_iter()
                .map(|e| (e.at_min, e.kind))
                .collect()
        };
        assert_eq!(schedule(3), schedule(3), "backlog must be seed-deterministic");
        assert_ne!(schedule(3), schedule(4), "different seeds must differ");
    }
}
