# Model honesty & future directions

*A consolidated accounting of every load-bearing simplification in hupsim, and
an argument about what to build next — extend the architecture that exists, or
rebuild on a different substrate.*

## What this model is

hupsim is a single-campus, room-level model of the Hospital of the University
of Pennsylvania's West Philadelphia campus: 1,115 individually modeled rooms
across a 42-unit roster, spanning the HUP Main wings, Clifton (the building
known publicly as the Pavilion; this model uses the name Clifton, and unit
display names still print "Pavilion 7 City" and the like), PCAM, and a
122-space emergency department. The model's two halves obey different
epistemic rules, and keeping them separate is the project's central
discipline.

**Structure is sourced-with-confidence.** The roster comes from OSINT — a
wayfinding directory for room ranges, job postings and nursing-placement
lists and award pages for current unit identities — and the 3D geometry from
a hand-surveyed Google Earth KML (ten buildings, three skybridge decks).
Every building, unit, and room carries a provenance tag
(`verified / reported / inferred / unverified`) plus a bed-map era, and the
tags surface in the application, not just in the data files.

**Dynamics are synthetic-but-shaped.** Arrival rates, lognormal
length-of-stay distributions by service line, diurnal arrival shapes, and the
discharge-hour curve are plausible professional-norm values chosen by the
author and held in visible parameter tables (`params.rs`, `RouteParams`).
They are not measurements of the real HUP, and no real patient data exists
anywhere in the project. Structure = sourced-with-confidence; dynamics =
synthetic. That sentence is the license under which every claim in this
series is issued.

**The engine is deterministic.** A seeded ChaCha8 stream drives every random
draw; reset replays a run exactly, and A/A comparisons are bit-identical.
That guarantee is what makes the series' analytic style possible: every
figure and headline number in studies [01](01-ed-to-bed-transport.md),
[02](02-skybridge-pcam-chps-access.md), [03](03-surge-experiments.md), and
[04](04-forecast-calibration.md) carries a seed, a reproduction command, and
a commit hash. Each study states its local caveats in its own text; this
document owns the model-wide ones.

**Reproduction.** This document introduces no new runs. Every number quoted
here reproduces through its home study's Reproduction block — studies 01–04,
at the EP-18 commit recorded in
[`roadmap/README.md`](../../../roadmap/README.md).

## The simplification ledger

Organized by layer. For each entry: the simplification, why it was chosen
(usually some combination of demonstration scope, determinism, and the
absence of real data), and what it costs — with the direction of bias where
that direction is knowable.

### 1. Structure & data

The structural layer's simplifications are mostly *documented uncertainty*
rather than smoothing, and that is deliberate.

- **Known discrepancies are kept visibly unresolved.** The Founders 8 MICU is
  modeled at its 12-bed wayfinding-directory range although ~24 beds are
  reported elsewhere; the HVICU is modeled on two candidate floors because
  the sources genuinely disagree; one Clifton wing carries an unresolved
  oncology-versus-cardiology service conflict. In each case the model records
  the conflict in the data with both sources tagged, rather than picking a
  winner silently. Cost: unit-level capacity numbers for those units are
  wrong in one direction or the other (the MICU, at 12 modeled beds against
  ~24 reported, likely *understates* real ICU capacity and therefore
  overstates ICU strain in surge runs). The point stands regardless: the
  model is honest about where it is guessing, and the discipline — not the
  guess — is the demonstrated skill.
- **Room plans are synthetic.** Interior layouts are generated
  double-loaded-corridor plans; only footprints, floor counts, and room
  *counts* are sourced. Costs nothing for capacity analytics; it means
  within-unit walking distances are decorative.
- **Clifton floor 13 is modeled as absent.** The licensed count of 504
  factors as 7 × 72 exactly and no source names a floor-13 unit, so the model
  omits it rather than inventing one. If a floor 13 exists with beds, the
  model undercounts Clifton capacity.
