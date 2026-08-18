//! EP-16 acceptance, headless: the real compiled hospital with the EP-10
//! processes on and the seeded demo census (main.rs parity — the huddle is
//! about a full house). Since EP-21 the seeded census is backdated and
//! `BacklogDischarges` schedules its residual-LOS exits, so the report reads
//! like a real morning sheet: LOS-gated anticipated discharges (no pre-sim
//! flood), finite boarding waits, and real bed churn in the log window. Run
//! to day-2 07:00; the report must populate all four sections sanely, match
//! the dashboard drawer rollups, and be seed-reproducible.

use hupsim_core::index::HospitalIndex;
use hupsim_core::matrix::RoomMatrix;
use hupsim_core::model::{Hospital, UnitType};
use hupsim_sim::prelude::*;
use hupsim_sim::report::{assemble, ReportInputs};

const SEED: u64 = 42;
/// Day-2 07:00 in sim minutes, reached in 15-min ticks (the sampling cadence).
const TARGET_MIN: f64 = 1440.0 + 7.0 * 60.0;

fn run_world_to(target_min: f64) -> (Hospital, HospitalIndex, SimEngine) {
    let loaded = hupsim_data::load_default().expect("embedded data compiles");
    let mut h = loaded.hospital;
    let idx = HospitalIndex::build(&h);
    hupsim_data::seed_demo_census(&mut h, &idx, SEED, 0.0);

    let mut eng = SimEngine::new(SEED);
    // SimDrive registration order: standing-census exits first (EP-21).
    eng.register(Box::new(BacklogDischarges::default()), true);
    eng.register(Box::new(AcuityDrift::default()), true);
    eng.register(Box::new(DiurnalFlow::default()), true);
    eng.register(Box::new(EdPressure::default()), true);
    let ticks = (target_min / 15.0) as usize;
    for _ in 0..ticks {
        eng.tick(&mut h, &idx, 15.0);
    }
    assert_eq!(eng.clock.now_min, target_min);
    (h, idx, eng)
}

fn run_world_to_seven_am() -> (Hospital, HospitalIndex, SimEngine) {
    run_world_to(TARGET_MIN)
}

fn report_of(h: &Hospital, idx: &HospitalIndex, eng: &SimEngine) -> hupsim_sim::report::MorningReport {
    // A fresh matrix for the end-of-run hospital — exactly what the app's
    // aggregate cache hands the report (the engine's copy is tick-start).
    let m = RoomMatrix::build(h, idx);
    assemble(&ReportInputs {
        hospital: h,
        matrix: &m,
        rolling: &eng.rolling,
        log: &eng.log,
        now_min: eng.clock.now_min,
        scenario_name: "smoke",
        seed: eng.seed(),
    })
}

#[test]
fn morning_report_populates_and_matches_the_drawer_rollups() {
    let (h, idx, eng) = run_world_to_seven_am();
    let report = report_of(&h, &idx, &eng);

    // 1 — overnight admits: the diurnal + ED day must have brought people in
    // during the trailing 12 h, and the section's groups must sum to it.
    assert!(report.overnight_total > 0, "a running house admits overnight");
    assert_eq!(
        report.overnight.iter().map(|r| r.count).sum::<usize>(),
        report.overnight_total
    );
    for row in &report.overnight {
        assert_eq!(
            row.tiers.iter().sum::<u32>() as usize,
            row.count,
            "tier mix must partition the {} admits",
            row.name
        );
    }
    // EP-21 restores the strict corroboration (the EP-23 re-pin accepted ED
    // decisions alone while the immortal-seeded house held zero bed churn):
    // with backdating + BacklogDischarges, a fully visible window must show
    // real bed admissions AND real inpatient discharges.
    assert!(
        (report.log_admissions > 0 && report.log_discharges > 0) || !report.log_covers_window,
        "a fully visible window must corroborate real bed churn (admissions {}, discharges {})",
        report.log_admissions,
        report.log_discharges
    );

    // 2 — anticipated discharges: LOS-gated now that every seeded patient is
    // backdated — a plausible morning list, no pre-sim flood.
    assert!(!report.discharges.is_empty());
    assert_eq!(
        report.discharges_pre_sim, 0,
        "the seeded census is backdated — the pre-sim rule covers nobody"
    );
    let inpatient_census: usize = {
        let m = RoomMatrix::build(&h, &idx);
        m.rollup_by_unit_type()
            .iter()
            .filter(|(t, _)| hupsim_sim::params::is_inpatient_unit(*t))
            .map(|(_, agg)| agg.census)
            .sum()
    };
    assert!(
        report.discharges.len() * 2 < inpatient_census,
        "anticipated discharges ({}) must be a morning-list fraction of the {} inpatients, \
         not ~half the house",
        report.discharges.len(),
        inpatient_census
    );
    for dc in &report.discharges {
        assert!(dc.los_hr.is_some(), "every discharge candidate has a known LOS now");
    }

    // 3 — boarding matches the hospital's own headline count exactly, and
    // every wait is finite hours (no "pre-sim" boarders).
    let stats = hupsim_core::aggregate::hospital_stats(&h);
    assert_eq!(report.boarders.len(), stats.boarding);
    for b in &report.boarders {
        let w = b.waited_hr.expect("backdated boarders wait a known number of hours");
        assert!(w > 0.0 && w.is_finite());
    }

    // 4 — pressure tables: numbers must equal the same matrix rollups the
    // EP-13 drawer reads (census/staffed per level of care, per line).
    let m = RoomMatrix::build(&h, &idx);
    for (t, agg) in m.rollup_by_unit_type() {
        let Some(row) = report.pressure_by_loc.iter().find(|r| r.name == t.label()) else {
            assert_eq!(agg.capacity, 0, "{} row missing despite beds", t.label());
            continue;
        };
        assert_eq!(row.census, agg.census, "{} census", t.label());
        assert_eq!(
            row.staffed,
            agg.capacity - agg.out_of_service,
            "{} staffed",
            t.label()
        );
        assert_eq!(row.boarding, agg.boarding, "{} boarding", t.label());
    }
    assert!(
        report
            .pressure_by_loc
            .iter()
            .any(|r| r.name == UnitType::Icu.label()),
        "the compiled hospital has ICU beds"
    );
    assert_eq!(
        report.pressure_by_line.len(),
        m.rollup_by_service_line(h.service_lines.len())
            .iter()
            .filter(|a| a.capacity > 0)
            .count(),
        "one pressure row per bedded service line"
    );
    // After a 31 h run the 24 h norms are warm: real z badges exist.
    assert!(
        report
            .pressure_by_loc
            .iter()
            .any(|r| matches!(r.occ_badge, hupsim_sim::report::NormBadge::Z(_) | hupsim_sim::report::NormBadge::Flat)),
        "trailing-24 h norms must be warm by day 2"
    );

    // The exported sheet is a valid document with the reproducibility line.
    let md = report.to_markdown();
    assert!(md.starts_with("# Morning report — day 2, 07:00"));
    assert!(md.contains(&format!("seed `{SEED}`")));
    assert!(md.contains("## Pressure — by service line"));
}

