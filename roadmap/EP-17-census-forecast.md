# EP-17 — Census forecasting (short-horizon fan)

**Size:** M · **Depends on:** EP-10 (diurnal structure), EP-13 (chart surface) · **Blocks:** EP-18

## Context

A short-horizon (6–24 h) census projection drawn as a fan on the census chart —
informatics credibility without ML theater: a transparent statistical baseline, honestly
calibrated, clearly labeled. The point being demonstrated is judgment (what a simple
model can and cannot claim), not model sophistication.

> **Note (2026-08-12, post-EP-16).** Two pickup decisions feed this brief:
> (1) **Event ledger** — calibration scoring (item 3) and EP-18 evidence want a typed
> record of admissions / ED admit decisions / discharges. The display log is a capped
> (2,000-entry) `Vec<String>`, and EP-16's log-corroboration scan is coupled to message
> prefixes and labeled "≥" when the log can't reach back — don't parse strings for
> scoring; decide here whether to add a small typed event ring buffer in `hupsim-sim`
> (EP-16's report scan migrates to it if built). (2) **Elapsed-LOS conditioning** — the
> expected-departures term is much better conditioned on each patient's elapsed LOS,
> which the standing census only has after EP-21 (seeded-census backdating). Prefer
> running EP-21 first; otherwise departures fall back to unconditional LOS draws.

## In scope

1. **Forecast model** (pure, in `hupsim-sim` beside `rolling.rs`): census(t+h) ≈
   current census + expected arrivals − expected departures over h, where expected
   arrivals come from the EP-10 diurnal λ(t) integrated over the horizon (the sim's own
   generator — the forecast is *model-consistent* by construction, and says so) and
   expected departures from the LOS/discharge-hour tables applied to the current
   standing census. Uncertainty band: Poisson-ish variance on both flows propagated to
   quantile curves (e.g. 10/50/90) — analytic, deterministic, no Monte Carlo.
2. **Presentation**: fan (median line + shaded quantile band) appended to the census
   sparkline in the EP-13 drawer and status bar chart, visually distinct from the
   *observed* line and the *norm band* (three encodings on one chart — apply the dataviz
   skill; the fan must not be confusable with history). Hospital-level first; per
   level-of-care fans in the drawer tabs if the chart stays legible.
3. **Calibration, honestly documented**: replay a long deterministic run (EP-10 on),
   score forecasts issued every sim-hour against realized census (coverage of the
   10–90 band, mean absolute error by horizon). Emit a small calibration table into a
   markdown doc (or the EP-18 case-study file) — including where it fails (regime
   changes, surge injections) — and a one-line honesty caption under the fan in-app
   ("model-consistent projection; see calibration notes").
4. **Interaction with interventions**: after an EP-15 experiment, the fan reflects the
   *live* scenario only; no forecast of hypotheticals (scope fence).
5. **Tests**: zero-flow world ⇒ flat fan at current census; arrivals-only crafted world
   ⇒ median rises by the integrated λ; band widens with horizon; determinism.

## Out of scope

- Learning from data, exogenous predictors, per-unit room-level forecasts,
  forecasting under interventions.

> **Completion note (2026-08-13).** Decisions taken, in the order the brief
> posed them:
>
> 1. **Typed event ledger: NOT built.** Calibration scores census against
>    census — the replay harness records realized census per tick and needs
>    no event record, typed or parsed. The display log stays display-only
>    (the EP-16 report scan keeps its documented prefix coupling; the new
>    EdPressure adoption log line was worded to dodge those parsers).
>    Revisit in EP-18 only if the case studies need in-app typed flow
>    records.
> 2. **EP-21's soft input was met and its open thread taken here:**
>    departures condition on elapsed LOS everywhere, and `EdPressure` now
>    **adopts** seeded/loaded ED occupants on its first step (one-shot,
>    reset-rearmed, `BacklogDischarges` mirror) — seeded boarders place,
>    seeded walk-ins release, day-1 boarding lists can legitimately be
>    empty (report_smoke re-pinned accordingly).
> 3. **Model** (`hupsim-sim/src/forecast.rs`, pure/RNG-free): net arrivals =
>    generator λ integral with future-ED treat-and-release churn (thinned
>    Poisson); departures = per-patient conditioned log-normal hazard on a
>    +12.5 h-shifted clock (the discharge curve's mean push), hour-modulated;
>    ED disposition posterior for bay occupants; zero-flow cohorts project
>    zero-flow. Both corrections were *forced by the calibration replay*
>    (+80 → −22 → −1 beds/day bias) and are kept for the record in
>    `hupsim/CALIBRATION.md`.
> 4. **Calibration**: `hupsim-app/tests/forecast_calibration.rs` (8 sim-day
>    seeded replay, hourly 24 h issues, day-1 vs regime segments, and a
>    surge-escape test that *asserts* the documented failure mode). Regime:
>    92 % 10–90 coverage at 12 h (nominal 80 % — Poisson band deliberately
>    conservative vs the generator's sub-Poisson thinning), MAE ≤ 7 beds,
>    |bias| < 2 beds/day. Headline pinned as
>    `forecast::FAN_COVERAGE_10_90_PCT` (test fails on ±3 % drift).
> 5. **Presentation**: fan appended to the drawer's new hospital strip and
>    the status-bar sparkline — clay dashed median + translucent 10–90 wedge
>    past a "now" divider, norm band clipped to history (clay = the kit's
>    "modeled, not observed" hue; it never co-occurs with the compare
>    chart). **Per-LOC row fans skipped** (brief's legibility fence, plus:
>    splitting hospital flows across groups is what the simple model can't
>    honestly claim). Caption + tooltips quote the measured coverage.
> 6. Paint QA per the EP-15 lesson: forced-open drawer at t0 (fan renders
>    with zero history — widget early-return relaxed for that) and at
>    day 2+ under a fast clock; both screenshotted clean.

## Verification / acceptance

- `cargo test --workspace` green; property tests above.
- Run: fan renders ahead of the census line, clearly distinguished from norm band and
  history; calibration table generated from a ≥5-sim-day replay and committed; the
  coverage number is stated wherever the fan is shown (tooltip is fine).
