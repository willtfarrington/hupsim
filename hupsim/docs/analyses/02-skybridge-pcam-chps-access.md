# Two questions that sound alike: PCAM access and CHPS access from the ED

*"How far is PCAM?" and "how far is CHPS?" route through different campus anatomy,
and the model's confidence policy on an unverified tunnel costs almost nothing.*

## Question

When the ED needs to move a patient toward the Perelman Center (PCAM) or toward the
Center for Human Phenomic Science (CHPS), what does the route actually look like — and
does the model's decision to exclude an unverified HUP Main↔Clifton tunnel from
default routing change any answer a capacity manager would care about?

The question matters because the two destinations are easy to conflate. The raw
topology data originally implied CHPS inpatient beds sat inside PCAM. The corrected
model places them at Ravdin 6 NE, in HUP Main — which moves the "CHPS bed"
conversation to the opposite side of the campus from the "PCAM procedure"
conversation.

## Data & methods

All travel-time parameters in this study are synthetic-but-shaped: corridor and
elevator constants are plausible professional-norm values held in visible tables, not
measurements of the real hospital. No real patient data exists anywhere in the
project. What is real-world-sourced is the structure the routes traverse: building
footprints and the skybridge decks come from a hand-surveyed Google Earth KML, and
the unit roster (including the CHPS bed placement) is OSINT-sourced with per-entry
confidence tags. One naming note: "The Pavilion" (3400 Civic Center) is called
**Clifton** in this model; unit display names still print "Pavilion 7 City" and the
like.