/// EP-21: the day-1 07:00 report on a fresh seeded start. The overnight
/// window reaches back past t0; backdated arrivals whose stamps fall inside
/// it are *intended* to count — the snapshot pretends the hospital predates
/// t0 (pinned here, changed nowhere).
#[test]
fn day_one_report_reads_like_a_morning_sheet() {
    let (h, idx, eng) = run_world_to(7.0 * 60.0);
    let report = report_of(&h, &idx, &eng);

    // Anticipated discharges: present, LOS-gated, a fraction of the house.
    let stats = hupsim_core::aggregate::hospital_stats(&h);
    assert!(!report.discharges.is_empty(), "day-1 morning list must populate");
    assert!(
        report.discharges.len() * 2 < stats.census,
        "morning list ({}) ≪ census ({})",
        report.discharges.len(),
        stats.census
    );
    assert_eq!(report.discharges_pre_sim, 0, "no pre-sim flood on a backdated seed");

    // Boarders: since EP-17 the ED *adopts* its seeded occupants — boarders
    // place first-fit as the morning finds beds — so a day-1 07:00 list may
    // legitimately be empty (the pre-EP-17 pin asserted seeded boarders
    // still waiting here; their waits grew without bound, the EP-21 open
    // thread). What must hold now: adoption fired, and anyone still listed
    // waits finite, bounded hours.
    assert!(
        eng.log
            .iter()
            .any(|l| l.message.starts_with("ED standing census: adopted")),
        "the ED must adopt its seeded occupants on the first tick"
    );
    for b in &report.boarders {
        let w = b.waited_hr.expect("backdated boarders show hours, not pre-sim");
        assert!(
            (0.0..36.0).contains(&w),
            "no unbounded seeded-boarder waits after adoption: {w} h"
        );
    }

    // Overnight pin: the section must equal the matrix-side count of
    // in-window admission stamps (inpatient level of care or boarding) —
    // including the *backdated* stamps in [-300, now]. At least one seeded
    // (negative-stamp) patient must be part of it.
    let m = RoomMatrix::build(&h, &idx);
    let window_start = eng.clock.now_min - hupsim_sim::report::OVERNIGHT_LOOKBACK_MIN;
    let mut expected = 0usize;
    let mut backdated_in_window = 0usize;
    for i in 0..m.len() {
        if m.occupancy[i] != hupsim_core::matrix::OccState::Occupied {
            continue;
        }
        let admit = m.admitted_at_min[i];
        if admit == hupsim_core::matrix::NO_ADMIT_MIN || (admit as f64) < window_start {
            continue;
        }
        if hupsim_sim::params::is_inpatient_unit(m.unit_type[i]) || m.boarding[i] {
            expected += 1;
            if admit < 0.0 {
                backdated_in_window += 1;
            }
        }
    }
    assert_eq!(
        report.overnight_total, expected,
        "overnight section must count exactly the in-window stamps"
    );
    assert!(
        backdated_in_window > 0,
        "backdated arrivals inside the trailing window are intended to count"
    );
}

#[test]
fn same_seed_reproduces_the_exact_report_text() {
    let (h1, idx1, eng1) = run_world_to_seven_am();
    let (h2, idx2, eng2) = run_world_to_seven_am();
    let a = report_of(&h1, &idx1, &eng1);
    let b = report_of(&h2, &idx2, &eng2);
    assert_eq!(a, b, "same seed, same world, same report model");
    assert_eq!(
        a.to_markdown(),
        b.to_markdown(),
        "the seed line makes the exported snapshot reproducible"
    );
}
