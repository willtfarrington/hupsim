# EP-9 completion handoff

**For:** the session completing EP-9 · **Written by:** the EP-10 session, 2026-08-12
**Read first:** `roadmap/EP-9-room-matrix.md` (the original brief), then this file. This
file narrows that brief to what is still open and freezes what later work already built
on top of it.

## Situation

The original EP-9 session hit its context limit mid-brief and never committed. A
subsequent session then executed **EP-10** (`EP-10-sim-realism.md`) to completion on top
of the partial EP-9 work, wiring some of EP-9's remaining glue in the process. **Both
efforts sit uncommitted in the working tree right now**, and `cargo test --workspace`
is green (104 tests) as handed off. Your job: finish only what remains of EP-9 without
disturbing the finished EP-10 work, then commit per the recipe below.

## Already done — do not redo, do not restructure

EP-9 brief items 1, 2, 3, 5, 7 are complete:

- `hupsim-core/src/matrix.rs` — `RoomMatrix` (columnar view; `build`/`refresh` fast
  path; vacancy masks, `count_by_unit`, rollups by unit type/service line/building,
  `hospital_stats`, `masked_mean_std`; rayon `par_*` variants behind the optional
  `parallel` feature) with full tests; wired into core's `lib.rs`/prelude. The
  workspace + core/data `Cargo.toml`s already carry the rayon feature plumbing.
- `hupsim-sim/src/rolling.rs` — `RollingSeries`/`MetricWindows`/`RollingStore`
  (96 × 15-min trailing-24 h ring buffers; warm-up returns `None`, never fake zeros)
  with tests; declared in hupsim-sim's `lib.rs` and exported via its prelude.
- Engine integration (done during EP-10 — treat as **frozen**):
  - `SimEngine.matrix: RoomMatrix` — refreshed every tick after due events, before
    processes; exposed read-only to processes as `SimCtx.matrix` (tick-start snapshot
    semantics documented in `process.rs`).
  - `SimEngine.rolling: RollingStore` — fed exactly when `Timeline::maybe_sample`
    returns `true` (one sampling clock, no drift). `SimEngine::reset` clears both.
- `UnitAggregate::absorb`, `UnitType::ALL`/`index()` exist in core and are consumed by
  both the matrix and the rolling store.

EP-10 is complete and verified (also uncommitted). Treat all of it as **frozen**: new
sim modules `params.rs`, `sampling.rs`, `acuity.rs` (AcuityDrift lives there now, not
demo.rs), `diurnal.rs` (DiurnalFlow), `edflow.rs` (EdPressure); `SimEventKind::Discharge`
gained an `expect_patient: Option<PatientId>` guard; `demo.rs` keeps only DemoFlow
(matrix-based candidates); app `SimDrive` registers drift/flow/diurnal/ED at indices
0–3 and the toolbar has four process toggles. If you believe something frozen must
change to finish your scope, stop and explain instead of changing it.

## Remaining EP-9 scope (your work)

1. **Aggregate cache** (brief item 4, first half). A Bevy resource caching per-unit
   `UnitAggregate`s, the UnitType/service-line/building rollups, and `HospitalStats`,
   keyed on `AppWorld.version`; the `ui.rs` navigator, inspectors, and status bar
   consume it instead of recomputing per egui frame. Suggested shape: a **new file**
   `crates/hupsim-app/src/agg_cache.rs` holding its own `RoomMatrix` plus cached
   results; on version mismatch, `RoomMatrix::refresh` + recompute via the matrix
   transforms. **Do not read `SimDrive.engine.matrix` for this** — the engine's copy
   only refreshes during ticks, so it is stale while paused and during editor
   mutations; the app cache keys on `AppWorld.version` and reads the `Hospital`
   directly. Note `sim_drive.rs` bumps `version` every fixed-update tick while the
   clock runs — the cache recomputing ≤10×/s is intended and cheap (refresh fast
   path); the win is that egui frames (often faster) and paused/editing frames stop
   rescanning entirely.
2. **Slab registry** (brief item 4, second half — closes EP-4's skipped item 8). A
   `HashMap<NodeId, Vec<Entity>>` (or equivalent resource) so `apply_colors` in
   `campus3d.rs` stops rescanning O(rooms × slabs) per dirty frame. Rebuild it where
   the scene is (re)built (`SceneRebuild` path).
3. **Benchmark** (brief item 6). `crates/hupsim-data/Cargo.toml` already forwards the
   `parallel` feature, and its comment promises `tests/matrix_bench.rs` — create
   exactly that file (or relocate it and fix the comment): an `#[ignore]`d timing test
   comparing scalar vs `par_*` transforms on the compiled real hospital (1,263 rooms)
   and a synthetic ~50k-room world (the "scales to a health system" datapoint). Run via
   `cargo test -p hupsim-data --features parallel -- --ignored`. Capture the resulting
   table as a completion note appended to `roadmap/EP-9-room-matrix.md`.
4. **Acceptance** (from the brief): workspace tests green; no per-frame full-room
   scans remain in the navigator/inspector/status-bar/apply_colors paths;
   `cargo build -p hupsim-core` (default features) stays rayon-free — verify with
   `cargo tree -p hupsim-core`.

## Constraints

- The cache and registry are **derived, read-only state**: they must never mutate sim
  state or consume the engine RNG. Determinism (ChaCha8 seed → identical replay,
  `reset()` exact) is the suite's strongest guarantee; the EP-10 determinism tests in
  `diurnal.rs`/`demo.rs` must stay green untouched.
- Stack pins: Bevy **0.19** + bevy_egui **0.41.1** (egui 0.35), rand **0.9.5**. Verify
  Bevy APIs against the vendored sources in `~/.cargo/registry/src` — training-data
  Bevy knowledge is stale.
- `ui.rs` and `world.rs` already contain uncommitted EP-10 edits (toolbar block,
  SimDrive). Edit around them; do not revert or reformat those regions.

## Commit recipe (only after `cargo test --workspace` is green)

Two commits, each of which builds green standalone:

1. `feat(hupsim): RoomMatrix + rolling-stats core (EP-9)` — stage ONLY:
   `Cargo.toml`, `Cargo.lock`,
   `crates/hupsim-core/Cargo.toml`, `crates/hupsim-core/src/{lib.rs, matrix.rs, aggregate.rs, model.rs}`,
   `crates/hupsim-data/Cargo.toml`, the new benchmark test file, and
   `crates/hupsim-sim/src/rolling.rs`.
   (`rolling.rs` is intentionally not yet compiled at this commit — its module
   declaration rides in commit 2 with the rest of the sim crate; say so in the commit
   body. Core changes are additive, so the old sim/app compile against them.)
2. `feat(hupsim): sim realism — diurnal flow, service LOS, ED boarding (EP-10); EP-9 completion (engine wiring, aggregate cache, slab registry)` —
   everything else in the tree.

Then update `roadmap/README.md`: EP-9 ☑ with **both** hashes (precedent: EP-7's box
lists two), EP-10 ☑ with the second hash. Remember the git root is one level up from
the workspace — scope paths deliberately.

## Completion report (hand back to the EP-10 session)

Finish by emitting a short report the owner can paste into the EP-10 session:
files added/changed for items 1–3; the benchmark table; both commit hashes; the
roadmap edits made; and any deviation from the frozen surfaces or constraints above
(ideally: none). That report lets the EP-10 session re-fit its output if anything moved.
