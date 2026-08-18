//! Compile the raw data files into a `Hospital`.

use crate::rawtopo::{parse_topology, RawTopology};
use crate::service_lines::{parse_service_lines, Assignment, ServiceLinesFile};
use crate::unitsfile::{
    parse_ranges, parse_units, LevelSpec, RoomSpec, TelemetryRule, UnitsFile,
};
use hupsim_core::prelude::*;
use hupsim_core::provenance::{Confidence, Era, Provenance};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("topology parse error: {0}")]
    Topology(serde_json::Error),
    #[error("units parse error: {0}")]
    Units(serde_json::Error),
    #[error("layout parse error: {0}")]
    Layout(serde_json::Error),
    #[error("service lines parse error: {0}")]
    ServiceLines(serde_json::Error),
    #[error("unit {unit} references unknown building {building}")]
    UnknownBuilding { unit: String, building: String },
    #[error("bad room spec in unit {unit}: {message}")]
    BadRoomSpec { unit: String, message: String },
    #[error("duplicate room id {0}")]
    DuplicateRoom(String),
    #[error("duplicate unit id {0}")]
    DuplicateUnit(String),
    #[error("edge {from} → {to} references {endpoint}, which is not a compiled building")]
    DanglingEdgeEndpoint {
        from: String,
        to: String,
        endpoint: String,
    },
    #[error(
        "unit {0} has no service-line assignment in service_lines.json — the mapping must stay total as units are added"
    )]
    MissingServiceLine(String),
    #[error("service-line assignment for {unit} references unknown line {line}")]
    UnknownServiceLine { unit: String, line: String },
    #[error("duplicate service-line assignment for unit {0}")]
    DuplicateServiceLineAssignment(String),
    #[error("duplicate service-line id {0}")]
    DuplicateServiceLineId(String),
}

fn conf(s: &str) -> Confidence {
    match s {
        "verified" => Confidence::Verified,
        "reported" => Confidence::Reported,
        "inferred" => Confidence::Inferred,
        _ => Confidence::Unverified,
    }
}

fn era(s: Option<&str>) -> Era {
    match s {
        Some("post_pavilion") => Era::PostPavilion,
        Some("pre_2021") => Era::Pre2021,
        Some("timeless") => Era::Timeless,
        _ => Era::Unknown,
    }
}

/// Fallback display name from an id tail: "wing.founders" → "Founders".
/// Nodes whose tail isn't a plain capitalized word (Clifton, PCAM) carry an
/// explicit `short_name` in the topology file instead.
fn short_name(id: &str) -> String {
    let tail = id.rsplit('.').next().unwrap_or(id);
    let mut c = tail.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => tail.to_string(),
    }
}

/// Room-id group per building: HUP Main wings share one number space.
fn room_group(building_id: &str) -> &str {
    if building_id.starts_with("wing.") {
        "hup_main"
    } else {
        match building_id {
            "bldg.pavilion" => "pavilion",
            "bldg.pcam" => "pcam",
            _ => "other",
        }
    }
}

fn room_kind(s: &str) -> RoomKind {
    match s {
        "inpatient" => RoomKind::Inpatient,
        "icu_bay" => RoomKind::IcuBay,
        "ed_bay" => RoomKind::EdBay,
        "obs" => RoomKind::Obs,
        "research" => RoomKind::Research,
        "labor_delivery" => RoomKind::LaborDelivery,
        "nursery_bay" => RoomKind::NurseryBay,
        _ => RoomKind::Other,
    }
}

fn unit_type(s: &str) -> UnitType {
    match s {
        "med_surg" => UnitType::MedSurg,
        "icu" => UnitType::Icu,
        "stepdown" => UnitType::Stepdown,
        "ed" => UnitType::Ed,
        "obs" => UnitType::Obs,
        "psych" => UnitType::Psych,
        "detox" => UnitType::Detox,
        "research" => UnitType::Research,
        "l_and_d" => UnitType::LaborDelivery,
        "nursery" => UnitType::Nursery,
        _ => UnitType::Other,
    }
}

fn connection_kind(s: &str) -> Option<ConnectionKind> {
    Some(match s {
        "bridge" => ConnectionKind::Bridge,
        "tunnel" => ConnectionKind::Tunnel,
        "connector_corridor" => ConnectionKind::ConnectorCorridor,
        "structural_adjacency" => ConnectionKind::StructuralAdjacency,
        "floor_continuity" => ConnectionKind::FloorContinuity,
        "no_physical_connection" => ConnectionKind::NoPhysicalConnection,
        _ => return None,
    })
}

fn build_buildings(topo: &RawTopology) -> Vec<Building> {
    topo.nodes
        .iter()
        .filter(|n| matches!(n.node_type.as_str(), "wing" | "building"))
        .map(|n| {
            let name = n.name.clone().unwrap_or_else(|| short_name(&n.id));
            Building {
                id: n.id.as_str().into(),
                short_name: n.short_name.clone().unwrap_or_else(|| short_name(&n.id)),
                name,
                kind: if n.node_type == "wing" {
                    BuildingKind::Wing
                } else {
                    BuildingKind::Building
                },
                built: n.built,
                aliases: n.aliases.clone(),
                floors: Vec::new(), // synthesized after units are known
                has_inpatient_beds: n.beds_bool(),
                provenance: Provenance {
                    confidence: conf(n.confidence.as_deref().unwrap_or("unverified")),
                    source: Some("hup_topology.json".into()),
                    era: Era::Timeless,
                    note: None,
                },
                note: n.note.clone(),
            }
        })
        .collect()
}

