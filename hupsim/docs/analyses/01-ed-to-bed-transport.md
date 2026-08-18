# ED-to-bed transport: how far is every inpatient bed from the ED?

*Every admission from the ED pays a transport cost, and in this model that cost is
set almost entirely by which side of one skybridge the bed is on.*

## Question

When the ED admits a patient, how long does the trip to the bed take — and does
the choice of unit change that time enough to matter clinically? A capacity
manager placing an unstable boarder wants to know whether "a bed is a bed" or
whether bed geography carries a built-in time penalty that should sit alongside
telemetry availability and nurse staffing in the placement decision.

## Data & methods

The model's ED sits on floor 1 of the Clifton tower (node `bldg.pavilion 1`; the
building known publicly as "The Pavilion" at 3400 Civic Center is called
**Clifton** in this model, though unit display names still print "Pavilion 7
City" and so on). This study computes the shortest-path transport time from the
ED to each of the 38 inpatient destinations in the roster — med-surg, stepdown,
and ICU floors across Clifton and the HUP Main complex (Silverstein, Founders,
Ravdin, Dulles, Rhoads).

Times come from the EP-6 route graph. Its nodes are (building, level) pairs. Its
edges are: the three skybridge decks hand-surveyed from a Google Earth KML (real
deck lengths); the Connector and Dietz & Watson corridors from the public
wayfinding directory, with lengths estimated as area-centroid distance times a
1.4 circuity factor and tagged inferred; derived ground-podium edges, because
the shared street-level Ground story is the campus's circulation layer; and
elevator hops costed at 2.5 minutes of wait plus 0.3 minutes per story. All
walking and elevator constants are inferred values collected in one visible
`RouteParams` table — they are plausible professional-norm numbers, not
measurements of the real HUP. Paths are plain Dijkstra with deterministic
tie-breaks. The unverified HUP Main–Clifton tunnel is excluded by default under
the project's confidence policy, so every ED-to-Main route must surface and
cross a skybridge.

No simulation is involved: the route graph is deterministic, with no RNG and no
seed. Reset replays exactly by construction.

**Provenance.** The building roster, floor stacking, and the three skybridge
deck geometries are real-world-sourced (wayfinding directory, hand-surveyed
KML) with per-entry confidence tags; some deck levels are inferred rather than
verified. All dynamic quantities — walk speed, elevator wait, per-story time,
corridor circuity — are synthetic-but-shaped parameters. No real patient data
exists anywhere in this project, and no transport time below is a measurement
of the real hospital.

**Reproduction.**

```
cargo test -p hupsim-data ed_to_bed -- --nocapture
```

Deterministic; no seed applies. Run at the EP-18 commit recorded in
[`roadmap/README.md`](../../../roadmap/README.md).

## Results

All 38 destinations, sorted ascending, exactly as the test prints them:

