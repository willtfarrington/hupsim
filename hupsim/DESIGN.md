# hupsim — HUP hospital-system simulator

A modular Rust desktop application that models the Hospital of the University of
Pennsylvania down to the individual patient-care room, renders the campus as an
architecturally simplified but topologically accurate 3D massing model, and
carries scaffolding for future time-series simulation.

Built to demonstrate hospitalist/informaticist systems thinking: the data layer
treats unit assignments as *hypotheses with provenance*, not facts, because
that is what they are at a real post-Pavilion HUP.

## Crate map

```
hupsim/
├── crates/
│   ├── hupsim-core   # Domain model. Zero UI deps. Buildings/floors/units/rooms,
│   │                 # patients + acuity, provenance tags, green→blue→red
│   │                 # colormap (Oklab), aggregation, census ops, scenario
│   │                 # ser/de, RN workload composite (EP-11), columnar room
│   │                 # matrix (EP-9), placement query (EP-14), route graph
│   │                 # (EP-6).
│   ├── hupsim-sim    # Simulation scaffolding. SimClock, event queue,
│   │                 # SimProcess trait, timeline sampler, rolling stats
│   │                 # (EP-9), flow/acuity processes (EP-10), RN roster
│   │                 # (EP-11), A/B experiments (EP-15), morning report
│   │                 # (EP-16), census forecast (EP-17), timed transport
│   │                 # (EP-6), standing-census exits (EP-21), demo processes.
│   ├── hupsim-data   # Compiler: hup_topology.json + units.json +
│   │                 # service_lines.json + layout.json → core::Hospital +
│   │                 # core::CampusLayout. Room generation from room-number
│   │                 # ranges. Demo census seeding.
│   └── hupsim-app    # Bevy + bevy_egui desktop app. 3D campus view,
│                     # 2D unit detail view, egui inspector/editor panels.
└── assets/data/      # The four data files. Human-editable JSON.
```

Dependency direction: `app → {data, sim} → core`. Core knows nothing about
Bevy; sim knows nothing about rendering. The eventual time-series work extends
`hupsim-sim` only.

## Domain model (hupsim-core)

- **Hospital** = flat `Vec<Building>` + `Vec<Unit>` + `Vec<Room>` +
  `Vec<Connection>` + elevator cores. A transient `HospitalIndex` (HashMaps)
  is rebuilt after structural edits; room-status edits never invalidate it.
- **Room** is the primary object. `RoomStatus = Vacant | Occupied(Patient) |
  OutOfService{reason}`. Kinds: licensed inpatient, ICU bay, ED bay (boarding-
  capable), obs, research, L&D, nursery.
- **Patient** is synthetic: alias, age, service, admit time (sim minutes),
  `Acuity { instability: 0.0..=1.0, trend_per_hr }`, boarding flag, isolation,
  telemetry. `instability` is deliberately abstract — a hook where a NEWS2-like
  score or a learned risk model plugs in later. Tiers: Stable < 0.25 ≤ Watcher
  < 0.5 ≤ Unstable < 0.8 ≤ Critical.
- **Provenance** on every building/unit/room: `verified | reported | inferred |
  unverified` + source string + era (`pre_2021` vs `post_pavilion`). The UI
  surfaces this; stale Guide-to-Units assignments are visibly stale.
- **Connections** mirror the topology graph edges: bridges, tunnels, connector
  corridors, structural adjacency, floor continuity. (The
  `no_physical_connection` kind remains in the model but its edges were
  archived with the out-of-scope detached sites.)

## Color model

User-specified spectrum: **green → blue → red** over patient instability
(0 → 0.5 → 1). Interpolation is piecewise in **Oklab** so the midpoints don't
turn muddy. Vacant rooms are neutral gray; out-of-service darker slate.
Unit/floor/building aggregate color = mean or worst-case instability of
occupied rooms (mode toggle), gray when empty.

## Simulation scaffolding (hupsim-sim)

- `SimClock { now_min, speed, running }` — sim minutes; app drives it from
  Bevy's `FixedUpdate`.
