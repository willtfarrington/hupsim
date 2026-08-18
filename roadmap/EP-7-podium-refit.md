# EP-7 — Podium refit + provenance hygiene

**Size:** M · **Depends on:** EP-5 · **Blocks:** — (EP-8+ assume the repo is clean)
**Status:** DONE (executed 2026-08-12, the session that authored this phase-2 roadmap).

## Context

Two persistent rendering defects survived the EP-0…EP-5 refit, both traced to the podium
being an *unrelated* polygon: the ground plate was an 11-vertex OSM silhouette (different
survey era than the KML wing traces — 57% of it covered empty ground, Silverstein
overhung it by up to 22.8 m) extruded 0→8 m as an opaque solid, geometrically swallowing
the bottom ~2 floors of all eight main-campus wings (wing floors start at
`ground_elevation_m = 0`, floor heights 3.6–4.0 m). Clifton/PCAM sit outside the plate
and were unaffected. Picking still worked (`Pickable::IGNORE`); the bug was visual.

Separately, two uncommitted working-tree regressions: `kml_import_report.md` had been
re-run against the already-imported layout (all survey deltas read 0.0), and
`scratch.md`'s roadmap summary block was deleted.

Owner decisions: podium at **one story (3.5 m)**; footprint = **union of the eight wing
traces + 3 m buffer, simplified**; computed offline in the tools path and committed;
restore both regressed files and guard the importer against silent no-op re-runs.

## What was done

1. `git restore` of `hupsim/assets/data/kml_import_report.md` and `scratch.md`.
2. **`Footprint::contains_point`** (even-odd ray cast) added to
   `crates/hupsim-core/src/layout.rs` — reused by the generator, compile tests, and the
   ribbon fix.
3. **`crates/hupsim-data/src/layoutdoc.rs`** (tools-gated): the layout.json document
   writer (`$schema_note`, canonical notes, point compaction) extracted out of
   `bin/kml_import.rs` so both one-shot tools serialize identically.
4. **`crates/hupsim-data/src/podium.rs`** (tools-gated): `derive_podium` — rasterize the
   wing union at 0.25 m → Felzenszwalb distance transform → **morphological closing with
   net buffer** (dilate 18 m, erode 15 m ⇒ +3 m from wing walls; closing is what removes
   the 9–26 m inter-wing bays no simplifier tolerance could) → hole fill → Moore boundary
   trace → Douglas-Peucker (1.5 m) + collinear/short-edge merge → 0.1 m rounding.
   Validates its own output (single component, simple CCW ring, ≤28 vertices, the full
   wing boundary — sampled at ≤1 m along every edge — ≥1 m inside) and fails loudly.
   Zero new dependencies.
5. **`bin/podium_gen.rs`**: replaces `ground_plates[0]` in `layout.json` with the derived
   ring, `extrude_m: 3.5`, derivation provenance; writes
   `assets/data/podium_report.md` (params, inputs, result stats, regen command).
   Result: **19 vertices, 24,260 m²** (vs 26,873 m² OSM blob), min wing clearance 1.51 m.
   Idempotent — second run is byte-identical.
6. **Tests**: `compile.rs` pin 8.0→3.5 plus `podium_contains_all_wing_footprints` and
   `podium_is_single_clean_ccw_ring` (area band 15,000–27,000 m², 12–28 verts, no
   self-intersection, min edge ≥2 m); `mesh.rs` "podium pin" — the committed ring
   triangulates to n−2 ears (no fan fallback); `kml.rs`
   `reimport_of_already_imported_layout_reports_zero_delta`.
7. **Ribbon fix** (`campus3d.rs`): geometry-less flow ribbons (D&W bridge, the Connector)
   drew at y=0.6 — *inside* the podium at their wing endpoints. Each ribbon now lifts to
   the tallest extruded plate either endpoint sits on (+0.6 m), so they run visibly along
   the podium top; off-plate edges stay at grade.
8. **Importer guard**: `ImportOutcome.max_centroid_delta_m`; `kml_import` refuses
   (exit 1, nothing written) when deltas are ~0 unless `--force` — the historical survey
   deltas can no longer be silently erased.
9. Docs: `DESIGN.md`, `hupsim/README.md`, roadmap `README.md` decision-3 addendum.

## Verification / acceptance (all confirmed)

- `cargo test --workspace` green; `cargo test -p hupsim-data --features tools` green.
- `podium_gen` run twice → byte-identical layout.json + podium_report.md.
- `kml_import` without `--force` → refuses, exit 1, no files written.
- Visual: podium hugs the wing block at one story; wing floors 2+ visible and tintable;
  D&W/Connector ribbons visible on the podium top; skybridge decks unchanged (8/12 m).
