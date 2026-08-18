# EP-9 completion report

**For:** the EP-10 session · **Written by:** the EP-9 completion session, 2026-08-12
**Answers:** `EP-9-completion-handoff.md`. That handoff's remaining-scope items 1–4 are
done and verified; **commits were deliberately NOT made** (owner instruction to this
session), so the handoff's commit recipe is still yours to execute. This file tells you
exactly what changed so you can re-fit if needed — short version: **no frozen surface
was touched; no deviations.**

## State of the tree

- `cargo test --workspace` → green: **104 passed + 1 ignored** (the new benchmark).
  The EP-10 determinism tests in `diurnal.rs`/`demo.rs` were not touched and pass.
- `cargo tree -p hupsim-core` (default features) → serde / serde_json / thiserror
  only. Rayon appears solely behind the `parallel` feature.
- Everything remains uncommitted, now including this session's additions below.

## What this session added/changed (EP-9 items 1–3 of your handoff)

**Added:**

1. `crates/hupsim-app/src/agg_cache.rs` — `AggCache` resource: version-keyed
   (`AppWorld.version`, 0 = never computed) cache holding its **own** `RoomMatrix`
   (per your trap callout — it does NOT read `SimDrive.engine.matrix`) plus per-unit
   aggregates, building rollups, per-(building, floor) rollups, `by_unit_type` /
   `by_service_line` rollups (EP-13's two groupings, precomputed and ready), and
   `HospitalStats`. `refresh_if_stale(&AppWorld)` is called by every consumer;
   recompute runs `RoomMatrix::refresh` (fast path when ids/unit assignments are
   unchanged) + matrix transforms. Read-only over the hospital; never touches the RNG.
2. `crates/hupsim-data/tests/matrix_bench.rs` — the `#[ignore]`d benchmark the
   hupsim-data `Cargo.toml` comment promised (comment left true, file exactly there).
   Real compiled hospital (1,263 rooms, demo census seeded) + synthetic 50,520 rooms
   (real rooms replicated 40×, fresh ids). Cross-checks par vs scalar (ints exact,
   floats 1e-4) before printing. Scalar-only fallback when built without the feature.

**Modified:**

3. `crates/hupsim-app/src/ui.rs` — cache consumer swaps only; the EP-10 toolbar block
   is untouched. `draw_ui` gained `mut cache: ResMut<AggCache>` and calls
   `cache.refresh_if_stale(&world)` once at frame start; `bottom_bar` (was
   `hospital_stats` room scan), `left_panel` (was `aggregate_building` +
   `aggregate_unit` per row), `building_inspector` / `floor_inspector` /
   `unit_inspector`, and `draw_hover_tooltip` (was `aggregate_building` per hover
   frame) now take `&AggCache` and read `cache.stats` / `cache.building(world, id)` /
   `cache.unit_at(up)` / `cache.floor(id, level)`. The
   `aggregate_building/aggregate_unit/hospital_stats` imports are gone from ui.rs.
4. `crates/hupsim-app/src/campus3d.rs` — `SlabRegistry(HashMap<NodeId, Vec<Entity>>)`
   resource (closes EP-4 item 8), cleared+filled in `build_scene` (both the `Startup`
   and `SceneRebuild` paths — `setup_scene`/`rebuild_scene` signatures gained
   `ResMut<SlabRegistry>`). `apply_colors` rewritten: reads the cache (no
   `aggregate_building` per slab), full slab pass only when `world.version` moved or
   slabs were freshly added (cheap map lookups); a hover/selection-only change
   re-tints just the buildings entering/leaving that state via the registry. Shared
   `tint_slab` helper keeps the emissive hover/selection logic identical to before.
5. `crates/hupsim-app/src/main.rs` — `mod agg_cache;` +
   `.init_resource::<agg_cache::AggCache>()` + `.init_resource::<campus3d::SlabRegistry>()`.
6. `roadmap/EP-9-room-matrix.md` — completion note appended with the full benchmark
   tables and acceptance evidence (single source of truth for the numbers).

## Benchmark result (summary — full tables in the EP-9 brief's completion note)

Release profile, `--features parallel`, best-of-N wall clock, Windows 11. Real
hospital (1,263 rooms): every transform ≤ ~3 µs scalar; `par_*` ≈ scalar + rayon
overhead (rows fit in one 4,096-row chunk) — HUP doesn't need parallelism, per the
brief. Synthetic 50,520 rooms: unit_aggregates 101.9 → 27.2 µs (3.75×),
rollup_by_unit_type 119.0 → 27.6 µs (4.31×), rollup_by_service_line 114.1 → 26.8 µs
(4.26×), hospital_stats 26.5 → 11.8 µs (2.25×), count_by_unit 15.5 → 11.8 µs (1.31×).
Full build 10.7 ms, status-column refresh 2.4 ms at 50k. Repro:
`cargo test -p hupsim-data --release --features parallel --test matrix_bench -- --ignored --nocapture`.

## Frozen surfaces — confirmation

Untouched by this session: `engine.rs`, `process.rs` (SimCtx semantics), `events.rs`
(discharge guard), `params.rs`, `sampling.rs`, `acuity.rs`, `diurnal.rs`, `edflow.rs`,
`demo.rs`, `rolling.rs`, `matrix.rs`, `world.rs` (SimDrive), the ui.rs toolbar block,
all four `Cargo.toml`s and `Cargo.lock`, and every data/asset file.

## Deviations from the handoff

None. Two flags worth knowing, both judged in-scope-neutral rather than deviations:

- `ui.rs::search_results` still iterates `h.rooms` — it is a text search that runs
  only while a query is typed, not an aggregate recompute; the brief's "no per-frame
  full-room scans" acceptance targets the always-on paths, which are all cached now.
  Fair game for a later cleanup if you disagree.
- `AggCache` recomputes up to 10×/s while the sim runs (every `sim_tick` version
  bump), exactly as your handoff predicted and intended (refresh fast path, ~25 µs
  at HUP scale per the bench).

## What remains for you (unchanged from your own recipe)

1. Commit 1 — `feat(hupsim): RoomMatrix + rolling-stats core (EP-9)`: stage
   `Cargo.toml`, `Cargo.lock`, `crates/hupsim-core/Cargo.toml`,
   `crates/hupsim-core/src/{lib.rs, matrix.rs, aggregate.rs, model.rs}`,
   `crates/hupsim-data/Cargo.toml`, `crates/hupsim-data/tests/matrix_bench.rs`,
   `crates/hupsim-sim/src/rolling.rs` (uncompiled at this commit — say so in the body).
2. Commit 2 — EP-10 + EP-9 completion: everything else, which now also includes
   `agg_cache.rs`, `main.rs`, `ui.rs`, `campus3d.rs`, and the three roadmap files
   (`EP-9-room-matrix.md` completion note, `EP-9-completion-handoff.md`, this report).
3. Roadmap `README.md`: EP-9 ☑ with both hashes, EP-10 ☑ with the second (EP-7
   two-hash precedent). Git root is one level up from the workspace — scope paths.
4. Optional, your call: DESIGN.md gained no matrix/rolling/cache paragraphs (neither
   handoff mandated it and EP-18 is the doc-refresh epic) — fold it into commit 2 if
   you'd rather document now.
