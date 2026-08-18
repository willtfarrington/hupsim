//! The compiled hospital model: buildings → floors → units → rooms, plus the
//! inter-building connection graph and elevator cores.
//!
//! `Hospital` is flat `Vec`s (stable order, serializable, diffable); fast
//! lookup lives in [`crate::index::HospitalIndex`], rebuilt after structural
//! edits.

use crate::ids::{NodeId, RoomId, ServiceLineId, UnitId};
use crate::patient::Patient;
use crate::provenance::Provenance;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HospitalMeta {
    pub name: String,
    #[serde(default)]
    pub subtitle: String,
    /// Compilation date of the underlying OSINT data (ISO 8601).
    #[serde(default)]
    pub data_compiled: String,
    #[serde(default)]
    pub known_limitations: Vec<String>,
    /// Non-fatal compile findings the owner should see (needs-ruling service
    /// lines, stale mapping entries). Surfaced by the data-source indicator.
    #[serde(default)]
    pub data_warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hospital {
    pub meta: HospitalMeta,
    pub buildings: Vec<Building>,
    pub units: Vec<Unit>,
    pub rooms: Vec<Room>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub elevator_cores: Vec<ElevatorCore>,
    /// Ordered service-line taxonomy (from service_lines.json); rollup order
    /// for dashboards. Empty in pre-EP-8 scenario saves.
    #[serde(default)]
    pub service_lines: Vec<ServiceLine>,
}

/// An umbrella service line ("Medicine", "Cardiac", …) units roll up into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceLine {
    pub id: ServiceLineId,
    /// Display name, e.g. "Women's Health & Neonatal".
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildingKind {
    /// A wing of the interconnected HUP Main complex.
    Wing,
    /// A free-standing building (Pavilion, PCAM).
    Building,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Building {
    pub id: NodeId,
    pub name: String,
    /// Short label for 3D view ("Founders", "Pavilion").
    pub short_name: String,
    pub kind: BuildingKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<i32>,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Floors bottom → top. May be sparse (only modeled floors).
    pub floors: Vec<FloorSpec>,
    pub has_inpatient_beds: bool,
    #[serde(default)]
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Building {
    pub fn floor(&self, level: &FloorLevel) -> Option<&FloorSpec> {
        self.floors.iter().find(|f| &f.level == level)
    }

    /// Story index of a floor label in the campus stacking convention:
    /// Ground = 0, floor N = N stories above grade — so a floor number lands
    /// at the same height in every building — EXCEPT that a building whose
    /// numbering skips 13 (has floors above 13 but no 13) compresses N > 13
    /// down one story: its physical 13th story is *labeled* 14. Floors whose
    /// label is outside the building's contiguous run therefore stack at
    /// their labeled level, visibly detached from the wing's own roofline —
    /// the retired Rhoads level-10 anomaly rendered this way pre-EP-23.
    /// `Named` floors have no numeric position and fall back to their list
    /// index.
    pub fn story_index(&self, level: &FloorLevel) -> i32 {
        match level {
            FloorLevel::Basement => -1,
            FloorLevel::Ground => 0,
            FloorLevel::Numbered(n) => {
                let has_13 = self.floors.iter().any(|f| f.level == FloorLevel::Numbered(13));
                let skips_13 = !has_13
                    && self
                        .floors
                        .iter()
                        .any(|f| matches!(f.level, FloorLevel::Numbered(m) if m > 13));
                if skips_13 && *n > 13 {
                    *n - 1
                } else {
                    *n
                }
            }
            FloorLevel::Named(_) => self
                .floors
                .iter()
                .position(|f| &f.level == level)
                .map(|i| i as i32)
                .unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloorSpec {
    pub level: FloorLevel,
    #[serde(default)]
    pub has_patient_rooms: bool,
    /// Non-bed contents worth showing (cath lab, ORs, cafeteria…).
    #[serde(default)]
    pub contents: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Floor levels: numbered, ground, or named. Ordering key puts Ground at 0,
/// basements below, numbered floors at their number.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FloorLevel {
    Basement,
    Ground,
    Numbered(i32),
    Named(String),
}

impl FloorLevel {
    /// Sort key; `Named` floors sort just above ground.
    pub fn order_key(&self) -> f64 {
        match self {
            FloorLevel::Basement => -1.0,
            FloorLevel::Ground => 0.0,
            FloorLevel::Numbered(n) => *n as f64,
            FloorLevel::Named(_) => 0.5,
        }
    }

    pub fn label(&self) -> String {
        match self {
            FloorLevel::Basement => "B".to_string(),
            FloorLevel::Ground => "G".to_string(),
            FloorLevel::Numbered(n) => n.to_string(),
            FloorLevel::Named(s) => s.clone(),
        }
    }
}

// Deliberately NOT PartialOrd: order_key() collides for distinct levels
// (Ground vs Numbered(0), any two Named), which would break the
// PartialOrd/PartialEq consistency contract. Sort with
// `a.order_key().total_cmp(&b.order_key())` instead.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitType {
    MedSurg,
    Icu,
    Stepdown,
    Ed,
    Obs,
    Psych,
    Detox,
    Research,
    LaborDelivery,
    Nursery,
    Other,
}

impl UnitType {
    /// Every variant, in the fixed rollup/display order (level-of-care
    /// dashboards and the matrix `rollup_by_unit_type` index against this).
    pub const ALL: [UnitType; 11] = [
        UnitType::MedSurg,
        UnitType::Icu,
        UnitType::Stepdown,
        UnitType::Ed,
        UnitType::Obs,
        UnitType::Psych,
        UnitType::Detox,
        UnitType::Research,
        UnitType::LaborDelivery,
        UnitType::Nursery,
        UnitType::Other,
    ];

    /// Position of this variant in [`UnitType::ALL`].
    pub fn index(self) -> usize {
        match self {
            UnitType::MedSurg => 0,
            UnitType::Icu => 1,
            UnitType::Stepdown => 2,
            UnitType::Ed => 3,
            UnitType::Obs => 4,
            UnitType::Psych => 5,
            UnitType::Detox => 6,
            UnitType::Research => 7,
            UnitType::LaborDelivery => 8,
            UnitType::Nursery => 9,
            UnitType::Other => 10,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            UnitType::MedSurg => "Med/Surg",
            UnitType::Icu => "ICU",
            UnitType::Stepdown => "Stepdown",
            UnitType::Ed => "Emergency",
            UnitType::Obs => "Observation",
            UnitType::Psych => "Psychiatry",
            UnitType::Detox => "Detox",
            UnitType::Research => "Research",
            UnitType::LaborDelivery => "Labor & Delivery",
            UnitType::Nursery => "Nursery/NICU",
            UnitType::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unit {
    pub id: UnitId,
    /// Display name, e.g. "Founders 14", "Pavilion 8 City".
    pub name: String,
    /// Clinical service, e.g. "General Medicine".
    pub service: String,
    /// Normalized umbrella line (service_lines.json). None only in pre-EP-8
    /// scenario saves; the compiler always resolves one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_line: Option<ServiceLineId>,
    pub unit_type: UnitType,
    pub building: NodeId,
    pub level: FloorLevel,
    /// Elevator core serving this unit's rooms (HUP Main wayfinding).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevator_core: Option<String>,
    /// Target patients-per-RN staffing ratio (units.json `staffing`,
    /// inferred professional norms). None only in pre-EP-8 saves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_rn_ratio: Option<f32>,
    #[serde(default)]
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomKind {
    /// Licensed inpatient bed room.
    Inpatient,
    IcuBay,
    /// ED treatment space — not licensed inpatient, but boards admitted patients.
    EdBay,
    Obs,
    Research,
    LaborDelivery,
    NurseryBay,
    Other,
}

impl RoomKind {
    pub fn label(self) -> &'static str {
        match self {
            RoomKind::Inpatient => "Inpatient room",
            RoomKind::IcuBay => "ICU bay",
            RoomKind::EdBay => "ED bay (boarding-capable)",
            RoomKind::Obs => "Observation",
            RoomKind::Research => "Research bed",
            RoomKind::LaborDelivery => "L&D room",
            RoomKind::NurseryBay => "Nursery bay",
            RoomKind::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RoomStatus {
    Vacant,
    Occupied { patient: Patient },
    OutOfService { reason: String },
}

impl RoomStatus {
    pub fn patient(&self) -> Option<&Patient> {
        match self {
            RoomStatus::Occupied { patient } => Some(patient),
            _ => None,
        }
    }

    pub fn patient_mut(&mut self) -> Option<&mut Patient> {
        match self {
            RoomStatus::Occupied { patient } => Some(patient),
            _ => None,
        }
    }

    pub fn is_vacant(&self) -> bool {
        matches!(self, RoomStatus::Vacant)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub id: RoomId,
    /// Display number as it appears on the door: "560", "8-17", "ED-12".
    pub number: String,
    pub unit: UnitId,
    pub kind: RoomKind,
    /// Can take a telemetry-requiring patient (units.json capability rules).
    #[serde(default, skip_serializing_if = "is_false")]
    pub telemetry_capable: bool,
    /// Airborne-isolation (negative-pressure) room; seeded deterministically
    /// onto the first rooms of each unit per units.json capability rules.
    #[serde(default, skip_serializing_if = "is_false")]
    pub negative_pressure: bool,
    #[serde(default = "default_vacant")]
    pub status: RoomStatus,
    #[serde(default)]
    pub provenance: Provenance,
}

fn default_vacant() -> RoomStatus {
    RoomStatus::Vacant
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    Bridge,
    Tunnel,
    ConnectorCorridor,
    StructuralAdjacency,
    /// Same floor numbers walkable across wings without changing level.
    FloorContinuity,
    /// Explicitly disconnected sites (ambulance transfer only). No live edge
    /// uses this since the out-of-scope sites were archived; kept so those
    /// edges parse again if scope re-expands.
    NoPhysicalConnection,
}

impl ConnectionKind {
    pub fn label(self) -> &'static str {
        match self {
            ConnectionKind::Bridge => "skybridge",
            ConnectionKind::Tunnel => "tunnel",
            ConnectionKind::ConnectorCorridor => "connector corridor",
            ConnectionKind::StructuralAdjacency => "structural adjacency",
            ConnectionKind::FloorContinuity => "floor continuity",
            ConnectionKind::NoPhysicalConnection => "no physical connection",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: ConnectionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Id of a `bridges[]` deck polygon in the campus layout, when the
    /// connection has surveyed geometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// Floor levels the connection spans, when known.
    #[serde(default)]
    pub levels: Vec<i32>,
    #[serde(default)]
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElevatorCore {
    pub name: String,
    /// Building the core belongs to (HUP Main cores are shared wayfinding).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub building: Option<NodeId>,
    #[serde(default)]
    pub serves_patient_floors: Vec<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
