//! `BedTransport` — timed intra-campus patient transport over the EP-6 route
//! graph. Turns transfer intents (today: ED boarder placements) into
//! depart → in-transport → arrive sequences whose durations come from the
//! physical campus topology: bridges, the Connector, the ground podium, and
//! elevator hops.
//!
//! State lives on the engine-owned [`TransportBoard`] (the `RnRoster`
//! precedent), shared with every process through `SimCtx`: requesters push
//! [`TransportRequest`]s and honor the `held` bed set; this process departs
//! them, carries the patient **off-bed** in `active` (the EP-6 "place for the
//! patient to exist off-bed"), and admits them on arrival. The process draws
//! no RNG — durations are deterministic route lookups — so it can register
//! anywhere without perturbing other processes' streams; requesters draw any
//! randomness (e.g. LOS) at request time so their pattern matches the
//! instant-transfer path.
//!
//! In-flight transports are engine state, not scenario state: like queued
//! events, they are dropped by save/load and cleared by reset (the
//! determinism tests below cover reset *with transports in flight*).

use crate::process::{SimCtx, SimProcess};
use hupsim_core::ids::{PatientId, RoomId};
use hupsim_core::model::{Hospital, RoomKind};
use hupsim_core::ops;
use hupsim_core::patient::Patient;
use hupsim_core::route::RouteGraph;
use std::collections::BTreeSet;

use crate::events::SimEventKind;
use hupsim_core::index::HospitalIndex;

/// Fallback transport duration when no route resolves (minutes). Should not
/// happen for compiled data — the reachability test guards it — but a wedge
/// must never be silent or infinite. PROVENANCE: synthetic.
pub const TRANSPORT_FALLBACK_MIN: f64 = 10.0;

/// Minimum minutes between arrival and any pre-drawn discharge firing, so a
/// discharge drawn short can never beat the patient to the bed.
const MIN_POST_ARRIVAL_MIN: f64 = 15.0;

#[derive(Debug, Clone)]
pub struct TransportRequest {
    pub from: RoomId,
    pub to: RoomId,
    pub patient: PatientId,
    /// Absolute sim minute of the scheduled inpatient discharge — drawn by
    /// the requester (same RNG pattern as the instant path), scheduled by
    /// this process at arrival.
    pub discharge_at_min: f64,
}

#[derive(Debug, Clone)]
pub struct ActiveTransport {
    /// The patient exists here, off-bed, while in transport.
    pub patient: Patient,
    pub origin: RoomId,
    pub dest: RoomId,
    pub depart_min: f64,
    pub eta_min: f64,
    /// Depart-time route summary, e.g. `"via Silverstein–Clifton skybridge"`.
    pub via: String,
    pub discharge_at_min: f64,
    waiting_logged: bool,
}

/// Engine-owned transport state, shared with all processes via `SimCtx`.
#[derive(Debug, Default)]
pub struct TransportBoard {
    /// Master switch for timed transports (UI toolbar). Off — or no graph —
    /// means requesters fall back to instant `ops::transfer`.
    pub timed: bool,
    /// Route graph over the campus; built by the app (it owns the layout).
    /// Headless engines without one keep instant transfers.
    pub graph: Option<RouteGraph>,
    pub requests: Vec<TransportRequest>,
    pub active: Vec<ActiveTransport>,
    /// Beds reserved for an inbound transport. Bed-hunting processes must
    /// skip these; ordered set so iteration (if ever needed) is
    /// deterministic.
    pub held: BTreeSet<RoomId>,
}

impl TransportBoard {
    /// True when transfer intents should detour through timed transport.
    pub fn timed_active(&self) -> bool {
        self.timed && self.graph.is_some()
    }

    pub fn is_held(&self, room: &RoomId) -> bool {
        self.held.contains(room)
    }

    /// Drop all in-flight state (reset/scenario load). Keeps the graph and
    /// the `timed` toggle — those are configuration, not run state.
    pub fn clear_flight(&mut self) {
        self.requests.clear();
        self.active.clear();
        self.held.clear();
    }
}