fn build_units_and_rooms(
    units_file: &UnitsFile,
    buildings: &[Building],
    lines_file: &ServiceLinesFile,
) -> Result<(Vec<Unit>, Vec<Room>, Vec<String>), CompileError> {
    let building_ids: HashSet<&str> = buildings.iter().map(|b| b.id.as_str()).collect();

    // Service-line taxonomy + mapping validation (EP-8). The mapping must be
    // total over the roster; needs-ruling entries surface as warnings.
    let mut line_ids: HashSet<&str> = HashSet::new();
    for l in &lines_file.lines {
        if !line_ids.insert(l.id.as_str()) {
            return Err(CompileError::DuplicateServiceLineId(l.id.clone()));
        }
    }
    let mut assignment_by_unit: HashMap<&str, &Assignment> = HashMap::new();
    for a in &lines_file.assignments {
        if !line_ids.contains(a.line.as_str()) {
            return Err(CompileError::UnknownServiceLine {
                unit: a.unit.clone(),
                line: a.line.clone(),
            });
        }
        if assignment_by_unit.insert(a.unit.as_str(), a).is_some() {
            return Err(CompileError::DuplicateServiceLineAssignment(a.unit.clone()));
        }
    }

    let mut units = Vec::new();
    let mut rooms: Vec<Room> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut seen_unit_ids: HashSet<String> = HashSet::new();
    let mut seen_room_ids: HashSet<String> = HashSet::new();

    for entry in &units_file.units {
        // A duplicate UnitId would silently corrupt the index (unit_pos is
        // last-wins while rooms_by_unit accumulates both units' rooms).
        if !seen_unit_ids.insert(entry.id.clone()) {
            return Err(CompileError::DuplicateUnit(entry.id.clone()));
        }
        if !building_ids.contains(entry.building.as_str()) {
            return Err(CompileError::UnknownBuilding {
                unit: entry.id.clone(),
                building: entry.building.clone(),
            });
        }
        let assignment = assignment_by_unit
            .get(entry.id.as_str())
            .copied()
            .ok_or_else(|| CompileError::MissingServiceLine(entry.id.clone()))?;
        if assignment.needs_owner_ruling {
            warnings.push(format!(
                "service line needs owner ruling: {} — {}",
                entry.id, entry.service
            ));
        }
        let level = match &entry.level {
            LevelSpec::Num(n) => FloorLevel::Numbered(*n),
            LevelSpec::Name(s) if s == "G" => FloorLevel::Ground,
            LevelSpec::Name(s) => FloorLevel::Named(s.clone()),
        };
        let unit_prov = Provenance {
            confidence: conf(&entry.confidence),
            source: entry.source.clone(),
            era: era(entry.era.as_deref()),
            note: entry.note.clone(),
        };
        units.push(Unit {
            id: entry.id.as_str().into(),
            name: entry.name.clone(),
            service: entry.service.clone(),
            service_line: Some(assignment.line.as_str().into()),
            unit_type: unit_type(&entry.unit_type),
            building: entry.building.as_str().into(),
            level,
            elevator_core: entry.elevator_core.clone(),
            target_rn_ratio: entry.staffing.as_ref().map(|s| s.target_rn_ratio),
            provenance: unit_prov.clone(),
            note: entry.note.clone(),
        });

        let unit_rooms_start = rooms.len();
        let group = room_group(&entry.building);
        let mut push_room = |id: String, number: String, kind: RoomKind, prov: Provenance| {
            if !seen_room_ids.insert(id.clone()) {
                return Err(CompileError::DuplicateRoom(id));
            }
            rooms.push(Room {
                id: id.into(),
                number,
                unit: entry.id.as_str().into(),
                kind,
                telemetry_capable: false,
                negative_pressure: false,
                status: RoomStatus::Vacant,
                provenance: prov,
            });
            Ok(())
        };

        match &entry.rooms {
            RoomSpec::Ranges { ranges, kind } => {
                let numbers =
                    parse_ranges(ranges).map_err(|message| CompileError::BadRoomSpec {
                        unit: entry.id.clone(),
                        message,
                    })?;
                let prov = Provenance::new(
                    Confidence::Verified,
                    "HUP wayfinding directory room-number ranges",
                    Era::Timeless,
                );
                for n in numbers {
                    push_room(
                        format!("room.{group}.{n}"),
                        n.to_string(),
                        room_kind(kind),
                        prov.clone(),
                    )?;
                }
            }
            RoomSpec::PavilionWing {
                floor,
                wing,
                kind,
                slots,
            } => {
                let prov = Provenance {
                    confidence: Confidence::Inferred,
                    source: Some(
                        "Pavilion published 1-72 City/Center/Campus numbering scheme".into(),
                    ),
                    era: Era::PostPavilion,
                    note: Some(
                        "504 licensed rooms vs 576 numbering slots — some numbers may not exist as rooms"
                            .into(),
                    ),
                };
                let numbers = match slots {
                    Some(s) => parse_ranges(s).map_err(|message| CompileError::BadRoomSpec {
                        unit: entry.id.clone(),
                        message,
                    })?,
                    None => wing.room_numbers(),
                };
                for n in numbers {
                    push_room(
                        format!("room.pavilion.{floor}.{n:02}"),
                        format!("{floor}-{n:02}"),
                        room_kind(kind),
                        prov.clone(),
                    )?;
                }
            }
            RoomSpec::Count {
                count,
                number_prefix,
                kind,
            } => {
                for i in 1..=*count {
                    let number = format!("{number_prefix}{i:02}");
                    push_room(
                        format!("room.{group}.{}", number.to_lowercase()),
                        number,
                        room_kind(kind),
                        unit_prov.clone(),
                    )?;
                }
            }
        }

        // Capability seeding (EP-8): rule lives in the unit's data entry, not
        // code constants. Deterministic — telemetry is all-or-none across the
        // unit; negative pressure marks the FIRST n rooms in generation order.
        if let Some(cap) = &entry.capabilities {
            let unit_rooms = &mut rooms[unit_rooms_start..];
            if cap.telemetry == TelemetryRule::All {
                for r in unit_rooms.iter_mut() {
                    r.telemetry_capable = true;
                }
            }
            for r in unit_rooms
                .iter_mut()
                .take(cap.negative_pressure_first_n as usize)
            {
                r.negative_pressure = true;
            }
        }
    }

    // A mapping entry whose unit is gone (renamed, archived) is stale data —
    // not fatal, but never silent.
    for a in &lines_file.assignments {
        if !seen_unit_ids.contains(a.unit.as_str()) {
            warnings.push(format!(
                "stale service-line assignment: {} is not in the unit roster",
                a.unit
            ));
        }
    }

    Ok((units, rooms, warnings))
}

/// Buildings that skip a floor label in their numbering series.
const SKIPPED_FLOORS: &[(&str, i32)] = &[("wing.founders", 13), ("bldg.pavilion", 13)];

/// Parse a topology `floor_span` like "G-7", "1-14", "G-11" into floor labels.
fn parse_floor_span(span: &str) -> Option<Vec<FloorLevel>> {
    let (lo, hi) = span.split_once('-')?;
    let hi: i32 = hi.trim().parse().ok()?;
    let mut out = Vec::new();
    match lo.trim() {
        "G" => {
            out.push(FloorLevel::Ground);
            out.extend((1..=hi).map(FloorLevel::Numbered));
        }
        lo_num => {
            let lo: i32 = lo_num.parse().ok()?;
            out.extend((lo..=hi).map(FloorLevel::Numbered));
        }
    }
    Some(out)
}

/// Synthesize per-building floor lists from the topology's authoritative
/// `floor_span` where present (falling back to the layout's story count),
/// dropping labels the building's numbering skips (no 13 in Founders or the
/// Pavilion), and marking which floors carry patient rooms. Unit levels
/// outside the base list are appended so every room belongs to a real floor
/// entry (this synthesized the Rhoads level-10 anomaly floor until EP-23
/// retired its unit as a directory artifact).
///
/// Every building gets a Ground story. The wayfinding directory prints a G
/// span for only four wings, but street level is "Ground" complex-wide (both
/// public entrances are labeled Ground Floor; the Connector is the FIRST
/// floor) and numbered floors align across wings, so buildings whose span
/// starts at 1 sit atop an unlisted ground story (see floor.hup_main.G).
/// Renderers stack floors by list position, so this is also what puts every
/// wing's floor N at the same height.
fn synthesize_floors(
    buildings: &mut [Building],
    units: &[Unit],
    layout: &CampusLayout,
    spans: &std::collections::HashMap<String, Vec<FloorLevel>>,
) {
    for b in buildings.iter_mut() {
        let unit_levels: Vec<&FloorLevel> = units
            .iter()
            .filter(|u| u.building == b.id)
            .map(|u| &u.level)
            .collect();
        let mut levels: Vec<FloorLevel> = match spans.get(b.id.as_str()) {
            Some(span) => span.clone(),
            None => {
                // Layout `levels` counts total stories including the ground
                // story, so the numbered floors run 1..=(levels - 1).
                let n = layout.building(&b.id).map(|l| l.levels).unwrap_or(0);
                (1..n as i32).map(FloorLevel::Numbered).collect()
            }
        };
        levels.retain(|lv| {
            !SKIPPED_FLOORS
                .iter()
                .any(|(id, n)| *id == b.id.as_str() && lv == &FloorLevel::Numbered(*n))
        });
        let inferred_ground = !levels.contains(&FloorLevel::Ground);
        let mut floors: Vec<FloorSpec> = levels
            .into_iter()
            .map(|level| FloorSpec {
                has_patient_rooms: unit_levels.contains(&&level),
                level,
                contents: Vec::new(),
                note: None,
            })
            .collect();
        if inferred_ground {
            floors.push(FloorSpec {
                level: FloorLevel::Ground,
                has_patient_rooms: unit_levels.contains(&&FloorLevel::Ground),
                contents: Vec::new(),
                note: Some(
                    "inferred street-level ground story — the wayfinding \
                     directory numbers this building from 1 (see floor.hup_main.G)"
                        .into(),
                ),
            });
        }
        // Unit levels outside the base list (anomalies, Ground units).
        for lv in unit_levels {
            if !floors.iter().any(|f| &f.level == lv) {
                floors.push(FloorSpec {
                    level: lv.clone(),
                    has_patient_rooms: true,
                    contents: Vec::new(),
                    note: Some("outside the building's documented floor span".into()),
                });
            }
        }
        floors.sort_by(|a, b| a.level.order_key().total_cmp(&b.level.order_key()));
        b.floors = floors;
    }
}

