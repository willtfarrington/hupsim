# EP-15 — Surge / what-if scenario tools

**Size:** M · **Depends on:** EP-13 (dashboard surface), EP-10 (dynamics) · **Blocks:** EP-18 (findings feed case studies)

## Context

Capacity-planning payoff built on the engine's strongest tested guarantee: seeded
deterministic replay (`SimEngine.reset()` + ChaCha8 reproduces a run exactly). That
makes honest A/B comparison possible: replay the same world with one intervention
changed and attribute every difference to the intervention.

## In scope

1. **Interventions** (each a small, reversible world/process modification staged before
   a run): close a unit (all its rooms → OOS with a labeled reason), inject an arrival
   wave (a scheduled surge of N extra admissions over a window, service-typed), flex
   staffing (override a unit's `target_rn_ratio` for the run — affects EP-11 workload,
   not physics).
2. **A/B runner**: from the current scenario + seed, run baseline and intervention
   variants for a configurable sim horizon (e.g. 48–72 h) headlessly-fast (the engine
   already ticks off-render; drive it in a loop without waiting on FixedUpdate), each
   collecting its own `Timeline` + EP-9 group series. Determinism means one run each is
   sufficient — no Monte Carlo needed for the A-vs-B delta (note honestly: it is one
   realization, not an expectation; EP-17's fan is the place for uncertainty).
3. **Compare view**: in the EP-13 drawer (new tab), overlaid A/B sparklines per selected
   group (census, boarding, occupancy z), with summary deltas (peak boarding, hours over
   95% occupancy, workload peaks). Clear labeling of which line is which; the
   intervention description printed with the chart.
4. **Session flow**: staging an experiment never mutates the live scenario — variants
   run on clones; the live sim continues untouched. Results are held in memory (one
   experiment at a time is fine).
5. **Tests**: A/A comparison (same seed, no intervention) produces identical timelines
   (the null-diff guard); unit-closure intervention conserves patients (nobody vanishes;
   census shifts to other units/boarding); ratio flex changes workload series only.

## Out of scope

- Persisting experiments; multi-seed Monte Carlo; optimization ("find the best
  intervention"). Transport effects (EP-6 not yet in the loop unless already landed).

## Verification / acceptance

- `cargo test --workspace` green; A/A null-diff test is the acceptance cornerstone.
- Run: close an ICU for 48 h against baseline — the compare tab shows boarding rising in
  the intervention line with a stated peak delta; flexing med-surg ratios shows workload
  relief with unchanged census lines.
