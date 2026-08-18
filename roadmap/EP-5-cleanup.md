# EP-5 — Code scope cleanup

**Size:** S · **Depends on:** EP-1 (cleanest after EP-4) · **Blocks:** —

## Context

With the model reduced to one campus and edges re-authored to concrete endpoints, several
core/compile constructs are dead or misleading. This endpoint removes them and adds one
missing safety guard. Paths relative to the hupsim workspace.

## In scope

1. **Remove `CampusZone`** (closed 3-variant enum, `crates/hupsim-core/src/model.rs:37-45`)
   and `Building.campus`:
   - `compile.rs:104-110` `campus_zone()` — delete.
   - The prelude re-export at `crates/hupsim-core/src/lib.rs:25` — delete too (the
     acceptance grep catches it if missed).
   - `crates/hupsim-app/src/ui.rs:383-388` — navigator currently groups by the 3 hardcoded
     zone titles; regroup by `has_inpatient_beds` ("Inpatient buildings" / "Support &
     ambulatory") or plain alphabetical — pick what reads best in the running app.
   - Any `index.rs`/test references.
   - Old scenario saves still parse: serde ignores unknown fields on load. Verify with a
     pre-change save file.
2. **Keep `ConnectionKind::NoPhysicalConnection`** (model.rs) — one tolerant line, may
   return if scope re-expands. "Zero live edges use it" is true only AFTER EP-1 (today
   two such edges compile into live Connections and a test asserts one exists — EP-1
   archives them); re-confirm post-EP-1 rather than trusting the count. The renderer
   skip is the `_ => continue` at `campus3d.rs:147`.
3. **Add `CompileError::DuplicateUnit`** in `compile.rs` `build_units_and_rooms`
   (~:156-199): unit-id uniqueness is currently unvalidated — a duplicate `UnitId`
   silently overwrites `HospitalIndex.unit_pos` (last-wins) while `rooms_by_unit`
   accumulates both units' rooms, corrupting aggregates/timeline. Mirror the existing
   `DuplicateRoom` guard (`compile.rs:197-198`) — but note NO test exercises the
   DuplicateRoom error path either; write the crafted-duplicate test from scratch (and
   consider covering both error paths while at it).
4. **Sweep dead compile arms** now unreachable after EP-1/EP-3: the `hup_cedar` and
   `isc` arms in `short_name()` (:45-59 — there is NO radnor arm there; radnor's only
   compile-code occurrence is the remap remnant at :359), cedar in `room_group()`
   (:62-73), and any leftover remap remnants. If EP-3 introduced a data-driven
   `short_name`, consider finishing the migration (all display names from data; the
   function reduces to a default-tail fallback).
5. **Docs sweep:** `DESIGN.md` and `README.md` in the workspace + the parent directory's
   `readme.md` — remove/redate multi-site and Cedar-era statements; note the KML as the
   geometry source and the archive file's existence.

## Out of scope

- Anything that changes model behavior or rendering beyond the navigator regrouping.
- `InsetAnnotation` type removal if EP-4 already did it (check first).

## Verification / acceptance

- `grep -rn "CampusZone" crates/` → 0 hits.
- `grep -rniE "cedar|radnor|\bisc\b" crates/` → 0 hits outside comments that explain the
  archive (ideally 0 everywhere). Known current comment hits to reword or consciously
  keep: `hupsim-core/src/layout.rs:3` and `:84`, `hupsim-app/src/campus3d.rs:172`,
  `hupsim-data/src/unitsfile.rs:53`, `model.rs:52`/`:269` (some are tied to the inset
  machinery EP-4 may already have removed — check first).
- New duplicate-unit test fails on a crafted duplicate and passes on live data.
- `cargo test --workspace` green; `cargo run -p hupsim-app` smoke: navigator regrouping
  looks sane; a pre-change scenario save still loads.
