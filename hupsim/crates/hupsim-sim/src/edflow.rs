//! `EdPressure` — ED arrivals, admit conversion, and boarding (EP-10).
//!
//! ED arrivals follow their own λ(t) into vacant ED bays. Each visit's
//! disposition is decided (deterministically, from the seeded RNG) at
//! arrival: most treat-and-release after a log-normal visit, a fraction
//! convert to inpatient admissions. When the admit decision fires and no
//! suitable vacant inpatient bed exists, the patient **boards in the ED
//! bay** (`boarding: true`) until one frees — the mechanism that makes
//! occupancy/boarding dashboards move credibly. Boarders are placed
//! oldest-first into the lowest-index suitable bed (first-fit, no RNG), and
//! `ops::transfer` clears the flag on arrival in a licensed bed.
//!
//! Bed queries go through the EP-9 matrix's tick-start columns — no
//! per-event scans of `Hospital.rooms`. All parameters: [`crate::params`]
//! (synthetic).
//!
//! Standing census (EP-17, closing an EP-21 open thread): on its first step
//! after construction or reset the process **adopts** ED occupants it has no
//! case for — seeded census, loaded scenarios — with episode timing
//! resampled conditioned above the elapsed stay, so seeded boarders place
//! and seeded walk-ins release instead of sitting immortal while their
//! displayed waits grow without bound.

use crate::events::SimEventKind;
use crate::params;
use crate::process::{SimCtx, SimProcess};
use crate::sampling;
use crate::transport::TransportRequest;
use hupsim_core::ids::{PatientId, RoomId};
use hupsim_core::matrix::{OccState, NO_UNIT};
use hupsim_core::model::UnitType;
use hupsim_core::ops;
use rand::Rng;

#[derive(Debug, Clone, Copy, PartialEq)]
enum EdDispo {
    Release,
    Admit,
}

#[derive(Debug, Clone)]
struct EdCase {
    bay: RoomId,
    patient: PatientId,
    /// Sim minute the in-treatment phase ends (walk-out or admit decision).
    due_min: f64,
    dispo: EdDispo,
    /// `Some(level of care)` once the admit decision has fired and the
    /// patient is boarding, waiting for a bed of that type.
    boarding_need: Option<UnitType>,
}

#[derive(Debug, Clone, Default)]
pub struct EdStats {
    pub arrivals: u32,
    pub releases: u32,
    pub admits: u32,
    /// Arrivals that found no vacant ED bay (walk-outs/diversion).
    pub diverted: u32,
    /// Standing ED occupants adopted at the start of a run (EP-17).
    pub adopted: u32,
}

pub struct EdPressure {
    /// ED arrivals per hour at shape multiplier 1.0.
    pub base_arrivals_per_hr: f64,
    counter: u64,
    /// Active ED visits, oldest first (arrival order). Iterated in order —
    /// deterministic, and boarders compete for beds oldest-first.
    cases: Vec<EdCase>,
    /// One-shot standing-census adoption latch (EP-17), rearmed by reset.
    adopted: bool,
    pub stats: EdStats,
}

impl Default for EdPressure {
    fn default() -> Self {
        Self {
            base_arrivals_per_hr: params::ED_BASE_ARRIVALS_PER_HR,
            counter: 0,
            cases: Vec::new(),
            adopted: false,
            stats: EdStats::default(),
        }
    }
}

impl SimProcess for EdPressure {
    fn name(&self) -> &str {
        "ED arrivals & boarding"
    }

    fn description(&self) -> &str {
        "ED arrivals with their own diurnal curve; ~30% convert to inpatient admissions and board in the ED when no suitable bed is vacant. Synthetic parameters — see params.rs."
    }

    fn reset(&mut self) {
        self.counter = 0;
        self.cases.clear();
        self.adopted = false;
        self.stats = EdStats::default();
    }

    fn step(&mut self, ctx: &mut SimCtx) {
        if ctx.dt_min <= 0.0 {
            return;
        }
        if !self.adopted {
            self.adopted = true;
            self.adopt_standing(ctx);
        }
        self.progress_cases(ctx);
        self.arrivals(ctx);
    }
}