- **Bridge deck levels and route constants are inferred.** The three
  skybridge deck lengths are surveyed KML geometry; which floor each deck
  lands on, and all walk-speed and elevator constants, are inferred and held
  in one visible table (`core::route::RouteParams`). The unverified HUP
  Main–Clifton tunnel is excluded by default. Consequence, stated in study
  01's own terms: route *minutes* are inferred-parameter estimates, and the
  robust content of the transport findings is topological — one skybridge,
  one funnel, a worst-case Rhoads traverse at roughly 3–4× the cost of a
  Clifton floor (15.7 min vs 4.3 min under the stated constants) — not the
  decimal minutes themselves.

### 2. Flow statistics

The dynamic layer trades realism for legibility and determinism, and every
trade is visible in the parameter tables.

- **Arrivals are independent thinned-Poisson-style draws from fixed diurnal
  tables** — hour-of-day shape × weekend factor, nothing else. No
  autocorrelation between days, no seasonality, no epidemic dynamics, no
  scheduled-OR calendar, no holiday effects. Real hospital demand is
  overdispersed and correlated; hupsim's is, if anything, *under*-dispersed
  (the generator's per-sub-step thinning realizes roughly a quarter of
  Poisson variance at 15-minute ticks, a fact study 04 measures and
  discloses). Bias direction: the model understates census volatility, so
  any claim about how often the house hits an occupancy threshold is
  optimistic.
- **LOS is drawn once per patient from a service-line lognormal,**
  independent of everything: no case-mix within a service, no acuity–LOS
  coupling, no readmission process, and no mortality — every exit is a
  discharge. The instability score exists but does not feed back into LOS.
- **ED admission conversion is a flat 30%.** Real conversion varies by hour,
  acuity mix, and season; a flat rate flattens the boarding signal's
  structure.
- **Automated bed placement is mechanical.** Direct admissions choose
  uniformly among vacant beds of the needed type; ED boarders take the first
  fitting bed in room order. No clinician preference, no service-cohorting
  pressure, no "keep the pod together" behavior — the interactive placement
  engine scores those trade-offs, but the sim's own flows do not consult it.
  Uniform direct admits smooth occupancy across sister units more than a
  real house does; first-fit boarders clump instead.
- **The discharge-hour curve delays but never advances exits.** A tentative
  exit instant is pushed to a weighted hour of the same or next day — a mean
  push of about +12.5 h, measured in study 04's calibration — and no
  mechanism ever pulls a discharge earlier. This matters below.
- **The instability score is abstract** — a 0–1 hook shaped like a
  NEWS2-class early-warning score, not a validated one. It colors rooms and
  drives nothing clinical.

### 3. Absent feedback loops — the non-linear hospital

This is the crux of the ledger, and the simplification that most separates
hupsim from the institution it depicts. A real hospital is a control system.
Saturation triggers ambulance diversion and EMS re-routing; visible boarding
depresses walk-in demand; a capacity crunch *accelerates discharges* — the
strongest homeostatic response a real house has, and every hospitalist has
felt the morning bed meeting produce it; workload degrades both LOS and
safety; transport and elevator congestion couple flows that look independent
on paper; and services behave strategically around the bed meeting itself.
Every one of those arrows points from *state* back into *rates*.

In hupsim, no such arrow exists. Demand and LOS are exogenous: the λ tables
and the lognormals never read the census. The only nonlinearity in the whole
system is the hard bed constraint — a patient cannot be placed in a bed that
does not exist. In queueing terms, hupsim is closer to a
deterministic-replay multi-server bed queue with state-blind arrivals —
admitted ED demand boards until a bed frees, house-full direct admits are
simply logged as turn-aways, and neither outcome ever feeds back into the
arrival rates — than to a behavioral hospital.

The consequences, worked through honestly:

- **Closures overstate boarding.** Study 03's ICU-closure experiment removes
  beds while demand and discharge behavior continue unchanged, so boarding
  accumulates with no compensating discharge acceleration. The measured
  boarding deltas are therefore upper-bound-flavored — real houses absorb
  more of a closure than this model lets them.
- **Waves understate chaos.** An injected arrival wave flows through
  unchanged triage, placement, and discharge machinery. The real disruption
  of a mass-casualty afternoon — triage re-prioritization, hallway medicine,
  degraded throughput everywhere — has no representation, so the wave's
  measured effect is smoother and smaller than reality's.