The routing graph's nodes are (building, level) pairs. Edges are corridors,
elevator hops, the three KML-surveyed skybridges, the Connector corridor joining
Clifton and PCAM at their first floors, and — behind a policy toggle, off by default —
a HUP Main↔Clifton tunnel whose existence is tagged unverified. A test harness
(living beside the `ed_to_bed` matrix in `hupsim-data`'s compile tests) runs
shortest-path queries from the ED, which sits at Clifton level 1, and prints four
things: minutes to every PCAM level, minutes to CHPS at Ravdin 6, a census of which
edge every ED → HUP Main inpatient route crosses, and a counterfactual rerun with
the tunnel admitted. The test asserts the funnel property itself — every ED → HUP
Main route crosses the one skybridge — so that claim cannot silently rot as the
graph evolves (the count of 19 destinations would track any future roster change).

The CHPS placement itself is a data correction, not a routing artifact. The CHPS
locations page places its 5 inpatient/research beds at Ravdin 6 NE; PCAM 4 South
holds only outpatient bays. The units.json entry carries that source and tag, and
the routes below render the corrected claim rather than the plausible-but-wrong one.

**Provenance.** Sourced with confidence tags: the building roster, footprints, and
the three skybridge decks (KML hand-survey, verified geometry); the CHPS bed
location (verified, sourced to the CHPS locations page); the tunnel (unverified —
which is the point of the counterfactual). Synthetic: every minute figure, since
walking speeds, corridor circuity multipliers, and elevator constants are inferred
parameters. Route minutes are estimates whose robust content is topological — which
bridge, how many cores — not numeric.

**Reproduction.**

```
cargo test -p hupsim-data pcam_chps -- --nocapture
```

Deterministic; no seed. Run at the EP-18 commit recorded in [`roadmap/README.md`](../../../roadmap/README.md).

## Results

### PCAM is a Clifton-side destination

From the ED (Clifton 1), PCAM is reached without touching HUP Main at all:

| Destination | Minutes | Path |
|---|---:|---|
| PCAM 1 | 3.1 | Connector corridor |
| PCAM 2 | 3.3 | Clifton–Perelman skybridge #1 |
| PCAM 3 | 6.1 | Clifton–Perelman skybridge #1 |
| PCAM 4 | 6.4 | Clifton–Perelman skybridge #1 |
| PCAM 5 | 6.7 | Clifton–Perelman skybridge #1 |
| PCAM 6 | 7.0 | Clifton–Perelman skybridge #1 |
| PCAM 7 | 7.3 | Clifton–Perelman skybridge #1 |
| PCAM 8 | 7.6 | Clifton–Perelman skybridge #1 |
| PCAM 9 | 7.9 | Clifton–Perelman skybridge #1 |
| PCAM 10 | 8.2 | Clifton–Perelman skybridge #1 |
| PCAM 11 | 8.5 | Clifton–Perelman skybridge #1 |
| PCAM 12 | 8.8 | Clifton–Perelman skybridge #1 |
| PCAM 13 | 9.1 | Clifton–Perelman skybridge #1 |
| PCAM 14 | 9.4 | Clifton–Perelman skybridge #1 |
| PCAM 15 | 9.7 | Clifton–Perelman skybridge #1 |
| PCAM G | 5.9 | Connector corridor |

The two ground-adjacent levels (PCAM 1 at 3.1 min, PCAM G at 5.9 min) ride the
Connector corridor; every upper floor crosses the first Clifton–Perelman skybridge
and then climbs — 3.3 min at PCAM 2, a one-time 2.8 min elevator step to PCAM 3
(6.1 min), then a steady 0.3 min per floor up to PCAM 15 (9.7 min).

### CHPS is a HUP Main destination

ED → CHPS (Ravdin 6) is 8.7 min, via the Silverstein–Clifton skybridge — the
opposite side of the campus from every PCAM route above. Had the model kept the raw
topology's PCAM placement for CHPS, the answer would have been a ~6-minute
Clifton-side walk to PCAM 4; the corrected answer is a cross-campus trip through the
HUP Main funnel. A bed conversation and a procedure/imaging conversation route
through different anatomy, and the difference exists in the model only because the
data correction was made and tagged rather than left plausible.

### The funnel, and the tunnel counterfactual

All 19 of 19 ED → HUP Main inpatient routes cross the Silverstein–Clifton
skybridge; that census is enforced by a test assertion. Admitting the unverified
HUP Main↔Clifton tunnel into routing reroutes exactly 5 of the 19 destinations —
the five Rhoads floors — and leaves 14 on the skybridge path:

| Destination | Skybridge (min) | With tunnel (min) | Saved (min) |
|---|---:|---:|---:|
| Rhoads 3 — ENT/Plastics Surgery | 14.5 | 14.1 | 0.4 |
| Rhoads 4 — GI/Bariatric Surgery | 14.8 | 14.4 | 0.4 |
| Rhoads 5 — SICU | 15.1 | 14.7 | 0.4 |
| Rhoads 6 — Surgical Unit | 15.4 | 15.0 | 0.4 |
| Rhoads 7 — Urology/Colorectal Surgery (worst case) | 15.7 | 15.3 | 0.4 |

The worst-case ED-to-bed route improves from 15.7 to 15.3 min. Nothing else moves.
The headline structure — one skybridge carrying every HUP Main admission route —
survives the policy choice entirely. That licenses a general principle: when a
conclusion is insensitive to an unverifiable edge, the model can afford epistemic
conservatism and exclude it by default; when a conclusion *would* hinge on such an
edge, the model should surface that dependence and force the verification question
rather than silently picking a side. Here the tunnel is worth 0.4 min on 5 of 19
routes, so conservatism is nearly free.

## Assumptions & limitations

- **Corridor weights are inferences, not surveys.** The Connector and D&W corridor
  edges are centroid-distance × circuity estimates; no indoor survey backs them.
  If real corridors are more circuitous than the multiplier assumes, the
  Clifton-side PCAM minutes are underestimates; the PCAM-vs-CHPS *directional*
  contrast is unaffected because it is topological.
- **Shared elevator constants.** Vertical travel uses the same per-floor and
  wait-time constants as study 01, with the same caveats: real elevator performance
  varies by bank, era, and time of day, and a slow core would stretch the
  0.3 min/floor PCAM gradient uniformly upward.
- **PCAM's elevator core is synthesized.** The topology has no authored core for
  PCAM; routing uses an inferred "circulation" core. Floor-to-floor increments
  within PCAM are therefore modeled convention, not building knowledge — the
  ranking of floors is trustworthy, the increments less so.
- **Bridge deck levels are partly inferred.** The KML fixes the bridges' plan
  positions; which floors they land on is partly inferred, which could shift where
  the vertical climb begins by a floor and the minutes by a few tenths.
- **The tunnel's existence is itself the open item.** The counterfactual quantifies
  what the tunnel would be worth if it exists as modeled; it does not resolve
  whether it exists. The 0.4 min figure inherits every corridor-weight caveat
  above, likely in the direction of understating a real tunnel's awkwardness
  (grade changes, security doors) and thus overstating its benefit.

## Operational read

A capacity committee should take three things from this study. First, requests that
sound like "get the patient to Perelman" split into two operationally different
moves: PCAM procedures and imaging are a short Clifton-side trip (3.1–9.7 min
modeled), while a CHPS research-bed placement is a cross-campus HUP Main admission
riding the same single-skybridge funnel as every other ED-to-bed transfer
(8.7 min modeled). Planning that treats them as one destination will misallocate
transport effort in both directions. Second, the funnel is the finding: one
skybridge carries 19/19 modeled admission routes to HUP Main, and no plausible
resolution of the tunnel question changes that. Third — the hedge — none of these
minutes are measurements. The parameters are synthetic-but-shaped, so the committee
should act on the structure (which bridge, which side of campus, how concentrated
the flow) and treat every decimal as an illustration to be replaced by timestamped
transport data before any staffing or escort decision is made. What the model does
establish firmly is where verification effort is and is not worth buying: the
tunnel, at 0.4 min on five routes, is not; the skybridge funnel's real-world
throughput is.

Part of the [hupsim case-study series](README.md) · Model-wide assumptions and the
road forward: [Model honesty & future
directions](05-model-honesty-and-future-directions.md)