impl EdPressure {
    /// One-shot adoption of standing ED occupants (EP-17, closing an EP-21
    /// open thread): a seeded census or a loaded scenario holds ED patients
    /// this process has no case for — without one they never release, never
    /// place, and their displayed boarding waits grow without bound. Mirror
    /// of `BacklogDischarges`: each unknown occupant gets a case whose
    /// timing is the episode log-normal resampled conditioned above the
    /// elapsed stay (prompt fallback in the deep tail); seeded boarders
    /// (`boarding: true` — the decision already fired pre-snapshot) go
    /// straight onto the bed hunt. Runs before arrivals on the first step
    /// after construction or reset; skips patients already cased so
    /// hand-crafted test states stay untouched. Deterministic: bays visit in
    /// row order off the seeded RNG.
    fn adopt_standing(&mut self, ctx: &mut SimCtx) {
        let m = ctx.matrix;
        let mut adopted = 0u32;
        for row in 0..m.len() {
            if m.unit_type[row] != UnitType::Ed {
                continue;
            }
            let Some(p) = ctx.hospital.rooms[row].status.patient() else {
                continue;
            };
            if self.cases.iter().any(|c| c.patient == p.id) {
                continue;
            }
            let bay = ctx.hospital.rooms[row].id.clone();
            let patient = p.id.clone();
            let boarding = p.boarding;
            // Future stamps (clock rewound) anchor at now, like the backlog.
            let anchor = p.admitted_at_min.unwrap_or(ctx.now_min).min(ctx.now_min);
            let elapsed_hr = (ctx.now_min - anchor) / 60.0;
            let case = if boarding {
                // Decision already fired pre-snapshot: hunt a bed now.
                let need = draw_admit_need(ctx.rng);
                EdCase {
                    bay,
                    patient,
                    due_min: ctx.now_min,
                    dispo: EdDispo::Admit,
                    boarding_need: Some(need),
                }
            } else {
                let will_admit = ctx.rng.random_bool(params::ED_ADMIT_FRACTION);
                let (dispo, los) = if will_admit {
                    (EdDispo::Admit, &params::ED_DECISION_TO_ADMIT)
                } else {
                    (EdDispo::Release, &params::ED_TREAT_RELEASE_LOS)
                };
                let due_min =
                    match crate::sampling::residual_total_los_hours(ctx.rng, los, elapsed_hr) {
                        Some(total_hr) => anchor + total_hr * 60.0,
                        None => ctx.now_min, // deep tail: dispose promptly
                    };
                EdCase {
                    bay,
                    patient,
                    due_min,
                    dispo,
                    boarding_need: None,
                }
            };
            self.cases.push(case);
            adopted += 1;
        }
        if adopted > 0 {
            self.stats.adopted = adopted;
            // Wording dodges the EP-16 report log-scan prefixes.
            ctx.log(format!("ED standing census: adopted {adopted} occupants"));
        }
    }

    /// Advance every active case: fire due dispositions, try to place
    /// boarders. Cases are consumed and re-collected to keep ordering (and
    /// therefore RNG consumption) deterministic.
    fn progress_cases(&mut self, ctx: &mut SimCtx) {
        // Per-tick vacancy pools per needed level of care, built lazily from
        // the tick-start matrix, consumed front-first as boarders place.
        let mut pools: Vec<Option<Vec<u32>>> = vec![None; UnitType::ALL.len()];

        let cases = std::mem::take(&mut self.cases);
        for mut case in cases {
            // The bay's occupant must still be our patient; anything else
            // means an external actor moved them — retire the case.
            let bay_row = ctx.matrix.row_of(&case.bay);
            let occupant_ok = bay_row
                .map(|r| &ctx.hospital.rooms[r as usize])
                .and_then(|room| room.status.patient())
                .is_some_and(|p| p.id == case.patient);
            if !occupant_ok {
                ctx.log(format!(
                    "ED case retired — bay {} no longer holds its patient",
                    case.bay
                ));
                continue;
            }

            match case.boarding_need {
                None if ctx.now_min < case.due_min => self.cases.push(case),
                None => match case.dispo {
                    EdDispo::Release => {
                        if let Ok(p) = ops::discharge(ctx.hospital, ctx.index, &case.bay) {
                            self.stats.releases += 1;
                            ctx.log(format!("ED release: {} ← {}", p.alias, case.bay));
                        }
                    }
                    EdDispo::Admit => {
                        // Decision to admit: draw the needed level of care,
                        // flag the patient as boarding, start hunting a bed.
                        let need = draw_admit_need(ctx.rng);
                        if let Some(p) = ctx.hospital.rooms[bay_row.unwrap() as usize]
                            .status
                            .patient_mut()
                        {
                            p.boarding = true;
                        }
                        self.stats.admits += 1;
                        ctx.log(format!(
                            "ED decision to admit ({}) — {} boarding",
                            need.label(),
                            case.bay
                        ));
                        case.boarding_need = Some(need);
                        self.cases.push(case);
                    }
                },
                Some(need) => {
                    if !self.try_place(ctx, &mut pools, &case, need) {
                        self.cases.push(case); // still boarding
                    }
                }
            }
        }
    }