fn build_connections(
    topo: &RawTopology,
    buildings: &[Building],
) -> Result<Vec<Connection>, CompileError> {
    let ids: HashSet<&str> = buildings.iter().map(|b| b.id.as_str()).collect();
    // Every edge endpoint must be a concrete wing/building; abstract anchors
    // (bldg.hup_main, campus.*) were re-authored away in the topology and any
    // survivor is a data error, not something to silently patch.
    topo.edges
        .iter()
        .filter_map(|e| {
            let kind = match connection_kind(&e.edge_type) {
                Some(k) => k,
                None => return None,
            };
            for endpoint in [&e.from, &e.to] {
                if !ids.contains(endpoint.as_str()) {
                    return Some(Err(CompileError::DanglingEdgeEndpoint {
                        from: e.from.clone(),
                        to: e.to.clone(),
                        endpoint: endpoint.clone(),
                    }));
                }
            }
            let mut note = e.note.clone();
            if let Some(status) = &e.status {
                note = Some(match note {
                    Some(n) => format!("[{status}] {n}"),
                    None => format!("[{status}]"),
                });
            }
            Some(Ok(Connection {
                from: e.from.as_str().into(),
                to: e.to.as_str().into(),
                kind,
                name: e.name.clone(),
                layout: e.layout.clone(),
                levels: e.levels_vec(),
                provenance: Provenance {
                    confidence: conf(e.confidence.as_deref().unwrap_or("unverified")),
                    source: Some("hup_topology.json edges".into()),
                    era: Era::Timeless,
                    note: None,
                },
                note,
            }))
        })
        .collect()
}

fn build_cores(topo: &RawTopology) -> Vec<ElevatorCore> {
    topo.elevator_cores
        .iter()
        .map(|c| {
            let building = if c.core.starts_with("Pavilion") {
                Some(NodeId::from("bldg.pavilion"))
            } else {
                let guess = format!("wing.{}", c.core.to_lowercase());
                Some(NodeId::from(guess.as_str()))
            };
            ElevatorCore {
                name: c.core.clone(),
                building,
                serves_patient_floors: c.serves_patient_floors.clone(),
                note: c.note.clone(),
            }
        })
        .collect()
}

