//! Transient lookup index over a [`Hospital`]. Rebuild after structural edits
//! (adding/removing rooms/units/buildings); room-*status* edits never
//! invalidate it because positions are stable.

use crate::ids::{NodeId, RoomId, UnitId};
use crate::model::Hospital;
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct HospitalIndex {
    pub building_pos: HashMap<NodeId, usize>,
    pub unit_pos: HashMap<UnitId, usize>,
    pub room_pos: HashMap<RoomId, usize>,
    /// Room indices per unit, in room-number order as authored.
    pub rooms_by_unit: HashMap<UnitId, Vec<usize>>,
    /// Unit ids per building, in authored order.
    pub units_by_building: HashMap<NodeId, Vec<UnitId>>,
}

impl HospitalIndex {
    pub fn build(h: &Hospital) -> Self {
        let mut idx = Self::default();
        for (i, b) in h.buildings.iter().enumerate() {
            idx.building_pos.insert(b.id.clone(), i);
            idx.units_by_building.entry(b.id.clone()).or_default();
        }
        for (i, u) in h.units.iter().enumerate() {
            idx.unit_pos.insert(u.id.clone(), i);
            idx.units_by_building
                .entry(u.building.clone())
                .or_default()
                .push(u.id.clone());
            idx.rooms_by_unit.entry(u.id.clone()).or_default();
        }
        for (i, r) in h.rooms.iter().enumerate() {
            idx.room_pos.insert(r.id.clone(), i);
            idx.rooms_by_unit.entry(r.unit.clone()).or_default().push(i);
        }
        idx
    }

    pub fn room_indices(&self, unit: &UnitId) -> &[usize] {
        self.rooms_by_unit
            .get(unit)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn unit_ids(&self, building: &NodeId) -> &[UnitId] {
        self.units_by_building
            .get(building)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use crate::provenance::Provenance;

    fn tiny_hospital() -> Hospital {
        Hospital {
            meta: HospitalMeta {
                name: "T".into(),
                ..Default::default()
            },
            buildings: vec![Building {
                id: "b1".into(),
                name: "B1".into(),
                short_name: "B1".into(),
                kind: BuildingKind::Wing,
                built: None,
                aliases: vec![],
                floors: vec![],
                has_inpatient_beds: true,
                provenance: Provenance::default(),
                note: None,
            }],
            units: vec![Unit {
                id: "u1".into(),
                name: "U1".into(),
                service: "GenMed".into(),
                service_line: None,
                unit_type: UnitType::MedSurg,
                building: "b1".into(),
                level: FloorLevel::Numbered(5),
                elevator_core: None,
                target_rn_ratio: None,
                provenance: Provenance::default(),
                note: None,
            }],
            rooms: vec![
                Room {
                    id: "r1".into(),
                    number: "501".into(),
                    unit: "u1".into(),
                    kind: RoomKind::Inpatient,
                    telemetry_capable: false,
                    negative_pressure: false,
                    status: RoomStatus::Vacant,
                    provenance: Provenance::default(),
                },
                Room {
                    id: "r2".into(),
                    number: "502".into(),
                    unit: "u1".into(),
                    kind: RoomKind::Inpatient,
                    telemetry_capable: false,
                    negative_pressure: false,
                    status: RoomStatus::Vacant,
                    provenance: Provenance::default(),
                },
            ],
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![],
        }
    }

    #[test]
    fn index_maps_everything() {
        let h = tiny_hospital();
        let idx = HospitalIndex::build(&h);
        assert_eq!(idx.building_pos.len(), 1);
        assert_eq!(idx.room_indices(&"u1".into()).len(), 2);
        assert_eq!(idx.unit_ids(&"b1".into()), &[UnitId::from("u1")]);
    }
}