- **The forecast's failure under regime change is the same exogeneity seen
  from the other side.** Study 04 shows the census fan escaping within
  ~4.2 h of a 40-admission surge injection: the forecast integrates the same
  fixed tables the generator draws from, so anything that moves the rates —
  a surge, a closure, a changed admit fraction — is invisible to it by
  construction. The generator cannot respond to state and the forecaster
  cannot see rate changes; these are one design decision wearing two hats.

Why accept all this? Because every feedback loop added is a parameter
surface invented — with no data to estimate diversion thresholds or
pressure–discharge elasticities against, a "behavioral" hupsim would be a
model of the author's intuitions dressed as mechanism. Exogenous dynamics
keep the synthetic layer small, legible, and honestly labeled. But the cost
is real and directional, and studies 03 and 04 both inherit it.

### 4. Evaluation

- **The calibration loop is closed.** Study 04's central caveat, restated at
  model level: the forecast integrates the sim's own λ tables, LOS
  lognormals, and discharge curve, so its measured near-unbiasedness (±2
  beds/day on a ~900-patient house; 92% coverage of the 10–90 band at 12 h
  against a nominal 80%) is a property of the loop, not evidence the method
  would calibrate against a real hospital. What the exercise demonstrates is
  the *practice* — coverage tables, failure modes asserted by tests, a
  pinned headline that drifts loudly — not predictive validity.
- **The A/B experiments are single realizations.** Determinism makes the
  A-versus-B delta exactly attributable to the intervention, but each arm is
  one draw from the process, not an expectation; a different seed gives a
  different delta. Study 03 says this in its own text; it bears repeating
  here because single-realization deltas are the series' weakest numbers.
- **UI paint layers have no automated coverage.** The engine and data layers
  are tested; the hand-painted egui charts are exercised only by eyeball
  passes. One such pass caught a grid-layout panic in the dashboard KPI
  chips (documented in the roadmap, fixed at `44f845c`) — a crash class the
  test suite would never have seen. The gap is known and open.
- **No claim in this series has touched real operational data** — by design
  (the demonstration must be shareable) and by necessity (no such data is
  available to it). Every "finding" is therefore a statement about the
  model, offered as evidence of method.

## Two roads forward

### Road A — extend

The current architecture absorbs new mechanisms cheaply, and the reason is
structural. The `SimProcess` trait is the plug-in point: every realism
feature to date — diurnal flow, ED pressure, staffing, transport, the
experiment interventions — arrived as a process implementing
`step(&mut SimCtx)` and registering on the engine, without touching the
engine itself. Determinism is preserved by construction (all draws come
through the seeded stream). The columnar room matrix and rolling-stats layer
already serve whole-house queries at interactive rates, and the EP-15 A/B
runner is a ready-made evaluation harness for any new mechanism: add a
feedback loop, replay the same closure experiment, and diff the arms to see
exactly what the mechanism changed.

The candidates below are sketches, not commitments — sized S/M/L, with
dependencies noted.

| candidate | mechanism | size | depends on |
|---|---|---|---|
| EP-24 Demand feedback | arrival thinning under saturation; turn-away accounting | M | EP-15 harness |
| EP-25 Overdispersed arrivals | day-level random effects / self-exciting stream | M | EP-17 recalibration |
| EP-26 Acuity-coupled LOS | instability modulates discharge hazard, drives ICU transfers | M/L | acuity layer |
| EP-27 Discharge acceleration | occupancy-conditional discharge hazard | S/M | — |
| EP-28 Monte-Carlo ensembles | N-seed A/B with CIs on deltas | S | EP-15 runner |
| EP-29 Transport-aware placement | EP-6 route minutes as a placement-score term | S | EP-6, EP-14 |
| EP-30 Real-data adapter | ADT-shaped import re-estimating the tables | L | changes the model's nature |

**EP-24, demand feedback.** ED arrival thinning as a function of boarding
and saturation — diversion, in effect — with explicit turn-away accounting
near 100% occupancy. This makes study 03's closure experiments honest at the
top of the occupancy range, where the current model's only responses are
silent turn-away log lines and an ever-growing boarding pool.