- `EventQueue` — min-heap of `ScheduledEvent { at_min, seq, kind }`;
  `SimEventKind = Admission | Discharge | Transfer | AcuityChange | Custom`.
- `SimProcess` trait — `step(&mut SimCtx)` per tick with `&mut Hospital`,
  dt, seeded ChaCha8 RNG, and the queue. Future ADT/LOS/deterioration models
  implement this trait and are registered on the engine.
- `Timeline` — fixed-interval per-unit samples (census, mean/worst
  instability, boarding count) in a bounded ring buffer: the substrate for the
  future time-series charts.
- Realism processes (EP-10/EP-21, default on — the clock just starts paused):
  `DiurnalFlow` (λ(t) arrival shapes sub-stepped ≤ 15 min, service-line LOS
  log-normals, discharge-hour shaping), `EdPressure` (ED arrivals → ~30%
  admit conversion → boarding until a bed frees; adopts standing ED occupants
  on its first step), `AcuityDrift` (instability drift with unit-typed
  volatility/reversion), and `BacklogDischarges` (one-shot residual-LOS exits
  for the seeded census, EP-21). Every synthetic parameter lives in one
  visible table (`params.rs`). The original flat demo flow stays registered
  but off, as a control signal.

Determinism: one `ChaCha8Rng` seeded from the scenario; same seed + same
actions ⇒ same trajectory.

## Room matrix & rolling statistics (EP-9)

- **`RoomMatrix`** (`hupsim-core/src/matrix.rs`): a derived columnar view of
  the room roster (unit/type/status/acuity columns) for whole-house scans;
  `Vec<Room>` stays the source of truth. The engine owns one, refreshed each
  tick after events and before processes; `SimCtx` exposes it read-only as a
  tick-start snapshot. The app keeps its own version-keyed copy in
  `AggCache`, deliberately not reading the engine's (which is stale while
  the clock is paused and the editor mutates the world).
- **`RollingStore`** (`hupsim-sim/src/rolling.rs`): trailing-24 h ring
  buffers (96 × 15-min samples) of census/occupancy/boarding per unit, level
  of care, service line, building, and hospital, yielding the z-scores the
  dashboards badge. Warm-up honesty rule: no filled window → no z, never a
  fake zero.
- Rayon-parallel rollups sit behind an optional core feature: ~4× on 50k
  synthetic rooms in the benchmark, no win at the real hospital's ~1,100 —
  kept for the day the matrix outgrows a single core.

## Dashboards (EP-12/EP-13)

- A hand-painted egui widget kit (`widgets.rs`: KPI chips, z-badges, banded
  sparklines, tier histograms, workload bars — no egui_plot) feeds two
  surfaces: the unit header strip above the room grid (EP-12) and the
  hospital drawer (EP-13) with level-of-care / service-line / building tabs,
  pressure-sorted (occupancy z, then boarding), plus drill-through that
  filters the navigator and tints the 3D slabs. View-models are pure
  functions, unit-tested headlessly; the paint layer itself is exercised by
  launching the app (a lesson recorded in the roadmap).

## Placement engine (EP-14)

