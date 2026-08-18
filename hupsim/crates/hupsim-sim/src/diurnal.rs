//! `DiurnalFlow` — direct/scheduled inpatient admissions with time-of-day
//! structure, and LOS-scheduled discharges (EP-10).
//!
//! Arrivals: λ(t) = base × hour-of-day shape × weekday/weekend factor,
//! integrated in ≤15-min sub-steps so high sim speeds don't flatten the
//! curve. Each admission lands in a vacant med/surg, stepdown, or ICU bed
//! picked from the EP-9 matrix's tick-start vacancy candidates (no
//! per-arrival room scans), carries the *destination unit's* service, an
//! initial acuity drawn for that level of care, and a discharge scheduled on
//! the event queue from the service line's log-normal LOS, pushed onto the
//! mid-day discharge curve. All parameters: [`crate::params`] (synthetic).

use crate::events::SimEventKind;
use crate::params;
use crate::process::{SimCtx, SimProcess};
use crate::sampling;
use hupsim_core::matrix::{OccState, NO_UNIT};
use hupsim_core::ops;
use rand::Rng;

pub struct DiurnalFlow {
    /// Whole-hospital direct admissions per hour at shape multiplier 1.0.
    pub base_admits_per_hr: f64,
    counter: u64,
    /// Admission *attempts* tallied by sim hour-of-day, before any bed
    /// search — the demand curve itself, used by shape-sanity tests and
    /// available to future dashboards.
    pub attempts_by_hour: [u32; 24],
    /// Attempts that found no vacant inpatient bed (house full).
    pub turned_away: u32,
}

impl Default for DiurnalFlow {
    fn default() -> Self {
        Self {
            base_admits_per_hr: params::DIURNAL_BASE_ADMITS_PER_HR,
            counter: 0,
            attempts_by_hour: [0; 24],
            turned_away: 0,
        }
    }
}

impl SimProcess for DiurnalFlow {
    fn name(&self) -> &str {
        "Diurnal admissions & discharges"
    }

    fn description(&self) -> &str {
        "Time-of-day shaped inpatient admissions (morning scheduled bump, afternoon peak, overnight trough, weekend dip) with service-specific log-normal LOS and mid-day-peaked discharges. Synthetic parameters — see params.rs."
    }

    fn reset(&mut self) {
        self.counter = 0;
        self.attempts_by_hour = [0; 24];
        self.turned_away = 0;
    }

    fn step(&mut self, ctx: &mut SimCtx) {
        if ctx.dt_min <= 0.0 {
            return;
        }
        let m = ctx.matrix;

        // Vacant inpatient beds as of tick start, consumed by swap_remove as
        // this tick admits into them. Freed-this-tick beds become visible
        // next tick — a ≤tick-length delay, invisible at sim scale.
        let mut candidates: Vec<u32> = (0..m.len() as u32)
            .filter(|&i| {
                m.occupancy[i as usize] == OccState::Vacant
                    && params::is_inpatient_unit(m.unit_type[i as usize])
            })
            .collect();

        let n_sub = (ctx.dt_min / params::SUBSTEP_MIN).ceil().max(1.0) as usize;
        let sub_dt = ctx.dt_min / n_sub as f64;
        let tick_start = ctx.now_min - ctx.dt_min;

        for s in 0..n_sub {
            let t = tick_start + sub_dt * (s as f64 + 1.0);
            let rate = params::diurnal_rate_per_hr(
                self.base_admits_per_hr,
                &params::INPATIENT_ARRIVAL_SHAPE,
                params::WEEKEND_INPATIENT_FACTOR,
                t,
            );
            let expected = rate * sub_dt / 60.0;
            let mut arrivals = expected.floor() as usize;
            if ctx
                .rng
                .random_bool((expected - expected.floor()).clamp(0.0, 1.0))
            {
                arrivals += 1;
            }

            for _ in 0..arrivals {
                self.attempts_by_hour[params::hour_of(t)] += 1;
                match self.admit_one(ctx, &mut candidates, t) {
                    Some(()) => {}
                    None => {
                        self.turned_away += 1;
                        ctx.log("admission turned away — no vacant inpatient bed");
                    }
                }
            }
        }
    }
}

