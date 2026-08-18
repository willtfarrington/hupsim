# Census forecast calibration (EP-17)

The census fan shown in the dashboard drawer and the status bar is a
**model-consistent projection**: expected arrivals integrate the sim's own
diurnal λ(t) tables, and expected departures apply the sim's own LOS
log-normals and discharge-hour curve to the standing census, conditioned on
each patient's elapsed stay (the EP-21 backdated stamps). It is a transparent
statistical baseline — no learning, no exogenous predictors, and deliberately
**no reading of the event queue**, which literally contains every scheduled
discharge. The model sees only what a bed-huddle observer could see.

This file is the honesty ledger the in-app caption points at. Regenerate with:

```
cargo test -p hupsim-app --test forecast_calibration -- --nocapture
```

The test also *pins* the headline number: if the measured regime 12 h coverage
drifts more than ±3 % from `forecast::FAN_COVERAGE_10_90_PCT`, the suite
fails until this file and the constant are re-issued together.

## Model (one paragraph)

census(t+h) = census(t) + net arrivals − expected departures. Arrivals:
direct-admission and ED λ(t) (hour-of-day shape × weekend factor) integrated
at the generator's own 15-min sub-step; future ED arrivals churn back out on
the treat-and-release clock (thinned Poisson), while future direct admits
stay (F(24 h | fresh) < 1 % on day-scale LOS). Departures: per standing
patient, the service line's log-normal conditioned above the elapsed stay,
on a clock shifted +12.5 h (the discharge-hour curve's mean push) and
hazard-modulated by the within-day discharge weights; ED occupants leave via
a disposition posterior (Bayes over the two episode log-normals given "still
in the bay at elapsed e"); boarders and no-flow units (psych, obs, L&D,
nursery) project zero-flow, exactly like the generator. Uncertainty:
Poisson arrivals + Poisson-binomial departures → normal 10/50/90 quantiles,
clamped at staffed beds.

## Measured calibration — seeded start, seed 42, 8 sim-days, hourly 24 h forecasts

Replay: the real compiled hospital (1,115 rooms), seeded demo census,
`BacklogDischarges` + `AcuityDrift` + `DiurnalFlow` + `EdPressure` at 15-min
ticks (the timeline cadence; transport untimed, as in headless experiments).
191 hourly forecast issues scored against realized census. Nominal band
coverage is **80 %** (10–90).

| segment | horizon | n | 10–90 coverage | MAE (beds) | bias (beds) |
|---|---|---|---|---|---|
| day 1 (settle-in) |  2 h | 23 | 91% | 3.8 | −1.4 |
| day 1 (settle-in) |  6 h | 23 | 78% | 8.0 | −3.1 |
| day 1 (settle-in) | 12 h | 23 | 87% | 8.5 | −4.1 |
| day 1 (settle-in) | 24 h | 23 | 100% | 7.8 | −2.4 |
| days 2–7 (regime) |  2 h | 145 | 92% | 3.2 | −0.5 |
| days 2–7 (regime) |  6 h | 145 | 90% | 5.2 | −0.9 |
| days 2–7 (regime) | 12 h | 145 | **92%** | 6.3 | −1.1 |
| days 2–7 (regime) | 24 h | 145 | 100% | 7.2 | −1.6 |

**Headline (quoted in-app): the 10–90 band covered 92 % of 12 h-ahead
forecasts over the settled regime.** MAE stays under ~7 beds out to 24 h on a
~900-patient house; residual bias is under 2 beds/day.

### Why coverage sits *above* the nominal 80 %

The band is deliberately conservative: arrival variance is taken as Poisson
(variance = integrated λ), but the generator thins arrivals per sub-step
(`floor + Bernoulli`), which is **sub-Poisson** at coarse tick sizes — the
replay's 15-min ticks realize roughly a quarter of the Poisson variance. A
narrower, better-scored band would be *less* robust across clock speeds (at
fine ticks the generator approaches true Poisson), so the conservative choice
stands and is disclosed here. Coverage of 100 % at 24 h is this same effect
compounding.

## Where it fails (measured, kept true by test)

- **Surge injections.** A 40-admission surge injected at day-3 12:00
  (invisible to the λ tables — a bus crash, a regional diversion) escaped the
  band of the fan issued at 08:00 within ~4.2 h: realized 929 beds crossed
  the q90 of 915. The `surge_injection_escapes_the_band` test *asserts* the
  miss so this paragraph cannot silently go stale. The fan is a baseline,
  not an early-warning system.
- **Regime changes.** Anything that moves the rates themselves — an EP-15
  unit closure, a ratio flex, a changed admit fraction — is outside the
  tables the model integrates. By the same mechanism as the surge, the fan
  will lag it. (Scope fence: after an EP-15 experiment the fan reflects the
  *live* scenario only; hypotheticals are never forecast.)
- **Day-1 settle-in.** The seeded snapshot's first hours mix backlog exits
  and ED adoption transients; 6 h coverage dips to 78 % with a −3 bed bias.
  Honest but visibly rougher than the regime.
- **Near saturation.** The model caps quantiles at staffed beds but does not
  queue: turn-aways and ED diversion near 100 % occupancy are not modeled.
  At the ~85 % regime this never binds; under surge scenarios it does.

## Model corrections the calibration forced (kept for the record)

The first draft had two failures this replay caught, both worth
remembering:

1. **ED churn of future arrivals** (+80 beds/day over-forecast): integrating
   ED arrivals into census without letting the ~70 % treat-and-release share
   leave again within the horizon. Fixed with thinned-Poisson net arrivals.
2. **Raw vs shaped LOS clock** (−22 beds/day under-forecast after fix 1):
   the discharge-hour curve delays every exit by ≈ 12.5 h on average
   (tentative instant ~uniform in the day → pushed to a weighted hour of the
   same/next day, never earlier), so hazards computed on the raw tables run
   ~10 % hot. Fixed by shifting the LOS clock.

A model this simple being ±2 beds/day unbiased is a property of the *closed
loop* (same tables in and out), not evidence it would calibrate against a
real hospital — that is the point the demo makes.

## Standing decisions (EP-17)

- **No per-group fans.** The drawer rows keep history-only sparklines: the
  22 px rows already carry three encodings, and splitting hospital flows
  across groups (vacancy-share placement, ED→ward transfer coupling) is
  exactly what this model cannot honestly claim.
- **No typed event ledger.** Calibration scores census against census — no
  event parsing needed — so the EP-16 note's proposed typed ring buffer was
  not built. The display log stays display-only; EP-18 should revisit only
  if its case studies need in-app typed flow records.
- **ED standing-census adoption** (closes the EP-21 open thread): seeded and
  scenario-loaded ED occupants are adopted by `EdPressure` on the first tick
  (residual episode clocks, seeded boarders straight onto the bed hunt), so
  the world the fan projects actually flows the way the tables say it does.