- **`hupsim-core/src/query.rs`**: `PlacementQuery` → hard filters (vacant +
  in-service, level-of-care band with an explicit adjacent-LOC overflow
  table, airborne isolation ⇒ negative-pressure room) → scoring:
  lexicographic level-of-care tier (an exact-LOC bed always outranks an
  overflow bed) then a weighted sum — service-line match 1.0, occupancy
  relief 0.8 (live occupancy blended 50/50 with the EP-9 z-score), RN
  headroom 0.6 (from the covering RN's live workload), telemetry fit 0.4.
  The score breakdown sums exactly to the scalar; when norms or roster make
  no claim a term scores a neutral 0.5.
- App side: a Placement tab beside the Inspector stages results as explicit
  snapshots (staleness banner, labeled truncation) with violet highlights in
  both views; acting on a result routes through the same `ops` used by the
  editor. Route/transport times are deliberately not yet a scoring term
  (future work; see `docs/analyses/`).

## Surge experiments (EP-15)

- **`hupsim-sim/src/experiment.rs`**: staged `Intervention`s — `CloseUnit`
  (vacant beds out of service, occupants evacuated first-fit to same-LOC
  beds, stranded occupants board in place, a sweep closes beds as they
  empty), `ArrivalWave` (N extra admissions paced over a window, unplaced
  demand retried and reported, never dropped), `FlexRatio` (staffing only,
  never bed physics). `run_experiment` clones the world per arm, seeds
  identical engines, pre-warms z-norms from the live run, backfills
  residual-LOS discharges identically in both arms, and ticks headlessly at
  the sampling cadence; the A/A null-diff (bit-identical arms) is the
  acceptance cornerstone. One deterministic realization per arm — perfect
  attribution, no distribution; the honesty caveat rides every result.
- The Compare drawer tab stages, runs, and overlays the arms (baseline blue,
  intervention clay), with the staged description and staleness printed.

## RN staffing model (EP-11)

- **`RnRoster`** (`hupsim-sim/src/roster.rs`, owned by the engine): every
  staffed unit (inpatient, ED, and other bedside-nursed types; research and
  "other" excluded) carries ⌈census ÷ patients-per-RN ratio⌉ anonymized RNs
  ("RN #1"…, min 1 while census > 0). The ratio is the EP-8
  `Unit.target_rn_ratio`, with an inferred level-of-care fallback for
  pre-EP-8 worlds.
- **Assignment** is deterministic and RNG-free: the unit's occupied rooms in
  corridor order split into one contiguous run per RN, then a bounded greedy
  pass moves boundary rooms to equalize the workload composite (runs stay
  contiguous; each boundary shifts at most a few rooms — "bounded swap
  distance"). Census changes re-derive only the affected unit.
- **Workload composite** (`hupsim-core/src/workload.rs`, shared by the sim,
  the EP-12 dashboards, and EP-14 scoring): weighted census-vs-ratio (1.0 =
  at ratio) + summed patient instability + per-patient isolation/telemetry
  overheads + trailing churn. The breakdown's components sum exactly to the
  scalar. Churn is a per-RN decaying event counter (4 h half-life) fed by
  occupancy diffing, so admits/discharges/transfers count no matter whether
  they came from queue events, flow processes, or the editor; boarding
  patients count like any other occupant. Weights are synthetic/inferred —
  one visible table, like `params.rs`.
- **Shift turnover** at 07:00/19:00 sim time: every roster re-derives with
  fresh RNs (renumbered, churn zeroed, manual pins cleared — that's the
  point of a handoff) and a handoff line goes to the sim log ("FOUNDERS 12:
  shift change, 5 RNs on"). Manual reassignment in the room inspector pins a
  room to an RN until the next turnover or a census-triggered re-derive that
  can't honor it (logged when dropped).
- **Ephemeral by design** (owner decision): rosters are NOT part of
  `Scenario`/schema v1 and are never serialized. They regenerate
  deterministically from the seed and current census on load/reset —
  assignments are shift-local operational state, not part of the saved
  world. This is a modeling decision, not an accident.

## Morning report (EP-16)

- **`hupsim-sim/src/report.rs`**: the bed-huddle morning report as a pure
  function — `assemble(hospital, matrix, rolling, log, now, meta)` →
  `MorningReport` — plus a deterministic markdown export (`to_markdown`).
  Four sections: overnight admits (trailing 12 h, grouped by service line
  with stability-tier mix; bed admissions and admitted boarders count, ED
  walk-ins don't), anticipated discharges, current boarders (longest wait
  first), and pressure tables per level of care and per service line
  (census/staffed, occupancy %, occupancy-z badge, boarding, likely
  discharges − overnight admits = expected net), sorted by the same
  pressure key as the EP-13 drawer.
- **Discharge readiness is a labeled heuristic**, stated in the report
  header: stable (instability < 0.25), not boarding, inpatient level of
  care, ≥ 75% of the service-typical median LOS from the EP-10 table.
  Patients present before sim start have no admission timestamp — they
  count as past threshold and are reported as "pre-sim", never silently
  included or dropped.
- **Log corroboration honesty**: bed-admission / ED-admit-decision /
  inpatient-discharge counts scanned from the bounded sim log are labeled
  as lower bounds ("≥") whenever the log no longer reaches back to the
  window start.
- **App side** (`report_panel.rs`): a floating egui window over either view
  (toolbar 📋 Report), holding an explicit snapshot with a staleness line
  (EP-14/EP-15 discipline) and a Refresh button; "Save report…" exports the
  markdown via `rfd` (pipe tables, seed + sim-minute line, so a snapshot is
  diffable and reproducible). Optional auto-open at 07:00 sim time
  (default off); the crossing detector ignores backwards moves and
  load-sized jumps so scenario loads never steal the screen.

## Census forecast fan (EP-17)

- **`hupsim-sim/src/forecast.rs`**: `forecast_census(hospital, index, now,
  params)` → `CensusFan` (median + 10/90 quantiles on a 15-min grid, default
  12 h horizon) — pure, RNG-free, analytic; no Monte Carlo. census(t+h) =
  census(t) + net arrivals − expected departures, where arrivals integrate
  the generator's own diurnal λ tables (future ED arrivals churn back out on
  the treat-and-release clock as a thinned Poisson) and departures apply the
  LOS log-normals to the standing census conditioned on each patient's
  elapsed stay (EP-21 stamps), on a clock shifted by the discharge curve's
  mean +12.5 h push and hazard-modulated by its within-day shape. Poisson +
  Poisson-binomial variance → normal quantiles, clamped at staffed beds.
- **Deliberately not an oracle**: the event queue holds every scheduled
  discharge; the forecast never reads it — it sees only what a bed-huddle
  observer could (who is in which bed, since when). That, plus the honesty
  ledger in `CALIBRATION.md` (measured coverage/MAE/bias, where it fails,
  and the two model corrections the replay forced), is the point being
  demonstrated. The measured headline (10–90 band covered 92 % of 12 h
  forecasts) is pinned as `forecast::FAN_COVERAGE_10_90_PCT`; the
  calibration test fails if the measurement drifts ±3 % from the pin.
- **Presentation**: the fan appends to the census sparklines (EP-13 drawer
  strip + status bar) past a vertical "now" divider — dashed clay median +
  translucent clay 10–90 wedge (the kit's "modeled, not observed" hue,
  validated against the chart blue in EP-15; the compare tab and the fan
  never co-occur, and the fan never forecasts hypotheticals). The blue norm
  band clips to the history side. Per-group and per-unit fans were
  deliberately skipped: the hospital-level flow split across groups is
  exactly what the simple model cannot honestly claim.
- **ED standing-census adoption** (closes an EP-21 open thread):
  `EdPressure` adopts seeded/loaded ED occupants on its first step after
  construction or reset — boarders straight onto the bed hunt, walk-ins on
  residual episode clocks — so seeded EDs converge to the live regime
  instead of holding immortal patients with unbounded displayed waits.

## Route graph & timed transport (EP-6)

- **`hupsim-core/src/route.rs`**: `RouteGraph::build(hospital, layout,
  RouteParams, TraversalPolicy)` — nodes are (building, level) pairs; edges
  come from `hospital.connections` (per-level, weighted by surveyed deck
  length where the `layout` bridge ref resolves, else centroid distance × a
  circuity factor), derived **podium** edges (buildings whose footprint sits
  on a ground plate are pairwise walkable at Ground — the authored edge set
  alone leaves Rhoads/Maloney/White disconnected; decision 3 makes the
  podium the shared circulation layer), and **elevator** hops (wait +
  per-story ride, pairwise over a core's building levels; PCAM, which has no
  authored core, gets a synthetic inferred "circulation" core). Plain
  Dijkstra, deterministic tie-breaks, no new dependencies. Unreachable pairs
  return errors, never panic. Confidence policy: verified/reported/inferred
  traversable (inferred flags the route), **unverified excluded by default**
  (the HUP Main↔Clifton tunnel, the contested Rhoads L10 continuity) behind
  `TraversalPolicy::include_unverified`. All constants in one visible
  `RouteParams` table; every non-surveyed weight is tagged
  `weight_inferred`.
- **`hupsim-sim/src/transport.rs`**: engine-owned `TransportBoard` (the
  roster precedent), shared with every process via `SimCtx`: `requests`,
  `active` (in-flight patients, **off-bed** — the owner-anticipated holding
  list), and `held` (beds reserved for inbound transports; `EdPressure` and
  `DiurnalFlow` bed hunts skip them). The RNG-free `BedTransport` process
  departs requests (frees the ED bay, logs
  `transport depart: … via …, N min`), and admits on arrival (service
  stamp + boarding clear mirror `ops::transfer`; the pre-drawn guarded
  discharge is scheduled then). If the held bed was taken by an editor
  action, arrival diverts first-fit within the destination unit or waits —
  a patient is never lost. Requesters draw LOS at request time so the RNG
  pattern matches the instant path; with `timed` off (or no graph — every
  headless/experiment engine) placements stay instantaneous and byte-perfect
  with pre-EP-6 behavior. In-flight transports are ephemeral like the event
  queue: dropped by reset/load, covered by a reset-mid-flight determinism
  test.
- **App**: toolbar `transport` checkbox (drives `engine.transport.timed`,
  not the process flag, so in-flight patients still arrive after a mid-run
  switch-off), an "In transit N" status-bar readout with per-patient ETA
  hover, and a route-graph rebuild on scenario load. The ED-to-bed
  transport-time analysis lives in README.md; the reproducible source is
  `cargo test -p hupsim-data ed_to_bed -- --nocapture`.

## Data files (assets/data)

- `hup_topology.json` — the OSINT property graph (campus/buildings/floors/
  edges/cores), copied from the parent directory; confidence-tagged.
- `units.json` — curated unit roster: service, type, building+floor, room
  ranges or bed counts, era + confidence + source per unit. EP-8 adds two
  per-unit rule blocks, both data (owner-correctable), not code constants:
  `capabilities` — telemetry `all|none` across the unit's rooms plus
  `negative_pressure_first_n`, seeded deterministically onto the FIRST rooms
  in generation order (no RNG) and compiled to `Room.telemetry_capable` /
  `Room.negative_pressure`; and `staffing` — `target_rn_ratio` (patients per
  RN), compiled to `Unit.target_rn_ratio`. Both are inferred professional
  norms, not sourced to HUP, and carry confidence + note.
- `service_lines.json` — EP-8: the ordered service-line taxonomy (ten
  umbrellas, Medicine → Other/Unassigned) and a **total** unit → line mapping
  compiled to `Unit.service_line`; a compiled unit with no assignment is a
  hard compile error, so the mapping cannot silently rot as units are added.
  Every assignment quotes the `service` string it derives from with a
  confidence on the derivation and a note wherever judgment was applied.
  EP-8 flagged the 11 units whose service was 'Unassigned (no public
  source)' as `needs_owner_ruling` — surfaced as `meta.data_warnings` at the
  status-bar data indicator and on stderr, never a silent bin. **EP-23
  (2026-08-13) ruled all 11** after a targeted OSINT session: five re-binned
  with dated evidence (Silverstein 7 → Women's Health verified via a DAISY
  award page; Rhoads 5 → the 24-bed SICU; Pavilion 7 Center → HVICU;
  12 Center → the 12-bed Oncology MICU; 12 City → Solid Oncology; plus
  Silverstein 8 and 7 Campus as owner-approved inferences and a Ravdin 6
  gyn/gyn-onc re-bin), and six retired to
  `archive/ep23_roster_rulings.json`. The flag mechanism remains live for
  future roster additions; the open set is pinned empty.
- `layout.json` — 3D massing: per-building footprint polygon (local ENU
  meters), story count/height, ground elevation, skybridge deck polygons,
  ground plates. Story counts include the shared street-level Ground story
  every building models: the wayfinding directory prints a G span for only
  four wings (Ravdin/Rhoads/Dulles/Gates), but both public entrances are
  labeled "Ground Floor" and the Connector is the first floor, so buildings
  the directory numbers from 1 (Silverstein, Founders) sit atop an inferred
  ground story and floor N aligns across wings. The eight wings share one
  3.8 m story height (their numbered floors are continuous plates, so
  per-wing heights would splay the same floor number apart vertically);
  Clifton/PCAM keep their own 4.4 m. Geometry source of truth is
  the owner's Google Earth Pro KML hand-survey (10 buildings + 3
  skybridges), imported by the EP-2 tool; the HUP Main podium plate is
  derived from the union of the eight wing footprints (+3 m buffer,
  morphological closing) by the EP-7 `podium_gen` tool and extrudes one
  story (3.5 m) — it is that shared Ground story — see
  `assets/data/podium_report.md`.
