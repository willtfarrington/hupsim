# EP-11 — RN staffing model + workload burden

**Size:** L · **Depends on:** EP-8 (ratios), EP-9 (matrix); EP-10 helps demos · **Blocks:** EP-12 (workload bars), EP-14 (RN headroom scoring)

## Context

No staff entity exists anywhere in the model. The unit dashboards need number-anonymized
RNs ("RN #1", "RN #2"…) with visible workload burden, and the placement engine (EP-14)
wants RN workload headroom as a scoring term. Owner decisions (all settled): ratio-based
deterministic auto-assignment **plus** acuity-balanced assignment, 7a/7p shift turnover,
and manual reassignment in the UI. Workload composite = census-vs-ratio + composite
patient instability + isolation/telemetry overhead + trailing-window churn.

RN assignments are **ephemeral sim state** (not part of `Scenario`/schema v1): they
regenerate deterministically from the seed and current census on load/reset. Document
this in DESIGN.md — it is a modeling decision, not an accident.

## In scope

1. **`RnRoster`** (new module in `hupsim-sim`): per inpatient/ED unit, a set of RNs
   sized from `Unit.target_rn_ratio` (EP-8) and current census (⌈census/ratio⌉, min 1
   when census > 0), each RN an anonymized index within the unit ("RN #1"…). Assignment
   maps each occupied room → an RN.
2. **Assignment algorithm** (deterministic, no RNG needed): rooms in unit-geography
   order (the existing room order is corridor order from the compiler) partitioned into
   contiguous runs — then **acuity-balanced**: a greedy pass moves boundary rooms to
   equalize composite workload (below) across RNs while keeping each RN's rooms
   geographically contiguous-ish (bounded swap distance). Deterministic tie-breaking by
   room index.
3. **Workload composite** (pure function, lives beside the matrix in core so UI and
   scoring share it): weighted sum of (a) census vs the unit ratio (1.0 = at ratio),
   (b) summed patient instability, (c) per-patient isolation and telemetry overheads,
   (d) churn — admits/discharges/transfers touching that RN's rooms in a trailing
   window (reuse the EP-9 ring-buffer machinery at RN granularity or a simple decaying
   counter fed by sim events; boarding patients count here). Output a scalar plus the
   component breakdown (the UI shows both).
4. **Shift turnover**: at 07:00 and 19:00 sim time, the roster re-derives (fresh
   assignment, renumbered RNs), workload churn counters reset, and a handoff line goes
   to the sim log ("Founders 12: shift change, 5 RNs on"). Manual overrides (below) do
   not survive turnover — that's the point of a handoff.
5. **Manual reassignment UI**: in the unit view, selecting an occupied room exposes its
   RN in the room inspector with a "reassign to RN #k" control; override marks the
   assignment pinned until the next turnover or a census-triggered re-derive that can't
   honor it (log when dropped). Queue mutations through the existing `UiAction` pattern
   (`ui.rs`).
6. **Recompute triggers**: census changes in a unit re-derive that unit's assignment
   incrementally (only the affected unit), honoring pins where possible.
7. **Tests**: determinism (same seed + same tick sequence ⇒ identical rosters);
   balance property (max−min composite workload across RNs within a tolerance on a
   crafted uneven-acuity unit); turnover renumbers and resets churn; pinned override
   survives an unrelated admission and dies at turnover.

## Out of scope

- Charge nurses, techs, licensed-ratio regulation modeling, skill mix.
- Any dashboard rendering (EP-12 draws the bars; this EP supplies the numbers).
- Persisting rosters into scenario saves.

## Verification / acceptance

- `cargo test --workspace` green including roster determinism tests.
- Run with EP-10 processes on: unit view inspector shows each occupied room's RN;
  reassignment works and logs; 07:00/19:00 handoffs appear in the log.
- Workload breakdown for any RN sums to its scalar; an isolation+telemetry patient
  visibly costs more than a stable walkie-talkie.
