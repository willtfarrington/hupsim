//! One-shot KML importer (feature `tools` only — never in the app graph).
//!
//! Converts the owner's Google Earth Pro hand-mapping of the HUP campus into
//! the committed `assets/data/layout.json`: surveyed building footprints
//! replace the hand-seeded tower rectangles, and skybridge polygons become
//! first-class `bridges[]` geometry. The runtime never parses KML — this tool
//! emits committed JSON artifacts.

use hupsim_core::geo::GeoFrame;
use hupsim_core::layout::{Bridge, CampusLayout, Footprint};
use hupsim_core::provenance::{Confidence, Era, Provenance};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use thiserror::Error;

/// Provenance source stamped on every footprint taken from the KML.
pub const KML_SOURCE: &str = "Google Earth Pro hand-mapping (owner), 2026-08";

/// Two vertices closer than this (meters) are degenerate duplicates.
const MIN_VERTEX_SPACING_M: f64 = 0.01;

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("XML parse error: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("XML encoding error: {0}")]
    Encoding(#[from] quick_xml::encoding::EncodingError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("placemark {name:?} is not in kml_mapping.json (typo tripwire — every placemark must be mapped)")]
    UnmappedPlacemark { name: String },
    #[error("mapping entry {name:?} matched no placemark in the KML")]
    MissingPlacemark { name: String },
    #[error("placemark {name:?}: {reason}")]
    BadRing { name: String, reason: String },
    #[error("mapping targets node {node:?} but layout.json has no such building")]
    NoSuchNode { node: String },
    #[error("unresolvable XML entity &{0};")]
    UnknownEntity(String),
}

// ---------------------------------------------------------------------------
// Mapping file (assets/data/kml_mapping.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Mapping {
    pub buildings: BTreeMap<String, BuildingMapping>,
    pub bridges: BTreeMap<String, BridgeMapping>,
}