- `archive/out_of_scope.json` — records archived when scope was reduced to
  the West Philadelphia campus (Cedar, Radnor, ISC, unmapped wings). Not
  read by the compiler; provenance preserved.
- `archive/ep23_roster_rulings.json` — the six units retired by the EP-23
  owner rulings (Founders 5 + 9, Rhoads 2 + 10, Pavilion 8 Campus +
  14 City), verbatim with their service-line assignments and the contested
  Rhoads level-10 topology edge. Same pattern: not compiled, provenance
  preserved, absence pinned by test.

Room generation: HUP Main rooms come from the wayfinding room-number ranges
(e.g. `931-949, 956-958 → Ravdin core, floor 9`); Pavilion rooms are floors
7-12 + 14 × numbers 1-72 mapped to City/Center/Campus wings by the published
numbering scheme — floor 13 is modeled as absent because 504 licensed rooms =
exactly 7 × 72 and no public source names any floor-13 unit. A
`pavilion_wing` spec may carry `slots` (range syntax) to take a partial wing
or a neighbour's slots — EP-23 splits floor 12's Center wing between the
12-bed Oncology MICU (13-24) and the 36-bed 12 Campus (25-60). CHPS/ED
rooms come from bed counts with confidence tags.

## Scenario files

Full world state (structure + census + sim time + RNG seed) serializes to a
single JSON scenario file via File▸Save/Load. Schema-versioned.