    /// First-fit placement of a boarder into a vacant bed of `need` type.
    /// Returns true when the patient was transferred out of the ED.
    fn try_place(
        &mut self,
        ctx: &mut SimCtx,
        pools: &mut [Option<Vec<u32>>],
        case: &EdCase,
        need: UnitType,
    ) -> bool {
        let m = ctx.matrix;
        let pool = pools[need.index()].get_or_insert_with(|| {
            (0..m.len() as u32)
                .filter(|&i| {
                    m.occupancy[i as usize] == OccState::Vacant && m.unit_type[i as usize] == need
                })
                .collect()
        });
        while let Some(&row) = pool.first() {
            let row = row as usize;
            if !ctx.hospital.rooms[row].status.is_vacant()
                || m.unit_pos[row] == NO_UNIT
                || ctx.transport.is_held(&ctx.hospital.rooms[row].id)
            {
                pool.remove(0); // stale, orphaned, or reserved — keep looking
                continue;
            }
            let dest = m.room_id(row as u32).clone();
            // Timed transport (EP-6): hold the bed and hand the intent to the
            // transport board instead of teleporting. The LOS draws happen
            // here either way, so the RNG pattern per placement is identical
            // in both modes; `BedTransport` schedules the discharge and
            // stamps the destination service at arrival.
            if ctx.transport.timed_active() {
                let line = ctx.hospital.units[m.unit_pos[row] as usize]
                    .service_line
                    .clone();
                let los_hr =
                    sampling::lognormal_hours(ctx.rng, &params::los_for(line.as_ref(), need));
                let dc_at = sampling::shape_discharge_min(ctx.rng, ctx.now_min + los_hr * 60.0);
                ctx.transport.held.insert(dest.clone());
                ctx.transport.requests.push(TransportRequest {
                    from: case.bay.clone(),
                    to: dest,
                    patient: case.patient.clone(),
                    discharge_at_min: dc_at,
                });
                pool.remove(0);
                return true; // the transport board owns the move from here
            }
            if ops::transfer(ctx.hospital, ctx.index, &case.bay, &dest).is_err() {
                pool.remove(0);
                continue;
            }
            pool.remove(0);
            // Landed in a licensed bed (ops::transfer cleared `boarding`).
            // Stamp the destination unit's service and schedule the
            // inpatient discharge from its service line's LOS.
            let (service, line) = {
                let u = &ctx.hospital.units[m.unit_pos[row] as usize];
                (u.service.clone(), u.service_line.clone())
            };
            let mut alias = String::new();
            if let Some(p) = ctx.hospital.rooms[row].status.patient_mut() {
                p.service = Some(service);
                alias = p.alias.clone();
            }
            let los_hr = sampling::lognormal_hours(ctx.rng, &params::los_for(line.as_ref(), need));
            let dc_at = sampling::shape_discharge_min(ctx.rng, ctx.now_min + los_hr * 60.0);
            ctx.queue.schedule(
                dc_at,
                SimEventKind::Discharge {
                    room: dest.clone(),
                    expect_patient: Some(case.patient.clone()),
                },
            );
            ctx.log(format!("boarding resolved: {alias} {} → {dest}", case.bay));
            return true;
        }
        false
    }

