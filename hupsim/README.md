# hupsim

**A room-level model of the Hospital of the University of Pennsylvania, as a
Rust desktop application.**

> Independent portfolio project — not affiliated with or endorsed by HUP, Penn
> Medicine, or the University of Pennsylvania. Structure is compiled from cited
> public sources with per-entry confidence tags; every patient, census, LOS,
> staffing, transport, and forecast number is simulated. The analyses are
> methodological demonstrations, not operational assessments of the real
> hospital. Full statement in the [repository README](../README.md).

3D campus massing → click a floor → 2D unit plan → click a room → edit the
patient. Occupancy renders as a green → blue → red stability spectrum. Built
as a demonstration of hospitalist-informaticist systems thinking: capacity,
acuity, boarding, and data provenance in one model.

```
cargo run --release
```

(first build takes a few minutes — it's Bevy; afterwards it's seconds)

## What this demonstrates

The written companion to the app: six analyses in
[`docs/analyses/`](docs/analyses/README.md), each a mini-paper whose every
number reproduces from a stated seed + command at the commit recorded in the
roadmap.

- [ED-to-bed transport, HUP Main vs Clifton](docs/analyses/01-ed-to-bed-transport.md)
  — nearest Clifton bed 4.3 min vs worst HUP Main 15.7, and every HUP Main
  route over the single Silverstein–Clifton skybridge.
- [Skybridge constraints on PCAM & CHPS access](docs/analyses/02-skybridge-pcam-chps-access.md)
  — PCAM and CHPS are deliberately distinct destinations, and the unverified
  tunnel would change routes by at most ~0.4 min.
- [Surge experiments](docs/analyses/03-surge-experiments.md) — closing the
  largest ICU strands 11 patients (only 4 ICU beds existed to evacuate
  into); a +40 arrival wave saturates med-surg for 39 h without mass
  boarding; a staffing flex relieves workload and deliberately cannot touch
  bed physics.
- [Forecast calibration](docs/analyses/04-forecast-calibration.md) — the fan
  covered 92% of 12 h forecasts in replay, and a 40-admission surge escapes
  its band in ~4 h.
- [Model honesty & future directions](docs/analyses/05-model-honesty-and-future-directions.md)
  — the consolidated simplification ledger and the extend-vs-rebuild road
  map. Read this one to know what the model deliberately does not claim.

## What you're looking at

- **3D campus view** — every modeled HUP Main wing (including the bedless
  Gates, Maloney, and White — because topological honesty matters when you
  walk consults), Clifton (the building formerly known as the Pavilion), and
  PCAM. Footprints and the three skybridge decks are hand-surveyed in Google
  Earth Pro (KML → layout importer, EP-2); the wing towers rise from a
  one-story connective podium derived from the union of their own footprints
  (+3 m buffer, EP-7 `podium_gen`). The podium is the campus's shared
  street-level **Ground story** — every building models one (the wayfinding
  directory prints a G span for only four wings, but both public entrances
  sit on "Ground" and the Connector is the first floor). Slabs stack at
  their labeled story over a single shared wing story height (3.8 m), so
  floor N sits at the same elevation in every wing and floors 1 and up stay
  visible above the podium rim. (The contested Rhoads "level 10" used to
  float at story 10 as a rendered anomaly; EP-23 ruled it a
  wayfinding-directory artifact and retired it to the archive.) Floor slabs
  tint by the census of the units on them.
- **Unit view** — click a floor slab (or a unit in the navigator) for a
  synthetic double-loaded-corridor plan of its rooms. The ED renders as a
  grid of its 122 treatment spaces. Orange dot = admitted patient boarding in
  a non-licensed space.
- **Inspector** — everything is editable at the room level: occupancy,
  patient alias/age/service, instability (0–1 slider), deterioration trend,
  telemetry, isolation, boarding; block/unblock rooms; add/delete rooms;
  admit, discharge, transfer.
