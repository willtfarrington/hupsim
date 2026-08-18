//! EP-14 headless acceptance smoke: the real compiled hospital with the
//! demo census and the EP-10/EP-11 stack, a 24 h warm-up, then the brief's
//! interactive checks run headlessly — an ICU + telemetry + airborne query
//! yields only vacant negative-pressure ICU rooms; the overflow toggle
//! surfaces stepdown candidates explicitly labeled and always below every
//! exact-LOC candidate; handing off a transfer through the sanctioned op
//! drops the destination room from the results on re-run.

use hupsim_core::index::HospitalIndex;
use hupsim_core::matrix::{OccState, RoomMatrix};
use hupsim_core::model::UnitType;
use hupsim_core::ops;
use hupsim_core::query::{self, LocTier, PlacementContext, PlacementQuery, PlacementWeights};
use hupsim_sim::prelude::*;

/// Assemble the scoring context the app's query panel builds each Run:
/// fresh matrix + aggregates, occupancy z from the rolling norms, covering
/// RNs from the roster.
fn run_placement(
    h: &hupsim_core::model::Hospital,
    idx: &HospitalIndex,
    eng: &SimEngine,
    q: &PlacementQuery,
) -> Vec<query::PlacementResult> {
    let m = RoomMatrix::build(h, idx);
    let aggs = m.unit_aggregates(h.units.len());
    let unit_occ_z: Vec<Option<f32>> = h
        .units
        .iter()
        .enumerate()
        .map(|(i, u)| {
            eng.rolling
                .units
                .get(&u.id)
                .and_then(|w| w.occupancy_frac.z(aggs[i].occupancy_frac()))
        })
        .collect();
    let covering = eng.roster.covering_rn_loads(h, idx);
    let ctx = PlacementContext {
        matrix: &m,
        unit_aggs: &aggs,
        unit_occ_z: &unit_occ_z,
        covering_rn: &covering,
    };
    query::run(&ctx, q, &PlacementWeights::default())
}

#[test]
fn icu_tele_airborne_query_matches_the_acceptance_run() {
    let (topo, units, layout, lines) = hupsim_data::io::embedded();
    let (mut h, _) =
        hupsim_data::compile(topo, units, layout, lines).expect("embedded data compiles");
    let idx = HospitalIndex::build(&h);
    hupsim_data::seed_demo_census(&mut h, &idx, 0xC0FFEE, 0.0);

    let mut eng = SimEngine::new(0xC0FFEE);
    eng.register(Box::new(AcuityDrift::default()), true);
    eng.register(Box::new(DiurnalFlow::default()), true);
    eng.register(Box::new(EdPressure::default()), true);
    for _ in 0..96 {
        eng.tick(&mut h, &idx, 15.0); // 24 h at the sampling cadence
    }

    // The demo day can saturate a level of care entirely (it does, at this
    // seed, for stepdown). Guarantee one vacant negative-pressure ICU bed
    // and one vacant stepdown bed by opening the first occupied candidate
    // through the sanctioned discharge op — the interactive flow where a bed
    // opens up — then bring the roster up to date the same way the app's
    // roster_sync does after paused-clock editor mutations.
    for (label, want) in [
        ("negative-pressure ICU", UnitType::Icu),
        ("stepdown", UnitType::Stepdown),
    ] {
        let rows: Vec<usize> = h
            .units
            .iter()
            .filter(|u| u.unit_type == want)
            .flat_map(|u| idx.room_indices(&u.id).to_vec())
            .filter(|&ri| want != UnitType::Icu || h.rooms[ri].negative_pressure)
            .collect();
        assert!(!rows.is_empty(), "the compiled hospital has {label} rooms");
        if !rows.iter().any(|&ri| h.rooms[ri].status.is_vacant()) {
            let room = h.rooms[rows[0]].id.clone();
            ops::discharge(&mut h, &idx, &room).expect("occupied room discharges");
        }
    }
    let now = eng.clock.now_min;
    let mut log = Vec::new();
    eng.roster.maintain(&h, &idx, now, &mut log);

    // 1. Hard filters: candidates are only vacant negative-pressure ICU rooms.
    let q = PlacementQuery {
        target: UnitType::Icu,
        telemetry: true,
        airborne: true,
        ..Default::default()
    };
    let results = run_placement(&h, &idx, &eng, &q);
    assert!(!results.is_empty(), "at least the opened NP ICU bed must qualify");
    let m = RoomMatrix::build(&h, &idx);
    for r in &results {
        let row = r.row as usize;
        assert_eq!(m.occupancy[row], OccState::Vacant);
        assert!(m.negative_pressure[row], "airborne query must only pass negative-pressure rooms");
        assert_eq!(m.unit_type[row], UnitType::Icu);
        assert_eq!(r.tier, LocTier::Exact);
    }

    // 2. Overflow surfaces stepdown, explicitly labeled, never above exact.
    let q_overflow = PlacementQuery {
        target: UnitType::Icu,
        overflow: true,
        ..Default::default()
    };
    let results = run_placement(&h, &idx, &eng, &q_overflow);
    assert!(
        results
            .iter()
            .any(|r| r.tier == LocTier::Overflow && r.unit_type == UnitType::Stepdown),
        "the overflow toggle must surface stepdown candidates"
    );
    let first_overflow = results.iter().position(|r| r.tier == LocTier::Overflow).unwrap();
    assert!(
        results[first_overflow..].iter().all(|r| r.tier == LocTier::Overflow),
        "every exact-LOC candidate must precede every overflow candidate"
    );

    // 3. Stage → act: a transfer staged from a real ICU patient goes through
    // ops::transfer, and the destination drops from the results on re-run.
    let from_row = (0..m.len())
        .find(|&i| m.occupancy[i] == OccState::Occupied && m.unit_type[i] == UnitType::Icu)
        .expect("the demo census occupies ICU beds");
    let from = m.room_id(from_row as u32).clone();
    let q_transfer = PlacementQuery {
        target: UnitType::Icu,
        from_room: Some(from.clone()),
        ..Default::default()
    };
    let results = run_placement(&h, &idx, &eng, &q_transfer);
    assert!(results.iter().all(|r| r.room != from), "the patient's own room is never a candidate");
    let dest = results.first().expect("an ICU bed is open").room.clone();
    ops::transfer(&mut h, &idx, &from, &dest).expect("handoff through the sanctioned op");
    eng.roster.maintain(&h, &idx, now, &mut log);
    let rerun = run_placement(&h, &idx, &eng, &q_transfer);
    assert!(
        rerun.iter().all(|r| r.room != dest),
        "the destination room must drop from the results once occupied"
    );
    // The vacated origin room is a fresh candidate for other queries.
    let q_plain = PlacementQuery {
        target: UnitType::Icu,
        ..Default::default()
    };
    assert!(
        run_placement(&h, &idx, &eng, &q_plain).iter().any(|r| r.room == from),
        "the vacated origin becomes available to subsequent queries"
    );
}
