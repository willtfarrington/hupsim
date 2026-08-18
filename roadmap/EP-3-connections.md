# EP-3 — Connection refit + podium + naming

**Size:** M · **Depends on:** EP-2 (`bridges[]` must exist in layout) · **Blocks:** EP-4

## Context

The topology's inter-building edges currently use abstract endpoints (`bldg.hup_main`, a
`building_complex` with no footprint), which the compiler patches with a hardcoded remap
(`bldg.hup_main` → `wing.silverstein`, `crates/hupsim-data/src/compile.rs:355-362`).
That remap is the data-side half of the "phantom PCAM road". Note the remap also serves
campus-level ids (`campus.west_philadelphia`, `campus.cedar_avenue`, `campus.radnor`)
used by the two `no_physical_connection` edges — deleting it is safe only because EP-1
archived those edges and updated the cedar test; verify that actually happened. This
endpoint re-authors the edges with concrete endpoints tied to the EP-2 bridge geometry,
specs the podium, and finishes the Clifton naming.

Decision record (roadmap/README.md, decisions 1, 3, 4) governs. Paths relative to the
hupsim workspace.

## In scope

### 1. Re-author `assets/data/hup_topology.json` edges

Replace the current bridge/corridor edge set with concrete endpoints:

| Edge | type | endpoints | `layout` ref | confidence |
|---|---|---|---|---|
| Silverstein↔Clifton skybridge | `bridge` | `wing.silverstein` ↔ `bldg.pavilion` | `bridge.silverstein_clifton` | verified (KML) |
| Clifton↔Perelman skybridge #1 | `bridge` | `bldg.pavilion` ↔ `bldg.pcam` | `bridge.clifton_pcam_1` | verified (KML) |
| Clifton↔Perelman skybridge #2 | `bridge` | `bldg.pavilion` ↔ `bldg.pcam` | `bridge.clifton_pcam_2` | verified (KML) |
| Dietz & Watson Bridge | `bridge` | pick the concrete HUP-Main wing per source (likely `wing.silverstein` or `wing.ravdin` east frontage) ↔ `bldg.pcam` | *(none — no KML geometry)* | verified (directory) — **alignment → open_questions** |
| The Connector | `connector_corridor`, level 1 | concrete wing ↔ `bldg.pcam` | none | verified (directory) |
| Pavilion↔PCAM connector | `connector_corridor` | `bldg.pavilion` ↔ `bldg.pcam` | none | verified (directory) |
| HUP Main↔Clifton tunnel | `tunnel` | concrete wing ↔ `bldg.pavilion` | none | **unverified** (unchanged) |
| Intra-complex adjacency/continuity edges | unchanged | already concrete wing↔wing | — | unchanged |

- Two parallel Clifton↔PCAM bridges between the same node pair are why edges carry an
  explicit `layout` reference rather than a (from,to) lookup. The `bridge.*` ids in the
  table are the ids EP-2 was instructed to emit — reconcile against the actual
  `bridges[]` ids in the committed layout if spellings differ.
- If the deprecated dangling `bldg.hup_main → bldg.penn_tower` edge is still in the file,
  EP-1 missed it — archive it per decision 2 (do not leave it; the acceptance grep below
  forbids `bldg.hup_main` edge endpoints).
- Every re-authored edge keeps/updates `confidence`, `name`, and `note` (provenance
  discipline). Add the D&W alignment question and the bridge-level estimates to
  `open_questions`.

### 2. Wire the `layout` edge ref through the pipeline

- `rawtopo.rs` RawEdge: add tolerant `layout: Option<String>`.
- `hupsim-core` `Connection` (model.rs): add `layout: Option<String>` (serde-defaulted).
- `compile.rs`: pass it through; **DELETE the remap closure** (:355-362) — all endpoints
  are now concrete; a dangling endpoint should now be a compile warning/error, not a
  silent patch.
- New compile test: every `Connection.layout` that is `Some` resolves to a `bridges[]` id
  in the layout (and conversely, warn on orphan bridge geometry).
- `ensure_ccw` currently runs only on `buildings` and `ground_plates`
  (`compile.rs:427-432`) — **add the `bridges[]` loop**. (EP-2's importer already emits
  CCW; this is belt-and-braces so later hand edits to layout.json can't feed the
  extruder CW rings.)

### 3. Podium spec (data only; render in EP-4)

- In `assets/data/layout.json`, on the HUP-Main complex OSM ground plate — currently the
  SOLE `ground_plates[]` entry, labeled "HUP Main — true OSM outline (wings above are
  hand-seeded massing)"; EP-2 regenerates the file, so match the HUP-Main plate whatever
  its exact post-import label — add `extrude_m: 8.0` (~2 stories) + provenance
  `confidence: inferred, note: "connective low-level podium; towers overhang plate by
  ~5-11 m in places (KML re-survey vs OSM plate) — accepted, see open_questions"`.
- `crates/hupsim-core/src/layout.rs` GroundPlate struct: tolerant `extrude_m: Option<f32>` (default None keeps
  the current 0.35 m plinth behavior for other plates).

### 4. Naming

- `bldg.pavilion` display: `short_name` "Pavilion" → **"Clifton"** (the `name` field is
  already "The Clifton Center for Medical Breakthroughs"; keep alias "The Pavilion").
  If short_name is still produced by the hardcoded `short_name()` in `compile.rs:45-59`,
  prefer adding an optional per-node `short_name` field in the topology JSON and using it
  when present (data-over-code; the full hardcode sweep is EP-5).

## Out of scope

- Any rendering change — EP-4 (the app will still draw old-style tubes for bridge edges
  until then; that's fine for one commit).
- Removing `CampusZone` / dead compile arms — EP-5.

## Verification / acceptance

- `cargo test --workspace` green, including the new layout-ref resolution test.
- Grep checks: `grep -n "bldg.hup_main" assets/data/hup_topology.json` → only the node
  definition and `parent:` references, never an edge endpoint; the remap closure is gone
  from compile.rs.
- `cargo run -p hupsim-app` smoke: app runs; navigator shows "Clifton" as the short name.
- `open_questions` contains the D&W alignment + bridge-level items.
