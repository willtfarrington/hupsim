//! Census operations — the only sanctioned ways to move patients. Used by both
//! the interactive editor and simulation processes so invariants live in one
//! place.

use crate::ids::RoomId;
use crate::index::HospitalIndex;
use crate::model::{Hospital, RoomStatus};
use crate::patient::Patient;
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum OpError {
    #[error("room not found: {0}")]
    RoomNotFound(RoomId),
    #[error("room {0} is not vacant")]
    NotVacant(RoomId),
    #[error("room {0} has no patient")]
    NoPatient(RoomId),
    #[error("room {0} is out of service")]
    OutOfService(RoomId),
}

fn room_idx(h: &Hospital, idx: &HospitalIndex, id: &RoomId) -> Result<usize, OpError> {
    idx.room_pos
        .get(id)
        .copied()
        .filter(|&i| i < h.rooms.len() && &h.rooms[i].id == id)
        .ok_or_else(|| OpError::RoomNotFound(id.clone()))
}

/// Place a patient in a vacant room.
pub fn admit(
    h: &mut Hospital,
    idx: &HospitalIndex,
    room: &RoomId,
    mut patient: Patient,
) -> Result<(), OpError> {
    let i = room_idx(h, idx, room)?;
    match &h.rooms[i].status {
        RoomStatus::Vacant => {
            patient.acuity = patient.acuity.clamped();
            h.rooms[i].status = RoomStatus::Occupied { patient };
            Ok(())
        }
        RoomStatus::Occupied { .. } => Err(OpError::NotVacant(room.clone())),
        RoomStatus::OutOfService { .. } => Err(OpError::OutOfService(room.clone())),
    }
}

/// Remove and return the patient from an occupied room.
pub fn discharge(
    h: &mut Hospital,
    idx: &HospitalIndex,
    room: &RoomId,
) -> Result<Patient, OpError> {
    let i = room_idx(h, idx, room)?;
    if matches!(h.rooms[i].status, RoomStatus::Occupied { .. }) {
        let prev = std::mem::replace(&mut h.rooms[i].status, RoomStatus::Vacant);
        match prev {
            RoomStatus::Occupied { patient } => Ok(patient),
            _ => unreachable!(),
        }
    } else {
        Err(OpError::NoPatient(room.clone()))
    }
}

/// Move a patient between rooms; clears any boarding flag if the destination
/// is a licensed bed.
pub fn transfer(
    h: &mut Hospital,
    idx: &HospitalIndex,
    from: &RoomId,
    to: &RoomId,
) -> Result<(), OpError> {
    // Validate destination BEFORE removing the patient so a failed transfer
    // leaves the world unchanged.
    let to_i = room_idx(h, idx, to)?;
    match &h.rooms[to_i].status {
        RoomStatus::Vacant => {}
        RoomStatus::Occupied { .. } => return Err(OpError::NotVacant(to.clone())),
        RoomStatus::OutOfService { .. } => return Err(OpError::OutOfService(to.clone())),
    }
    let mut patient = discharge(h, idx, from)?;
    use crate::model::RoomKind;
    // Boarding ends only on arrival in a licensed bed; ED bays, obs, research,
    // and other non-licensed spaces keep the flag.
    if matches!(
        h.rooms[to_i].kind,
        RoomKind::Inpatient | RoomKind::IcuBay | RoomKind::LaborDelivery | RoomKind::NurseryBay
    ) {
        patient.boarding = false;
    }
    h.rooms[to_i].status = RoomStatus::Occupied { patient };
    Ok(())
}

/// Mark a vacant room out of service (blocked bed).
pub fn set_out_of_service(
    h: &mut Hospital,
    idx: &HospitalIndex,
    room: &RoomId,
    reason: impl Into<String>,
) -> Result<(), OpError> {
    let i = room_idx(h, idx, room)?;
    match &h.rooms[i].status {
        RoomStatus::Occupied { .. } => Err(OpError::NotVacant(room.clone())),
        _ => {
            h.rooms[i].status = RoomStatus::OutOfService {
                reason: reason.into(),
            };
            Ok(())
        }
    }
}

/// Return an out-of-service room to service.
pub fn return_to_service(
    h: &mut Hospital,
    idx: &HospitalIndex,
    room: &RoomId,
) -> Result<(), OpError> {
    let i = room_idx(h, idx, room)?;
    if matches!(h.rooms[i].status, RoomStatus::OutOfService { .. }) {
        h.rooms[i].status = RoomStatus::Vacant;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use crate::patient::{Acuity, Patient};
    use crate::provenance::Provenance;

    fn world() -> (Hospital, HospitalIndex) {
        let h = Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![],
            units: vec![Unit {
                id: "u".into(),
                name: "U".into(),
                service: "GenMed".into(),
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
                Room {
                    id: "r1".into(),
                    number: "1".into(),
                    unit: "u".into(),
                    kind: RoomKind::Inpatient,
                    telemetry_capable: false,
                    negative_pressure: false,
                    status: RoomStatus::Vacant,
                    provenance: Provenance::default(),
                },
                Room {
                    id: "r2".into(),
                    number: "2".into(),
                    unit: "u".into(),
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
        };
        let idx = HospitalIndex::build(&h);
        (h, idx)
    }

    fn pt(name: &str) -> Patient {
        Patient {
            id: name.into(),
            alias: name.into(),
            age: Some(60),
            service: None,
            admitted_at_min: None,
            acuity: Acuity::default(),
            boarding: false,
            isolation: None,
            telemetry: false,
            note: String::new(),
        }
    }

    #[test]
    fn admit_discharge_roundtrip() {
        let (mut h, idx) = world();
        admit(&mut h, &idx, &"r1".into(), pt("A")).unwrap();
        assert!(admit(&mut h, &idx, &"r1".into(), pt("B")).is_err());
        let p = discharge(&mut h, &idx, &"r1".into()).unwrap();
        assert_eq!(p.alias, "A");
        assert!(h.rooms[0].status.is_vacant());
    }

    #[test]
    fn transfer_moves_patient_and_fails_atomically() {
        let (mut h, idx) = world();
        admit(&mut h, &idx, &"r1".into(), pt("A")).unwrap();
        admit(&mut h, &idx, &"r2".into(), pt("B")).unwrap();
        // Destination occupied → error, source untouched.
        assert_eq!(
            transfer(&mut h, &idx, &"r1".into(), &"r2".into()),
            Err(OpError::NotVacant("r2".into()))
        );
        assert!(h.rooms[0].status.patient().is_some());
        // Free r2 and retry.
        discharge(&mut h, &idx, &"r2".into()).unwrap();
        transfer(&mut h, &idx, &"r1".into(), &"r2".into()).unwrap();
        assert!(h.rooms[0].status.is_vacant());
        assert_eq!(h.rooms[1].status.patient().unwrap().alias, "A");
    }

    #[test]
    fn oos_blocks_admission() {
        let (mut h, idx) = world();
        set_out_of_service(&mut h, &idx, &"r1".into(), "terminal clean").unwrap();
        assert!(admit(&mut h, &idx, &"r1".into(), pt("A")).is_err());
        return_to_service(&mut h, &idx, &"r1".into()).unwrap();
        assert!(admit(&mut h, &idx, &"r1".into(), pt("A")).is_ok());
    }
}