    /// New ED arrivals into vacant bays, λ(t)-shaped, sub-stepped like
    /// [`crate::diurnal::DiurnalFlow`].
    fn arrivals(&mut self, ctx: &mut SimCtx) {
        let m = ctx.matrix;
        let mut bays: Vec<u32> = (0..m.len() as u32)
            .filter(|&i| {
                m.occupancy[i as usize] == OccState::Vacant
                    && m.unit_type[i as usize] == UnitType::Ed
            })
            .collect();

        let n_sub = (ctx.dt_min / params::SUBSTEP_MIN).ceil().max(1.0) as usize;
        let sub_dt = ctx.dt_min / n_sub as f64;
        let tick_start = ctx.now_min - ctx.dt_min;

        for s in 0..n_sub {
            let t = tick_start + sub_dt * (s as f64 + 1.0);
            let rate = params::diurnal_rate_per_hr(
                self.base_arrivals_per_hr,
                &params::ED_ARRIVAL_SHAPE,
                params::WEEKEND_ED_FACTOR,
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
                self.stats.arrivals += 1;
                if !self.arrive_one(ctx, &mut bays, t) {
                    self.stats.diverted += 1;
                    ctx.log("ED arrival diverted — all bays full");
                }
            }
        }
    }

    fn arrive_one(&mut self, ctx: &mut SimCtx, bays: &mut Vec<u32>, t: f64) -> bool {
        loop {
            if bays.is_empty() {
                return false;
            }
            let k = ctx.rng.random_range(0..bays.len());
            let row = bays.swap_remove(k) as usize;
            if !ctx.hospital.rooms[row].status.is_vacant() {
                continue;
            }
            let bay = ctx.matrix.room_id(row as u32).clone();

            self.counter += 1;
            let instability = sampling::draw_initial_instability(ctx.rng, UnitType::Ed);
            let patient = sampling::synth_patient(
                ctx.rng,
                "ed",
                self.counter,
                t,
                instability,
                None,
                "synthetic ED visit",
            );
            let pid = patient.id.clone();
            if ops::admit(ctx.hospital, ctx.index, &bay, patient).is_err() {
                continue;
            }

            // Disposition decided now, timing drawn now — one fixed RNG
            // pattern per arrival keeps the stream easy to reason about.
            let will_admit = ctx.rng.random_bool(params::ED_ADMIT_FRACTION);
            let (dispo, los) = if will_admit {
                (EdDispo::Admit, &params::ED_DECISION_TO_ADMIT)
            } else {
                (EdDispo::Release, &params::ED_TREAT_RELEASE_LOS)
            };
            let due_min = t + sampling::lognormal_hours(ctx.rng, los) * 60.0;
            self.cases.push(EdCase {
                bay,
                patient: pid,
                due_min,
                dispo,
                boarding_need: None,
            });
            return true;
        }
    }
}