#[derive(Debug, Deserialize)]
pub struct BuildingMapping {
    pub node: String,
    /// false = the existing (OSM-verified) footprint stays; the KML trace is
    /// a cross-check input and bridge anchor only.
    #[serde(default = "default_true")]
    pub replace_footprint: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct BridgeMapping {
    pub id: String,
    pub from: String,
    pub to: String,
    pub level: u32,
    pub deck_height_m: f32,
}

// ---------------------------------------------------------------------------
// KML parsing
// ---------------------------------------------------------------------------

/// A placemark's entity-decoded name and its outer LinearRing in (lon, lat),
/// exactly as authored (closing duplicate still present).
#[derive(Debug)]
pub struct Placemark {
    pub name: String,
    pub ring: Vec<[f64; 2]>,
}

/// Extract every `<Placemark>` with its outer boundary ring. Names are
/// XML-entity-decoded (the Clifton placemark stores `&quot;Pavilion&quot;`);
/// inner boundaries (holes) are ignored, second outer rings are an error.
pub fn parse_kml(kml: &str) -> Result<Vec<Placemark>, ImportError> {
    let mut reader = Reader::from_str(kml);
    let mut placemarks = Vec::new();

    let mut in_placemark = false;
    let mut in_name = false;
    let mut in_outer = false;
    let mut in_coordinates = false;
    let mut name_buf = String::new();
    let mut coord_buf = String::new();
    let mut ring: Option<Vec<[f64; 2]>> = None;

    loop {
        match reader.read_event()? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"Placemark" => {
                    in_placemark = true;
                    name_buf.clear();
                    ring = None;
                }
                b"name" if in_placemark => in_name = true,
                b"outerBoundaryIs" if in_placemark => in_outer = true,
                b"coordinates" if in_outer => {
                    in_coordinates = true;
                    coord_buf.clear();
                }
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"Placemark" => {
                    in_placemark = false;
                    let name = name_buf.trim().to_string();
                    let bad = |reason: &str| ImportError::BadRing {
                        name: name.clone(),
                        reason: reason.into(),
                    };
                    if name.is_empty() {
                        return Err(bad("placemark has no <name>"));
                    }
                    let ring = ring.take().ok_or_else(|| bad("placemark has no outer ring"))?;
                    placemarks.push(Placemark { name, ring });
                }
                b"name" => in_name = false,
                b"outerBoundaryIs" => in_outer = false,
                b"coordinates" if in_coordinates => {
                    in_coordinates = false;
                    if ring.is_some() {
                        return Err(ImportError::BadRing {
                            name: name_buf.trim().to_string(),
                            reason: "multiple outer rings in one placemark".into(),
                        });
                    }
                    ring = Some(parse_coordinates(&coord_buf, name_buf.trim())?);
                }
                _ => {}
            },
            Event::Text(t) => {
                if in_name {
                    name_buf.push_str(&t.decode()?);
                } else if in_coordinates {
                    coord_buf.push_str(&t.decode()?);
                }
            }
            // quick-xml 0.41 emits `&quot;` etc. as separate reference events.
            Event::GeneralRef(r) => {
                if in_name || in_coordinates {
                    let buf = if in_name { &mut name_buf } else { &mut coord_buf };
                    if let Some(ch) = r.resolve_char_ref()? {
                        buf.push(ch);
                    } else {
                        let entity = r.decode()?.into_owned();
                        let resolved = quick_xml::escape::resolve_xml_entity(&entity)
                            .ok_or(ImportError::UnknownEntity(entity))?;
                        buf.push_str(resolved);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(placemarks)
}

/// Parse a KML `<coordinates>` blob: whitespace-separated `lon,lat[,alt]`.
fn parse_coordinates(text: &str, placemark: &str) -> Result<Vec<[f64; 2]>, ImportError> {
    let bad = |reason: String| ImportError::BadRing {
        name: placemark.to_string(),
        reason,
    };
    let mut out = Vec::new();
    for tuple in text.split_whitespace() {
        let mut parts = tuple.split(',');
        let lon = parts.next().and_then(|s| s.parse::<f64>().ok());
        let lat = parts.next().and_then(|s| s.parse::<f64>().ok());
        match (lon, lat) {
            (Some(lon), Some(lat)) => out.push([lon, lat]),
            _ => return Err(bad(format!("malformed coordinate tuple {tuple:?}"))),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Ring hygiene + projection
// ---------------------------------------------------------------------------

/// Project a (lon, lat) ring into the local frame with full ring hygiene:
/// drop the KML LinearRing's duplicated closing vertex, then assert first ≠
/// last and no consecutive pair closer than 1 cm (a duplicate vertex lies on
/// every nearby candidate ear of the boundary-inclusive ear-clipper, blocks
/// them all, and the fan fallback mis-renders concave polygons). The result
/// is normalized CCW and rounded to 0.1 m (the file's existing precision).
pub fn project_ring(
    frame: &GeoFrame,
    name: &str,
    ring: &[[f64; 2]],
) -> Result<Vec<[f32; 2]>, ImportError> {
    let bad = |reason: String| ImportError::BadRing {
        name: name.to_string(),
        reason,
    };
    let mut pts: Vec<[f64; 2]> = ring
        .iter()
        .map(|&[lon, lat]| frame.forward(lat, lon))
        .collect();

    let dist = |a: [f64; 2], b: [f64; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();

    // Drop the closing duplicate, then re-check: a ring that STILL closes on
    // itself had a doubled closing vertex — that is data corruption, not style.
    match (pts.first().copied(), pts.last().copied()) {
        (Some(first), Some(last)) if dist(first, last) < MIN_VERTEX_SPACING_M => {
            pts.pop();
        }
        _ => return Err(bad("ring is not closed (KML LinearRing must repeat its first vertex)".into())),
    }
    if pts.len() < 3 {
        return Err(bad(format!("only {} unique vertices", pts.len())));
    }
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let d = dist(pts[i], pts[j]);
        if d < MIN_VERTEX_SPACING_M {
            return Err(bad(format!(
                "vertices {i} and {j} are {d:.4} m apart (< 1 cm) — degenerate duplicate"
            )));
        }
    }

    let mut fp = Footprint {
        vertices: pts
            .iter()
            .map(|p| [(p[0] * 10.0).round() as f32 / 10.0, (p[1] * 10.0).round() as f32 / 10.0])
            .collect(),
    };
    fp.ensure_ccw();
    Ok(fp.vertices)
}

// ---------------------------------------------------------------------------
// Import + merge
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ImportOutcome {
    /// The merged layout: surveyed footprints in, non-geometry fields kept,
    /// bridges added. `notes` / schema note are left for the caller to curate.
    pub layout: CampusLayout,
    /// Human-readable delta + cross-check report (markdown).
    pub report: String,
    /// Largest old→new centroid shift among replaced footprints, in meters.
    /// ~0 means the layout already matches the KML (a re-run of the importer),
    /// in which case rewriting the report would erase the historical survey
    /// deltas — callers should refuse to write.
    pub max_centroid_delta_m: f32,
}

/// Run the import: parse the KML, project every ring about the layout's OWN
/// origin, merge into the existing layout, and build the report.
pub fn import(kml: &str, mapping: &Mapping, layout_json: &str) -> Result<ImportOutcome, ImportError> {
    let mut layout: CampusLayout = serde_json::from_str(layout_json)?;
    let frame = GeoFrame::new(layout.origin_lat, layout.origin_lon);
    let placemarks = parse_kml(kml)?;

    let mut seen: BTreeMap<&str, bool> = mapping
        .buildings
        .keys()
        .chain(mapping.bridges.keys())
        .map(|k| (k.as_str(), false))
        .collect();

    let mut report = String::new();
    let mut wing_rows: Vec<String> = Vec::new();
    let mut crosscheck: Vec<String> = Vec::new();
    let mut bridges: Vec<Bridge> = Vec::new();
    let mut max_centroid_delta_m: f32 = 0.0;

    for pm in &placemarks {
        let vertices = project_ring(&frame, &pm.name, &pm.ring)?;
        if let Some(bm) = mapping.buildings.get(&pm.name) {
            *seen.get_mut(pm.name.as_str()).unwrap() = true;
            let building = layout
                .buildings
                .iter_mut()
                .find(|b| b.node.as_str() == bm.node)
                .ok_or_else(|| ImportError::NoSuchNode { node: bm.node.clone() })?;
            let old = building.footprint.clone();
            let new = Footprint { vertices: vertices.clone() };
            if bm.replace_footprint {
                // Keep levels / floor_height_m / ground_elevation_m; the old
                // provenance described geometry that no longer exists.
                building.footprint = new.clone();
                building.provenance = Provenance::new(Confidence::Verified, KML_SOURCE, Era::Timeless);
                let (oc, nc) = (old.centroid(), new.centroid());
                let delta = ((nc[0] - oc[0]).powi(2) + (nc[1] - oc[1]).powi(2)).sqrt();
                max_centroid_delta_m = max_centroid_delta_m.max(delta);
                wing_rows.push(centroid_delta_row(&bm.node, &old, &new));
            } else {
                crosscheck.push(crosscheck_section(&bm.node, &pm.name, &old, &new));
            }
        } else if let Some(bm) = mapping.bridges.get(&pm.name) {
            *seen.get_mut(pm.name.as_str()).unwrap() = true;
            bridges.push(Bridge {
                id: bm.id.clone(),
                from: bm.from.as_str().into(),
                to: bm.to.as_str().into(),
                level: bm.level,
                deck_height_m: bm.deck_height_m,
                vertices,
                provenance: Provenance {
                    confidence: Confidence::Inferred,
                    source: Some(format!("Deck outline: {KML_SOURCE}")),
                    era: Era::Timeless,
                    note: Some(
                        "Level/deck height inferred: Silverstein↔Clifton ≈ L3 (KML LookAt \
                         altitude 12.0 m corroborates); Clifton↔Perelman ≈ L2"
                            .into(),
                    ),
                },
            });
        } else {
            return Err(ImportError::UnmappedPlacemark { name: pm.name.clone() });
        }
    }

    if let Some((name, _)) = seen.iter().find(|(_, seen)| !**seen) {
        return Err(ImportError::MissingPlacemark { name: name.to_string() });
    }

    bridges.sort_by(|a, b| a.id.cmp(&b.id));
    layout.bridges = bridges;

    writeln!(report, "# KML import report\n").unwrap();
    writeln!(report, "Source: `{KML_SOURCE}` — projected equirectangularly about the layout origin ({}, {}).\n", layout.origin_lat, layout.origin_lon).unwrap();
    writeln!(report, "## Wing footprints: old (hand-seeded rectangle) → new (surveyed trace)\n").unwrap();
    writeln!(report, "| node | old centroid (m) | new centroid (m) | delta (m) |").unwrap();
    writeln!(report, "|---|---|---|---|").unwrap();
    for row in &wing_rows {
        writeln!(report, "{row}").unwrap();
    }
    writeln!(report, "\n## Cross-check: KML hand-trace vs kept OSM footprint\n").unwrap();
    writeln!(report, "These footprints were NOT replaced (`replace_footprint: false`) — the OSM polygons stay authoritative; the traces are cross-check inputs and bridge anchors.\n").unwrap();
    for section in &crosscheck {
        writeln!(report, "{section}").unwrap();
    }
    writeln!(report, "## Bridges (new `bridges[]` array)\n").unwrap();
    for b in &layout.bridges {
        writeln!(
            report,
            "- `{}` — {} ↔ {}, level {}, deck {} m, {} vertices, deck area {:.1} m²",
            b.id,
            b.from.as_str(),
            b.to.as_str(),
            b.level,
            b.deck_height_m,
            b.vertices.len(),
            Footprint { vertices: b.vertices.clone() }.signed_area()
        )
        .unwrap();
    }

    Ok(ImportOutcome { layout, report, max_centroid_delta_m })
}

fn centroid_delta_row(node: &str, old: &Footprint, new: &Footprint) -> String {
    let (oc, nc) = (old.centroid(), new.centroid());
    let (dx, dy) = (nc[0] - oc[0], nc[1] - oc[1]);
    format!(
        "| `{node}` | ({:.1}, {:.1}) | ({:.1}, {:.1}) | {:.1} (dx {dx:+.1}, dy {dy:+.1}) |",
        oc[0],
        oc[1],
        nc[0],
        nc[1],
        (dx * dx + dy * dy).sqrt()
    )
}

fn bbox(f: &Footprint) -> [f32; 4] {
    let mut b = [f32::MAX, f32::MIN, f32::MAX, f32::MIN]; // min_x, max_x, min_y, max_y
    for v in &f.vertices {
        b[0] = b[0].min(v[0]);
        b[1] = b[1].max(v[0]);
        b[2] = b[2].min(v[1]);
        b[3] = b[3].max(v[1]);
    }
    b
}

fn crosscheck_section(node: &str, placemark: &str, osm: &Footprint, trace: &Footprint) -> String {
    let (oc, tc) = (osm.centroid(), trace.centroid());
    let (dx, dy) = (tc[0] - oc[0], tc[1] - oc[1]);
    let (oa, ta) = (osm.signed_area().abs(), trace.signed_area().abs());
    let (ob, tb) = (bbox(osm), bbox(trace));
    let mut s = String::new();
    writeln!(s, "### `{node}` (\"{placemark}\")\n").unwrap();
    writeln!(s, "- centroid offset (trace − OSM): dx {dx:+.1} m, dy {dy:+.1} m, dist {:.1} m", (dx * dx + dy * dy).sqrt()).unwrap();
    writeln!(s, "- area: OSM {oa:.0} m² vs trace {ta:.0} m² (delta {:+.1}%)", (ta - oa) / oa * 100.0).unwrap();
    writeln!(
        s,
        "- bbox deltas (trace − OSM): west {:+.1}, east {:+.1}, south {:+.1}, north {:+.1} m",
        tb[0] - ob[0],
        tb[1] - ob[1],
        tb[2] - ob[2],
        tb[3] - ob[3]
    )
    .unwrap();
    s
}

// ---------------------------------------------------------------------------
// Tests — synthetic fixture keeps the tool CI-testable without the real KML.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 2 buildings + 1 bridge, all with the KML closing duplicate; Beta is
    /// authored clockwise to exercise CCW normalization, and Alpha's name
    /// carries XML entities like the real Clifton placemark.
    const SYNTH_KML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2">
<Document>
  <name>synthetic campus</name>
  <Placemark>
    <name>Alpha &quot;Tower&quot;</name>
    <Polygon><outerBoundaryIs><LinearRing><coordinates>
      -75.19300,39.94900,0 -75.19250,39.94900,0 -75.19250,39.94940,0 -75.19300,39.94940,0 -75.19300,39.94900,0
    </coordinates></LinearRing></outerBoundaryIs></Polygon>
  </Placemark>
  <Placemark>
    <name>Beta</name>
    <Polygon><outerBoundaryIs><LinearRing><coordinates>
      -75.19200,39.94900,0 -75.19200,39.94940,0 -75.19150,39.94940,0 -75.19150,39.94900,0 -75.19200,39.94900,0
    </coordinates></LinearRing></outerBoundaryIs></Polygon>
  </Placemark>
  <Placemark>
    <name>bridge Alpha-Beta</name>
    <Polygon><outerBoundaryIs><LinearRing><coordinates>
      -75.19250,39.94918,0 -75.19200,39.94918,0 -75.19200,39.94922,0 -75.19250,39.94922,0 -75.19250,39.94918,0
    </coordinates></LinearRing></outerBoundaryIs></Polygon>
  </Placemark>
</Document>
</kml>"#;

    const SYNTH_MAPPING: &str = r#"{
      "buildings": {
        "Alpha \"Tower\"": {"node": "wing.silverstein"},
        "Beta": {"node": "wing.ravdin"}
      },
      "bridges": {
        "bridge Alpha-Beta": {"id": "bridge.alpha_beta", "from": "wing.silverstein", "to": "wing.ravdin", "level": 3, "deck_height_m": 12.0}
      }
    }"#;

    const SYNTH_LAYOUT: &str = r#"{
      "origin_lat": 39.949,
      "origin_lon": -75.1925,
      "buildings": [
        {
          "node": "wing.silverstein",
          "footprint": { "vertices": [[-105, 95], [-28, 95], [-28, 155], [-105, 155]] },
          "levels": 12,
          "floor_height_m": 3.8,
          "provenance": { "confidence": "inferred", "source": "old seed rectangle", "era": "timeless" }
        },
        {
          "node": "wing.ravdin",
          "footprint": { "vertices": [[-100, 35], [-33, 35], [-33, 95], [-100, 95]] },
          "levels": 9,
          "floor_height_m": 3.8,
          "provenance": { "confidence": "inferred", "source": "old seed rectangle", "era": "timeless" }
        }
      ]
    }"#;

    fn synth_import() -> ImportOutcome {
        let mapping: Mapping = serde_json::from_str(SYNTH_MAPPING).unwrap();
        import(SYNTH_KML, &mapping, SYNTH_LAYOUT).unwrap()
    }

    #[test]
    fn entity_decoded_names_and_rings_parse() {
        let pms = parse_kml(SYNTH_KML).unwrap();
        assert_eq!(pms.len(), 3, "Document <name> must not create a placemark");
        assert_eq!(pms[0].name, "Alpha \"Tower\"");
        assert_eq!(pms[0].ring.len(), 5, "closing duplicate still present pre-hygiene");
    }

    #[test]
    fn import_replaces_footprints_and_keeps_non_geometry_fields() {
        let out = synth_import();
        let alpha = out.layout.building(&"wing.silverstein".into()).unwrap();
        assert_eq!(alpha.footprint.vertices.len(), 4, "closing dupe dropped");
        assert!(alpha.footprint.signed_area() > 0.0, "CCW");
        assert_eq!(alpha.levels, 12, "levels preserved");
        assert_eq!(alpha.floor_height_m, 3.8, "floor height preserved");
        assert_eq!(alpha.provenance.confidence, Confidence::Verified);
        assert!(alpha.provenance.source.as_deref().unwrap().contains("Google Earth Pro"));

        let beta = out.layout.building(&"wing.ravdin".into()).unwrap();
        assert!(beta.footprint.signed_area() > 0.0, "CW input normalized to CCW");
    }

    #[test]
    fn import_emits_ccw_bridge_with_id() {
        let out = synth_import();
        assert_eq!(out.layout.bridges.len(), 1);
        let b = &out.layout.bridges[0];
        assert_eq!(b.id, "bridge.alpha_beta");
        assert_eq!(b.level, 3);
        assert_eq!(b.deck_height_m, 12.0);
        assert!(Footprint { vertices: b.vertices.clone() }.signed_area() > 0.0, "CCW");
    }

    #[test]
    fn imported_layout_compiles() {
        let out = synth_import();
        let json = serde_json::to_string(&out.layout).unwrap();
        let (topo, units, _, lines) = crate::io::embedded();
        assert!(crate::compile::compile(topo, units, &json, lines).is_ok());
    }

    #[test]
    fn old_layout_without_bridges_still_parses() {
        let layout: CampusLayout = serde_json::from_str(SYNTH_LAYOUT).unwrap();
        assert!(layout.bridges.is_empty());
    }

    #[test]
    fn reimport_of_already_imported_layout_reports_zero_delta() {
        // First import moves the seed rectangles to the surveyed traces…
        let first = synth_import();
        assert!(
            first.max_centroid_delta_m > 1.0,
            "fresh import must report a real shift, got {}",
            first.max_centroid_delta_m
        );
        // …re-importing the already-imported layout is a no-op: the delta
        // collapses to ~0, which is the signal the CLI uses to refuse to
        // overwrite the historical survey report.
        let mapping: Mapping = serde_json::from_str(SYNTH_MAPPING).unwrap();
        let reimported_json = serde_json::to_string(&first.layout).unwrap();
        let second = import(SYNTH_KML, &mapping, &reimported_json).unwrap();
        assert!(
            second.max_centroid_delta_m < 0.05,
            "re-import must report ~0 delta, got {}",
            second.max_centroid_delta_m
        );
    }

    #[test]
    fn unmapped_placemark_is_hard_error() {
        let mapping: Mapping = serde_json::from_str(
            r#"{"buildings": {"Beta": {"node": "wing.ravdin"}},
                "bridges": {"bridge Alpha-Beta": {"id": "b", "from": "a", "to": "b", "level": 1, "deck_height_m": 4.0}}}"#,
        )
        .unwrap();
        let err = import(SYNTH_KML, &mapping, SYNTH_LAYOUT).unwrap_err();
        assert!(matches!(err, ImportError::UnmappedPlacemark { name } if name == "Alpha \"Tower\""));
    }

    #[test]
    fn mapping_entry_without_placemark_is_hard_error() {
        let mut mapping: Mapping = serde_json::from_str(SYNTH_MAPPING).unwrap();
        mapping.buildings.insert(
            "Gamma".into(),
            BuildingMapping { node: "wing.gamma".into(), replace_footprint: true },
        );
        let err = import(SYNTH_KML, &mapping, SYNTH_LAYOUT).unwrap_err();
        assert!(matches!(err, ImportError::MissingPlacemark { name } if name == "Gamma"));
    }

    #[test]
    fn degenerate_duplicate_vertex_is_rejected() {
        let frame = GeoFrame::new(39.949, -75.1925);
        // Second vertex repeated (closer than 1 cm) inside the ring.
        let ring = [
            [-75.19300, 39.94900],
            [-75.19250, 39.94900],
            [-75.19250000001, 39.94900],
            [-75.19250, 39.94940],
            [-75.19300, 39.94900],
        ];
        let err = project_ring(&frame, "dupe", &ring).unwrap_err();
        assert!(matches!(err, ImportError::BadRing { .. }));
    }

    #[test]
    fn unclosed_ring_is_rejected() {
        let frame = GeoFrame::new(39.949, -75.1925);
        let ring = [
            [-75.19300, 39.94900],
            [-75.19250, 39.94900],
            [-75.19250, 39.94940],
        ];
        let err = project_ring(&frame, "open", &ring).unwrap_err();
        assert!(matches!(err, ImportError::BadRing { .. }));
    }
}
