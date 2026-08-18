//! EP-11 headless acceptance smoke: the real compiled hospital with the
//! demo census, the EP-10 process stack on, two sim days. Verifies what the
//! interactive acceptance run checks by eye: every occupied room of a
//! staffed unit has an RN, roster sizes follow ⌈census/ratio⌉, workload
//! breakdowns sum to their scalars, and the 07:00/19:00 handoff lines land
//! in the sim log.

use hupsim_core::index::HospitalIndex;
use hupsim_core::workload;
use hupsim_sim::prelude::*;

#[test]
fn real_world_rosters_staff_every_occupied_room_and_log_handoffs() {
    let (topo, units, layout, lines) = hupsim_data::io::embedded();
    let (mut h, _) =
        hupsim_data::compile(topo, units, layout, lines).expect("embedded data compiles");
    let idx = HospitalIndex::build(&h);
    hupsim_data::seed_demo_census(&mut h, &idx, 0xC0FFEE, 0.0);

    let mut eng = SimEngine::new(0xC0FFEE);
    eng.register(Box::new(AcuityDrift::default()), true);
    eng.register(Box::new(DiurnalFlow::default()), true);
    eng.register(Box::new(EdPressure::default()), true);
    eng.log_cap = 100_000; // keep every handoff line for the assertions

    for _ in 0..(2 * 96) {
        eng.tick(&mut h, &idx, 15.0); // 2 sim days at the timeline cadence
    }

    let mut assigned = 0usize;
    for u in h.units.iter().filter(|u| workload::is_rn_staffed(u.unit_type)) {
        let ur = eng.roster.unit(&u.id).expect("staffed unit has a roster");
        let census = idx
            .room_indices(&u.id)
            .iter()
            .filter(|&&i| h.rooms[i].status.patient().is_some())
            .count();
        if census > 0 {
            let ratio = workload::effective_rn_ratio(u);
            assert_eq!(
                ur.rn_count,
                ((census as f32 / ratio).ceil() as usize).max(1),
                "{}: roster size must track census/ratio",
                u.name
            );
        }
        for &ri in idx.room_indices(&u.id) {
            let room = &h.rooms[ri];
            if room.status.patient().is_some() {
                assert!(
                    ur.assignment.contains_key(&room.id),
                    "{}: occupied room {} has no RN",
                    u.name,
                    room.number
                );
                assigned += 1;
            }
        }
        for w in &ur.workloads {
            let sum = w.census_vs_ratio + w.instability + w.overhead + w.churn;
            assert!((w.scalar() - sum).abs() < 1e-5, "breakdown must sum to scalar");
        }
    }
    assert!(
        assigned > 400,
        "demo census should occupy hundreds of staffed rooms, got {assigned}"
    );

    // Two sim days cross four shift boundaries (07:00/19:00 daily).
    let handoffs = eng
        .log
        .iter()
        .filter(|l| l.message.contains("shift change"))
        .count();
    assert!(
        handoffs >= 4,
        "07:00/19:00 handoff lines must appear in the log, got {handoffs}"
    );
}