/// Compile all four JSON documents into a Hospital + CampusLayout.
pub fn compile(
    topology_json: &str,
    units_json: &str,
    layout_json: &str,
    service_lines_json: &str,
) -> Result<(Hospital, CampusLayout), CompileError> {
    let topo = parse_topology(topology_json).map_err(CompileError::Topology)?;
    let units_file = parse_units(units_json).map_err(CompileError::Units)?;
    let lines_file =
        parse_service_lines(service_lines_json).map_err(CompileError::ServiceLines)?;
    let mut layout: CampusLayout =
        serde_json::from_str(layout_json).map_err(CompileError::Layout)?;
    for b in &mut layout.buildings {
        b.footprint.ensure_ccw();
    }
    for g in &mut layout.ground_plates {
        g.footprint.ensure_ccw();
    }
    for br in &mut layout.bridges {
        br.ensure_ccw();
    }

    let mut buildings = build_buildings(&topo);
    let (units, rooms, data_warnings) =
        build_units_and_rooms(&units_file, &buildings, &lines_file)?;
    let spans: std::collections::HashMap<String, Vec<FloorLevel>> = topo
        .nodes
        .iter()
        .filter_map(|n| {
            let span = n.floor_span.as_deref()?;
            Some((n.id.clone(), parse_floor_span(span)?))
        })
        .collect();
    synthesize_floors(&mut buildings, &units, &layout, &spans);
    let connections = build_connections(&topo, &buildings)?;
    let elevator_cores = build_cores(&topo);

    let mut known_limitations = topo.meta.known_limitations.clone();
    known_limitations.push(
        "Pavilion floor 13 is modeled as absent: 504 licensed rooms = exactly 7 floors × 72 slots and no public source names any floor-13 unit. If 13 exists it is shell/interstitial. EP-23 (2026-08-13) additionally retired the 8 Campus and 14 City wings (no evidence of distinct inpatient units), so 456 of the 504 slots are modeled.".into(),
    );
    known_limitations.push(
        "CHPS inpatient research beds are modeled at Ravdin 6 NE (5 beds, med.upenn.edu/chps) — correcting the topology file's PCAM placement. PCAM carries no inpatient rooms here.".into(),
    );
    known_limitations.push(
        "HUP Main wayfinding room-number ranges (588 directory slots; 488 modeled after the EP-23 retirements of Founders 5/9 and Rhoads 2/10) likely overcount currently-staffed HUP Main beds; system-wide licensed total is ~800 (Penn Surgery: 'over 800'), so treat HUP Main slot counts as physical rooms, not staffed capacity.".into(),
    );

    let service_lines: Vec<ServiceLine> = lines_file
        .lines
        .iter()
        .map(|l| ServiceLine {
            id: l.id.as_str().into(),
            name: l.name.clone(),
            description: l.description.clone(),
        })
        .collect();

    let hospital = Hospital {
        meta: HospitalMeta {
            name: if topo.meta.subject.is_empty() {
                "Hospital of the University of Pennsylvania".into()
            } else {
                topo.meta.subject.clone()
            },
            subtitle: "hupsim — room-level occupancy & stability model".into(),
            data_compiled: topo.meta.compiled.clone(),
            known_limitations,
            data_warnings,
        },
        buildings,
        units,
        rooms,
        connections,
        elevator_cores,
        service_lines,
    };
    Ok((hospital, layout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::embedded;

    fn compiled() -> (Hospital, CampusLayout) {
        let (t, u, l, s) = embedded();
        compile(t, u, l, s).expect("embedded data must compile")
    }

    #[test]
    fn compiles_embedded_data() {
        let (h, layout) = compiled();
        assert!(h.buildings.len() >= 10, "8 wings + Pavilion + PCAM");
        assert!(h.units.len() >= 40);
        assert!(!layout.buildings.is_empty());
    }

    #[test]
    fn hup_main_range_room_count_is_exact() {
        let (h, _) = compiled();
        // Sum of all wayfinding room-number ranges on the MODELED roster.
        // The directory total is 588; EP-23 retired Founders 5 (32),
        // Founders 9 (24), Rhoads 2 (22), and Rhoads 10 (22) to the archive,
        // leaving 488 (see archive/ep23_roster_rulings.json).
        let hup_main_ranged = h
            .rooms
            .iter()
            .filter(|r| r.id.as_str().starts_with("room.hup_main.") && r.number.parse::<u32>().is_ok())
            .count();
        assert_eq!(hup_main_ranged, 488);
    }

    #[test]
    fn pavilion_room_count_is_pinned() {
        let (h, _) = compiled();
        let pav = h
            .rooms
            .iter()
            .filter(|r| r.id.as_str().starts_with("room.pavilion.") && !r.number.starts_with("ED"))
            .count();
        // 504 licensed = exactly 7 floors (7-12, 14) × 72 slots; floor 13
        // deliberately absent. EP-23 retired 8 Campus + 14 City (48 slots) as
        // not-distinct-inpatient-units → 456 modeled. Floor 12 still models
        // all 72 slots: City 24 + Oncology MICU 12 (13-24) + Campus 36 (25-60).
        assert_eq!(pav, 456);
    }

    #[test]
    fn referential_integrity() {
        let (h, layout) = compiled();
        let idx = HospitalIndex::build(&h);
        for u in &h.units {
            assert!(
                idx.building_pos.contains_key(&u.building),
                "unit {} → missing building {}",
                u.id,
                u.building
            );
        }
        for r in &h.rooms {
            assert!(
                idx.unit_pos.contains_key(&r.unit),
                "room {} → missing unit {}",
                r.id,
                r.unit
            );
        }
        // Every layout node must exist as a building.
        for bl in &layout.buildings {
            assert!(
                idx.building_pos.contains_key(&bl.node),
                "layout references missing building {}",
                bl.node
            );
        }
        // No duplicate room ids (compile() enforces; double-check).
        let mut ids: Vec<&str> = h.rooms.iter().map(|r| r.id.as_str()).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(before, ids.len());
    }

    #[test]
    fn floor_lists_follow_documented_spans() {
        let (h, _) = compiled();
        let floors = |id: &str| -> Vec<FloorLevel> {
            h.buildings
                .iter()
                .find(|b| b.id.as_str() == id)
                .unwrap()
                .floors
                .iter()
                .map(|f| f.level.clone())
                .collect()
        };
        // Rhoads: G-7 span, no phantom 8. The former level-10 anomaly floor
        // is GONE: EP-23 ruled rooms 1035-1056 a directory artifact and
        // retired the unit that synthesized it (archive/ep23_roster_rulings).
        let rhoads = floors("wing.rhoads");
        assert!(rhoads.contains(&FloorLevel::Ground));
        assert!(!rhoads.contains(&FloorLevel::Numbered(8)));
        assert!(
            !rhoads.contains(&FloorLevel::Numbered(10)),
            "EP-23: the anomaly floor must stay retired"
        );
        // Dulles: G-6 span, no phantom 7.
        let dulles = floors("wing.dulles");
        assert!(dulles.contains(&FloorLevel::Ground));
        assert!(!dulles.contains(&FloorLevel::Numbered(7)));
        // Founders and Pavilion skip 13 but keep 14.
        for id in ["wing.founders", "bldg.pavilion"] {
            let f = floors(id);
            assert!(!f.contains(&FloorLevel::Numbered(13)), "{id} must skip 13");
            assert!(f.contains(&FloorLevel::Numbered(14)));
        }
        // Clifton: G-17 span minus the skipped 13 = exactly the 17 stories
        // stated by the topology and OSM building:levels.
        let clifton = floors("bldg.pavilion");
        assert_eq!(clifton.len(), 17);
        assert!(clifton.contains(&FloorLevel::Ground));
        assert!(clifton.contains(&FloorLevel::Numbered(17)));
    }

    #[test]
    fn every_building_starts_at_ground() {
        // Street level is Ground complex-wide (both public entrances are
        // labeled Ground Floor; the Connector is the FIRST floor). Buildings
        // the directory numbers from 1 get an inferred ground story so the
        // shared numbered floors stack at the same story index everywhere.
        let (h, _) = compiled();
        for b in &h.buildings {
            assert_eq!(
                b.floors.first().map(|f| f.level.clone()),
                Some(FloorLevel::Ground),
                "{} must model a ground story at the bottom",
                b.id
            );
        }
    }

    #[test]
    fn numbered_floors_align_across_wings() {
        // The verified floor_continuity edges (walk Silverstein 9 → Ravdin 9
        // without changing level) require floor N to sit at the same story —
        // and therefore, with the uniform wing story height, the same render
        // elevation — in every wing. `Building::story_index` is what the
        // renderer stacks by: Ground = 0, floor N = story N.
        let (h, _) = compiled();
        let story = |id: &str, n: i32| -> i32 {
            let b = h.buildings.iter().find(|b| b.id.as_str() == id).unwrap();
            assert!(
                b.floors.iter().any(|f| f.level == FloorLevel::Numbered(n)),
                "{id} has no floor {n}"
            );
            b.story_index(&FloorLevel::Numbered(n))
        };
        for n in [8, 9, 10, 11, 12] {
            assert_eq!(story("wing.silverstein", n), n);
            assert_eq!(story("wing.founders", n), n);
        }
        assert_eq!(story("wing.ravdin", 9), 9);
        assert_eq!(story("wing.ravdin", 6), 6);
        assert_eq!(story("wing.dulles", 6), 6);
        // EP-23: the contested Rhoads level-10 anomaly was ruled a directory
        // artifact and retired — Rhoads tops out at its documented floor 7.
        {
            let rhoads = h
                .buildings
                .iter()
                .find(|b| b.id.as_str() == "wing.rhoads")
                .unwrap();
            assert!(
                !rhoads
                    .floors
                    .iter()
                    .any(|f| f.level == FloorLevel::Numbered(10)),
                "EP-23: Rhoads must have no floor 10"
            );
        }
        assert_eq!(story("wing.rhoads", 7), 7);
        // 13-skippers compress above 13: labeled 14 is the 13th story…
        assert_eq!(story("wing.founders", 14), 13);
        assert_eq!(story("bldg.pavilion", 14), 13);
        assert_eq!(story("bldg.pavilion", 17), 16);
        // …but a building that HAS a 13 (PCAM's dense fallback) does not.
        assert_eq!(story("bldg.pcam", 13), 13);
    }

    #[test]
    fn wing_story_heights_are_uniform() {
        // The eight wings share continuous floor plates, so they must share
        // one story height — otherwise the same floor number renders at
        // different elevations across wings no matter how stories align.
        let (_, layout) = compiled();
        for b in layout.buildings.iter().filter(|b| b.node.as_str().starts_with("wing.")) {
            assert_eq!(
                b.floor_height_m, 3.8,
                "{} floor height must match the shared wing story height",
                b.node
            );
        }
    }

    #[test]
    fn out_of_scope_sites_are_absent() {
        // Cedar, Radnor, ISC, and the unmapped wings were archived to
        // assets/data/archive/out_of_scope.json (EP-1); the compiled model
        // must not contain any trace of them.
        let (h, layout) = compiled();
        for id in [
            "bldg.hup_cedar",
            "bldg.radnor",
            "bldg.isc",
            "wing.gibson",
            "wing.donner",
            "wing.devon",
            "wing.centrex",
        ] {
            assert!(
                h.buildings.iter().all(|b| b.id.as_str() != id),
                "{id} should be archived, not compiled"
            );
            assert!(
                layout.buildings.iter().all(|b| b.node.as_str() != id),
                "{id} should have no layout entry"
            );
        }
        assert!(h.units.iter().all(|u| !u.id.as_str().starts_with("unit.cedar.")));
        assert!(
            h.connections
                .iter()
                .all(|c| c.kind != ConnectionKind::NoPhysicalConnection),
            "no_physical_connection edges left with Cedar/Radnor"
        );
        assert!(layout.insets.is_empty(), "the Cedar inset was the only inset");
    }

    #[test]
    fn connection_layout_refs_resolve_to_bridge_decks() {
        let (h, layout) = compiled();
        let deck_ids: HashSet<&str> = layout.bridges.iter().map(|b| b.id.as_str()).collect();
        // Every `layout` ref points at a real deck polygon…
        for c in &h.connections {
            if let Some(l) = &c.layout {
                assert!(
                    deck_ids.contains(l.as_str()),
                    "connection {} → {} references unknown bridge deck {l}",
                    c.from,
                    c.to
                );
            }
        }
        // …and no deck polygon is orphaned (geometry without a graph edge).
        let referenced: HashSet<&str> = h
            .connections
            .iter()
            .filter_map(|c| c.layout.as_deref())
            .collect();
        for deck in &layout.bridges {
            assert!(
                referenced.contains(deck.id.as_str()),
                "bridge deck {} has no topology edge referencing it",
                deck.id
            );
        }
        assert_eq!(deck_ids.len(), 3, "the three KML skybridges");
    }

    #[test]
    fn edge_endpoints_are_concrete_buildings() {
        // The bldg.hup_main → wing.silverstein remap is gone; every edge must
        // land on a compiled wing/building directly.
        let (h, _) = compiled();
        let ids: HashSet<&str> = h.buildings.iter().map(|b| b.id.as_str()).collect();
        for c in &h.connections {
            for end in [&c.from, &c.to] {
                assert!(
                    ids.contains(end.as_str()),
                    "edge {} → {} has non-building endpoint {end}",
                    c.from,
                    c.to
                );
            }
        }
    }

    #[test]
    fn pavilion_short_name_is_clifton() {
        let (h, _) = compiled();
        let pav = h
            .buildings
            .iter()
            .find(|b| b.id.as_str() == "bldg.pavilion")
            .unwrap();
        assert_eq!(pav.short_name, "Clifton");
        assert!(pav.aliases.iter().any(|a| a == "The Pavilion"));
    }

    #[test]
    fn short_names_are_data_driven() {
        // Non-derivable display names live in the topology file, not in
        // compile-code special cases (the function is a plain-word fallback).
        let (h, _) = compiled();
        let pcam = h
            .buildings
            .iter()
            .find(|b| b.id.as_str() == "bldg.pcam")
            .unwrap();
        assert_eq!(pcam.short_name, "PCAM");
    }

    const TINY_TOPO: &str = r#"{"nodes": [
        {"id": "wing.test", "type": "wing", "has_inpatient_beds": true}
    ]}"#;
    const TINY_LAYOUT: &str = r#"{"origin_lat": 0.0, "origin_lon": 0.0, "buildings": []}"#;
    const TINY_LINES: &str = r#"{
        "lines": [{"id": "line.medicine", "name": "Medicine"}],
        "assignments": [
            {"unit": "unit.test.a", "line": "line.medicine", "confidence": "verified"},
            {"unit": "unit.test.b", "line": "line.medicine", "confidence": "verified"}
        ]}"#;

    fn tiny_unit(id: &str, prefix: &str) -> String {
        format!(
            r#"{{"id": "{id}", "name": "Test", "service": "GenMed",
                 "unit_type": "med_surg", "building": "wing.test", "level": 5,
                 "rooms": {{"source": "count", "count": 2, "number_prefix": "{prefix}", "kind": "inpatient"}},
                 "confidence": "verified"}}"#
        )
    }

    #[test]
    fn duplicate_unit_id_is_a_compile_error() {
        // Same unit id twice, distinct room numbers — only the unit guard can
        // catch this (rooms alone would not collide).
        let units = format!(
            r#"{{"units": [{}, {}]}}"#,
            tiny_unit("unit.test.a", "A"),
            tiny_unit("unit.test.a", "B")
        );
        let err = compile(TINY_TOPO, &units, TINY_LAYOUT, TINY_LINES).unwrap_err();
        assert!(
            matches!(&err, CompileError::DuplicateUnit(id) if id == "unit.test.a"),
            "expected DuplicateUnit, got: {err}"
        );
    }

    #[test]
    fn duplicate_room_id_is_a_compile_error() {
        // Distinct unit ids, colliding room numbers in the shared wing.*
        // number space (room_group maps every wing to "hup_main").
        let units = format!(
            r#"{{"units": [{}, {}]}}"#,
            tiny_unit("unit.test.a", "A"),
            tiny_unit("unit.test.b", "A")
        );
        let err = compile(TINY_TOPO, &units, TINY_LAYOUT, TINY_LINES).unwrap_err();
        assert!(
            matches!(&err, CompileError::DuplicateRoom(id) if id == "room.hup_main.a01"),
            "expected DuplicateRoom, got: {err}"
        );
    }

    #[test]
    fn missing_service_line_mapping_is_a_compile_error() {
        // The mapping must stay total: unit.test.b compiled with no
        // assignment must fail loudly, not silently bin.
        let units = format!(
            r#"{{"units": [{}, {}]}}"#,
            tiny_unit("unit.test.a", "A"),
            tiny_unit("unit.test.b", "B")
        );
        let lines = r#"{
            "lines": [{"id": "line.medicine", "name": "Medicine"}],
            "assignments": [
                {"unit": "unit.test.a", "line": "line.medicine", "confidence": "verified"}
            ]}"#;
        let err = compile(TINY_TOPO, &units, TINY_LAYOUT, lines).unwrap_err();
        assert!(
            matches!(&err, CompileError::MissingServiceLine(id) if id == "unit.test.b"),
            "expected MissingServiceLine, got: {err}"
        );
    }

    #[test]
    fn unknown_service_line_is_a_compile_error() {
        let units = format!(r#"{{"units": [{}]}}"#, tiny_unit("unit.test.a", "A"));
        let lines = r#"{
            "lines": [{"id": "line.medicine", "name": "Medicine"}],
            "assignments": [
                {"unit": "unit.test.a", "line": "line.bogus", "confidence": "verified"}
            ]}"#;
        let err = compile(TINY_TOPO, &units, TINY_LAYOUT, lines).unwrap_err();
        assert!(
            matches!(&err, CompileError::UnknownServiceLine { unit, line }
                if unit == "unit.test.a" && line == "line.bogus"),
            "expected UnknownServiceLine, got: {err}"
        );
    }

    #[test]
    fn stale_service_line_assignment_is_a_warning_not_an_error() {
        let units = format!(r#"{{"units": [{}]}}"#, tiny_unit("unit.test.a", "A"));
        // unit.test.b has an assignment but is not in the roster.
        let (h, _) = compile(TINY_TOPO, &units, TINY_LAYOUT, TINY_LINES).unwrap();
        assert!(
            h.meta
                .data_warnings
                .iter()
                .any(|w| w.contains("stale") && w.contains("unit.test.b")),
            "stale assignment must surface as a warning: {:?}",
            h.meta.data_warnings
        );
    }

    #[test]
    fn every_unit_resolves_to_a_service_line_and_ratio() {
        let (h, _) = compiled();
        let line_ids: HashSet<&str> =
            h.service_lines.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(line_ids.len(), 10, "the ten-umbrella starting taxonomy");
        for u in &h.units {
            let line = u
                .service_line
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no service line", u.id));
            assert!(
                line_ids.contains(line.as_str()),
                "{} → unresolved line {line}",
                u.id
            );
            assert!(
                u.target_rn_ratio.is_some(),
                "{} has no target RN ratio",
                u.id
            );
        }
        // Taxonomy order is the dashboard rollup order.
        assert_eq!(h.service_lines.first().unwrap().id.as_str(), "line.medicine");
        assert_eq!(h.service_lines.last().unwrap().id.as_str(), "line.other");
    }

    #[test]
    fn ep23_rulings_leave_no_units_awaiting_ruling() {
        // EP-8 flagged the 11 'Unassigned (no public source)' units for
        // owner ruling; EP-23 (2026-08-13) ruled every one — five re-binned
        // with evidence, six retired to the archive. The open set is EMPTY
        // and must stay that way: a new flag means a new roster entry
        // awaiting ruling, which is fine, but never silent.
        let (h, _) = compiled();
        let flagged: Vec<&str> = h
            .meta
            .data_warnings
            .iter()
            .filter(|w| w.starts_with("service line needs owner ruling"))
            .map(|w| {
                w.strip_prefix("service line needs owner ruling: ")
                    .unwrap()
                    .split_once(" — ")
                    .unwrap()
                    .0
            })
            .collect();
        assert_eq!(flagged, Vec::<&str>::new(), "EP-23 ruled the full set");
        // No stale assignments in the real data.
        assert!(
            h.meta.data_warnings.iter().all(|w| !w.contains("stale")),
            "real mapping must have no stale entries: {:?}",
            h.meta.data_warnings
        );
    }

    #[test]
    fn ep23_retired_units_are_absent() {
        // EP-1 archive pattern, EP-23 edition: the six retired units live in
        // assets/data/archive/ep23_roster_rulings.json and must not creep
        // back into the compiled roster without a fresh ruling.
        let (h, _) = compiled();
        const RETIRED: [&str; 6] = [
            "unit.founders.5",
            "unit.founders.9",
            "unit.rhoads.2",
            "unit.rhoads.10",
            "unit.pavilion.8.campus",
            "unit.pavilion.14.city",
        ];
        for id in RETIRED {
            assert!(
                !h.units.iter().any(|u| u.id.as_str() == id),
                "{id} was retired in EP-23"
            );
            assert!(
                !h.rooms.iter().any(|r| r.unit.as_str() == id),
                "{id} must own no rooms"
            );
        }
    }

    #[test]
    fn capability_seeding_is_deterministic_and_pinned() {
        let (h, _) = compiled();
        let unit_rooms = |uid: &str| -> Vec<&Room> {
            h.rooms.iter().filter(|r| r.unit.as_str() == uid).collect()
        };
        // SICU (Rhoads 5 since EP-23): telemetry throughout; negative
        // pressure on the FIRST rooms in generation order = lowest
        // wayfinding numbers. Rooms 5001-5024 = the reported 24 beds.
        let sicu = unit_rooms("unit.rhoads.5");
        assert_eq!(sicu.len(), 24);
        assert!(sicu.iter().all(|r| r.telemetry_capable));
        let np: Vec<&str> = sicu
            .iter()
            .filter(|r| r.negative_pressure)
            .map(|r| r.number.as_str())
            .collect();
        assert_eq!(np, vec!["5001", "5002"]);
        // General medicine (Founders 12): no telemetry; first two rooms NP.
        let f12 = unit_rooms("unit.founders.12");
        assert!(f12.iter().all(|r| !r.telemetry_capable));
        let np: Vec<&str> = f12
            .iter()
            .filter(|r| r.negative_pressure)
            .map(|r| r.number.as_str())
            .collect();
        assert_eq!(np, vec!["1260", "1261"]);
        // Founders 8 (the MICU since EP-23): 12 directory rooms as ICU bays,
        // telemetry throughout, two NP rooms per the ICU convention.
        let f8 = unit_rooms("unit.founders.8");
        assert_eq!(f8.len(), 12);
        assert!(f8.iter().all(|r| r.telemetry_capable));
        let np: Vec<&str> = f8
            .iter()
            .filter(|r| r.negative_pressure)
            .map(|r| r.number.as_str())
            .collect();
        assert_eq!(np, vec!["875", "876"]);
        // Pavilion Center wing: first slots under the published numbering are
        // 13 and 14; rooms are acuity-adaptable (telemetry throughout).
        let p7c = unit_rooms("unit.pavilion.7.center");
        assert!(p7c.iter().all(|r| r.telemetry_capable));
        let np: Vec<&str> = p7c
            .iter()
            .filter(|r| r.negative_pressure)
            .map(|r| r.number.as_str())
            .collect();
        assert_eq!(np, vec!["7-13", "7-14"]);
        // EP-23 floor-12 split via pavilion_wing `slots`: the Oncology MICU
        // takes Center slots 13-24 (12 rooms); 12 Campus absorbs 25-60
        // (36 rooms) matching its verified bed count. Floor total stays 72.
        let onc_micu = unit_rooms("unit.pavilion.12.center");
        assert_eq!(onc_micu.len(), 12);
        let np: Vec<&str> = onc_micu
            .iter()
            .filter(|r| r.negative_pressure)
            .map(|r| r.number.as_str())
            .collect();
        assert_eq!(np, vec!["12-13", "12-14"]);
        let p12campus = unit_rooms("unit.pavilion.12.campus");
        assert_eq!(p12campus.len(), 36);
        assert_eq!(p12campus.first().unwrap().number.as_str(), "12-25");
        assert_eq!(p12campus.last().unwrap().number.as_str(), "12-60");
        // CHPS research beds: no capabilities seeded.
        let chps = unit_rooms("unit.ravdin.6ne.chps");
        assert!(chps.iter().all(|r| !r.telemetry_capable && !r.negative_pressure));
        // ED: monitored throughout, two NP treatment spaces.
        let ed = unit_rooms("unit.pavilion.ed");
        assert_eq!(ed.len(), 122);
        assert!(ed.iter().all(|r| r.telemetry_capable));
        assert_eq!(ed.iter().filter(|r| r.negative_pressure).count(), 2);
        // Hospital-wide pin: the seeding rules sum to exactly 79 NP rooms
        // (was 90 pre-EP-23: −12 with the six retirements, +1 when Founders 8
        // became the MICU and took the 2-NP ICU convention).
        assert_eq!(h.rooms.iter().filter(|r| r.negative_pressure).count(), 79);
    }

    #[test]
    fn rn_ratio_targets_follow_level_of_care() {
        let (h, _) = compiled();
        for u in &h.units {
            let ratio = u.target_rn_ratio.unwrap();
            match u.unit_type {
                UnitType::Icu => assert_eq!(ratio, 2.0, "{} ICU must target 2:1", u.id),
                UnitType::Stepdown => {
                    assert_eq!(ratio, 3.0, "{} stepdown must target 3:1", u.id)
                }
                UnitType::MedSurg => assert!(
                    (4.0..=5.0).contains(&ratio),
                    "{} med-surg must target 4-5:1, got {ratio}",
                    u.id
                ),
                _ => assert!(
                    (2.0..=5.0).contains(&ratio),
                    "{} ratio {ratio} out of range",
                    u.id
                ),
            }
        }
    }

    #[test]
    fn hup_main_podium_plate_is_extruded() {
        let (_, layout) = compiled();
        let podium = layout
            .ground_plates
            .iter()
            .find(|g| g.extrude_m.is_some())
            .expect("the HUP Main plate carries the podium extrusion");
        // One story (EP-7): floors 2+ of every wing clear the podium rim.
        assert_eq!(podium.extrude_m, Some(3.5));
    }

    #[test]
    fn podium_contains_all_wing_footprints() {
        // EP-7 invariant: the plate is derived from the union of the wing
        // traces, so no tower may overhang it (the old OSM plate's accepted
        // imperfection is retired). Sampled along every wing edge (≤2 m), not
        // just at vertices — simplification could otherwise cut inward
        // between two distant vertices of a long straight wall undetected.
        let (_, layout) = compiled();
        let podium = layout.ground_plates.iter().find(|g| g.extrude_m.is_some()).unwrap();
        for b in layout.buildings.iter().filter(|b| b.node.as_str().starts_with("wing.")) {
            let v = &b.footprint.vertices;
            let n = v.len();
            for i in 0..n {
                let (a, c) = (v[i], v[(i + 1) % n]);
                let len = ((c[0] - a[0]).powi(2) + (c[1] - a[1]).powi(2)).sqrt();
                let steps = (len / 2.0).ceil().max(1.0) as usize;
                for s in 0..steps {
                    let t = s as f32 / steps as f32;
                    let p = [a[0] + t * (c[0] - a[0]), a[1] + t * (c[1] - a[1])];
                    assert!(
                        podium.footprint.contains_point(p),
                        "{} boundary point {:?} (edge {}) is outside the podium plate",
                        b.node.as_str(),
                        p,
                        i
                    );
                }
            }
        }
    }

    #[test]
    fn podium_is_single_clean_ccw_ring() {
        let (_, layout) = compiled();
        let podium = layout.ground_plates.iter().find(|g| g.extrude_m.is_some()).unwrap();
        let v = &podium.footprint.vertices;
        let area = podium.footprint.signed_area();
        assert!(area > 0.0, "podium ring must be CCW");
        // Anchors: > the bare wing union (~14,730 m², the plate must contain
        // it) and < the retired OSM blob (26,873 m², which covered 57% empty
        // ground — the derived plate must be tighter than that).
        assert!(
            (15_000.0..27_000.0).contains(&area),
            "podium area {area:.0} m² outside sanity band"
        );
        assert!(
            (12..=28).contains(&v.len()),
            "podium ring has {} vertices — not the clean simplified outline",
            v.len()
        );
        // Simple ring: no two non-adjacent edges may cross.
        let n = v.len();
        let orient = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| -> f32 {
            (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
        };
        for i in 0..n {
            for j in i + 1..n {
                if (j + 1) % n == i || (i + 1) % n == j {
                    continue;
                }
                let (a, b) = (v[i], v[(i + 1) % n]);
                let (c, d) = (v[j], v[(j + 1) % n]);
                let crossing = (orient(a, b, c) > 0.0) != (orient(a, b, d) > 0.0)
                    && (orient(c, d, a) > 0.0) != (orient(c, d, b) > 0.0);
                assert!(!crossing, "podium edges {i} and {j} cross");
            }
        }
        // No degenerate short edges survive simplification.
        for i in 0..n {
            let j = (i + 1) % n;
            let len = ((v[i][0] - v[j][0]).powi(2) + (v[i][1] - v[j][1]).powi(2)).sqrt();
            assert!(len >= 2.0, "podium edge {i} is only {len:.2} m");
        }
    }

    #[test]
    fn bedless_wings_present_for_topological_honesty() {
        let (h, _) = compiled();
        for id in ["wing.gates", "wing.maloney", "wing.white"] {
            let b = h.buildings.iter().find(|b| b.id.as_str() == id).unwrap();
            assert!(!b.has_inpatient_beds);
        }
    }

    // --- EP-6: route graph over the compiled campus ------------------------

    fn route_graph(include_unverified: bool) -> (Hospital, CampusLayout, RouteGraph) {
        let (h, layout) = compiled();
        let g = RouteGraph::build(
            &h,
            &layout,
            RouteParams::default(),
            TraversalPolicy { include_unverified },
        );
        (h, layout, g)
    }

    #[test]
    fn route_founders_to_pcam_crosses_a_bridge_or_connector() {
        let (_h, _l, g) = route_graph(false);
        let from = g
            .node_index(&"wing.founders".into(), &FloorLevel::Numbered(9))
            .expect("Founders 9 node");
        let to = g
            .node_index(&"bldg.pcam".into(), &FloorLevel::Numbered(2))
            .expect("PCAM 2 node");
        let r = g.route(from, to).expect("Founders 9 → PCAM 2 must be routable");
        assert!(
            r.hops
                .iter()
                .any(|hop| matches!(hop.kind, RouteEdgeKind::Bridge | RouteEdgeKind::Connector)),
            "route must cross a skybridge or Connector edge, never teleport: {:?}",
            r.hops.iter().map(|h| h.kind).collect::<Vec<_>>()
        );
        assert!(r.minutes > 0.0 && r.minutes < 60.0, "minutes = {}", r.minutes);
    }

    #[test]
    fn route_weights_positive_and_tunnel_policy() {
        let (_h, _l, g) = route_graph(false);
        assert!(g.edges.len() > 50, "expected a rich edge set, got {}", g.edges.len());
        for e in &g.edges {
            assert!(e.minutes > 0.0, "non-positive weight: {:?}", e);
        }
        assert!(
            !g.edges.iter().any(|e| e.kind == RouteEdgeKind::Tunnel),
            "unverified tunnel must be excluded by default"
        );
        let (_h, _l, g) = route_graph(true);
        assert!(
            g.edges.iter().any(|e| e.kind == RouteEdgeKind::Tunnel),
            "tunnel available behind the include_unverified toggle"
        );
    }

    #[test]
    fn every_staffed_unit_is_routable_from_the_ed() {
        let (h, _l, g) = route_graph(false);
        let ed = h
            .units
            .iter()
            .find(|u| u.unit_type == UnitType::Ed)
            .expect("ED unit");
        let from = g.node_index(&ed.building, &ed.level).expect("ED node");
        let mut unreachable: Vec<String> = Vec::new();
        for u in &h.units {
            let Some(to) = g.node_index(&u.building, &u.level) else {
                unreachable.push(format!("{} (no node {} {})", u.id, u.building, u.level.label()));
                continue;
            };
            if to != from && g.route(from, to).is_none() {
                unreachable.push(format!("{} ({} {})", u.id, u.building, u.level.label()));
            }
        }
        assert!(
            unreachable.is_empty(),
            "units unreachable from the ED without unverified edges: {unreachable:?}"
        );
    }

    /// EP-6 validation analysis: ED-to-bed transport minutes, Clifton vs the
    /// HUP Main wings, printed for the README write-up
    /// (`cargo test -p hupsim-data ed_to_bed -- --nocapture`).
    #[test]
    fn ed_to_bed_transport_analysis() {
        let (h, _l, g) = route_graph(false);
        let ed = h
            .units
            .iter()
            .find(|u| u.unit_type == UnitType::Ed)
            .expect("ED unit");
        let from = g.node_index(&ed.building, &ed.level).expect("ED node");
        let mut rows: Vec<(String, f32, String)> = Vec::new();
        for u in &h.units {
            if !matches!(
                u.unit_type,
                UnitType::MedSurg | UnitType::Icu | UnitType::Stepdown
            ) {
                continue;
            }
            let Some(to) = g.node_index(&u.building, &u.level) else {
                continue;
            };
            let Some(r) = (to != from).then(|| g.route(from, to)).flatten() else {
                continue;
            };
            rows.push((
                format!("{} ({} {})", u.name, u.building, u.level.label()),
                r.minutes,
                r.via_summary(),
            ));
        }
        assert!(!rows.is_empty());
        rows.sort_by(|a, b| a.1.total_cmp(&b.1));
        println!("ED-to-bed transport minutes (ED = {} {}):", ed.building, ed.level.label());
        for (name, min, via) in &rows {
            println!("  {min:5.1}  {name}  [{via}]");
        }
        // The structural claim behind the analysis: every Clifton bed floor is
        // reachable elevator-only, while HUP Main wings require crossing to
        // the old campus — so the *nearest* inpatient floors are Clifton's.
        let clifton_best = rows
            .iter()
            .filter(|r| r.0.contains("bldg.pavilion"))
            .map(|r| r.1)
            .fold(f32::INFINITY, f32::min);
        let hup_main_best = rows
            .iter()
            .filter(|r| r.0.contains("wing."))
            .map(|r| r.1)
            .fold(f32::INFINITY, f32::min);
        assert!(
            clifton_best < hup_main_best,
            "nearest Clifton bed floor ({clifton_best} min) should beat nearest HUP Main wing ({hup_main_best} min)"
        );
    }

    /// EP-18 case-study analysis: skybridge constraints on PCAM/CHPS access,
    /// printed for `docs/analyses/`
    /// (`cargo test -p hupsim-data pcam_chps -- --nocapture`).
    ///
    /// PCAM the building and CHPS the inpatient unit are deliberately
    /// distinct destinations: CHPS's 5 inpatient/research beds are modeled at
    /// Ravdin 6 NE per the CHPS locations page — PCAM 4 South holds only
    /// outpatient bays (see units.json). The printout covers (1) ED → PCAM by
    /// level, (2) ED → CHPS, (3) the skybridge funnel census over every HUP
    /// Main inpatient destination, and (4) what admitting the unverified
    /// HUP Main↔Clifton tunnel would change — the confidence policy's
    /// operational consequence, stated with numbers.
    #[test]
    fn pcam_chps_access_analysis() {
        let (h, _l, g) = route_graph(false);
        let ed = h
            .units
            .iter()
            .find(|u| u.unit_type == UnitType::Ed)
            .expect("ED unit");
        let from = g.node_index(&ed.building, &ed.level).expect("ED node");

        // --- 1: ED → PCAM, every level the graph knows. --------------------
        let pcam: NodeId = "bldg.pcam".into();
        println!("ED → PCAM (bldg.pcam), by level:");
        let mut pcam_levels: Vec<&FloorLevel> = g
            .nodes
            .iter()
            .filter(|n| n.building == pcam)
            .map(|n| &n.level)
            .collect();
        pcam_levels.sort_by_key(|l| l.label());
        for level in &pcam_levels {
            let to = g.node_index(&pcam, level).expect("PCAM node");
            match g.route(from, to) {
                Some(r) => println!(
                    "  {:5.1}  PCAM {}  [{}]",
                    r.minutes,
                    level.label(),
                    r.via_summary()
                ),
                None => println!("    n/a  PCAM {}  [unreachable]", level.label()),
            }
        }

        // --- 2: ED → CHPS (the distinct destination). ----------------------
        let chps = h
            .units
            .iter()
            .find(|u| u.id.as_str() == "unit.ravdin.6ne.chps")
            .expect("CHPS unit");
        let to = g.node_index(&chps.building, &chps.level).expect("CHPS node");
        let r = g.route(from, to).expect("ED → CHPS must be routable");
        println!(
            "ED → CHPS ({} {}): {:.1} min  [{}]",
            chps.building,
            chps.level.label(),
            r.minutes,
            r.via_summary()
        );

        // --- 3: the funnel census. -----------------------------------------
        // Every HUP Main inpatient destination: does the route cross the
        // Silverstein↔Clifton skybridge?
        let crosses_sky_bridge = |r: &hupsim_core::route::Route| {
            r.hops.iter().any(|hop| {
                hop.kind == hupsim_core::route::RouteEdgeKind::Bridge
                    && ((hop.from_building.as_str() == "wing.silverstein"
                        && hop.to_building.as_str() == "bldg.pavilion")
                        || (hop.from_building.as_str() == "bldg.pavilion"
                            && hop.to_building.as_str() == "wing.silverstein"))
            })
        };
        let mut total = 0usize;
        let mut funneled = 0usize;
        for u in &h.units {
            if !matches!(
                u.unit_type,
                UnitType::MedSurg | UnitType::Icu | UnitType::Stepdown
            ) || !u.building.as_str().starts_with("wing.")
            {
                continue;
            }
            let Some(to) = g.node_index(&u.building, &u.level) else {
                continue;
            };
            let Some(r) = g.route(from, to) else { continue };
            total += 1;
            if crosses_sky_bridge(&r) {
                funneled += 1;
            }
        }
        println!(
            "funnel census: {funneled}/{total} ED → HUP Main inpatient routes cross the \
             Silverstein–Clifton skybridge"
        );
        assert_eq!(
            funneled, total,
            "with the tunnel excluded, every ED → HUP Main route must use the one skybridge"
        );

        // --- 4: what the unverified tunnel would change. -------------------
        let (h2, _l2, g2) = route_graph(true);
        let ed2 = h2
            .units
            .iter()
            .find(|u| u.unit_type == UnitType::Ed)
            .expect("ED unit");
        let from2 = g2.node_index(&ed2.building, &ed2.level).expect("ED node");
        println!("with the unverified HUP Main↔Clifton tunnel admitted:");
        let mut changed = 0usize;
        let mut kept = 0usize;
        for u in &h.units {
            if !matches!(
                u.unit_type,
                UnitType::MedSurg | UnitType::Icu | UnitType::Stepdown
            ) || !u.building.as_str().starts_with("wing.")
            {
                continue;
            }
            let (Some(a), Some(b)) = (
                g.node_index(&u.building, &u.level)
                    .and_then(|to| g.route(from, to)),
                g2.node_index(&u.building, &u.level)
                    .and_then(|to| g2.route(from2, to)),
            ) else {
                continue;
            };
            if (a.minutes - b.minutes).abs() > 0.05 {
                changed += 1;
                println!(
                    "  {:5.1} → {:5.1}  {} ({} {})  [now {}]",
                    a.minutes,
                    b.minutes,
                    u.name,
                    u.building,
                    u.level.label(),
                    b.via_summary()
                );
            } else {
                kept += 1;
            }
        }
        println!(
            "  {changed} destinations reroute, {kept} keep the skybridge path"
        );
    }
}