| Minutes | Destination | Node | Route |
|--------:|-------------|------|-------|
| 4.3 | Pavilion 7 City — Vascular Surgical PCU | `bldg.pavilion 7` | direct |
| 4.3 | Pavilion 7 Center — Heart & Vascular ICU (wing inferred) | `bldg.pavilion 7` | direct |
| 4.3 | Pavilion 7 Campus — Heart & Vascular / Thoracic Ward | `bldg.pavilion 7` | direct |
| 4.6 | Pavilion 8 City — Cardiac | `bldg.pavilion 8` | direct |
| 4.6 | Pavilion 8 Center — Cardiac | `bldg.pavilion 8` | direct |
| 4.9 | Pavilion 9 Campus — CV Surgical PCU | `bldg.pavilion 9` | direct |
| 4.9 | Pavilion 9 Center — CV Surgical PCU | `bldg.pavilion 9` | direct |
| 4.9 | Pavilion 9 City — HVICU | `bldg.pavilion 9` | direct |
| 5.2 | Pavilion 10 Campus — Neurosurgery | `bldg.pavilion 10` | direct |
| 5.2 | Pavilion 10 Center — Neurology | `bldg.pavilion 10` | direct |
| 5.2 | Pavilion 10 City — Neuro ICU | `bldg.pavilion 10` | direct |
| 5.5 | Pavilion 11 City — Transplant Surgery | `bldg.pavilion 11` | direct |
| 5.5 | Pavilion 11 Center — Transplant ICU | `bldg.pavilion 11` | direct |
| 5.5 | Pavilion 11 Campus — Oncology (Cellular Therapy) | `bldg.pavilion 11` | direct |
| 5.8 | Pavilion 12 Campus — Solid Oncology | `bldg.pavilion 12` | direct |
| 5.8 | Pavilion 12 Center — Oncology MICU (12 beds; wing inferred) | `bldg.pavilion 12` | direct |
| 5.8 | Pavilion 12 City — Solid Oncology | `bldg.pavilion 12` | direct |
| 6.1 | Pavilion 14 Campus — Liquid Oncology | `bldg.pavilion 14` | direct |
| 6.1 | Pavilion 14 Center — Liquid Oncology | `bldg.pavilion 14` | direct |
| 7.7 | Silverstein 7 — Women's Health (Antepartum/Postpartum) | `wing.silverstein 7` | via Silverstein–Clifton skybridge |
| 8.0 | Silverstein 8 — Postpartum / Mother-Baby (inferred) | `wing.silverstein 8` | via Silverstein–Clifton skybridge |
| 8.3 | Silverstein 9 — Pulmonology | `wing.silverstein 9` | via Silverstein–Clifton skybridge |
| 8.6 | Silverstein 10 | `wing.silverstein 10` | via Silverstein–Clifton skybridge |
| 8.7 | Ravdin 6 — Gyn / Gyn-Oncology | `wing.ravdin 6` | via Silverstein–Clifton skybridge |
| 8.9 | Silverstein 11 | `wing.silverstein 11` | via Silverstein–Clifton skybridge |
| 9.2 | Silverstein 12 — General Medicine | `wing.silverstein 12` | via Silverstein–Clifton skybridge |
| 9.6 | Ravdin 9 | `wing.ravdin 9` | via Silverstein–Clifton skybridge |
| 9.9 | Dulles 6 | `wing.dulles 6` | via Silverstein–Clifton skybridge |
| 10.4 | Founders 8 — MICU | `wing.founders 8` | via Silverstein–Clifton skybridge |
| 11.0 | Founders 10 — Advanced Medicine | `wing.founders 10` | via Silverstein–Clifton skybridge |
| 11.3 | Founders 11 — Advanced Medicine | `wing.founders 11` | via Silverstein–Clifton skybridge |
| 11.6 | Founders 12 — General Medicine | `wing.founders 12` | via Silverstein–Clifton skybridge |
| 14.4 | Founders 14 — General Medicine | `wing.founders 14` | via Silverstein–Clifton skybridge |
| 14.5 | Rhoads 3 — ENT/Plastics Surgery | `wing.rhoads 3` | via Silverstein–Clifton skybridge |
| 14.8 | Rhoads 4 — GI/Bariatric Surgery | `wing.rhoads 4` | via Silverstein–Clifton skybridge |
| 15.1 | Rhoads 5 — SICU | `wing.rhoads 5` | via Silverstein–Clifton skybridge |
| 15.4 | Rhoads 6 — Surgical Unit | `wing.rhoads 6` | via Silverstein–Clifton skybridge |
| 15.7 | Rhoads 7 — Urology/Colorectal Surgery | `wing.rhoads 7` | via Silverstein–Clifton skybridge |

![Compiled campus at t0](img/campus-3d.png)

*App seed 0xC0FFEE, day 1 00:00 (census 898/1089, 82%), captured at the EP-18
commit. The Silverstein–Clifton skybridge is visible as the single link between
the two bed towers.*

Three structures stand out.

