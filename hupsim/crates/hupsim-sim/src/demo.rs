//! DEMO process — the original proof-of-loop flow generator, kept behind its
//! toolbar toggle. Superseded for realism by [`crate::diurnal::DiurnalFlow`]
//! + [`crate::edflow::EdPressure`] (EP-10): no time-of-day structure, one
//! global LOS, no ED/boarding. Useful as a flat-rate control signal.

use crate::process::{SimCtx, SimProcess};
use crate::sampling;
use hupsim_core::matrix::OccState;
use hupsim_core::ops;
use hupsim_core::patient::{Acuity, Patient};
use rand::Rng;

/// Poisson-ish arrivals into random vacant beds + LOS-based discharges of
/// stable patients. Gives the census a pulse. Demo only.
pub struct DemoFlow {
    pub arrivals_per_hr: f64,
    pub avg_los_hr: f64,
    counter: u64,
}

impl Default for DemoFlow {
    fn default() -> Self {
        Self {
            arrivals_per_hr: 2.0,
            avg_los_hr: 72.0,
            counter: 0,
        }
    }
}

impl DemoFlow {
    fn synth_patient(&mut self, ctx: &mut SimCtx) -> Patient {
        self.counter += 1;
        let initial = (b'A' + ctx.rng.random_range(0..26u8)) as char;
        let surname = sampling::SURNAMES[ctx.rng.random_range(0..sampling::SURNAMES.len())];
        let instability: f32 = {
            // Skewed toward stable: square a uniform.
            let u: f32 = ctx.rng.random_range(0.0..1.0);
            (u * u * 0.85).clamp(0.0, 1.0)
        };
        Patient {
            id: format!("demo.pt.{}", self.counter).into(),
            alias: format!("{initial}. {surname}"),
            age: Some(ctx.rng.random_range(19..97)),
            service: None,
            admitted_at_min: Some(ctx.now_min),
            acuity: Acuity {
                instability,
                trend_per_hr: ctx.rng.random_range(-0.02..0.02),
            },
            boarding: false,
            isolation: None,
            telemetry: instability > 0.35,
            note: "synthetic demo patient".into(),
        }
    }
}

impl SimProcess for DemoFlow {
    fn name(&self) -> &str {
        "Arrivals & discharges (demo)"
    }

    fn description(&self) -> &str {
        "Flat-rate random arrivals and LOS-based discharges. Demo only — superseded by the diurnal + ED processes."
    }

    fn reset(&mut self) {
        self.counter = 0;
    }

    fn step(&mut self, ctx: &mut SimCtx) {
        let dt_hr = ctx.dt_min / 60.0;
        if dt_hr <= 0.0 {
            return;
        }
        let m = ctx.matrix;

        // Arrivals: expected count this tick; fractional part is a Bernoulli.
        // Candidates come from the tick-start matrix (EP-9) — one columnar
        // pass per tick instead of a room scan per arrival.
        let expected = self.arrivals_per_hr * dt_hr;
        let mut arrivals = expected.floor() as usize;
        if ctx.rng.random_bool((expected - expected.floor()).clamp(0.0, 1.0)) {
            arrivals += 1;
        }
        let mut vacant: Vec<u32> = (0..m.len() as u32)
            .filter(|&i| {
                m.occupancy[i as usize] == OccState::Vacant
                    && crate::params::is_inpatient_unit(m.unit_type[i as usize])
            })
            .collect();
        for _ in 0..arrivals {
            let room = loop {
                if vacant.is_empty() {
                    break None;
                }
                let k = ctx.rng.random_range(0..vacant.len());
                let row = vacant.swap_remove(k);
                if ctx.hospital.rooms[row as usize].status.is_vacant() {
                    break Some(m.room_id(row).clone());
                }
            };
            let Some(room) = room else {
                ctx.log("demo arrival turned away — no vacant staffed bed");
                break;
            };
            let patient = self.synth_patient(ctx);
            let alias = patient.alias.clone();
            if ops::admit(ctx.hospital, ctx.index, &room, patient).is_ok() {
                ctx.log(format!("demo admission: {alias} → {room}"));
            }
        }

        // Discharges: stable patients leave with prob dt/LOS. Candidates from
        // the matrix's occupied+instability columns (tick-start snapshot).
        let p_dc = (dt_hr / self.avg_los_hr).clamp(0.0, 1.0);
        let candidates: Vec<u32> = (0..m.len() as u32)
            .filter(|&i| {
                m.occupancy[i as usize] == OccState::Occupied && m.instability[i as usize] < 0.3
            })
            .collect();
        for row in candidates {
            if ctx.rng.random_bool(p_dc) {
                let room = m.room_id(row).clone();
                if let Ok(p) = ops::discharge(ctx.hospital, ctx.index, &room) {
                    ctx.log(format!("demo discharge: {} ← {room}", p.alias));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acuity::AcuityDrift;
    use crate::engine::SimEngine;
    use crate::testworld::world;
    use hupsim_core::model::{Hospital, UnitType};

    fn run(seed: u64, ticks: usize) -> Hospital {
        let (mut h, idx) = world(&[("u", UnitType::MedSurg, 20)]);
        let mut eng = SimEngine::new(seed);
        eng.register(Box::new(DemoFlow::default()), true);
        eng.register(Box::new(AcuityDrift::default()), true);
        for _ in 0..ticks {
            eng.tick(&mut h, &idx, 30.0);
        }
        h
    }

    #[test]
    fn demo_flow_fills_beds_and_stays_in_bounds() {
        let h = run(42, 200); // 100 sim-hours at 2/hr arrivals into 20 beds
        let census = h
            .rooms
            .iter()
            .filter(|r| r.status.patient().is_some())
            .count();
        assert!(census > 0, "demo flow should admit somebody");
        for r in &h.rooms {
            if let Some(p) = r.status.patient() {
                assert!((0.0..=1.0).contains(&p.acuity.instability));
            }
        }
    }

    #[test]
    fn same_seed_same_world() {
        let a = run(7, 50);
        let b = run(7, 50);
        assert_eq!(a, b, "simulation must be deterministic per seed");
    }

    #[test]
    fn different_seed_different_world() {
        let a = run(1, 50);
        let b = run(2, 50);
        assert_ne!(a, b);
    }

    #[test]
    fn reset_reproduces_fresh_run() {
        // A dirty engine that is reset must reproduce a fresh engine's run
        // exactly — including process-internal state like DemoFlow's counter.
        let fresh = run(7, 50);

        let (mut h, idx) = world(&[("u", UnitType::MedSurg, 20)]);
        let mut eng = SimEngine::new(999);
        eng.register(Box::new(DemoFlow::default()), true);
        eng.register(Box::new(AcuityDrift::default()), true);
        for _ in 0..30 {
            eng.tick(&mut h, &idx, 30.0); // dirty the engine + process state
        }
        eng.reset(7);
        let (mut h2, idx2) = world(&[("u", UnitType::MedSurg, 20)]);
        for _ in 0..50 {
            eng.tick(&mut h2, &idx2, 30.0);
        }
        assert_eq!(fresh, h2, "reset engine must replay identically to a fresh one");
    }
}
