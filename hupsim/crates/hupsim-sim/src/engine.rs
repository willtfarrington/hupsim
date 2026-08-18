//! The simulation engine: owns the clock, event queue, registered processes,
//! seeded RNG, timeline, and log. The app drives `tick()` from its fixed
//! timestep; everything downstream is deterministic given the seed.

use crate::clock::SimClock;
use crate::events::{EventQueue, ScheduledEvent, SimEventKind};
use crate::process::{SimCtx, SimProcess};
use crate::rolling::RollingStore;
use crate::roster::RnRoster;
use crate::timeline::Timeline;
use crate::transport::TransportBoard;
use hupsim_core::index::HospitalIndex;
use hupsim_core::matrix::RoomMatrix;
use hupsim_core::model::Hospital;
use hupsim_core::ops;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimLogEntry {
    pub t_min: f64,
    pub message: String,
}

pub struct RegisteredProcess {
    pub process: Box<dyn SimProcess>,
    pub enabled: bool,
}

pub struct SimEngine {
    pub clock: SimClock,
    pub queue: EventQueue,
    pub processes: Vec<RegisteredProcess>,
    pub rng: ChaCha8Rng,
    pub timeline: Timeline,
    /// Columnar room view (EP-9), refreshed every tick after due events and
    /// handed read-only to processes via [`SimCtx::matrix`]. Read-only over
    /// the hospital and RNG-free, so it cannot perturb determinism.
    pub matrix: RoomMatrix,
    /// Trailing-24 h norms (EP-9), fed exactly when the timeline samples —
    /// one sampling clock, no drift.
    pub rolling: RollingStore,
    /// RN staffing model (EP-11): ephemeral, deterministic, never saved —
    /// maintained at the end of every tick (and by the app after paused
    /// editor mutations).
    pub roster: RnRoster,
    /// Timed-transport state (EP-6): requests, in-flight patients, held beds,
    /// and the route graph. Ephemeral like the queue — never saved; in-flight
    /// transports drop on reset/load.
    pub transport: TransportBoard,
    pub log: Vec<SimLogEntry>,
    pub log_cap: usize,
    seed: u64,
}

