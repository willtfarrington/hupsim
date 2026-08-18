# EP-14 — Query / staging panel (room placement engine)

**Size:** L · **Depends on:** EP-9 (matrix), EP-11 (RN headroom), EP-13 (drawer/highlight patterns) · **Blocks:** — (EP-6 later adds an optional transport-time term)

## Context

The interactive payoff of the matrix: a click-prep/configure surface to stage
transformations that answer placement questions — "which ICU rooms are available to this
patient?", "what is statistically the best room given these criteria?" Owner decisions
(settled): a **dockable panel in the main window** (query and map visible together;
results highlight live in the 3D campus and unit views); ranking with **level-of-care
appropriateness dominant** — in the owner's words, overflow to the appropriate level of
care trumps service/unit-specific placement conventions in a clinical crunch — then
service-line match, unit occupancy headroom, RN workload headroom, and telemetry
availability. Transport distance was deliberately **not** selected; EP-6 may add it as an
optional term later.

## In scope

1. **Query model** (core, beside the matrix): a `PlacementQuery` — patient needs either
   picked from a real patient (click a room → "find placement") or staged hypothetically
   (criteria form): required level of care (`UnitType` target), telemetry need, isolation
   status (airborne ⇒ negative-pressure required), service line, current location
   (optional). **Hard filters** run first on matrix masks: vacant + in-service, LOC
   band (exact target, plus explicitly-labeled adjacent-LOC overflow candidates — e.g.
   stepdown shown for an ICU-target search *only* under an "overflow" toggle, never
   silently), capability satisfaction. **Scoring** on survivors: lexicographic tier for
   LOC appropriateness (exact target ≻ acceptable overflow), then a weighted sum of
   service-line match, unit occupancy headroom (prefer lower occupancy and lower
   pressure z from EP-9), RN workload headroom (EP-11 composite of the covering RN),
   telemetry fit (needing-and-capable ≻ needing-and-portable-monitor). Weights in one
   visible constants table; every result carries its per-criterion breakdown.
2. **Panel UI**: right-side dockable panel (coexists with the inspector — either a tab
   of the right panel or a second collapsible panel; pick what reads best) with criteria
   chips (LOC, telemetry, isolation, service, overflow toggle), a Run control, and a
   ranked result list: room id, unit, score bar with breakdown segments (EP-12 widget
   kit), and the reasons ("MICU — exact LOC · service match · unit at 78% (z −0.4) ·
   RN #2 light · tele capable").
3. **Live highlighting**: results tint their floor slabs in the 3D view (reuse the
   emissive highlight path) and outline their cells when the containing unit is open in
   unit view; selecting a result focuses it (existing `Focus`/`Selection` machinery).
   Clearing the query clears highlights.
4. **Stage → act**: from a result, one click hands off to the existing transfer/admit
   ops (`ops.rs` via `UiAction`) — the query panel is decision support, the op is the
   existing sanctioned mutation path. Determinism note: querying is read-only and must
   not touch the RNG.
5. **Tests**: matrix-level — hard filters produce exactly the expected candidate sets on
   crafted worlds (isolation excludes non-negative-pressure; overflow toggle admits
   stepdown for ICU target only when on); scoring — LOC tier strictly dominates (a
   worse-scored exact-LOC room always outranks a better-scored overflow room); breakdown
   sums match; determinism (same world ⇒ identical ranking).

## Out of scope

- Transport-time scoring (EP-6 hook), multi-patient batch placement, auto-placement.
- Any new clinical attributes (EP-8 fixed the column set this works over).

## Verification / acceptance

- `cargo test --workspace` green; scoring/filter tests as above.
- Run with EP-10/EP-11 on: stage an ICU-need + telemetry + airborne query — candidates
  are only vacant negative-pressure ICU rooms; toggling overflow surfaces stepdown
  clearly labeled; highlights track results; handing off a transfer works and the moved
  patient's room drops from the results on re-run.
