//! Census/acuity aggregation: room → unit → floor/building rollups that drive
//! the color spectrum at every zoom level.

use crate::color::{stability_color, Rgb, COLOR_VACANT};
use crate::ids::{NodeId, UnitId};
use crate::index::HospitalIndex;
use crate::model::{FloorLevel, Hospital, RoomStatus};
use serde::{Deserialize, Serialize};

/// How multi-room aggregates map onto the spectrum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AggregateMode {
    /// Mean instability of occupied rooms — overall unit tone.
    #[default]
    Mean,
    /// Worst (max) instability — "is anyone crashing in there".
    Worst,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UnitAggregate {
    pub capacity: usize,
    pub census: usize,
    pub out_of_service: usize,
    pub boarding: usize,
    pub mean_instability: Option<f32>,
    pub worst_instability: Option<f32>,
}

impl UnitAggregate {
    pub fn occupancy_frac(&self) -> f32 {
        let staffed = self.capacity.saturating_sub(self.out_of_service);
        if staffed == 0 {
            0.0
        } else {
            self.census as f32 / staffed as f32
        }
    }

    pub fn instability(&self, mode: AggregateMode) -> Option<f32> {
        match mode {
            AggregateMode::Mean => self.mean_instability,
            AggregateMode::Worst => self.worst_instability,
        }
    }

    /// Spectrum color; vacant-gray when nobody is home.
    pub fn color(&self, mode: AggregateMode) -> Rgb {
        match self.instability(mode) {
            Some(x) => stability_color(x),
            None => COLOR_VACANT,
        }
    }

    /// Fold another aggregate into this one (unit → building/floor/group
    /// rollups). Public since EP-9: the app-side aggregate cache and the
    /// rolling-stats groups combine per-unit aggregates this way.
    pub fn absorb(&mut self, other: &UnitAggregate) {
        // Recombine means census-weighted; worst is max.
        let self_sum = self.mean_instability.unwrap_or(0.0) * self.census as f32;
        let other_sum = other.mean_instability.unwrap_or(0.0) * other.census as f32;
        self.capacity += other.capacity;
        self.out_of_service += other.out_of_service;
        self.boarding += other.boarding;
        let total_census = self.census + other.census;
        self.mean_instability = if total_census > 0 {
            Some((self_sum + other_sum) / total_census as f32)
        } else {
            None
        };
        self.worst_instability = match (self.worst_instability, other.worst_instability) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.census = total_census;
    }
}

pub fn aggregate_unit(h: &Hospital, idx: &HospitalIndex, unit: &UnitId) -> UnitAggregate {
    let mut agg = UnitAggregate::default();
    let mut sum = 0.0f32;
    for &ri in idx.room_indices(unit) {
        let room = &h.rooms[ri];
        agg.capacity += 1;
        match &room.status {
            RoomStatus::Vacant => {}
            RoomStatus::OutOfService { .. } => agg.out_of_service += 1,
            RoomStatus::Occupied { patient } => {
                agg.census += 1;
                if patient.boarding {
                    agg.boarding += 1;
                }
                let x = patient.acuity.instability.clamp(0.0, 1.0);
                sum += x;
                agg.worst_instability = Some(agg.worst_instability.map_or(x, |w: f32| w.max(x)));
            }
        }
    }
    if agg.census > 0 {
        agg.mean_instability = Some(sum / agg.census as f32);
    }
    agg
}

/// Aggregate across all units of a building, optionally restricted to one floor.
pub fn aggregate_building(
    h: &Hospital,
    idx: &HospitalIndex,
    building: &NodeId,
    level: Option<&FloorLevel>,
) -> UnitAggregate {
    let mut agg = UnitAggregate::default();
    for uid in idx.unit_ids(building) {
        if let Some(ui) = idx.unit_pos.get(uid) {
            if let Some(lv) = level {
                if &h.units[*ui].level != lv {
                    continue;
                }
            }
            let ua = aggregate_unit(h, idx, uid);
            agg.absorb(&ua);
        }
    }
    agg
}

/// Whole-hospital headline numbers for the status bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HospitalStats {
    pub capacity: usize,
    pub census: usize,
    pub out_of_service: usize,
    pub boarding: usize,
    pub critical: usize,
    pub unstable: usize,
}

pub fn hospital_stats(h: &Hospital) -> HospitalStats {
    let mut s = HospitalStats::default();
    for room in &h.rooms {
        s.capacity += 1;
        match &room.status {
            RoomStatus::Vacant => {}
            RoomStatus::OutOfService { .. } => s.out_of_service += 1,
            RoomStatus::Occupied { patient } => {
                s.census += 1;
                if patient.boarding {
                    s.boarding += 1;
                }
                use crate::patient::StabilityTier as T;
                match patient.acuity.tier() {
                    T::Critical => s.critical += 1,
                    T::Unstable => s.unstable += 1,
                    _ => {}
                }
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use crate::patient::{Acuity, Patient};
    use crate::provenance::Provenance;

    fn room(id: &str, unit: &str, status: RoomStatus) -> Room {
        Room {
            id: id.into(),
            number: id.into(),
            unit: unit.into(),
            kind: RoomKind::Inpatient,
            telemetry_capable: false,
            negative_pressure: false,
            status,
            provenance: Provenance::default(),
        }
    }

    fn occupied(instability: f32) -> RoomStatus {
        RoomStatus::Occupied {
            patient: Patient {
                id: "p".into(),
                alias: "P".into(),
                age: None,
                service: None,
                admitted_at_min: None,
                acuity: Acuity {
                    instability,
                    trend_per_hr: 0.0,
                },
                boarding: false,
                isolation: None,
                telemetry: false,
                note: String::new(),
            },
        }
    }

    #[test]
    fn unit_aggregate_mean_and_worst() {
        let h = Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![],
            units: vec![Unit {
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
            }],
            rooms: vec![
                room("r1", "u", occupied(0.2)),
                room("r2", "u", occupied(0.8)),
                room("r3", "u", RoomStatus::Vacant),
                room(
                    "r4",
                    "u",
                    RoomStatus::OutOfService {
                        reason: "clean".into(),
                    },
                ),
            ],
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![],
        };
        let idx = HospitalIndex::build(&h);
        let a = aggregate_unit(&h, &idx, &"u".into());
        assert_eq!(a.capacity, 4);
        assert_eq!(a.census, 2);
        assert_eq!(a.out_of_service, 1);
        assert!((a.mean_instability.unwrap() - 0.5).abs() < 1e-6);
        assert!((a.worst_instability.unwrap() - 0.8).abs() < 1e-6);
        // 2 occupied of 3 staffed beds
        assert!((a.occupancy_frac() - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn empty_unit_is_vacant_gray() {
        let a = UnitAggregate::default();
        assert_eq!(a.color(AggregateMode::Mean), COLOR_VACANT);
    }
}
