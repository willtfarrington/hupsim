# EP-9 — RoomMatrix + rolling-stats engine

**Size:** L · **Depends on:** EP-8 · **Blocks:** EP-11, EP-12, EP-13, EP-14

## Context

The informatics endpoint of this phase: reconfigure the room data as a **matrix** — all
1,263 rooms × their clinical variables — for rapid, compiled transformations, abstracted
only at the UI surface. Favorable ground: `Hospital.rooms` is already one flat,
stably-ordered `Vec` (`model.rs`); `HospitalIndex.rooms_by_unit` yields dense `usize` row
indices; the compiler emits each unit's rooms consecutively (`compile.rs`). Obstacles:
`RoomStatus::Occupied { patient }` inlines a 10-field struct with heap `String`s into every
row; every join hashes a `String` id; and there is **no rolling statistics machinery
anywhere** — `Timeline` (`hupsim-sim/src/timeline.rs`, 15-min unit samples, 2-week cap) is
raw samples whose only consumer is the census sparkline.

Owner decision (settled): **derived columnar view**. `Vec<Room>` stays the source of
truth and the serialization format (scenario schema v1 untouched, determinism tests
untouched); the matrix is rebuilt/refreshed keyed on the existing `AppWorld.version`
counter. Rayon-parallel transforms behind an optional feature, with a benchmark, as an
architecture demonstration — 1,263 rooms don't *need* parallelism; the point is that the
pattern scales.

## In scope

1. **`RoomMatrix`** — new module `crates/hupsim-core/src/matrix.rs`. Dense `u32` row per
   room in `Hospital.rooms` order; columns (struct-of-arrays): `unit_pos: u32`,
   `unit_type: UnitType`, `service_line: u16`, `building_pos: u16`, occupancy state
   (vacant/occupied/OOS as a u8), `boarding: bool`, `telemetry_capable`,
   `negative_pressure`, patient `instability: f32`, `trend_per_hr: f32`, `telemetry_need`,
   isolation flag, `admitted_at_min: f32`. Constructor `RoomMatrix::build(&Hospital,
   &HospitalIndex)`; `refresh(&Hospital)` fast-path that rewrites only the
   status-derived columns (structure changes rebuild). Side tables map row ↔ `RoomId`.
2. **Transform API**: filter/aggregate over columns without touching `Vec<Room>` —
   e.g. `count_by(unit_pos, predicate)`, masked z-score inputs, vacancy masks per
   `(unit_type, capability)` combination. Behind `#[cfg(feature = "parallel")]`, the same
   transforms via rayon `par_iter`; default build adds no dependencies (hupsim-core's
   only deps today are serde, serde_json, and thiserror — keep the default build that
   lean; rayon only behind the optional feature).
3. **Rolling stats** — new module (core or sim; suggest `hupsim-sim/src/rolling.rs`, it
   is time-series machinery): fixed-size ring buffer per tracked series — **96 samples ×
   15 sim-min = trailing 24 h** (owner decision), oldest replaced by newest exactly as
   the owner described. Tracked per unit AND per rollup group (service line, level of
   care, building, hospital): census, occupancy fraction, boarding, mean instability.
   Exposes `mean()`, `stddev()`, `z(current)`, `band()` (mean ± kσ for chart bands).
   Fed from the existing `Timeline::maybe_sample` cadence — do not add a second sampling
   clock; warm-up behavior (< window filled) returns `None` z rather than fake zeros.
4. **Aggregate cache.** A Bevy resource caching `UnitAggregate`/rollup results keyed on
   `AppWorld.version` — today the navigator recomputes ~10 building rollups every egui
   frame (`ui.rs` navigator + inspectors + status bar; only `apply_colors` memoizes).
   Also close EP-4's skipped item 8: slab registry `HashMap<NodeId, Vec<Entity>>` so
   `apply_colors` (`campus3d.rs`) stops rescanning O(rooms×slabs) per dirty frame.
5. **Group rollups**: aggregate by `UnitType` (level of care) and by service line
   (EP-8) — the two groupings EP-13 renders. Built on the matrix, not on room scans.
