//! Shared writer for the committed `assets/data/layout.json` document.
//!
//! Both one-shot tools (`kml_import`, `podium_gen`) serialize through this
//! module, so the `$schema_note`, the canonical notes, and the point
//! formatting can never drift apart depending on which tool wrote last.

use hupsim_core::layout::{Bridge, BuildingLayout, CampusLayout, GroundPlate, InsetAnnotation};
use serde::Serialize;

/// layout.json with its leading `$schema_note` — `CampusLayout` itself has no
/// such field, so a plain round-trip would silently drop the documentation.
#[derive(Serialize)]
struct LayoutDoc<'a> {
    #[serde(rename = "$schema_note")]
    schema_note: &'a str,
    origin_lat: f64,
    origin_lon: f64,
    buildings: &'a [BuildingLayout],
    ground_plates: &'a [GroundPlate],
    bridges: &'a [Bridge],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    insets: &'a [InsetAnnotation],
    notes: &'a [String],
}

pub const SCHEMA_NOTE: &str = "3D massing layout. Local ENU meters (x=east, y=north) about origin \
    39.9490,-75.1925 (near 34th St & Convention Ave). Wing footprints are surveyed hand-traces \
    (Google Earth Pro, owner, 2026-08) imported by the kml_import tool — see \
    assets/data/kml_mapping.json and kml_import_report.md. Clifton (formerly labeled \"Pavilion\"; \
    node id stays bldg.pavilion) and PCAM keep their verified OSM polygons (way/749302010, \
    way/50033009); the HUP Main podium plate is derived from the union of the eight wing \
    footprints by the podium_gen tool — see assets/data/podium_report.md. bridges[] holds \
    skybridge deck polygons; the topology graph references them by id. levels counts total \
    above-grade stories INCLUDING the shared street-level ground story every building models \
    (the wayfinding directory prints a G span for only four wings, but street level is Ground \
    complex-wide and the Connector is the first floor); the extruded podium plate is that \
    ground story for the eight HUP Main wings. The eight wings share one 3.8 m story height: \
    their numbered floors are continuous plates (verified floor_continuity edges), so per-wing \
    heights would splay the same floor number to different elevations; renderers stack each \
    floor at its labeled story (Ground = 0, floor N = N, 13-skippers compress above 13).";

pub const NOTES: &[&str] = &[
    "34th Street runs through the gap between the HUP Main blob (west) and the Clifton/PCAM block (east).",
    "Wing footprints are owner hand-traces; adjacent wings share party walls, so traces may abut or slightly overlap.",
    "An unnamed 10-level OSM tower (way/749302008) sits in PCAM's NE notch — not rendered; likely a PCAM annex/connector.",
];

/// Serialize a `CampusLayout` as the committed layout.json document:
/// `$schema_note` first, canonical notes last, `[x, y]` pairs one per line,
/// trailing newline. Byte-stable for identical input.
pub fn to_layout_json(layout: &CampusLayout) -> serde_json::Result<String> {
    let notes: Vec<String> = NOTES.iter().map(|s| s.to_string()).collect();
    let doc = LayoutDoc {
        schema_note: SCHEMA_NOTE,
        origin_lat: layout.origin_lat,
        origin_lon: layout.origin_lon,
        buildings: &layout.buildings,
        ground_plates: &layout.ground_plates,
        bridges: &layout.bridges,
        insets: &layout.insets,
        notes: &notes,
    };
    Ok(compact_point_arrays(&serde_json::to_string_pretty(&doc)?) + "\n")
}

/// serde_json's pretty printer puts every number of every `[x, y]` vertex on
/// its own line; collapse those innermost pairs back to one line per point so
/// the committed file stays readable and diffable.
fn compact_point_arrays(json: &str) -> String {
    let lines: Vec<&str> = json.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let open = lines[i];
        let is_number = |s: &str| s.trim_end_matches(',').parse::<f64>().is_ok();
        if open.trim() == "[" && i + 3 < lines.len() {
            let (a, b, close) = (lines[i + 1].trim(), lines[i + 2].trim(), lines[i + 3].trim());
            if is_number(a) && is_number(b) && (close == "]" || close == "],") {
                out.push(format!("{open}{} {}{close}", a, b));
                i += 4;
                continue;
            }
        }
        out.push(open.to_string());
        i += 1;
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hupsim_core::layout::Footprint;

    #[test]
    fn round_trip_parses_and_compacts_points() {
        let layout = CampusLayout {
            origin_lat: 39.949,
            origin_lon: -75.1925,
            buildings: vec![BuildingLayout {
                node: hupsim_core::ids::NodeId("wing.test".into()),
                footprint: Footprint::rect(0.0, 0.0, 10.0, 5.0),
                levels: 3,
                floor_height_m: 4.0,
                ground_elevation_m: 0.0,
                provenance: Default::default(),
            }],
            ..Default::default()
        };
        let json = to_layout_json(&layout).unwrap();
        assert!(json.ends_with('\n'));
        assert!(json.contains("$schema_note"));
        // Vertex pairs are one line each, not exploded.
        assert!(json.contains("[0.0, 0.0]"));
        // The document (minus the note) still parses back into a CampusLayout.
        let back: CampusLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(back.buildings.len(), 1);
        assert_eq!(back.notes.len(), NOTES.len());
    }
}