Known accepted consequence of the EP-1 scope reduction: scenario files saved
before the out-of-scope archival embed the full `Hospital` and will resurrect
the archived buildings/units in the navigator when loaded (mesh-less,
layout-less) — accepted for a dev tool. Saves from before the EP-5 cleanup
also carry a per-building `campus` field; serde ignores it on load.

## App (hupsim-app)

- **Campus3D view**: extruded footprint prisms, one mesh per floor slab,
  colored by floor aggregate; bedless wings in neutral; surveyed skybridge
  decks extruded at their real heights, geometry-less connector edges as
  faint ground ribbons. Each slab sits at its label's story index
  (`Building::story_index`: Ground = 0, floor N = story N, 13-skippers
  compress above 13), and the eight wings share one 3.8 m story height, so
  HUP Main's shared floor numbers sit at the same elevation in every wing
  and the skybridge decks land beside their stated levels. (The contested
  Rhoads "level 10" floor used to float at story 10 as the rendered anomaly;
  EP-23 ruled it a directory artifact and retired it — Rhoads now tops out
  at its documented floor 7.) Orbit/pan/zoom camera.
  Click floor → unit; hover tooltips.
- **UnitDetail view**: synthetic double-loaded-corridor 2D layout of the
  unit's rooms in room-number order, colored by patient stability; click room
  → inspector.
- **egui**: left navigator tree + search; right room/patient inspector-editor
  (incl. the room's RN, workload breakdown, and pin/reassign controls);
  top toolbar (view toggle, morning report, sim run/pause/speed, save/load,
  aggregate mode); legend + status bar (hospital census, boarding count);
  the EP-16 morning-report window floats over either view.
