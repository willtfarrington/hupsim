# EP-8 — Clinical data enrichment

**Size:** M · **Depends on:** EP-7 · **Blocks:** EP-9 (matrix columns), EP-11 (ratios), EP-13 (service-line rollups), EP-14 (capability filters)

## Context

The phase-2 dashboards and the room-placement query engine need three kinds of data the
model does not carry today:

- **Service lines.** `Unit.service` (`crates/hupsim-core/src/model.rs`) is free text —
  35 distinct strings across the 48 units in `assets/data/units.json`, 11 of them
  literally `"Unassigned (no public source)"`, plus near-duplicates ("Advanced Medicine
  (general medicine w/ telemetry)" vs "(medical/telemetry)"). Hospital-wide rollups by
  department/umbrella need a normalized taxonomy.
- **Room capabilities.** `Room` has no capability fields — isolation and telemetry exist
  only as *patient* attributes (`patient.rs`), so "which vacant rooms can take this
  airborne-isolation, telemetry-requiring patient" is unanswerable. Owner selected two
  columns: `telemetry_capable` and `negative_pressure`.
- **Target RN ratios.** No staffing data exists anywhere (grep for nurse/ratio: only
  "staffed" as `capacity − out_of_service`). EP-11's assignment engine needs per-unit
  targets.

Owner decision: Claude drafts the service-line mapping, **owner reviews every entry**;
the 11 unassigned units are explicitly flagged for owner ruling, not silently binned.

## In scope

1. **`ServiceLine` taxonomy.** New data file `assets/data/service_lines.json`: an
   ordered list of umbrella lines — suggested starting set: Medicine, Cardiac,
   Critical Care, Surgery, Oncology, Neuro, Women's Health & Neonatal, Emergency,
   Behavioral Health, Other/Unassigned — plus a `unit → line` mapping covering all 48
   units, each entry carrying provenance (`confidence`, `source` naming the unit's
   `service` string it was derived from, `note` where judgment was applied). Units whose
   `service` is "Unassigned (no public source)" map to Other/Unassigned with
   `confidence: unverified` and a `needs_owner_ruling: true` flag the compiler surfaces
   as a warning list (visible in the data-source indicator or log, not a hard error).
2. **Compiler plumbing.** `Unit.service_line: Option<ServiceLineId>` (serde-defaulted so
   old scenario saves parse), populated in `crates/hupsim-data/src/compile.rs` where
   `Unit` is built from `UnitEntry` (`unitsfile.rs`). Missing mapping for a unit = hard
   `CompileError` (the mapping file must stay total as units are added).
3. **Room capability columns.** `Room.telemetry_capable: bool` and
   `Room.negative_pressure: bool` (serde-defaulted false). Seeded from unit-level rules
   in `units.json` room-generation specs (e.g. ICU/stepdown/ED units telemetry-capable
   throughout; med-surg telemetry per the unit's service note; 1–2 negative-pressure
   rooms per inpatient unit, deterministic choice — first room numbers, not RNG),
   provenance `inferred` documented in each unit entry. Keep the seeding rule in data,
   not code constants, so the owner can correct per-unit.
4. **Target RN ratios.** Per-unit `target_rn_ratio` in `units.json` (patients per RN:
   ICU 2, stepdown 3, med-surg 4–5, ED zoned; L&D/nursery per judgment), provenance
   tagged; compiled onto `Unit`.
5. **Tests**: every unit resolves to a service line; capability seeding is deterministic
   and matches pinned counts for a few known units; old scenario saves (pre-EP-8) still
   load with defaulted fields.
6. **Docs**: DESIGN.md data-file section gains the new file + fields.

## Out of scope

- Any UI beyond surfacing the needs-ruling warning list. Dashboards consume this in
  EP-12/EP-13; the query panel in EP-14.
- Patient-side attributes (already exist) and acuity model changes.

## Verification / acceptance

- `cargo test --workspace` green; new tests as above.
- Owner has reviewed the mapping file diff — every non-obvious assignment carries a
  `note`, and the needs-ruling list matches the 11 unassigned units.
- A pre-EP-8 scenario save loads without error.