/// Draw the level of care an ED-converted admission needs, from
/// [`params::ED_ADMIT_MIX`].
fn draw_admit_need(rng: &mut rand_chacha::ChaCha8Rng) -> UnitType {
    let mut x: f64 = rng.random_range(0.0..1.0);
    for (t, p) in params::ED_ADMIT_MIX {
        if x < p {
            return t;
        }
        x -= p;
    }
    params::ED_ADMIT_MIX[0].0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::SimEngine;
    use crate::testworld::world;
    use hupsim_core::model::Hospital;

    fn boarding_count(h: &Hospital) -> usize {
        h.rooms
            .iter()
            .filter_map(|r| r.status.patient())
            .filter(|p| p.boarding)
            .count()
    }

    #[test]
    fn boarding_occurs_when_inpatient_capacity_is_constrained() {
        // 12 ED bays feeding 3 med/surg + 1 ICU + 1 stepdown beds: admits
        // must board. LOS median ≈ 4 days ≫ the 3-day run, so beds that
        // fill stay full.
        let (mut h, idx) = world(&[
            ("ed", UnitType::Ed, 12),
            ("ms", UnitType::MedSurg, 3),
            ("icu", UnitType::Icu, 1),
            ("sd", UnitType::Stepdown, 1),
        ]);
        let mut eng = SimEngine::new(9);
        eng.register(Box::new(EdPressure::default()), true);

        let mut max_boarding = 0;
        let mut saw_resolution = false;
        for _ in 0..(3 * 96) {
            eng.tick(&mut h, &idx, 15.0);
            max_boarding = max_boarding.max(boarding_count(&h));
            if eng.log.iter().any(|l| l.message.contains("boarding resolved")) {
                saw_resolution = true;
            }
        }
        assert!(
            max_boarding >= 1,
            "constrained inpatient capacity must produce ED boarding"
        );
        assert!(
            saw_resolution,
            "the first few admits should have found the initially-vacant beds"
        );
        // Sustained pressure also overflows the bays themselves.
        assert!(
            eng.log.iter().any(|l| l.message.contains("diverted"))
                || max_boarding + 5 < 12,
            "expected diversion pressure in a 12-bay ED under this load"
        );
    }

    #[test]
    fn boarders_transfer_out_when_a_bed_frees() {
        // One med/surg bed, occupied; one boarder waiting. Discharging the
        // bed must pull the boarder out of the ED on the next tick.
        let (mut h, idx) = world(&[("ed", UnitType::Ed, 4), ("ms", UnitType::MedSurg, 1)]);
        let mut eng = SimEngine::new(1);
        let mut ed = EdPressure::default();
        // No new arrivals: isolate the boarding→placement mechanics.
        ed.base_arrivals_per_hr = 0.0;
        // Hand-craft the state: bed occupied, bay holds a boarding admit.
        let bed_patient = crate::sampling::synth_patient(
            &mut eng.rng,
            "seed",
            1,
            0.0,
            0.3,
            Some("ms service".into()),
            "",
        );
        hupsim_core::ops::admit(&mut h, &idx, &"ms.r0".into(), bed_patient).unwrap();
        let boarder = crate::sampling::synth_patient(
            &mut eng.rng,
            "seed",
            2,
            0.0,
            0.5,
            None,
            "",
        );
        let boarder_id = boarder.id.clone();
        hupsim_core::ops::admit(&mut h, &idx, &"ed.r0".into(), boarder).unwrap();
        h.rooms
            .iter_mut()
            .find(|r| r.id == "ed.r0".into())
            .and_then(|r| r.status.patient_mut())
            .unwrap()
            .boarding = true;
        ed.cases.push(EdCase {
            bay: "ed.r0".into(),
            patient: boarder_id.clone(),
            due_min: 0.0,
            dispo: EdDispo::Admit,
            boarding_need: Some(UnitType::MedSurg),
        });
        eng.register(Box::new(ed), true);

        // Bed still occupied: boarder stays put.
        eng.tick(&mut h, &idx, 15.0);
        assert!(boarding_count(&h) == 1, "no bed yet — still boarding");

        // Free the bed; next tick the boarder must move in, flag cleared.
        hupsim_core::ops::discharge(&mut h, &idx, &"ms.r0".into()).unwrap();
        eng.tick(&mut h, &idx, 15.0);
        let ms_patient = h
            .rooms
            .iter()
            .find(|r| r.id == "ms.r0".into())
            .and_then(|r| r.status.patient())
            .expect("boarder should occupy the freed bed");
        assert_eq!(ms_patient.id, boarder_id);
        assert!(!ms_patient.boarding, "transfer into a licensed bed clears boarding");
        assert_eq!(
            ms_patient.service.as_deref(),
            Some("ms service"),
            "admission carries the destination unit's service"
        );
        assert_eq!(boarding_count(&h), 0);
    }

    /// EP-17: standing ED occupants (seeded census / loaded scenario) are
    /// adopted on the first step — seeded boarders hunt beds, seeded
    /// walk-ins release on residual episode clocks — instead of sitting
    /// immortal (the EP-21 open thread).
    #[test]
    fn standing_ed_occupants_are_adopted_and_flow() {
        let (mut h, idx) = world(&[("ed", UnitType::Ed, 6), ("ms", UnitType::MedSurg, 4)]);
        // A seeded boarder (decision pre-snapshot), a backdated walk-in past
        // its median episode, and an unknown-stamp occupant.
        for (room, name, admitted, boarding) in [
            ("ed.r0", "boarder", Some(-400.0), true),
            ("ed.r1", "walkin", Some(-6.0 * 60.0), false),
            ("ed.r2", "legacy", None, false),
        ] {
            let mut p = crate::sampling::synth_patient(
                &mut rand::SeedableRng::seed_from_u64(1),
                "x",
                0,
                0.0,
                0.3,
                None,
                "",
            );
            p.id = name.into();
            p.alias = name.into();
            p.admitted_at_min = admitted;
            p.boarding = boarding;
            hupsim_core::ops::admit(&mut h, &idx, &room.into(), p).unwrap();
        }
        let mut eng = SimEngine::new(5);
        let mut ed = EdPressure::default();
        ed.base_arrivals_per_hr = 0.0; // isolate adoption mechanics
        eng.register(Box::new(ed), true);

        eng.tick(&mut h, &idx, 15.0);
        assert!(
            eng.log
                .iter()
                .any(|l| l.message == "ED standing census: adopted 3 occupants"),
            "all three standing occupants get cases"
        );
        // The boarder finds a vacant med/surg bed within the first ticks.
        eng.tick(&mut h, &idx, 15.0);
        let placed = h
            .rooms
            .iter()
            .filter(|r| r.unit == "ms".into())
            .filter_map(|r| r.status.patient())
            .any(|p| p.id == "boarder".into());
        assert!(placed, "the adopted boarder must place into the vacant bed");

        // Within a few days every adopted occupant has flowed: released,
        // admitted (bay → bed), or boarding — none left caseless forever.
        for _ in 0..(4 * 96) {
            eng.tick(&mut h, &idx, 15.0);
        }
        for name in ["walkin", "legacy"] {
            let still_in_bay = h
                .rooms
                .iter()
                .filter(|r| r.unit == "ed".into())
                .filter_map(|r| r.status.patient())
                .any(|p| p.id == name.into() && !p.boarding);
            assert!(
                !still_in_bay,
                "{name} should have released, transferred, or be boarding by day 4"
            );
        }
    }

    /// Adoption is deterministic and one-shot, and a reset rearms it.
    #[test]
    fn adoption_is_deterministic_and_rearms_on_reset() {
        let seeded = || {
            let (mut h, idx) = world(&[("ed", UnitType::Ed, 4), ("ms", UnitType::MedSurg, 2)]);
            for i in 0..3 {
                let mut p = crate::sampling::synth_patient(
                    &mut rand::SeedableRng::seed_from_u64(9),
                    "s",
                    i,
                    0.0,
                    0.3,
                    None,
                    "",
                );
                p.id = format!("s{i}").into();
                p.admitted_at_min = Some(-100.0 * (i as f64 + 1.0));
                p.boarding = i == 0;
                hupsim_core::ops::admit(&mut h, &idx, &format!("ed.r{i}").into(), p).unwrap();
            }
            (h, idx)
        };
        let run = |seed: u64| {
            let (mut h, idx) = seeded();
            let mut eng = SimEngine::new(seed);
            eng.register(Box::new(EdPressure::default()), true);
            for _ in 0..96 {
                eng.tick(&mut h, &idx, 15.0);
            }
            h
        };
        assert_eq!(run(3), run(3), "same seed, same adopted trajectories");

        // Reset rearms: the fresh world's occupants get adopted again.
        let (mut h, idx) = seeded();
        let mut eng = SimEngine::new(3);
        eng.register(Box::new(EdPressure::default()), true);
        eng.tick(&mut h, &idx, 15.0);
        eng.reset(3);
        let (mut h2, idx2) = seeded();
        eng.tick(&mut h2, &idx2, 15.0);
        assert!(
            eng.log
                .iter()
                .any(|l| l.message.contains("adopted 3 occupants")),
            "a reset engine must re-adopt the standing census"
        );
    }
}