impl DiurnalFlow {
    /// Place one admission from the candidate pool. Draws rooms until one is
    /// verified vacant in the live world (a candidate can be stale if another
    /// process filled it this tick). `None` when the pool runs dry.
    fn admit_one(&mut self, ctx: &mut SimCtx, candidates: &mut Vec<u32>, t: f64) -> Option<()> {
        let m = ctx.matrix;
        loop {
            if candidates.is_empty() {
                return None;
            }
            let k = ctx.rng.random_range(0..candidates.len());
            let row = candidates.swap_remove(k) as usize;
            if !ctx.hospital.rooms[row].status.is_vacant()
                || ctx.transport.is_held(&ctx.hospital.rooms[row].id)
            {
                continue; // stale candidate — filled or reserved this tick
            }
            let up = m.unit_pos[row];
            if up == NO_UNIT {
                continue;
            }
            let (service, line) = {
                let u = &ctx.hospital.units[up as usize];
                (u.service.clone(), u.service_line.clone())
            };
            let ut = m.unit_type[row];

            self.counter += 1;
            let instability = sampling::draw_initial_instability(ctx.rng, ut);
            let patient = sampling::synth_patient(
                ctx.rng,
                "adm",
                self.counter,
                t,
                instability,
                Some(service),
                "synthetic direct admission",
            );
            let pid = patient.id.clone();
            let alias = patient.alias.clone();
            let room_id = m.room_id(row as u32).clone();

            if ops::admit(ctx.hospital, ctx.index, &room_id, patient).is_err() {
                continue; // lost a race within the tick; try another bed
            }

            // Discharge: service-line log-normal LOS, shaped onto the
            // mid-day curve, guarded by patient id so the event can never
            // hit a successor occupant of the room.
            let los_hr = sampling::lognormal_hours(ctx.rng, &params::los_for(line.as_ref(), ut));
            let dc_at = sampling::shape_discharge_min(ctx.rng, t + los_hr * 60.0);
            ctx.queue.schedule(
                dc_at,
                SimEventKind::Discharge {
                    room: room_id.clone(),
                    expect_patient: Some(pid),
                },
            );
            ctx.log(format!("admission: {alias} → {room_id}"));
            return Some(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acuity::AcuityDrift;
    use crate::edflow::EdPressure;
    use crate::engine::SimEngine;
    use crate::testworld::world;
    use hupsim_core::aggregate::aggregate_unit;
    use hupsim_core::model::{Hospital, UnitType};
    use hupsim_core::patient::{Acuity, Patient};

    /// The EP-10 process roster on a mixed world with a deliberately tight
    /// ICU so boarding paths get exercised by the determinism tests too.
    fn engine_with_processes(seed: u64) -> SimEngine {
        let mut eng = SimEngine::new(seed);
        eng.register(Box::new(DiurnalFlow::default()), true);
        eng.register(Box::new(EdPressure::default()), true);
        eng.register(Box::new(AcuityDrift::default()), true);
        eng
    }

    fn mixed_world() -> (Hospital, hupsim_core::index::HospitalIndex) {
        world(&[
            ("ms1", UnitType::MedSurg, 30),
            ("ms2", UnitType::MedSurg, 30),
            ("icu", UnitType::Icu, 4),
            ("sd", UnitType::Stepdown, 6),
            ("ed", UnitType::Ed, 12),
        ])
    }

    fn run(seed: u64, ticks: usize, dt_min: f64) -> Hospital {
        let (mut h, idx) = mixed_world();
        let mut eng = engine_with_processes(seed);
        for _ in 0..ticks {
            eng.tick(&mut h, &idx, dt_min);
        }
        h
    }

    #[test]
    fn same_seed_same_world() {
        assert_eq!(run(7, 200, 30.0), run(7, 200, 30.0));
    }

    #[test]
    fn different_seed_different_world() {
        assert_ne!(run(1, 200, 30.0), run(2, 200, 30.0));
    }

    #[test]
    fn reset_reproduces_fresh_run() {
        let fresh = run(7, 200, 30.0);

        let (mut h, idx) = mixed_world();
        let mut eng = engine_with_processes(999);
        for _ in 0..77 {
            eng.tick(&mut h, &idx, 30.0); // dirty engine + process internals
        }
        eng.reset(7);
        let (mut h2, idx2) = mixed_world();
        for _ in 0..200 {
            eng.tick(&mut h2, &idx2, 30.0);
        }
        assert_eq!(fresh, h2, "reset engine must replay identically");
    }

    #[test]
    fn overnight_demand_below_afternoon_demand() {
        // Drive the process directly so its attempt counters stay readable
        // (the engine boxes processes). Attempts are tallied before any bed
        // search, so skipping event application cannot distort the curve.
        use crate::events::EventQueue;
        use hupsim_core::matrix::RoomMatrix;
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;

        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 60)]);
        let mut flow = DiurnalFlow::default();
        let mut matrix = RoomMatrix::build(&h, &idx);
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let mut queue = EventQueue::new();
        let mut log = Vec::new();
        let mut transport = crate::transport::TransportBoard::default();
        let dt = 15.0;
        for tick in 0..(4 * 96) {
            matrix.refresh(&h, &idx);
            let mut ctx = crate::process::SimCtx {
                hospital: &mut h,
                index: &idx,
                matrix: &matrix,
                now_min: (tick + 1) as f64 * dt,
                dt_min: dt,
                rng: &mut rng,
                queue: &mut queue,
                log: &mut log,
                transport: &mut transport,
            };
            flow.step(&mut ctx);
        }
        let overnight: u32 = (2..=5).map(|hh| flow.attempts_by_hour[hh]).sum();
        let afternoon: u32 = (14..=17).map(|hh| flow.attempts_by_hour[hh]).sum();
        assert!(
            afternoon > overnight * 3,
            "afternoon demand ({afternoon}) should dwarf overnight ({overnight}): {:?}",
            flow.attempts_by_hour
        );
    }

