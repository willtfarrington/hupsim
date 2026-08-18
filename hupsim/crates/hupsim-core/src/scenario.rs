//! Scenario files: the full serializable world state. One JSON document
//! captures structure + census + sim clock + RNG seed, so a scenario is
//! reproducible and diffable.

use crate::model::Hospital;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SCENARIO_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ScenarioMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// RFC 3339 timestamps, set by the caller (core stays clock-free).
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub modified: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub schema_version: u32,
    pub meta: ScenarioMeta,
    /// Simulation clock, minutes since scenario epoch.
    #[serde(default)]
    pub sim_time_min: f64,
    /// Seed for the deterministic simulation RNG.
    #[serde(default)]
    pub rng_seed: u64,
    pub hospital: Hospital,
}

#[derive(Debug, Error)]
pub enum ScenarioError {
    #[error("scenario JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported scenario schema version {found} (this build reads ≤ {supported})")]
    SchemaVersion { found: u32, supported: u32 },
}

impl Scenario {
    pub fn new(name: impl Into<String>, hospital: Hospital) -> Self {
        Self {
            schema_version: SCENARIO_SCHEMA_VERSION,
            meta: ScenarioMeta {
                name: name.into(),
                ..Default::default()
            },
            sim_time_min: 0.0,
            rng_seed: 0xC0FFEE,
            hospital,
        }
    }

    pub fn to_json(&self) -> Result<String, ScenarioError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(s: &str) -> Result<Self, ScenarioError> {
        let sc: Scenario = serde_json::from_str(s)?;
        if sc.schema_version > SCENARIO_SCHEMA_VERSION {
            return Err(ScenarioError::SchemaVersion {
                found: sc.schema_version,
                supported: SCENARIO_SCHEMA_VERSION,
            });
        }
        Ok(sc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::HospitalMeta;

    fn empty_hospital() -> Hospital {
        Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![],
            units: vec![],
            rooms: vec![],
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![],
        }
    }

    #[test]
    fn roundtrip() {
        let sc = Scenario::new("test", empty_hospital());
        let json = sc.to_json().unwrap();
        let back = Scenario::from_json(&json).unwrap();
        assert_eq!(sc, back);
    }

    /// EP-21: backdated (negative) admit stamps ride the existing
    /// `Option<f64>` field — schema v1, byte-honest round-trip.
    #[test]
    fn negative_admit_stamps_roundtrip() {
        use crate::model::{Room, RoomKind, RoomStatus, Unit, UnitType, FloorLevel};
        use crate::patient::{Acuity, Patient};
        use crate::provenance::Provenance;

        let mut h = empty_hospital();
        h.units.push(Unit {
            id: "u".into(),
            name: "U".into(),
            service: "S".into(),
            service_line: None,
            unit_type: UnitType::MedSurg,
            building: "b".into(),
            level: FloorLevel::Numbered(1),
            elevator_core: None,
            target_rn_ratio: None,
            provenance: Provenance::default(),
            note: None,
        });
        h.rooms.push(Room {
            id: "r".into(),
            number: "1".into(),
            unit: "u".into(),
            kind: RoomKind::Inpatient,
            telemetry_capable: false,
            negative_pressure: false,
            status: RoomStatus::Occupied {
                patient: Patient {
                    id: "p".into(),
                    alias: "P. Backdated".into(),
                    age: Some(60),
                    service: None,
                    admitted_at_min: Some(-5432.1),
                    acuity: Acuity { instability: 0.2, trend_per_hr: 0.0 },
                    boarding: false,
                    isolation: None,
                    telemetry: false,
                    note: String::new(),
                },
            },
            provenance: Provenance::default(),
        });
        let sc = Scenario::new("backdated", h);
        assert_eq!(sc.schema_version, SCENARIO_SCHEMA_VERSION, "schema v1 untouched");
        let back = Scenario::from_json(&sc.to_json().unwrap()).unwrap();
        assert_eq!(sc, back);
        let p = back.hospital.rooms[0].status.patient().unwrap();
        assert_eq!(p.admitted_at_min, Some(-5432.1));
    }

    #[test]
    fn future_schema_rejected() {
        let mut sc = Scenario::new("test", empty_hospital());
        sc.schema_version = 999;
        let json = sc.to_json().unwrap();
        assert!(Scenario::from_json(&json).is_err());
    }
}