/// `"Founders 5"` — the transport log's endpoint vocabulary.
fn room_node_label(h: &Hospital, idx: &HospitalIndex, room: &RoomId) -> String {
    let loc = idx
        .room_pos
        .get(room)
        .and_then(|&ri| idx.unit_pos.get(&h.rooms[ri].unit))
        .map(|&ui| (&h.units[ui].building, &h.units[ui].level));
    match loc {
        Some((b, l)) => {
            let short = h
                .buildings
                .iter()
                .find(|bl| &bl.id == b)
                .map(|bl| bl.short_name.as_str())
                .unwrap_or_else(|| b.as_str());
            format!("{} {}", short, l.label())
        }
        None => room.to_string(),
    }
}

#[derive(Debug, Default)]
pub struct BedTransport;

impl SimProcess for BedTransport {
    fn name(&self) -> &str {
        "Bed transport"
    }

    fn description(&self) -> &str {
        "Timed intra-campus transports over the physical route graph (bridges, Connector, podium, elevators). Durations from surveyed geometry where it exists; inferred walk/elevator constants otherwise — see core::route::RouteParams."
    }

    fn step(&mut self, ctx: &mut SimCtx) {
        if ctx.dt_min <= 0.0 {
            return;
        }
        self.departs(ctx);
        self.arrivals(ctx);
    }

    // All state lives on the engine-owned board; `SimEngine::reset` clears it
    // (with in-flight transports) via `TransportBoard::clear_flight`.
    fn reset(&mut self) {}
}

impl BedTransport {
    fn departs(&mut self, ctx: &mut SimCtx) {
        let requests = std::mem::take(&mut ctx.transport.requests);
        for req in requests {
            // The origin must still hold the requested patient; anything else
            // means an external actor moved them — cancel and release the bed.
            let occupant_ok = ctx
                .index
                .room_pos
                .get(&req.from)
                .and_then(|&i| ctx.hospital.rooms[i].status.patient())
                .is_some_and(|p| p.id == req.patient);
            if !occupant_ok {
                ctx.transport.held.remove(&req.to);
                ctx.log(format!(
                    "transport canceled — {} no longer holds its patient",
                    req.from
                ));
                continue;
            }
            let (minutes, via) = match ctx
                .transport
                .graph
                .as_ref()
                .expect("timed requests exist only when a graph does")
                .route_rooms(ctx.hospital, ctx.index, &req.from, &req.to)
            {
                Ok(route) => (route.minutes as f64, route.via_summary()),
                Err(e) => {
                    ctx.log(format!("transport fallback — {e}"));
                    (TRANSPORT_FALLBACK_MIN, "route unavailable".to_string())
                }
            };
            let Ok(patient) = ops::discharge(ctx.hospital, ctx.index, &req.from) else {
                ctx.transport.held.remove(&req.to);
                continue;
            };
            let from_label = room_node_label(ctx.hospital, ctx.index, &req.from);
            let to_label = room_node_label(ctx.hospital, ctx.index, &req.to);
            ctx.log(format!(
                "transport depart: {} — {} → {} {}, {:.0} min",
                patient.alias, from_label, to_label, via, minutes
            ));
            ctx.transport.active.push(ActiveTransport {
                patient,
                origin: req.from,
                dest: req.to,
                depart_min: ctx.now_min,
                eta_min: ctx.now_min + minutes,
                via,
                discharge_at_min: req.discharge_at_min,
                waiting_logged: false,
            });
        }
    }