impl SimEngine {
    pub fn new(seed: u64) -> Self {
        Self {
            clock: SimClock::default(),
            queue: EventQueue::new(),
            processes: Vec::new(),
            rng: ChaCha8Rng::seed_from_u64(seed),
            timeline: Timeline::default(),
            matrix: RoomMatrix::default(),
            rolling: RollingStore::default(),
            roster: RnRoster::default(),
            transport: TransportBoard::default(),
            log: Vec::new(),
            log_cap: 2000,
            seed,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Reset clock/queue/RNG/timeline/log — and each registered process's
    /// internal state — so the same seed reproduces a fresh run exactly.
    pub fn reset(&mut self, seed: u64) {
        self.clock = SimClock {
            speed: self.clock.speed,
            ..SimClock::default()
        };
        self.queue.clear();
        self.rng = ChaCha8Rng::seed_from_u64(seed);
        self.timeline.clear();
        self.matrix = RoomMatrix::default();
        self.rolling.clear();
        self.roster.clear();
        self.transport.clear_flight();
        self.log.clear();
        self.seed = seed;
        for rp in &mut self.processes {
            rp.process.reset();
        }
    }

    pub fn register(&mut self, process: Box<dyn SimProcess>, enabled: bool) {
        self.processes.push(RegisteredProcess { process, enabled });
    }

    /// Advance the world by `dt_min` sim minutes: apply due events, run
    /// enabled processes, advance the clock, sample the timeline.
    /// No-ops when `dt_min <= 0` (paused).
    pub fn tick(&mut self, hospital: &mut Hospital, index: &HospitalIndex, dt_min: f64) {
        if dt_min <= 0.0 {
            return;
        }
        let now = self.clock.now_min + dt_min;

        for ev in self.queue.pop_due(now) {
            Self::apply_event(hospital, index, &mut self.log, ev);
        }

        // Refresh the columnar view after events, before processes, so masks
        // reflect the tick-start world (processes verify against `hospital`
        // for anything they mutated mid-tick).
        self.matrix.refresh(hospital, index);

        for rp in self.processes.iter_mut().filter(|rp| rp.enabled) {
            let mut ctx = SimCtx {
                hospital,
                index,
                matrix: &self.matrix,
                now_min: now,
                dt_min,
                rng: &mut self.rng,
                queue: &mut self.queue,
                log: &mut self.log,
                transport: &mut self.transport,
            };
            rp.process.step(&mut ctx);
        }

        self.clock.now_min = now;
        if self.timeline.maybe_sample(now, hospital, index) {
            self.rolling.record(hospital, index);
        }

        // RN roster maintenance sees the end-of-tick world: churn diffing,
        // 07:00/19:00 turnover, census-triggered re-derives (EP-11).
        self.roster.maintain(hospital, index, now, &mut self.log);

        if self.log.len() > self.log_cap {
            let drop = self.log.len() - self.log_cap;
            self.log.drain(0..drop);
        }
    }

    fn apply_event(
        hospital: &mut Hospital,
        index: &HospitalIndex,
        log: &mut Vec<SimLogEntry>,
        ev: ScheduledEvent,
    ) {
        let outcome = match ev.kind {
            SimEventKind::Admission { room, patient } => {
                ops::admit(hospital, index, &room, *patient)
                    .map(|_| format!("admission → {room}"))
            }
            SimEventKind::Discharge {
                room,
                expect_patient,
            } => {
                let still_there = expect_patient.as_ref().is_none_or(|pid| {
                    index
                        .room_pos
                        .get(&room)
                        .and_then(|&i| hospital.rooms.get(i))
                        .and_then(|r| r.status.patient())
                        .is_some_and(|p| &p.id == pid)
                });
                if still_there {
                    ops::discharge(hospital, index, &room)
                        .map(|p| format!("discharge {} ← {room}", p.alias))
                } else {
                    // Not an error: the patient moved on (transfer, earlier
                    // discharge) — the event is simply obsolete.
                    Ok(format!("discharge skipped — patient moved: {room}"))
                }
            }
            SimEventKind::Transfer { from, to } => ops::transfer(hospital, index, &from, &to)
                .map(|_| format!("transfer {from} → {to}")),
            SimEventKind::AcuityChange { room, instability } => index
                .room_pos
                .get(&room)
                .copied()
                .ok_or(ops::OpError::RoomNotFound(room.clone()))
                .and_then(|i| {
                    hospital.rooms[i]
                        .status
                        .patient_mut()
                        .map(|p| {
                            p.acuity.instability = instability.clamp(0.0, 1.0);
                            format!("acuity change in {room} → {instability:.2}")
                        })
                        .ok_or(ops::OpError::NoPatient(room.clone()))
                }),
            SimEventKind::Custom { tag, .. } => Ok(format!("custom event: {tag}")),
        };
        let message = match outcome {
            Ok(m) => m,
            Err(e) => format!("event dropped: {e}"),
        };
        log.push(SimLogEntry {
            t_min: ev.at_min,
            message,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hupsim_core::model::*;
    use hupsim_core::patient::{Acuity, Patient};
    use hupsim_core::provenance::Provenance;

    fn world(rooms: usize) -> (Hospital, HospitalIndex) {
        let h = Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![],
            units: vec![Unit {
                id: "u".into(),
                name: "U".into(),
                service: "GenMed".into(),
                service_line: None,
                unit_type: UnitType::MedSurg,
                building: "b".into(),
                level: FloorLevel::Numbered(1),
                elevator_core: None,
                target_rn_ratio: None,
                provenance: Provenance::default(),
                note: None,
            }],
            rooms: (0..rooms)
                .map(|i| Room {
                    id: format!("r{i}").into(),
                    number: format!("{i}"),
                    unit: "u".into(),
                    kind: RoomKind::Inpatient,
                    telemetry_capable: false,
                    negative_pressure: false,
                    status: RoomStatus::Vacant,
                    provenance: Provenance::default(),
                })
                .collect(),
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![],
        };
        let idx = HospitalIndex::build(&h);
        (h, idx)
    }

    fn pt(name: &str, instability: f32) -> Patient {
        Patient {
            id: name.into(),
            alias: name.into(),
            age: None,
            service: None,
            admitted_at_min: Some(0.0),
            acuity: Acuity {
                instability,
                trend_per_hr: 0.0,
            },
            boarding: false,
            isolation: None,
            telemetry: false,
            note: String::new(),
        }
    }

    #[test]
    fn scheduled_admission_applies_on_time() {
        let (mut h, idx) = world(2);
        let mut eng = SimEngine::new(1);
        eng.queue.schedule(
            30.0,
            SimEventKind::Admission {
                room: "r0".into(),
                patient: Box::new(pt("A", 0.4)),
            },
        );
        eng.tick(&mut h, &idx, 15.0); // t=15, not yet
        assert!(h.rooms[0].status.is_vacant());
        eng.tick(&mut h, &idx, 15.0); // t=30, due
        assert_eq!(h.rooms[0].status.patient().unwrap().alias, "A");
        assert_eq!(eng.clock.now_min, 30.0);
    }

    #[test]
    fn conflicting_event_drops_gracefully() {
        let (mut h, idx) = world(1);
        let mut eng = SimEngine::new(1);
        eng.queue.schedule(
            10.0,
            SimEventKind::Admission {
                room: "r0".into(),
                patient: Box::new(pt("A", 0.1)),
            },
        );
        eng.queue.schedule(
            10.0,
            SimEventKind::Admission {
                room: "r0".into(),
                patient: Box::new(pt("B", 0.1)),
            },
        );
        eng.tick(&mut h, &idx, 10.0);
        // First admission lands, second logs a drop — world stays consistent.
        assert_eq!(h.rooms[0].status.patient().unwrap().alias, "A");
        assert!(eng.log.iter().any(|l| l.message.contains("dropped")));
    }

    #[test]
    fn paused_tick_is_noop() {
        let (mut h, idx) = world(1);
        let mut eng = SimEngine::new(1);
        eng.tick(&mut h, &idx, 0.0);
        assert_eq!(eng.clock.now_min, 0.0);
    }

    #[test]
    fn guarded_discharge_skips_when_patient_moved() {
        let (mut h, idx) = world(2);
        let mut eng = SimEngine::new(1);
        eng.queue.schedule(
            5.0,
            SimEventKind::Admission {
                room: "r0".into(),
                patient: Box::new(pt("A", 0.2)),
            },
        );
        // Guarded against a *different* patient: must not fire on A.
        eng.queue.schedule(
            10.0,
            SimEventKind::Discharge {
                room: "r0".into(),
                expect_patient: Some("somebody-else".into()),
            },
        );
        eng.tick(&mut h, &idx, 10.0);
        assert!(
            h.rooms[0].status.patient().is_some(),
            "guarded discharge must not evict a different patient"
        );
        assert!(eng.log.iter().any(|l| l.message.contains("skipped")));

        // Guarded with the right id: fires normally.
        eng.queue.schedule(
            20.0,
            SimEventKind::Discharge {
                room: "r0".into(),
                expect_patient: Some("A".into()),
            },
        );
        eng.tick(&mut h, &idx, 10.0);
        assert!(h.rooms[0].status.is_vacant());
    }

    #[test]
    fn rolling_norms_feed_at_timeline_cadence_and_reset() {
        let (mut h, idx) = world(4);
        let mut eng = SimEngine::new(1);
        // 96 timeline samples (15-min default cadence) fill the 24 h window.
        for _ in 0..96 {
            eng.tick(&mut h, &idx, 15.0);
        }
        assert_eq!(eng.timeline.len(), 96);
        assert!(eng.rolling.hospital.census.is_warm());
        assert_eq!(eng.rolling.hospital.census.len(), eng.timeline.len());
        assert!(!eng.roster.units.is_empty(), "ticking derives RN rosters");
        eng.reset(1);
        assert!(eng.rolling.hospital.census.is_empty(), "reset drops rolling history");
        assert!(eng.matrix.is_empty(), "reset drops the derived matrix");
        assert!(eng.roster.units.is_empty(), "reset drops the RN rosters");
    }
}
