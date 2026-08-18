//! hupsim-sim — simulation layer for the HUP model.
//!
//! Infrastructure: a sim clock, a deterministic event queue, the
//! [`process::SimProcess`] trait, a bounded [`timeline::Timeline`] sampler,
//! and trailing-24 h [`rolling`] norms fed at the timeline's cadence (EP-9).
//!
//! Models (EP-10): [`diurnal::DiurnalFlow`] (time-of-day shaped admissions
//! with service-specific LOS), [`edflow::EdPressure`] (ED arrivals, admit
//! conversion, boarding), and [`acuity::AcuityDrift`] (level-of-care-aware
//! instability drift). Parameters are synthetic-but-shaped, in one visible
//! table: [`params`]. The original proof-of-loop demo process remains in
//! [`demo`], clearly labeled.
//!
//! Standing census (EP-21): [`backlog::BacklogDischarges`] — a one-shot
//! process that schedules residual-LOS exits for the seeded (or loaded)
//! census, so a mid-stream world breathes instead of saturating; the
//! conditioned resample lives in [`sampling`].
//!
//! Staffing (EP-11): [`roster::RnRoster`] — per-unit anonymized RNs with
//! deterministic acuity-balanced room assignment, 07:00/19:00 shift
//! turnover, and per-RN workload burden (composite in
//! `hupsim_core::workload`). Ephemeral sim state — never serialized.
//!
//! Experiments (EP-15): [`experiment`] — staged interventions (unit closure,
//! arrival wave, ratio flex) and the headless A/B runner that replays the
//! current world under baseline vs intervention with one shared seed.
//!
//! Reporting (EP-16): [`report`] — the bed-huddle morning report: a pure,
//! deterministic snapshot (overnight admits, anticipated discharges,
//! boarding, pressure tables) with a markdown export.
//!
//! Forecasting (EP-17): [`forecast`] — the analytic short-horizon census
//! fan (median + 10/90 quantiles), model-consistent with the generator's own
//! λ/LOS/discharge tables, conditioned on the standing census's elapsed
//! stays. Pure and RNG-free; never reads the event queue.
//!
//! Transport (EP-6): [`transport`] — timed intra-campus bed transports over
//! the core route graph (bridges, Connector, podium, elevators). State lives
//! on the engine-owned [`transport::TransportBoard`]; the RNG-free
//! [`transport::BedTransport`] process departs requests, carries patients
//! off-bed, and admits them on arrival. In-flight transports are ephemeral —
//! dropped on reset/load, like queued events.

pub mod acuity;
pub mod backlog;
pub mod clock;
pub mod demo;
pub mod diurnal;
pub mod edflow;
pub mod engine;
pub mod events;
pub mod experiment;
pub mod forecast;
pub mod params;
pub mod process;
pub mod report;
pub mod rolling;
pub mod roster;
pub mod sampling;
pub mod timeline;
pub mod transport;

pub mod prelude {
    pub use crate::acuity::AcuityDrift;
    pub use crate::backlog::BacklogDischarges;
    pub use crate::clock::SimClock;
    pub use crate::demo::DemoFlow;
    pub use crate::diurnal::DiurnalFlow;
    pub use crate::edflow::EdPressure;
    pub use crate::engine::{SimEngine, SimLogEntry};
    pub use crate::events::{EventQueue, ScheduledEvent, SimEventKind};
    pub use crate::experiment::{
        run_experiment, summarize, ExperimentResult, ExperimentSpec, Intervention, MetricSeries,
        ProcessToggles, RunSeries, SeriesKey, SeriesSummary, VariantRun,
    };
    pub use crate::forecast::{forecast_census, CensusFan, ForecastParams};
    pub use crate::process::{SimCtx, SimProcess};
    pub use crate::report::{
        assemble as assemble_report, sim_time_label, BoarderRow, DischargeRow, MorningReport,
        NormBadge, OvernightRow, PressureRow, ReportInputs,
    };
    pub use crate::rolling::{MetricWindows, RollingSeries, RollingStore, WINDOW_24H};
    pub use crate::roster::{RnRoster, UnitRoster};
    pub use crate::timeline::{Timeline, TimelineSample, UnitSample};
    pub use crate::transport::{
        ActiveTransport, BedTransport, TransportBoard, TransportRequest,
    };
}

/// Shared synthetic worlds for this crate's tests.
#[cfg(test)]
pub(crate) mod testworld {
    use hupsim_core::index::HospitalIndex;
    use hupsim_core::model::*;
    use hupsim_core::provenance::Provenance;

    /// Build a hospital from `(unit_id, unit_type, bed_count)` specs. Room
    /// kinds follow the unit type (ICU → IcuBay, ED → EdBay, else Inpatient);
    /// ICU/stepdown rooms are telemetry-capable.
    pub fn world(spec: &[(&str, UnitType, usize)]) -> (Hospital, HospitalIndex) {
        let mut units = Vec::new();
        let mut rooms = Vec::new();
        for (uid, ut, beds) in spec {
            units.push(Unit {
                id: (*uid).into(),
                name: uid.to_uppercase(),
                service: format!("{uid} service"),
                service_line: None,
                unit_type: *ut,
                building: "b".into(),
                level: FloorLevel::Numbered(1),
                elevator_core: None,
                target_rn_ratio: None,
                provenance: Provenance::default(),
                note: None,
            });
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
            buildings: vec![],
            units,
            rooms,
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![],
        };
        let idx = HospitalIndex::build(&h);
        (h, idx)
    }
}