    fn arrivals(&mut self, ctx: &mut SimCtx) {
        let active = std::mem::take(&mut ctx.transport.active);
        for mut t in active {
            if ctx.now_min < t.eta_min {
                ctx.transport.active.push(t);
                continue;
            }
            // Preferred bed, else first-fit within the destination unit (the
            // bed may have been taken by an editor action; patients do wait
            // in hallways). Deterministic: unit room order.
            let dest_unit = ctx
                .index
                .room_pos
                .get(&t.dest)
                .map(|&i| ctx.hospital.rooms[i].unit.clone());
            let mut landing: Option<RoomId> = None;
            if let Some(&i) = ctx.index.room_pos.get(&t.dest) {
                if ctx.hospital.rooms[i].status.is_vacant() {
                    landing = Some(t.dest.clone());
                }
            }
            if landing.is_none() {
                if let Some(unit) = dest_unit.as_ref() {
                    landing = ctx
                        .index
                        .room_indices(unit)
                        .iter()
                        .copied()
                        .find(|&i| {
                            let r = &ctx.hospital.rooms[i];
                            r.status.is_vacant() && !ctx.transport.is_held(&r.id)
                        })
                        .map(|i| ctx.hospital.rooms[i].id.clone());
                }
            }
            let Some(landing) = landing else {
                if !t.waiting_logged {
                    ctx.log(format!(
                        "transport waiting — no vacant bed for {} at {}",
                        t.patient.alias,
                        room_node_label(ctx.hospital, ctx.index, &t.dest)
                    ));
                    t.waiting_logged = true;
                }
                ctx.transport.active.push(t);
                continue;
            };
            let diverted = landing != t.dest;
            // Same invariant as ops::transfer: landing in a licensed bed
            // clears the boarding flag; carry the destination unit's service.
            let li = ctx.index.room_pos[&landing];
            let mut patient = t.patient.clone();
            if matches!(
                ctx.hospital.rooms[li].kind,
                RoomKind::Inpatient | RoomKind::IcuBay | RoomKind::LaborDelivery | RoomKind::NurseryBay
            ) {
                patient.boarding = false;
            }
            if let Some(&ui) = ctx.index.unit_pos.get(&ctx.hospital.rooms[li].unit) {
                patient.service = Some(ctx.hospital.units[ui].service.clone());
            }
            let alias = patient.alias.clone();
            let pid = patient.id.clone();
            if ops::admit(ctx.hospital, ctx.index, &landing, patient).is_err() {
                // Cannot happen — landing was verified vacant this tick — but
                // a lost patient is the one unacceptable failure: keep
                // carrying and retry next tick.
                ctx.transport.active.push(t);
                continue;
            }
            ctx.transport.held.remove(&t.dest);
            if diverted {
                ctx.log(format!(
                    "transport diverted: {alias} → {landing} ({} unavailable)",
                    t.dest
                ));
            }
            ctx.log(format!("transport arrive: {alias} → {landing}"));
            ctx.queue.schedule(
                t.discharge_at_min.max(ctx.now_min + MIN_POST_ARRIVAL_MIN),
                SimEventKind::Discharge {
                    room: landing,
                    expect_patient: Some(pid),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edflow::EdPressure;
    use crate::engine::SimEngine;
    use crate::testworld::world;
    use hupsim_core::layout::{BuildingLayout, CampusLayout, Footprint, GroundPlate};
    use hupsim_core::model::*;
    use hupsim_core::provenance::Provenance;
    use hupsim_core::route::{RouteGraph, RouteParams, TraversalPolicy};

    /// Two towers on one podium plate: ED at tower A ground, inpatient beds
    /// up tower B — so ED-to-bed transports really traverse elevator + podium
    /// hops with route-derived durations.
    fn two_building_world() -> (Hospital, HospitalIndex, RouteGraph) {
        fn building(id: &str, floors: std::ops::RangeInclusive<i32>) -> Building {
            Building {
                id: id.into(),
                name: id.to_string(),
                short_name: id.to_string(),
                kind: BuildingKind::Wing,
                built: None,
                aliases: vec![],
                floors: floors
                    .map(|n| FloorSpec {
                        level: if n == 0 {
                            FloorLevel::Ground
                        } else {
                            FloorLevel::Numbered(n)
                        },
                        has_patient_rooms: true,
                        contents: vec![],
                        note: None,
                    })
                    .collect(),
                has_inpatient_beds: true,
                provenance: Provenance::default(),
                note: None,
            }
        }
        fn unit(id: &str, ut: UnitType, bldg: &str, level: i32) -> Unit {
            Unit {
                id: id.into(),
                name: id.to_uppercase(),
                service: format!("{id} service"),
                service_line: None,
                unit_type: ut,
                building: bldg.into(),
                level: if level == 0 {
                    FloorLevel::Ground
                } else {
                    FloorLevel::Numbered(level)
                },
                elevator_core: None,
                target_rn_ratio: None,
                provenance: Provenance::default(),
                note: None,
            }
        }
        let mut rooms = Vec::new();
        let spec: &[(&str, UnitType, usize)] = &[
            ("ed", UnitType::Ed, 10),
            ("ms", UnitType::MedSurg, 6),
            ("icu", UnitType::Icu, 2),
            ("sd", UnitType::Stepdown, 2),
        ];
        for (uid, ut, beds) in spec {
            let kind = match ut {
                UnitType::Icu => RoomKind::IcuBay,
                UnitType::Ed => RoomKind::EdBay,
                _ => RoomKind::Inpatient,
            };
            for i in 0..*beds {
                rooms.push(Room {
                    id: format!("{uid}.r{i}").into(),
                    number: format!("{i}"),
                    unit: (*uid).into(),
                    kind,
                    telemetry_capable: matches!(ut, UnitType::Icu | UnitType::Stepdown),
                    negative_pressure: false,
                    status: RoomStatus::Vacant,
                    provenance: Provenance::default(),
                });
            }
        }
        let h = Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![building("twr.a", 0..=2), building("twr.b", 0..=3)],
            units: vec![
                unit("ed", UnitType::Ed, "twr.a", 0),
                unit("ms", UnitType::MedSurg, "twr.b", 2),
                unit("icu", UnitType::Icu, "twr.b", 3),
                unit("sd", UnitType::Stepdown, "twr.b", 3),
            ],
            rooms,
            connections: vec![],
            elevator_cores: vec![ElevatorCore {
                name: "B core".into(),
                building: Some("twr.b".into()),
                serves_patient_floors: vec![2, 3],
                note: None,
            }],
            service_lines: vec![],
        };
        let square = |cx: f32| Footprint {
            vertices: vec![
                [cx - 10.0, -10.0],
                [cx + 10.0, -10.0],
                [cx + 10.0, 10.0],
                [cx - 10.0, 10.0],
            ],
        };
        let layout = CampusLayout {
            origin_lat: 0.0,
            origin_lon: 0.0,
            buildings: vec![
                BuildingLayout {
                    node: "twr.a".into(),
                    footprint: square(0.0),
                    levels: 3,
                    floor_height_m: 4.0,
                    ground_elevation_m: 0.0,
                    provenance: Provenance::default(),
                },
                BuildingLayout {
                    node: "twr.b".into(),
                    footprint: square(100.0),
                    levels: 4,
                    floor_height_m: 4.0,
                    ground_elevation_m: 0.0,
                    provenance: Provenance::default(),
                },
            ],
            ground_plates: vec![GroundPlate {
                footprint: Footprint {
                    vertices: vec![[-50.0, -50.0], [150.0, -50.0], [150.0, 50.0], [-50.0, 50.0]],
                },
                label: None,
                extrude_m: Some(3.5),
                provenance: Provenance::default(),
            }],
            bridges: vec![],
            insets: vec![],
            notes: vec![],
        };
        let graph =
            RouteGraph::build(&h, &layout, RouteParams::default(), TraversalPolicy::default());
        let idx = HospitalIndex::build(&h);
        (h, idx, graph)
    }

    fn engine_with_transport(seed: u64, graph: RouteGraph) -> SimEngine {
        let mut eng = SimEngine::new(seed);
        eng.transport.timed = true;
        eng.transport.graph = Some(graph);
        eng.register(Box::new(EdPressure::default()), true);
        eng.register(Box::new(BedTransport), true);
        eng
    }

    fn run(seed: u64, ticks: usize, dt: f64) -> Hospital {
        let (mut h, idx, graph) = two_building_world();
        let mut eng = engine_with_transport(seed, graph);
        for _ in 0..ticks {
            eng.tick(&mut h, &idx, dt);
        }
        h
    }

    #[test]
    fn timed_placement_departs_and_arrives() {
        let (mut h, idx, graph) = two_building_world();
        let mut eng = engine_with_transport(11, graph);
        let mut saw_depart = false;
        let mut saw_off_bed = false;
        let mut saw_arrive = false;
        for _ in 0..(4 * 96) {
            eng.tick(&mut h, &idx, 15.0);
            saw_depart |= eng.log.iter().any(|l| l.message.starts_with("transport depart"));
            saw_off_bed |= !eng.transport.active.is_empty();
            saw_arrive |= eng.log.iter().any(|l| l.message.starts_with("transport arrive"));
            if saw_depart && saw_arrive {
                break;
            }
        }
        assert!(saw_depart, "ED admits must depart as timed transports");
        assert!(saw_off_bed, "patients exist off-bed while in transport");
        assert!(saw_arrive, "transports must arrive");
        assert!(
            !eng.log.iter().any(|l| l.message.contains("boarding resolved")),
            "timed mode replaces the instant boarding-resolved path"
        );
        // Every departed transport's route summary made it into the log.
        assert!(eng
            .log
            .iter()
            .filter(|l| l.message.starts_with("transport depart"))
            .all(|l| l.message.contains(" min")));
    }

    #[test]
    fn held_beds_are_not_double_booked() {
        let (mut h, idx, graph) = two_building_world();
        let mut eng = engine_with_transport(5, graph);
        for _ in 0..(6 * 96) {
            eng.tick(&mut h, &idx, 15.0);
            // Invariant: a held bed is vacant (it is being reserved), and no
            // two actives target the same bed.
            for room in &eng.transport.held {
                let i = idx.room_pos[room];
                assert!(
                    h.rooms[i].status.is_vacant(),
                    "held bed {room} must stay vacant until arrival"
                );
            }
            let mut dests: Vec<&RoomId> =
                eng.transport.active.iter().map(|t| &t.dest).collect();
            dests.sort();
            let n = dests.len();
            dests.dedup();
            assert_eq!(n, dests.len(), "two transports target one bed");
        }
    }

    #[test]
    fn same_seed_same_world_with_transport() {
        assert_eq!(run(21, 300, 30.0), run(21, 300, 30.0));
    }

    #[test]
    fn reset_replays_exactly_with_transports_in_flight() {
        let fresh = run(7, 300, 30.0);

        let (mut h, idx, graph) = two_building_world();
        let mut eng = engine_with_transport(999, graph);
        // Dirty the engine until at least one transport is genuinely in
        // flight, so reset provably clears mid-transport state.
        let mut ticks = 0;
        while eng.transport.active.is_empty() && ticks < 2000 {
            eng.tick(&mut h, &idx, 30.0);
            ticks += 1;
        }
        assert!(
            !eng.transport.active.is_empty(),
            "test needs a transport in flight before reset"
        );
        eng.reset(7);
        assert!(eng.transport.active.is_empty(), "reset drops in-flight transports");
        assert!(eng.transport.held.is_empty(), "reset drops held beds");

        let (mut h2, idx2, _g) = two_building_world();
        for _ in 0..300 {
            eng.tick(&mut h2, &idx2, 30.0);
        }
        assert_eq!(fresh, h2, "reset engine must replay identically");
    }

    #[test]
    fn no_graph_keeps_instant_transfers() {
        let (mut h, idx) = world(&[("ed", UnitType::Ed, 10), ("ms", UnitType::MedSurg, 4)]);
        let mut eng = SimEngine::new(3);
        eng.transport.timed = true; // on, but no graph — instant path stays
        eng.register(Box::new(EdPressure::default()), true);
        eng.register(Box::new(BedTransport), true);
        for _ in 0..(4 * 96) {
            eng.tick(&mut h, &idx, 15.0);
        }
        assert!(
            !eng.log.iter().any(|l| l.message.starts_with("transport depart")),
            "without a route graph the instant path must be untouched"
        );
    }
}
