# EP-6 — Flow-graph enrichment (deferred / optional)

**Size:** L · **Depends on:** EP-3 (edges), ideally EP-4/EP-5 · **Blocks:** —
**Status:** deferred — not on the critical path; pick up when the refit (EP-0…EP-5) is done.

## Context

The whole point of keeping the Connector corridors and the Dietz & Watson bridge as data
(decision 1) is this endpoint: the "navigation-and-patient-flow-conscious topology"
payoff. Today `hupsim_core::ops::transfer` is instantaneous room→room and consults the
connection graph not at all; the sim has no notion of transport time, transport teams, or
route. This endpoint makes intra-campus patient movement traverse the physical graph.

Architecture rule (strict): time-series/simulation models go in **`hupsim-sim`** as
`SimProcess` impls, never in the app. Crate order `app → {data, sim} → core`.

## Scope sketch (refine at pickup)

1. **Route graph in core:** derive a traversal graph from `hospital.connections` +
   `elevator_cores` — nodes = (building, level) pairs, edges = floor_continuity /
   structural_adjacency / bridge / connector_corridor / tunnel / elevator hops, each with
   a traversal-minutes weight (walk speed × distance where geometry exists — bridges have
   real lengths from `layout.bridges[]`, a structure **created by EP-2** (see
   EP-2-kml-importer.md); if it is absent from the tree, EP-2 has not run — either wait
   or measure the 3 skybridge polygons in the source KML directly. Elevators get a
   wait+ride constant; inferred weights tagged as such). Shortest-path helper (plain
   Dijkstra, no new deps). Decide a **confidence traversal policy**: verified/reported
   edges traversable; inferred traversable but flagged; the unverified HUP Main↔Clifton
   tunnel excluded by default (or behind a toggle).
2. **Transport process in sim:** a `SimProcess` that turns transfer intents into
   timed transport: depart event → in-transport state → arrive event, duration from the
   route. Requires a place for the patient to exist off-bed — either an
   `in_transport` holding list on the sim side or a room-status extension; design it
   deterministically (ChaCha8 RNG from the engine, reset() replays exactly — the
   engine's strongest tested guarantee, do not break it).
3. **Surfacing:** transport log lines with route summaries ("Founders 5 → Clifton 8 via
   Silverstein↔Clifton skybridge, 14 min"); optionally a UI overlay highlighting the
   active route's edges in the 3D view.
4. **Validation questions this unlocks (portfolio-relevant):** ED-to-bed transport times
   HUP Main vs Clifton; how the skybridge topology constrains PCAM/CHPS access (note:
   CHPS inpatient beds are modeled at Ravdin 6 NE, deliberately correcting the raw
   topology's PCAM placement — PCAM access and CHPS access are distinct destinations);
   elevator core loading (core→building assignment comes from the `build_cores` name
   heuristic, `compile.rs:397-415` — all 8 cores currently resolve, but the derivation
   inherits that heuristic). Document at least one such analysis in the workspace README when done.

## Verification / acceptance (sketch)

- Route tests: known pairs produce expected hop sequences (e.g. Founders→PCAM must cross
  via a skybridge or Connector edge, never teleport); weights positive; unreachable pairs
  reported, not panicked.
- Determinism test: reset/replay with in-flight transports reproduces exactly.
- `cargo test --workspace` green; visual smoke with the transport process enabled.