- **Provenance badges** — every building, unit, and room carries
  `verified / reported / inferred / unverified` plus a bed-map era tag.
  Pre-2021 Guide-to-Units assignments are visibly flagged as stale, because
  at the real HUP they are.

## The color spectrum

Instability 0 → 1 maps green → blue → red (piecewise Oklab, so the midpoints
stay clean). Tiers: Stable < 0.25 ≤ Watcher < 0.5 ≤ Unstable < 0.8 ≤
Critical. Vacant rooms are gray, out-of-service darker. Aggregation to
unit/floor/building is census-weighted mean or worst-case (toolbar toggle) —
distinguishing "full of stable patients" from "half-full of crashing ones."

## Simulation scaffolding

The time-series machinery underneath is deliberately small and visible:

- `SimClock` (sim-minutes, speed multiplier) driven from Bevy's fixed
  timestep
- a deterministic event queue (`Admission / Discharge / Transfer /
  AcuityChange / Custom`)
- the **`SimProcess` trait** — future ADT generators, LOS models, and
  deterioration models implement `step(&mut SimCtx)` and register on the
  engine; seeded ChaCha8 RNG makes every run reproducible
- a bounded **`Timeline`** sampling per-unit census/acuity every 15 sim-min —
  the substrate the dashboards, rolling z-score norms, and forecast fan all
  read from

The realism processes (`diurnal`, `ED`, `drift`) default on — the clock just
starts paused; the original flat demo flow stays registered off as a control
signal (toolbar: `flow`).

## Census forecast fan (EP-17)

The dashboard drawer and status bar append a short-horizon (12 h) census
projection to the census sparkline: a dashed clay median with a shaded 10–90
band past a "now" divider. It is a transparent analytic baseline built from
the sim's own λ/LOS/discharge tables applied to the standing census — no
learning, no Monte Carlo, and deliberately no peeking at the event queue.
Its measured honesty (coverage, error by horizon, the failure modes, and the
two model corrections the calibration replay forced) lives in
[`CALIBRATION.md`](CALIBRATION.md); the headline coverage number is quoted
in the chart tooltips and pinned by the calibration test.

## Patient transport & the route graph (EP-6)

Intra-campus moves are no longer teleports. A route graph derived from the
topology's connections + elevator cores + the surveyed layout — nodes are
(building, level) pairs; edges are the three KML skybridges (real deck
lengths), the Dietz & Watson bridge and Connector corridors (inferred
centroid distances), wing floor-continuity/adjacency links, the shared
ground podium, and elevator hops (wait + per-story ride) — feeds a
`BedTransport` sim process: ED bed placements now depart, exist off-bed in
transit, and arrive minutes later, with the route in the log:

```
transport depart: Rivera.184 — Clifton 1 → Founders 8 via Silverstein–Clifton skybridge, 10 min
```

Confidence policy: verified/reported edges traversable, inferred traversable
but tagged, and the **unverified** HUP Main↔Clifton tunnel (plus the
contested Rhoads L10 link) excluded by default. All walk/elevator constants
are inferred and live in one visible table (`core::route::RouteParams`);
weights from surveyed geometry are marked as such.

**ED-to-bed transport times, Clifton vs HUP Main** (from
`cargo test -p hupsim-data ed_to_bed -- --nocapture`; ED = Clifton 1):

| destination | minutes | route |
|---|---|---|
| Pavilion 7 (nearest Clifton beds, incl. the HVICU) | 4.3 | elevator only |
| Pavilion 14 (farthest Clifton beds) | 6.1 | elevator only |
| Silverstein 7 — Women's Health (nearest HUP Main beds) | 7.7 | Silverstein–Clifton skybridge |
| Founders 8 — MICU | 10.4 | Silverstein–Clifton skybridge |
| Rhoads 5 — SICU | 15.1 | skybridge → podium → Rhoads core |
| Rhoads 7 — Urology/Colorectal (worst case) | 15.7 | skybridge → podium → Rhoads core |