**The table splits cleanly in two.** The 19 Clifton destinations are
elevator-only trips of 4.3 minutes (all three Pavilion 7 units) to 6.1 minutes
(Pavilion 14), climbing in even 0.3-minute steps per floor — the inferred
per-story elevator constant showing through directly. The 19 HUP Main
destinations run 7.7 minutes (Silverstein 7) to 15.7 minutes (Rhoads 7), and a
1.6-minute gap (7.7 − 6.1) separates the slowest Clifton floor from the fastest
Main floor. No destination falls between the classes.

**Every Main-bound transport crosses one bridge.** All 19 HUP Main routes carry
the same landmark: the Silverstein–Clifton skybridge. With the unverified
tunnel excluded, that deck is the sole ED-to-Main artery in the graph — a
single funnel through which every westbound admission, in this model, must
pass.

**The worst case is a down-across-up traverse.** Rhoads 7 at 15.7 minutes costs
15.7 / 4.3 = 3.65 times the nearest Clifton bed — roughly 3–4x the cost of
staying in the tower. The clinically pointed comparison is the SICU: placing a
deteriorating surgical boarder in Rhoads 5 (15.1 min) rather than a Clifton
floor such as Pavilion 7 (4.3 min) carries a built-in 15.1 − 4.3 = 10.8-minute
transport difference. When the patient is the escalating one, bed geography is
a clinical variable, not a housekeeping detail.

The robust content of this table is topological, not numeric. One bridge means
one funnel; elevator-only versus cross-campus is a real distinction the sourced
geometry forces. The specific minutes inherit every inferred constant in
`RouteParams` — halve the elevator wait and the whole table shifts, but the two
classes and the funnel do not move.

## Assumptions & limitations

- **Unlimited elevator and bridge capacity.** Transports never interact: no
  elevator queueing, no bridge congestion, no bed-ahead-of-you delay. This
  biases every time low, and biases the Clifton times (a single elevator hop
  is the entire trip) relatively more favorably than a real morning would.
- **Constant walk speed, acuity-blind.** An ICU transport with a travel team,
  monitor, pumps, and a ventilator moves slower and costs more staff than the
  model knows. This understates exactly the trips where minutes matter most —
  the ICU rows are the least trustworthy numbers in the table, in the
  direction of optimism.
- **No porter/transporter staffing layer.** The clock starts when the trip
  starts; dispatch delay, porter availability, and hand-off time are absent.
  Real door-to-bed intervals would be longer everywhere.
- **Deck levels partly inferred.** The KML fixes the skybridge footprints, but
  which floors some decks land on is inferred, so the vertical component of
  cross-campus routes carries roster-level uncertainty.
- **Minutes not field-validated.** No route here has been walked and timed;
  the constants are plausible-norm values, and the absolute minutes should be
  read as order-of-magnitude with credible relative structure.
- **One-way analysis.** Return trips, equipment staging, and the porter's
  deadhead back to the ED are ignored, so total transport workload is
  understated by at least a factor of two.
- **Transport time is not yet in the placement score.** The EP-14 placement
  logic does not consume these minutes — deliberate sequencing, noted as
  future work in the capstone — so the simulation currently places patients
  as if this table did not exist.

## Operational read

What a capacity committee can take from this study is the shape, not the
minutes. The model's geometry — most of it sourced from real wayfinding and
survey data — makes ED-to-bed transport a two-class system: tower beds reached
by elevator alone, and cross-campus beds that all fund a single skybridge
crossing, with the far surgical tower costing roughly 3–4x the nearest floor.
That argues for (a) treating bed geography as an explicit input when placing
patients whose next event may be an emergency, and (b) treating the one bridge
as critical infrastructure whose closure would sever, not lengthen, the
default ED-to-Main route. What the committee should *not* take away is any
specific number: every minute in the table is built from inferred, synthetic
constants, one plausible parameterization among many, and none has been
validated against the real hospital's transport logs. The topology is the
finding; the minutes are an illustration.

Part of the [hupsim case-study series](README.md) · Model-wide assumptions and
the road forward: [Model honesty & future
directions](05-model-honesty-and-future-directions.md)
