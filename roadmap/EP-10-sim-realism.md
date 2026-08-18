# EP-10 — Sim realism: diurnal flow, LOS, ED pressure

**Size:** M · **Depends on:** EP-9 (rolling stats consume the signal) · **Blocks:** EP-12/EP-13 demos, EP-17

## Context

Deviation-from-norm indicators are only meaningful against plausible dynamics. Today the
demo sim (`hupsim-sim/src/demo.rs`) is a flat random walk (`AcuityDrift`) plus a
constant-rate arrival/discharge process (`DemoFlow`, 2/hr Poisson-ish, discharge when
instability < 0.3). No time-of-day structure, no service-specific length of stay, no ED
boarding pressure. Owner decision: build realism **before** the dashboards so norms,
z-scores, and over-capacity indicators show real structure from day one.

Architecture rule (strict, from EP-6's brief): simulation models live in **`hupsim-sim`**
as `SimProcess` impls, never in the app. Determinism is the engine's strongest tested
guarantee — single `ChaCha8Rng`, FIFO event tie-breaking, `reset()` replays exactly.
Every new process must preserve it (no wall-clock, no HashMap iteration order).

## In scope

1. **`DiurnalFlow`** (replaces or supersedes `DemoFlow`; keep the old one behind its
   toggle if trivial): arrival intensity λ(t) shaped by sim time-of-day — morning
   scheduled-admission bump, afternoon ED-driven peak, overnight trough; weekday/weekend
   factor. Admissions carry `service` drawn from the *destination unit's* service (the
   current `DemoFlow` leaves patient service `None` — fix that), and an initial acuity
   drawn per service. Discharge timing from **service-specific LOS distributions**
   (log-normal-ish via the seeded RNG; parameters in one visible table, tagged as
   synthetic/inferred), with a discharge-hour shape (mid-day peak — the "discharge by
   noon" lever EP-15/EP-17 will exploit).
2. **`EdPressure`**: ED arrivals into the 122 ED bays with their own λ(t); a fraction
   convert to admissions needing an inpatient bed; when no suitable vacant bed exists the
   patient **boards in the ED** (`boarding: true`, the existing flag) until a bed frees.
   This is the mechanism that makes occupancy/boarding dashboards move credibly.
3. **Acuity realism pass** on `AcuityDrift`: keep the random walk but tie drift/volatility
   to unit type (ICU patients wander more; obs less), still mean-reverting, still
   deterministic.
4. **Efficiency**: the existing `DemoFlow` rebuilds a full vacant-room `Vec` by scanning
   all 1,263 rooms *per arrival* — new processes should query the EP-9 matrix/masks
   instead of rescanning (`SimCtx` may need read access to the matrix or a vacancy
   index; keep the borrow story simple — a per-tick refreshed mask is fine).
5. **Config surface**: per-process enable toggles in the toolbar (pattern exists for
   drift/flow at `ui.rs` toolbar); parameters as constants-with-provenance in the sim
   crate, not UI-editable yet.
6. **Tests**: determinism (same seed ⇒ identical world after N ticks; reset/replay
   exact — mirror `demo.rs`'s existing tests); shape sanity (overnight arrival rate <
   afternoon rate over a long deterministic run; boarding occurs when ICU capacity is
   artificially constrained).

## Out of scope

- Transport/routing time (EP-6), RN staffing (EP-11), forecasting (EP-17).
- Real HUP arrival data — parameters are synthetic-but-shaped, honestly labeled.

## Verification / acceptance

- `cargo test --workspace` green including new determinism tests.
- Run 3+ sim days at high speed: census sparkline shows a visible diurnal wave; ED
  boarding count rises and falls; a unit's 24 h z-score (EP-9) reacts to an injected
  surge (manually seed extra arrivals) and settles back.
- No process scans `Hospital.rooms` per event when a matrix mask can answer it.
