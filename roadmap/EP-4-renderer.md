# EP-4 — Renderer refit

**Size:** M · **Depends on:** EP-3 · **Blocks:** — (EP-5 is cleanest after this)

## Context

`crates/hupsim-app/src/campus3d.rs:135-170` draws every connection as a
centroid-to-centroid `Cuboid` tube at hardcoded heights (bridge 12.0 / corridor 4.5 /
tunnel −2.5), using `Footprint::centroid()` — a naive vertex mean
(`crates/hupsim-core/src/layout.rs:22-29`) that lands outside PCAM's concave outline.
This produced the "phantom road": a ~256 m grey bar crossing the campus. EP-2/EP-3 gave
us real bridge polygons (`layout.bridges[]`) and concrete edge endpoints; this endpoint
makes the renderer use them, adds the podium, and fixes scene lifecycle.

Bevy 0.19 + bevy_egui 0.41.1 (egui 0.35). Verify Bevy APIs against vendored sources in
`~/.cargo/registry/src` when in doubt — training-data Bevy knowledge is stale.
Paths relative to the hupsim workspace.

**Precondition check before starting:** this brief builds on EP-2/EP-3 outputs —
`Connection.layout`, `layout.bridges[]`, `deck_height_m`, `GroundPlate.extrude_m`. If a
grep of the crates/data finds none of these, the prior endpoints have not run (a dirty
working tree is NOT evidence they have): stop and execute EP-2/EP-3 first.

## In scope

1. **Render skybridges from geometry.** For each `Connection` with `layout: Some(id)`,
   extrude the matching `bridges[]` polygon (reuse `mesh.rs` `extrude_polygon`) as a slab
   from `deck_height_m` to `deck_height_m + ~3 m`. Delete the tube path for bridge-kind
   connections that have a layout ref. Result: three short skybridges visibly spanning
   the streets at deck height; **no tube crosses the campus**.
2. **Subtle rendering for geometry-less flow edges** (decision 1): the Dietz & Watson
   bridge, the two Connector corridors, and the unverified tunnel get either (a) a thin
   translucent ground-level ribbon between nearest-footprint-edge points, or
   (b) hover/selection-only display. Recommendation: (b) for the tunnel (unverified),
   (a) at low alpha for D&W + Connectors. Do NOT reintroduce centroid tubes.
3. **Bridge hover/labels.** Bridge entities become pickable (currently everything
   connection-ish is `Pickable::IGNORE`) with observers like the existing `FloorSlab`
   pattern (`campus3d.rs:182-207`); hovering shows name + kind + confidence (the `name`
   field — e.g. "Dietz and Watson Bridge" — is currently dropped on the floor).
4. **Shoelace area centroid.** Add `Footprint::area_centroid()` (shoelace-weighted) in
   `layout.rs` with a unit test on a concave polygon; switch label anchors and any
   remaining nearest-point math to it. Keep the old `centroid()` only if something still
   legitimately wants a vertex mean (probably nothing).
5. **Podium.** Extrude the HUP-Main ground plate per its new `extrude_m` field (EP-3);
   plates without the field keep the 0.35 m plinth. Podium material slightly distinct
   from tower slabs (e.g. the plate color darkened), non-pickable.
6. **Remove the inset mechanism.** `campus3d.rs:172-179` (inset labels + the magic
   `- 90.0` offset) and the `InsetAnnotation` usage — post-EP-1/EP-2, Cedar is archived
   and `insets[]` is empty or absent (against a pre-EP-1 tree it is still populated —
   see the precondition check above). (Type removal from `layout.rs` can wait for EP-5 if it's serde-tolerant.)
7. **Scene rebuild on load.** `setup_scene` is `Startup`-only (`main.rs:59`), so
   `UiAction::Load` (`ui.rs:1055-1071`) leaves stale meshes bound to dead ids. Refactor
   into a `build_scene(...)` called at startup AND on an explicit rebuild trigger (event
   or a `scene_built_version` vs `world.version` comparison — note `apply_colors` already
   diffs `version`, so only the entity set is stale). Despawn the old scene root
   recursively and **remove the Mesh/StandardMaterial assets you created** (hold handles
   on the scene-root component) so repeated loads don't leak.
8. *(Optional stretch, only if quick)* `apply_colors` rescans O(rooms×slabs) at 10 Hz
   while the sim runs (`campus3d.rs:210-262`); a `HashMap<NodeId, Vec<Entity>>` slab
   registry built during `build_scene` makes it O(dirty). Skip if it grows the diff.

## Out of scope

- Data edits (EP-3 done), CampusZone/dead-code sweep (EP-5), transport simulation (EP-6).

## Verification / acceptance

- `cargo test --workspace` green (mesh tests incl. the Perelman pin **added by EP-2** —
  not the pre-existing `pavilion_osm_footprint_triangulates_cleanly`, which pins a
  different building — plus the new area-centroid test).
- Visual smoke (`cargo run -p hupsim-app`), against the KML as reference:
  - Three skybridges at deck height spanning Silverstein↔Clifton and Clifton↔Perelman ×2;
    no long tube anywhere; nothing floats mid-air near PCAM.
  - Hovering a skybridge shows its name/kind/confidence.
  - Podium reads as a ~2-story connective mass under the eight towers; towers rise from it.
  - No Cedar/inset label in the void.
- Load-scenario round-trip: save, admit/discharge a patient, load — scene rebuilds, no
  stale slabs, colors correct, memory does not balloon after several load cycles.