6. **Benchmark**: an `#[ignore]`d timing test (run via `cargo test -- --ignored`) or a
   tools-gated bin comparing scalar vs `parallel`-feature transforms on the real 1,263
   rooms and on a synthetic 50k-room matrix (the "scales to a health system" datapoint).
   No new default dependencies; rayon only inside the optional feature.
7. **Determinism guard**: matrix refresh must not perturb sim behavior — reset/replay
   tests stay green; add a test asserting `RoomMatrix::build` twice ⇒ identical columns.

## Out of scope

- Making the matrix the primary store (rejected — schema bump + ops rewrite).
- Any visible dashboard UI (EP-12/EP-13). The status bar may switch to cached stats.
- Forecasting (EP-17) — rolling.rs should just make band/z data easy to consume.

## Verification / acceptance

- `cargo test --workspace` green, including determinism replay tests untouched.
- Navigator/inspector frame cost: no per-frame full-room scans remain (inspect: the
  cache is consulted; version bump invalidates once).
- Benchmark table (scalar vs parallel, 1.3k vs 50k rooms) captured in the brief's
  completion note or DESIGN.md.
- `cargo build -p hupsim-core` (default features) adds zero dependencies.

---

## Completion note (2026-08-12)

Executed across two sessions: the core matrix/rolling modules landed first (and the
EP-10 session wired them into the engine — see `EP-9-completion-handoff.md`); the
app-side aggregate cache (`agg_cache.rs`), the `campus3d.rs` slab registry, and the
benchmark were finished by the EP-9 completion session. Commits deferred to the EP-10
session per owner instruction — see `EP-9-completion-report.md`.

**Benchmark** (item 6) — `cargo test -p hupsim-data --release --features parallel
--test matrix_bench -- --ignored --nocapture`, Windows 11, best-of-N wall clock:

### Real hospital — 1,263 rooms

| transform | scalar (µs) | parallel (µs) | speedup |
|---|---:|---:|---:|
| build (full) | 191.5 | — | — |
| refresh (fast path) | 23.4 | — | — |
| unit_aggregates | 2.7 | 2.9 | 0.93× |
| rollup_by_unit_type | 3.1 | 3.3 | 0.94× |
| rollup_by_service_line | 2.9 | 3.1 | 0.94× |
| hospital_stats | 0.7 | 0.7 | 1.00× |
| count_by_unit (vacant) | 0.4 | 0.8 | 0.50× |
| masked_mean_std (instability) | 0.9 | — | — |

### Synthetic health system — 50,520 rooms (real rooms replicated 40×)

| transform | scalar (µs) | parallel (µs) | speedup |
|---|---:|---:|---:|
| build (full) | 10,677.2 | — | — |
| refresh (fast path) | 2,384.6 | — | — |
| unit_aggregates | 101.9 | 27.2 | 3.75× |
| rollup_by_unit_type | 119.0 | 27.6 | 4.31× |
| rollup_by_service_line | 114.1 | 26.8 | 4.26× |
| hospital_stats | 26.5 | 11.8 | 2.25× |
| count_by_unit (vacant) | 15.5 | 11.8 | 1.31× |
| masked_mean_std (instability) | 35.0 | — | — |

Reading: at 1,263 rooms every transform is ≤ ~3 µs scalar — there is nothing for
parallelism to win (the rows fit inside one 4,096-row chunk, so `par_*` is scalar
plus rayon overhead), which is the brief's own point: HUP does not *need* it. At
50,520 rooms the identical code gets 2–4× on the aggregation transforms — the
"scales to a health system" datapoint. `build`/`refresh` are deliberately scalar
(side-table construction and status-column rewrites). Parallel partials merge in
fixed chunk order, so results are deterministic run-to-run.

Acceptance verified: `cargo test --workspace` green (104 tests + 1 ignored bench);
determinism replay tests untouched; `RoomMatrix::build` twice ⇒ identical columns
(`build_twice_is_identical`); `cargo tree -p hupsim-core` (default features) shows
serde/serde_json/thiserror only — no rayon; no per-frame full-room scans remain
(status bar, navigator, inspectors, tooltips, and `apply_colors` all read the
version-keyed `AggCache`, which invalidates once per `AppWorld.version` bump).