    #[test]
    fn census_shows_a_visible_diurnal_wave() {
        // A world with headroom (steady-state census below capacity, so the
        // wave isn't clipped at 100%), run past the LOS horizon so scheduled
        // discharges are active, then require a visible intra-day swing.
        let (mut h, idx) = world(&[
            ("ms1", UnitType::MedSurg, 150),
            ("ms2", UnitType::MedSurg, 150),
            ("ms3", UnitType::MedSurg, 150),
            ("icu", UnitType::Icu, 30),
            ("sd", UnitType::Stepdown, 30),
            ("ed", UnitType::Ed, 40),
        ]);
        let mut eng = SimEngine::new(11);
        let mut flow = DiurnalFlow::default();
        flow.base_admits_per_hr = 3.0;
        let mut ed = EdPressure::default();
        ed.base_arrivals_per_hr = 3.0;
        eng.register(Box::new(flow), true);
        eng.register(Box::new(ed), true);

        let mut census_by_tick: Vec<(f64, usize)> = Vec::new();
        for _ in 0..(8 * 96) {
            eng.tick(&mut h, &idx, 15.0); // 8 sim days
            let census = h
                .rooms
                .iter()
                .filter(|r| r.status.patient().is_some())
                .count();
            census_by_tick.push((eng.clock.now_min, census));
        }
        // Discharges begin well before day 5; judge the last three days.
        let mut waved_days = 0;
        for day in 5..8 {
            let day_vals: Vec<usize> = census_by_tick
                .iter()
                .filter(|(t, _)| (*t / 1440.0).floor() as usize == day)
                .map(|(_, c)| *c)
                .collect();
            let (min, max) = (
                *day_vals.iter().min().unwrap(),
                *day_vals.iter().max().unwrap(),
            );
            if max - min >= 5 {
                waved_days += 1;
            }
        }
        assert!(
            waved_days >= 2,
            "census should swing visibly within steady-state days"
        );
    }

    #[test]
    fn rolling_z_reacts_to_surge_and_settles() {
        // EP-10 acceptance: a unit's 24 h z-score reacts to an injected
        // surge and settles back once the surge resolves and the trailing
        // window absorbs it. Rate tuned so the world sits in steady state
        // (census ≈ half of capacity) instead of pinned full.
        let (mut h, idx) = world(&[("ms1", UnitType::MedSurg, 40), ("ms2", UnitType::MedSurg, 40)]);
        let mut eng = SimEngine::new(5);
        let mut flow = DiurnalFlow::default();
        flow.base_admits_per_hr = 0.5;
        eng.register(Box::new(flow), true);

        // Warm-up to steady state: 5 sim days ≫ the 24 h window.
        for _ in 0..(5 * 96) {
            eng.tick(&mut h, &idx, 15.0);
        }
        let unit = "ms1".into();
        let z_before = {
            let census = aggregate_unit(&h, &idx, &unit).census as f32;
            eng.rolling.units[&unit].census.z(census)
        };

        // Inject a surge: immediate admissions into vacant ms1 beds, each
        // resolving (discharging) 8 h later.
        let vacant: Vec<_> = h
            .rooms
            .iter()
            .filter(|r| r.unit == unit && r.status.is_vacant())
            .map(|r| r.id.clone())
            .take(12)
            .collect();
        assert!(vacant.len() >= 8, "test world should have vacancy headroom");
        for (i, room) in vacant.iter().enumerate() {
            eng.queue.schedule(
                eng.clock.now_min + 1.0,
                SimEventKind::Admission {
                    room: room.clone(),
                    patient: Box::new(Patient {
                        id: format!("surge.{i}").into(),
                        alias: format!("Surge {i}"),
                        age: Some(50),
                        service: Some("surge test".into()),
                        admitted_at_min: Some(eng.clock.now_min),
                        acuity: Acuity {
                            instability: 0.4,
                            trend_per_hr: 0.0,
                        },
                        boarding: false,
                        isolation: None,
                        telemetry: false,
                        note: String::new(),
                    }),
                },
            );
            eng.queue.schedule(
                eng.clock.now_min + 8.0 * 60.0,
                SimEventKind::Discharge {
                    room: room.clone(),
                    expect_patient: Some(format!("surge.{i}").into()),
                },
            );
        }
        eng.tick(&mut h, &idx, 15.0);
        let z_surge = {
            let census = aggregate_unit(&h, &idx, &unit).census as f32;
            eng.rolling.units[&unit].census.z(census).unwrap()
        };
        assert!(
            z_surge > 2.0,
            "z after +{} surge admissions should spike, got {z_surge} (before: {z_before:?})",
            vacant.len()
        );

        // 48 h later: surge resolved, window fully post-surge — z settles.
        for _ in 0..(2 * 96) {
            eng.tick(&mut h, &idx, 15.0);
        }
        let census = aggregate_unit(&h, &idx, &unit).census as f32;
        if let Some(z) = eng.rolling.units[&unit].census.z(census) {
            assert!(
                z.abs() < 2.0,
                "z should settle once the surge resolves, got {z} (surge peak {z_surge})"
            );
        }
    }
}
