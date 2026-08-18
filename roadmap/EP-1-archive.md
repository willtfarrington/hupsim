# EP-1 — Archive out-of-scope data

**Size:** S · **Depends on:** EP-0 · **Blocks:** EP-2 (the regenerated layout must not
contain entries that would immediately be archived)

## Context

The project scope is now HUP West Philadelphia only. The data files still contain HUP
Cedar (a detached hospital rendered via a deliberately falsified footprint + inset),
Radnor, ISC, and four unmapped/vestigial wings. Per the decision record
(roadmap/README.md): **archive, don't delete** — move the records to
`hupsim/assets/data/archive/out_of_scope.json` so the compiler never sees them but the
research (all provenance tags intact) is preserved.

All paths below are relative to the hupsim workspace
(`hupsim/`, directly under the repository root).

## In scope — complete enumeration (do not trim)

Create `assets/data/archive/out_of_scope.json` with a `$schema_note` explaining what it
is and why (scope reduced to HUP West Philadelphia campus, 2026-08), then MOVE (remove
from live file, add to archive verbatim):

**From `assets/data/hup_topology.json`:**
- Nodes: `bldg.hup_cedar`, `campus.cedar_avenue`, `bldg.radnor`, `campus.radnor`,
  `bldg.isc`, `campus.offsite`, `wing.donner`, `wing.devon`, `wing.gibson`,
  `wing.centrex`, and the 3 topology unit nodes `unit.cedar.psych`, `unit.cedar.detox`,
  `unit.cedar.crc`.
- Edges (five total): both `no_physical_connection` edges (west_philadelphia↔cedar_avenue,
  west_philadelphia↔radnor); the deprecated `bldg.penn_tower` bridge edge (already
  dangling today — no `bldg.penn_tower` node exists, so it is already absent from the
  compiled model; archiving it changes no behavior); and **BOTH
  Gibson structural_adjacency edges** — `wing.gates↔wing.gibson` (confidence
  **verified** — must land in the archive, otherwise it silently vanishes via the
  dangling-edge filter in `crates/hupsim-data/src/compile.rs:369-371`) and
  `wing.founders↔wing.gibson` (inferred).
- Prose: remove archived ids from `derived_views.*`; move Cedar/Centrex/Gibson/Donner
  items out of `open_questions` and `meta.known_limitations` into the archive file's own
  notes. **Split mixed sentences** — both lists mix archived wings (Gibson, Donner, Devon,
  Centrex) with retained ones (Maloney, White) in the same sentence; keep the retained
  halves coherent in place rather than relocating whole sentences.

**From `assets/data/units.json`:**
- **SIX Cedar units** (not three): `unit.cedar.psych`, `unit.cedar.detox`,
  `unit.cedar.crc`, `unit.cedar.5nw`, `unit.cedar.6nw`, `unit.cedar.ed`. Verify with a
  grep for `unit.cedar` before and after; the archive must contain all six with their
  room specs and provenance.

**From `assets/data/layout.json`:**
- The `bldg.hup_cedar` building entry (the ~2.8 km-translated fake footprint — note in
  the archive that its vertices are IN THE TRANSLATED FRAME, not real coordinates).
- The `wing.gibson`, `wing.donner`, and `wing.devon` building entries (≈ lines 56/63/77)
  — their topology nodes are being archived, and `compile()` passes layout buildings
  through unfiltered, so leaving them fails the `referential_integrity` test AND the
  acceptance grep below. **Four layout buildings go to the archive, not one.**
- The entire `insets[]` array (Cedar was its only user).
- The Centrex line in `notes[]` and Cedar prose in the `$schema_note`.
- **Same-commit constraint:** layout entries must be pruned in the SAME commit as their
  topology nodes — the `referential_integrity` test (`compile.rs:523`) asserts every
  layout node exists as a building.

**Tests (three break — update, don't delete coverage):**
- `cedar_is_disconnected_and_inset` (`compile.rs:589`) — remove or invert into a test
  asserting Cedar ids are absent from the compiled model.
- `bedless_wings_present_for_topological_honesty` (`compile.rs:601`) — drop `wing.gibson`
  (and any other archived wing) from the expected list; `gates`/`maloney`/`white` stay.
- `compiles_embedded_data` (`compile.rs:490-498`) — `buildings.len() >= 13` fails at 10
  (8 wings + Pavilion/Clifton + PCAM); adjust to `>= 10`. The `units.len() >= 40`
  assertion survives (54 − 6 = 48). The 588/504 room-count tests and floor-span tests are
  unaffected — do not touch them.

## Out of scope

- Dead Rust code arms referencing cedar/radnor/isc (`short_name`, `room_group`,
  `campus_zone`, connection remap) — they become unreachable but harmless; EP-5 sweeps them.
- `CampusZone` enum removal — EP-5.
- Any KML/layout regeneration — EP-2.

## Known accepted consequence

Old scenario save files embed the full `Hospital` and will resurrect Cedar buildings in
the navigator when loaded (mesh-less, layout-less). Accepted for a dev tool; add one line
to `DESIGN.md` noting it.

## Verification / acceptance

- `cargo test --workspace` green.
- `grep -riE "cedar|radnor|bldg\.isc|donner|devon|gibson|centrex|penn_tower" assets/data/ --include="*.json"`
  hits ONLY `assets/data/archive/out_of_scope.json`. (The `isc` alternative MUST stay
  bounded as `bldg\.isc` — a bare `isc` matches the word "discrepancy" in four in-scope
  units.json notes and can never pass.)
- `cargo run -p hupsim-app` smoke: app opens, navigator/3D shows exactly 10 buildings
  (Founders, Silverstein, Rhoads, Ravdin, Dulles, Gates, Maloney, White, Pavilion/Clifton,
  PCAM), no Cedar inset label floating in the void.
- The archive file round-trips as valid JSON and contains: 13 nodes, **5 edges** (2
  no_physical_connection + penn_tower + 2 Gibson adjacency), 6 units, **4 layout
  buildings** (hup_cedar, gibson, donner, devon), the insets array, and a dated
  `$schema_note`.
- Commit with a message like `data(hupsim): archive out-of-scope sites and wings (EP-1)`.
