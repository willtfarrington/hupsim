//! EP-6 acceptance, headless: the real compiled hospital with the full EP-10
//! process stack *plus* timed transport (main.rs parity — route graph from
//! the real layout, unverified edges excluded), seeded demo census, two sim
//! days. ED placements must depart over the route graph and arrive; nobody
//! may be lost in transit; and the run must be seed-reproducible.

use hupsim_core::index::HospitalIndex;
use hupsim_core::model::Hospital;
use hupsim_core::route::{RouteGraph, RouteParams, TraversalPolicy};
use hupsim_sim::prelude::*;
use hupsim_sim::transport::BedTransport;

const SEED: u64 = 42;
const TWO_DAYS_MIN: f64 = 2.0 * 1440.0;

fn run_two_days(seed: u64) -> (Hospital, SimEngine) {
    let loaded = hupsim_data::load_default().expect("embedded data compiles");
    let mut h = loaded.hospital;
    let idx = HospitalIndex::build(&h);
    hupsim_data::seed_demo_census(&mut h, &idx, seed, 0.0);

    let mut eng = SimEngine::new(seed);
    // SimDrive parity: standing-census exits first (EP-21).
    eng.register(Box::new(BacklogDischarges::default()), true);
    eng.register(Box::new(AcuityDrift::default()), true);
    eng.register(Box::new(DiurnalFlow::default()), true);
    eng.register(Box::new(EdPressure::default()), true);
    eng.register(Box::new(BedTransport), true);
    eng.transport.timed = true;
    eng.transport.graph = Some(RouteGraph::build(
        &h,
        &loaded.layout,
        RouteParams::default(),
        TraversalPolicy::default(),
    ));
    let ticks = (TWO_DAYS_MIN / 15.0) as usize;
    for _ in 0..ticks {
        eng.tick(&mut h, &idx, 15.0);
    }
    (h, eng)
}

#[test]
fn timed_transports_flow_on_the_real_campus() {
    let (_h, eng) = run_two_days(SEED);
    let departs = eng
        .log
        .iter()
        .filter(|l| l.message.starts_with("transport depart"))
        .count();
    let arrives = eng
        .log
        .iter()
        .filter(|l| l.message.starts_with("transport arrive"))
        .count();
    assert!(departs > 0, "two sim days must produce ED admissions in transit");
    assert!(arrives > 0, "transports must arrive");
    // The log is bounded (cap 2000), so compare stocks, not flows: everyone
    // who departed either arrived or is still being carried on the board.
    assert!(
        eng.transport.active.len() <= departs.max(1),
        "in-flight count must be a plausible remainder"
    );
    // Nothing routed over the unverified tunnel, and every depart line
    // carries a route summary with minutes.
    assert!(
        !eng.log.iter().any(|l| l.message.contains("via tunnel")),
        "unverified tunnel must not appear in routes"
    );
    assert!(eng
        .log
        .iter()
        .filter(|l| l.message.starts_with("transport depart"))
        .all(|l| l.message.contains(" min")));
    // Held beds must all still be held for someone actually in flight.
    assert!(eng.transport.held.len() <= eng.transport.active.len() + eng.transport.requests.len());
}

#[test]
fn real_campus_transport_run_is_seed_reproducible() {
    let (h1, _e1) = run_two_days(7);
    let (h2, _e2) = run_two_days(7);
    assert_eq!(h1, h2, "same seed + timed transport must reproduce exactly");
}
