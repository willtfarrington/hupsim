# EP-2 — Geo helper + KML importer + regenerated layout

**Size:** L · **Depends on:** EP-1 · **Blocks:** EP-3, EP-4

## Context

The owner hand-mapped the HUP campus in Google Earth Pro:
`../source material/hospital of the university of pennsylvania.kml` (in the project's
`source material` folder above the hupsim workspace; git-tracked since EP-0). It contains 10 building polygons and 3 skybridge
polygons in lat/lon (altitude 0, clamped). This endpoint builds the one-shot importer
that converts it into the committed `assets/data/layout.json`, replacing the hand-seeded
tower rectangles with surveyed footprints and introducing first-class bridge geometry.

**Frame math is already validated:** an equirectangular projection about the layout's
EXISTING origin (`origin_lat: 39.949`, `origin_lon: -75.1925` — present in layout.json
but currently read by no code) reproduces the existing ENU frame. KML Clifton bbox
x[−14.3, 141.4] y[−116.4, 14.9] vs the in-file OSM Pavilion polygon x[−14.7, 140.3]
y[−133.9, 14.4] — ≈0.5 m agreement on the shared north/west extremes and ~1.1 m on the
east (all inside the ±1.5 m fixture tolerance below). So KML footprints
drop directly into the existing coordinate space and the OSM ground plates stay aligned.
(The ~17 m southern delta is hand-trace-vs-OSM extent difference — exactly what the
cross-check report exists to surface.)

Paths relative to the hupsim workspace.

## In scope

### 1. Geo helper — `crates/hupsim-core/src/geo.rs` (new, ~40 lines)

Equirectangular forward/inverse: `x = (lon − lon0)·cos(lat0)·111_320`,
`y = (lat − lat0)·111_320` (meters; x=east, y=north — matches the layout frame per
`crates/hupsim-core/src/layout.rs:11-12`). Origin comes from `CampusLayout.origin_lat/lon`
— do NOT hardcode. Unit tests: round-trip, plus the Clifton-vs-OSM bbox agreement above
as a fixture assertion (±1.5 m on the shared extremes).

### 2. Importer CLI — `crates/hupsim-data/src/bin/kml_import.rs` (new)

- Behind a cargo feature: `[features] tools = ["dep:quick-xml"]` and
  `[[bin]] name = "kml_import"` with `required-features = ["tools"]`, plus
  `quick-xml = { version = "0.41", optional = true }` under `[dependencies]` —
  quick-xml **0.41.0 is already in Cargo.lock** via Bevy's Linux windowing stack
  (wayland-scanner), so align with that pin. **The app dependency graph must not gain
  quick-xml without the feature** (note: the `cargo tree` acceptance check below is clean
  on this Windows host; on Linux it would show the unrelated wayland-scanner hit).
  Runtime never parses KML — the importer emits committed JSON artifacts.
- Input: the KML + a checked-in mapping file `assets/data/kml_mapping.json`:

```json
{
  "buildings": {
    "Silverstein Pavilion":        {"node": "wing.silverstein"},
    "Ravdin Building":             {"node": "wing.ravdin"},
    "Rhoads Pavilion":             {"node": "wing.rhoads"},
    "White Memorial Building":     {"node": "wing.white"},
    "Dulles Building":             {"node": "wing.dulles"},
    "Gates Building":              {"node": "wing.gates"},
    "Founders Pavilion":           {"node": "wing.founders"},
    "Maloney Building1":           {"node": "wing.maloney"},
    "Clifton Center (previously labeled as \"Pavilion\")": {"node": "bldg.pavilion", "replace_footprint": false},
    "Perelman Center for Advanced Medicine":               {"node": "bldg.pcam",     "replace_footprint": false}
  },
  "bridges": {
    "skybridge between Silverstein and Clifton": {"id": "bridge.silverstein_clifton", "from": "wing.silverstein", "to": "bldg.pavilion", "level": 3, "deck_height_m": 12.0},
    "skybridge #1 between Clifton and Perelman": {"id": "bridge.clifton_pcam_1", "from": "bldg.pavilion", "to": "bldg.pcam", "level": 2, "deck_height_m": 8.0},
    "skybridge #2 between Clifton and Perelman": {"id": "bridge.clifton_pcam_2", "from": "bldg.pavilion", "to": "bldg.pcam", "level": 2, "deck_height_m": 8.0}
  }
}
```

  Note the node id stays **`bldg.pavilion`** for Clifton — `room.pavilion.*` ids and the
  504-room test must not move. Bridge levels/heights are `inferred` (decision 4:
  Silverstein↔Clifton ≈ level 3, KML LookAt altitude 12.0 m corroborates; Clifton↔Perelman
  ≈ level 2). Unmapped placemark names = hard error (typo tripwire). Match against
  the XML-entity-DECODED placemark name — the KML stores the Clifton name as
  `&quot;Pavilion&quot;`; quick-xml's unescaping handles this, naive string extraction
  will miss the key and trip the hard error.

