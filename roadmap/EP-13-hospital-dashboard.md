# EP-13 — Hospital-wide aggregate dashboard

**Size:** M · **Depends on:** EP-12 (widget kit), EP-8/EP-9 (groupings + rollups) · **Blocks:** EP-14, EP-15, EP-16, EP-17 all render into or beside it

## Context

The main (campus) window's only aggregate surface is the bottom status bar: hospital
census, boarding, critical/unstable counts, one unlabeled sparkline. The owner wants
aggregate breakdowns **by level of care** (all units of a kind — `UnitType` is already a
clean enum), **by service line / umbrella department** (EP-8 taxonomy), and by building —
with the same deviation-from-norm indicators as the unit strip. Widgets come from EP-12's
kit; rollup data from EP-9's group ring buffers. Hand-painted egui only.

## In scope

1. **Dashboard panel** in the Campus3D view: a toggleable bottom drawer above the status
   bar (toolbar button + keyboard shortcut), sized so the 3D campus stays usable when
   open. Content in three switchable tabs (or side-by-side columns if width allows):
   - **Level of care**: one row per `UnitType` with beds (ICU, stepdown, med-surg, ED,
     obs, L&D, nursery): census/capacity chip, occupancy % with z badge, boarding,
     worst-instability swatch, per-group sparkline with 24h norm band.
   - **Service line**: same row anatomy per EP-8 line; the Other/Unassigned line renders
     with its unverified-provenance marker so it never reads as ground truth.
   - **Building**: same rows per bedded building (the navigator's grouping logic exists;
     reuse the index, not fresh scans).
2. **Drill-through**: clicking a row focuses the group — level-of-care/service rows
   filter the navigator to member units and tint the 3D slabs of member floors
   (emissive highlight path exists in `apply_colors`); building rows behave like a
   navigator building click. Escape/toggle clears.
3. **Status-bar upgrade**: the existing census sparkline gains the 24h norm band and
   moves to the shared widget; hospital-level z badge beside census. Keep the bar
   one line tall.
4. **Pressure ordering**: within each tab, rows sort by a simple pressure key
   (occupancy z first, then boarding) so the surging group is always on top — the
   at-a-glance triage read the owner is after. Sorting is stable/deterministic.
5. **Tests**: view-model assembly per tab (groups × cache → rows) unit-tested,
   including the warm-up state and empty groups (e.g. a service line with zero beds).

## Out of scope

- Query/staging (EP-14), what-if controls (EP-15), morning-report export (EP-16),
  forecast fan (EP-17) — this EP builds the surface they later plug into.
- Historical charting beyond the 24h/2-week ring buffers.

## Verification / acceptance

- `cargo test --workspace` green; view-model tests as above.
- Run with EP-10 on: the drawer opens over the 3D view; the three tabs populate; a
  constrained-ICU scenario (force ICU rooms OOS) pushes Critical Care / ICU rows to the
  top with a positive z and visible boarding; drill-through highlights the right slabs
  and units; status-bar band renders.
- Panel closed: zero measurable frame cost (cache-only reads, gated painting).