**EP-25, overdispersed and correlated arrivals.** A day-level random effect
or a Hawkes-style self-exciting component behind the existing diurnal shape,
followed by recalibrating study 04's bands against the noisier stream. This
directly stresses the disclosed sub-Poisson conservatism: the fan's 92%
coverage should *fall* toward nominal, and watching it do so would itself be
a good study.

**EP-26, acuity-coupled LOS and deterioration pathways.** Instability
trajectories that modulate discharge hazards and generate ICU-transfer
demand — coupling the acuity layer to flow for the first time, and turning
the color spectrum from a display into a mechanism.

**EP-27, discharge acceleration under pressure.** The homeostatic lever: an
occupancy-conditional multiplier on the discharge hazard, with an auditable
"pressure-discharges" counter so the mechanism's activity is visible in
every run. This is the single highest-value realism gain per line of code in
the list, and it directly perturbs study 03's boarding deltas — the numbers
most biased by its absence.

**EP-28, Monte-Carlo experiment ensembles.** N-seed A/B with distribution
summaries and confidence intervals on the deltas. Retires the
single-realization caveat outright, and the work is embarrassingly parallel.

**EP-29, transport-aware placement.** The EP-6 route minutes as a term in
the placement score — already flagged as future work in the code.

**EP-30, the real-data adapter.** An ADT-event-shaped import (CSV,
FHIR-adjacent) that re-estimates the parameter tables from data and tags
every estimated parameter with its provenance. This is the bridge out of the
closed loop — and the moment it is crossed, the model stops being what it
is. Which is the argument for Road B.

### Road B — rebuild

There is a point at which extending is the dishonest choice, and it arrives
with the data. The moment real operational data becomes the substrate, the
hard problems invert. Parameter *authorship* — the current model's whole
dynamic layer — becomes parameter *estimation*, with identification problems
attached. Validation inverts too: pinned self-consistency tests (the closed
loop scoring itself) give way to validation against ground truth — face
validity with clinicians, trace validity on individual patient paths, and
statistical validation of the Sargent-school kind against held-out census.
Privacy, governance, and data engineering stop being out of scope and become
first-class architecture. And the engine itself is wrong for the job: a
fixed-tick loop coupled to Bevy's frame clock is the right shape for an
interactive demonstration, but an estimation-and-forecast tool wants a
discrete-event core on calendar time, with the UI as a client of a
simulation service rather than its owner.

What survives a rebuild — because it was right, not because it is sunk cost:

- the **provenance discipline** (every structural fact tagged with source and
  confidence, discrepancies recorded rather than smoothed);
- the **determinism-and-replay ethic** (seeded streams, exact reset, A/A
  bit-identity as a tested invariant);
- the **honesty-ledger practice** (measured calibration before claims,
  pinned headlines that fail tests when they drift, failure modes asserted
  so documentation cannot rot);
- the **A/B attribution design** (same world, one change, exact diff);
- the **separation of structure from dynamics** (slow, sourced facts in one
  layer; fast, estimated behavior in another).

**The verdict, plainly:** extend while the purpose is demonstration and
method — Road A's candidates each sharpen an honest weakness this ledger
names, and the architecture takes them without strain. Rebuild the day the
question becomes "what will *our* census be on Thursday," because that
question deserves estimation, external validation, and governance, not a
larger synthetic model. The two roads share their most important artifact:
the habits.

## What the series demonstrates

The series is a demonstration of method, not of features. Every quantitative
claim in it is reproducible from a stated seed, a fenced command, and a
recorded commit — reproducibility as a norm, not a virtue reserved for
papers. The calibration came before the claims, and its failure modes are
asserted by tests, so the documentation cannot quietly outlive its own
truth: when the surge stops escaping the band, a test fails and the prose
must be re-earned. Every structural fact carries a provenance tag, and the
places where sources conflict are displayed as conflicts rather than
resolved by fiat. The experiments were designed for attribution — one world,
one change, an exact diff — before they were designed for effect size. These
are the working habits of a continuous-improvement analytics practice:
measure before claiming, pin what you publish, tag what you know and how you
know it, and build the comparison into the design rather than the
discussion. The model is synthetic; the habits are not.

Part of the [hupsim case-study series](README.md)