- **Ring hygiene (correctness, not cosmetics):** drop the KML LinearRing's duplicated
  closing vertex; assert first ≠ last after the drop; assert no consecutive vertices
  < 1 cm apart. Rationale: the ear-clipper's `point_in_triangle` is boundary-inclusive —
  a duplicate vertex lies on every nearby candidate ear, blocks them all, and the fan
  fallback mis-renders concave polygons (Perelman: 25 unique vertices, concave, ~3 m
  notch). Normalize ALL emitted rings (buildings AND bridges) to CCW in the importer
  itself — compile's `ensure_ccw` currently covers only buildings and ground plates;
  EP-3 extends it to `bridges[]` as belt-and-braces for later hand edits.

- **Merge, don't overwrite, layout.json:** preserve each building's non-geometry fields
  (`levels`, `floor_height_m`, …) — BUT **regenerate provenance notes that describe old
  geometry**. The old hand-seed notes assert now-wrong positions (the KML is a re-survey:
  White moved SW→NE corner, Rhoads ~60 m north, Silverstein NE→SE). New provenance:
  `source: "Google Earth Pro hand-mapping (owner), 2026-08"`, `confidence: verified`.

- **Footprint policy:** the KML replaces the 8 hand-seeded tower rectangles outright
  (post-EP-1 count — before EP-1, layout.json still holds 11 wing rectangles plus Cedar;
  confirm EP-0/EP-1 actually ran before starting).
  For Clifton and PCAM the verified OSM polygons (way/749302010, way/50033009) remain the
  rendered footprints (`replace_footprint: false`); the hand-traces are cross-check inputs
  and bridge anchors (~2 m agreement at anchor points).

- **`bridges[]` — new layout.json array + `CampusLayout` field** (`layout.rs`):
  `{ id, from, to, level, deck_height_m, vertices: Vec<[f32;2]>, provenance }`.
  Geometry lives in layout (the meters-frame document); the topology edge will reference
  a bridge by id in EP-3 (two parallel Clifton↔PCAM bridges between the same node pair
  make (from,to) lookup ambiguous — hence explicit ids). Serde-default the field so old
  layouts still parse.

- **Reports (stdout, human-readable):** per-wing old→new centroid delta table; KML-trace
  vs OSM discrepancy report for Clifton/PCAM (centroid offset, area delta, bbox deltas).
  Paste both into the commit message or a `assets/data/kml_import_report.md`.

### 3. Tests

- Synthetic KML fixture (small, 2 buildings + 1 bridge, with closing-dupe vertices) in
  `crates/hupsim-data/tests/` or `#[cfg(test)]` — importer round-trip produces layout JSON
  the loader compiles. This keeps the tool CI-testable without the real KML.
- Pin the **imported Perelman polygon** in a `mesh.rs` triangulation test (n−2 triangles,
  shoelace area within the existing pin's 1e-3 relative tolerance), like the existing
  Pavilion pin (`mesh.rs:211-233`). It is the worst polygon in the set. EP-4's acceptance
  refers to this test as "the Perelman pin" — it does not exist until this endpoint adds it.

### 4. Run the importer for real

Regenerate `assets/data/layout.json`, eyeball the report, commit the artifacts
(layout.json + kml_mapping.json + report).

## Out of scope

- Topology edge re-authoring and the podium spec — EP-3.
- Rendering bridges — EP-4 (until then the new `bridges[]` array is loaded but unused;
  the old centroid-tube rendering keeps drawing from `hospital.connections`).

## Verification / acceptance

- `cargo test -p hupsim-data --features tools` and `cargo test --workspace` green.
- `cargo tree -p hupsim-app | grep quick-xml` → no hit (feature gating works).
- Regenerated layout compiles + renders: `cargo run -p hupsim-app` shows the towers at
  their surveyed positions over the (unchanged) OSM ground plates; nothing floats off-plate
  unexpectedly (small overhangs expected per decision 3 — podium comes in EP-3/EP-4).
- 3 bridge entries exist, emitted CCW, ids exactly `bridge.silverstein_clifton`,
  `bridge.clifton_pcam_1`, `bridge.clifton_pcam_2`.
- Delta + cross-check reports produced and committed.