The structural finding: with the tunnel excluded as unverified, **every**
ED-to-HUP-Main transport funnels over the single Silverstein↔Clifton
skybridge — the worst-case (Rhoads) pays a full down-across-up traverse at
~3–4× the cost of a Clifton floor. PCAM access rides the same funnel plus
the Clifton↔PCAM decks; note CHPS inpatient beds are deliberately modeled at
Ravdin 6 NE (8.7 min), a distinct destination from PCAM itself. Numbers are
inferred-parameter estimates, not measurements — the topology dependence
(one bridge, one funnel) is the robust part. Both threads are developed as
full case studies:
[transport](docs/analyses/01-ed-to-bed-transport.md) and
[PCAM/CHPS access](docs/analyses/02-skybridge-pcam-chps-access.md).

## Data

Four human-editable JSON files in `assets/data/` (embedded into the binary
as fallback):

| file | contents | provenance |
|---|---|---|
| `hup_topology.json` | single-campus property graph: wings, floors, elevator cores, bridge/tunnel/connector edges | compiled OSINT, confidence-tagged |
| `units.json` | 42-unit roster with room-generation rules (incl. partial-wing `slots`), room-capability rules (telemetry / negative pressure), target RN ratios | wayfinding directory (room ranges), job postings + NURS 2450 placements + award pages + fellowship pages (current unit identities), each entry sourced; capability + staffing rules are inferred norms, per-entry noted |
| `service_lines.json` | ordered service-line taxonomy + total unit→line mapping (EP-8) | drafted by Claude from each unit's `service` string; EP-23 (2026-08-13) ruled all 11 formerly-flagged no-source units after a targeted OSINT session — zero `needs_owner_ruling` flags outstanding |
| `layout.json` | 3D massing: footprints, levels, skybridge decks, ground plates | Google Earth Pro KML hand-survey (10 buildings + 3 skybridges); podium plate derived from the wing union (`podium_gen`, see `podium_report.md`) |

Out-of-scope sites (Cedar Avenue, Radnor, ISC, unmapped wings) are archived
with their provenance in `assets/data/archive/out_of_scope.json`, and the
six units retired by the EP-23 rulings (Founders 5 + 9, Rhoads 2 + 10,
Pavilion 8 Campus + 14 City) in
`assets/data/archive/ep23_roster_rulings.json` — archived, not deleted.

1,115 rooms compile out of these: 488 HUP Main wayfinding slots, 456
Clifton rooms (floor 13 modeled as absent since the 504 licensed = 7×72
exactly and no source names a floor-13 unit; EP-23 retired the 8 Campus and
14 City wings), 122 ED spaces, L&D, the ICN, and the 5 CHPS research beds
at Ravdin 6 NE.
Known discrepancies (e.g. the Founders 8 MICU's 12-bed directory range vs
the ~24 beds reported elsewhere) are recorded in the data rather than
smoothed over — see the *Data limitations* panel in-app.

## Crates

```
hupsim-core   domain model, colormap, aggregation, census ops, scenarios
hupsim-sim    clock, event queue, SimProcess trait, timeline   ← extend here
hupsim-data   JSON → Hospital compiler, room generation, demo census
hupsim-app    Bevy 0.19 + bevy_egui 0.41 desktop app
```

Dependency direction is strictly `app → {data, sim} → core`; nothing below
the app knows Bevy exists. See `DESIGN.md` for the full architecture.

## Scenarios

**Save…/Load…** round-trips the entire world state (structure + census + sim
clock + RNG seed) as one schema-versioned JSON document — reproducible,
diffable, shareable.

## Controls

| input | action |
|---|---|
| left/right-drag | orbit |
| middle-drag | pan |
| wheel | zoom |
| click floor slab | inspect floor → open unit |
| click room (unit view) | inspect/edit patient |
| 🔎 navigator | search rooms, units, patients |
