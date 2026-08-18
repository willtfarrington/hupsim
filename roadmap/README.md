# hupsim Single-Campus Refit — Roadmap

Master roadmap for refitting the hupsim model to the hand-mapped Google Earth KML
(`../source material/hospital of the university of pennsylvania.kml`) and reducing scope to a single
campus: **HUP West Philadelphia only**.

Planned 2026-08-11. Supersedes the earlier (never-executed) multi-site expansion concept —
PPMC, GSPP/Rittenhouse, and HUP Cedar are **out of scope**; there are no site tabs and no
network view. The KML maps 10 buildings and 3 skybridges; Donner, Devon, Gibson, Centrex,
Cedar, Radnor, and ISC were deliberately excluded by the author.

> **Commit hashes in this roadmap.** The `☑ hash` cells and in-brief references
> (`44f845c`, `0b2f3c2`, `754e5aa`, …) point into the project's *private* development
> history. When the repository was published (2026-08-18) it was re-initialized on a
> single clean commit, so those hashes do not resolve here; they are kept as the
> chronological record of which brief landed when. Every analysis in
> `hupsim/docs/analyses/` reproduces from the current tree with its stated seed + command.

## How to use this roadmap

Each `EP-N-*.md` brief is **self-contained**: a session that has read only that brief plus
the code can execute it. Hand one brief to one session. Execute in order (below), verify
the acceptance criteria, commit, then check the box here.

Workspace: `hupsim/` (the Rust workspace, directly under the repository root).

Git root: the `hupsim` repository root, one level above the workspace. The repo is
single-project now (`README.md`, `hupsim/`, `roadmap/`, `source material/`), so commit
paths no longer need scoping. *(Until 2026-08-15 the project lived two levels deep inside
the multi-project `PRIVATE-1/` repo as `000 - hospital capacity modeling v1.0.0 - hup campus/`;
the executed briefs below were written against that layout, and their older path mentions
were updated in place.)*

## Phase 1 — single-campus refit (complete)

| # | Brief | Size | Depends on | Done |
|---|-------|------|-----------|------|
| EP-0 | [Baseline & hygiene](EP-0-baseline.md) | S | — | ☑ `4575c3a` |
| EP-1 | [Archive out-of-scope data](EP-1-archive.md) | S | EP-0 | ☑ `e8b35dc` |
| EP-2 | [Geo helper + KML importer + regenerated layout](EP-2-kml-importer.md) | L | EP-1 | ☑ `5f2284e` |
| EP-3 | [Connection refit + podium + naming](EP-3-connections.md) | M | EP-2 | ☑ `9b0715c` |
| EP-4 | [Renderer refit](EP-4-renderer.md) | M | EP-3 | ☑ `d853487` |
| EP-5 | [Code scope cleanup](EP-5-cleanup.md) | S | EP-1 (cleanest after EP-4) | ☑ `972a3d1` |

EP-1 deliberately precedes EP-2 so the regenerated layout never contains entries that
would immediately be archived. (Status boxes were stale until 2026-08-12 — the ☑ marks
above are reconciled against `git log`.)

> **EP-9/EP-10 two-hash note (2026-08-12).** The original EP-9 session hit its context
> limit uncommitted; EP-10 then completed on top of it, and a follow-up session finished
> EP-9's remaining scope (aggregate cache, slab registry, benchmark — see
> `EP-9-completion-handoff.md` / `EP-9-completion-report.md`). History is split so both
> commits build green standalone: `907b061` carries the EP-9 core (matrix, rolling
> buffers — uncompiled there, benchmark), `10b8a47` carries EP-10 plus EP-9's engine
> wiring and app-side cache/registry. Hence EP-9's box lists both hashes (EP-7 precedent).

## Phase 2 — informatics, dashboards & flow (planned 2026-08-12)

Sequenced with the project owner (Q&A of 2026-08-11/12): **foundation first** — data +
matrix + statistics layers before any dashboard; **sim realism before dashboards** so
deviation-from-norm indicators demo against plausible diurnal dynamics; EP-6 (the
original flow-graph endpoint, brief unchanged) slots after the query panel, whose scoring
it can later extend with a transport-time term. One brief per session, same rules as
phase 1.

| # | Brief | Size | Depends on | Done |
|---|-------|------|-----------|------|
| EP-7 | [Podium refit + provenance hygiene](EP-7-podium-refit.md) | M | EP-5 | ☑ `9443511` + `516cef0` |
| EP-8 | [Clinical data enrichment](EP-8-clinical-data.md) | M | EP-7 | ☑ `2a50af1` |
| EP-9 | [RoomMatrix + rolling-stats engine](EP-9-room-matrix.md) | L | EP-8 | ☑ `907b061` + `10b8a47` |
| EP-10 | [Sim realism: diurnal flow, LOS, ED pressure](EP-10-sim-realism.md) | M | EP-9 | ☑ `10b8a47` |
| EP-11 | [RN staffing model + workload burden](EP-11-rn-staffing.md) | L | EP-8, EP-9 | ☑ `33987ea` |
| EP-12 | [Unit dashboard (header strip)](EP-12-unit-dashboard.md) | M | EP-9, EP-10, EP-11 | ☑ `9ef50bc` |
| EP-13 | [Hospital-wide aggregate dashboard](EP-13-hospital-dashboard.md) | M | EP-12 | ☑ `0825000` |
| EP-14 | [Query / staging panel (placement engine)](EP-14-query-panel.md) | L | EP-9, EP-11, EP-13 | ☑ `f198a1c` |
| EP-6 | [Flow-graph enrichment](EP-6-flow-enrichment.md) | L | EP-3 (met) | ☑ `835a427` |
| EP-15 | [Surge / what-if scenario tools](EP-15-surge-scenarios.md) | M | EP-13 | ☑ `5648ccf` |
| EP-19 | [Dashboard drawer height papercut](EP-19-drawer-height.md) | S | EP-13, EP-15 | ☑ `0b2f3c2` |
| EP-20 | [Camera pan discoverability & bindings](EP-20-camera-pan.md) | S | — | ☑ `df040d2` |
| EP-22 | [3D label overlay papercut](EP-22-label-overlay.md) | S | — | ☑ `b11fe89` |
| EP-23 | [Service-line rulings for the 11 unsourced units](EP-23-service-line-rulings.md) | S→M (owner escalated retirements) | EP-8 (met) — run before EP-21/EP-17 (LOS regime) | ☑ `b6a28da` |
| EP-16 | [Bed-huddle morning report](EP-16-bed-huddle.md) | M | EP-13 | ☑ `2871496` |
| EP-21 | [Seeded-census admission backdating](EP-21-census-backdating.md) | M | EP-10, EP-16 | ☑ `705613e` |
| EP-17 | [Census forecasting (fan)](EP-17-census-forecast.md) | M | EP-10, EP-13 | ☑ `d008f86` |
| EP-18 | [Case studies + doc refresh](EP-18-case-studies.md) | S/M | EP-6, EP-15, EP-17 | ☑ `a0b3ef4` |

Standing decisions for phase 2 (owner Q&A, 2026-08-11/12): trailing-24h ring-buffer
norms (96 × 15-min samples, z-scores); derived columnar matrix (`Vec<Room>` stays source
of truth, schema v1); RN model with acuity-balanced assignment, 7a/7p turnover, manual
reassignment; workload = ratio + instability + isolation/telemetry + churn; service-line
taxonomy drafted-for-owner-review; room capability columns limited to telemetry-capable
+ negative-pressure; placement scoring with level-of-care appropriateness dominant
(crunch overflow trumps service convention), no transport term until EP-6; hand-painted
egui charts (no egui_plot); unit dashboard as a header strip above the room grid.

## Decision record (settled with the project owner, 2026-08-11)

1. **Connections.** The 3 KML skybridges (Silverstein↔Clifton; Clifton↔Perelman ×2) are
   the verified, rendered connections. The wayfinding-directory-sourced level-1
   "Connector" corridor edges and the verified "Dietz & Watson Bridge" edge (HUP
   Main↔PCAM — absent from the KML) **stay in the topology for flow modeling** but render
   subtly or not at all. The unverified HUP Main↔Clifton tunnel stays flagged unverified.
2. **Out-of-scope data is archived, not deleted** → `assets/data/archive/out_of_scope.json`.
   The compiler never sees it; the research is preserved for possible re-expansion.
3. **Low-level interconnection = extruded podium.** The existing OSM HUP-Main ground plate
   (currently a 0.35 m plinth) is extruded to ~2 stories, provenance `inferred`; KML towers
   rise out of it. Known, accepted imperfection: the re-surveyed KML tower positions
   overhang the plate by ~5–11 m in places (only Ravdin is fully inside). No
   polygon-boolean dependency; convex-hull fallback only if it grates visually.
   > **Addendum (2026-08-12, EP-7).** It grated. Owner requested a true union-footprint
   > podium at one story (3.5 m). Derived offline by the tools-path `podium_gen` bin
   > (raster union of the eight wing traces + 3 m buffer + 15 m morphological closing +
   > simplification → 19 vertices; zero new dependencies, so "no polygon-boolean
   > dependency" still holds even in the tools graph). The OSM plate and its accepted
   > overhang imperfection are retired — wings sit fully inside the plate by
   > construction, enforced by compile-time tests. See `assets/data/podium_report.md`.
   > **Addendum (2026-08-12, ground-story alignment).** Owner flagged that roughly half
   > the buildings modeled a Ground floor and half didn't, so floor N rendered a story
   > apart across wings that share numbered floors. Verified against the Connector
   > wayfinding PDF: the split is a labeling artifact — both public entrances are marked
   > "Ground Floor", the Connector is the FIRST floor, and PCAM's East Pavilion is G-2,
   > so street level is Ground complex-wide even where the directory numbers a tower
   > from 1. Every building now compiles a Ground story at the bottom of its floor list
   > (four wings verified from their G spans; Silverstein/Founders/Maloney/White +
   > Clifton G-17/PCAM tagged in data or synthesized with an `inferred` note), and the
   > 3.5 m podium now *is* that shared ground layer — the single layer showing the
   > ground-level interconnection — with floors 1+ above the rim. Bonus corroboration:
   > the surveyed Silverstein↔Clifton deck (12.0 m, "level 3") only lines up with
   > Silverstein 3 under the G+numbered stacking (11.4 m vs the old 7.6 m). Alignment
   > enforced by `numbered_floors_align_across_wings` / `every_building_starts_at_ground`
   > compile tests. EP-8…EP-18 briefs reference floor labels and levels of care, never
   > stacking heights — unaffected.
   > Follow-up same day: index alignment alone left two visible offsets. (1) Per-wing
   > floor heights (3.6/3.8/4.0 m) splayed same-numbered floors apart vertically — the
   > wings share continuous floor plates, so they now share one 3.8 m story height
   > (`wing_story_heights_are_uniform` pins it; Clifton/PCAM keep 4.4 m). (2) Slabs now
   > stack at their *labeled* story (`Building::story_index`; Ground = 0, floor N = N,
   > 13-skippers compress above 13) instead of floor-list position, so the contested
   > Rhoads level-10 floor sits level with Founders 10, floating above Rhoads' G-7
   > roofline — rendering the anomaly's actual claim instead of hiding it at story 8.
   > **Addendum (2026-08-13, EP-23):** the anomaly was ruled a wayfinding-directory
   > artifact and the floating floor retired with its unit (the stacking rule stays).
4. **Bridge levels are estimates tagged `inferred`:** Silverstein↔Clifton ≈ level 3
   (KML LookAt altitude 12.0 m corroborates); both Clifton↔Perelman ≈ level 2. Listed in
   `open_questions` for on-the-ground verification.

Judgment calls made during planning (owner saw these at plan approval):
- **Clifton/PCAM rendered footprints default to the verified OSM polygons**; the KML
  hand-traces serve as cross-check inputs and bridge anchors, with per-building opt-in
  replacement in the mapping file. (The hand-trace vs OSM southern extent of Clifton
  differs ~17 m; north/east extremes agree sub-meter.)
- The building node id stays **`bldg.pavilion`** (rooms `room.pavilion.*`, 504-room test
  untouched); only the display `short_name` becomes "Clifton".

## The "phantom PCAM road" (context for several briefs)

The old render showed a ~256 m grey elevated bar crossing the campus toward PCAM. The
data was right; the render was wrong: compile remaps `bldg.hup_main` → `wing.silverstein`
(`compile.rs:355-362`), and `campus3d.rs:135-170` draws every connection as a
centroid-to-centroid cuboid tube at hardcoded heights, using a naive vertex-mean centroid
that lands outside PCAM's concave outline. EP-3 (data) + EP-4 (render) kill it.

## Risks / open items

1. ~~Podium/tower overhang — accepted, documented (decision 3).~~ **Resolved by EP-7**
   (2026-08-12): plate derived from the wing union; overhang impossible by construction.
2. Bridge levels are inferred pending on-the-ground verification.
3. Old scenario save files embed the full Hospital and will resurrect archived buildings
   in the navigator when loaded — accepted for a dev tool, documented, not engineered around.
4. Clifton hand-trace vs OSM extent delta (~17 m south) — the EP-2 cross-check report
   surfaces it; OSM stays authoritative unless the owner opts in to the trace.
5. Dietz & Watson bridge + Connector cross-street reality is directory-sourced and
   unconfirmed on the ground — kept, flagged, `open_questions`.
6. ~~Eleven units (260 of 1,263 rooms) carry service "Unassigned (no public source)"
   flagged `needs_owner_ruling` (EP-8).~~ **Resolved by EP-23** (2026-08-13): all 11
   ruled after a targeted OSINT session — five re-binned with dated evidence, six
   retired to `archive/ep23_roster_rulings.json`; zero flags outstanding, roster now
   42 units / 1,115 rooms. New open items spawned by the rulings:
   - **Founders 8 MICU capacity**: the directory range 875-886 models only 12 beds;
     reported MICU capacity is likely larger (historically 24). Directory-honest for
     now — revisit when a bed-count source appears.
   - **HVICU floor 7 vs floor 9**: Penn's CCM-fellowship page places the Heart &
     Vascular ICU on Pavilion 7; a careers requisition placed it on floor 9. Both
     modeled (7 Center + 9 City), cross-noted, possibly one unit — resolve with a
     wing-naming source.
   - **11 Campus conflict**: a 2026 requisition pairs "Cardiology PCU — 8 Center/11
     Campus" against the 2025 Oncology/BMT sources; kept Oncology/BMT, unresolved.
   - **Founders 9 / Rhoads 2 current use**: both retired without a replacement
     identity; restore from the archive if a source appears.
   - ~~**Sim tuning is now stale**: `params.rs` sizes arrivals for "~950 inpatient
     beds" (now ~990 licensed-modeled → 1,115 incl. ED/CHPS; inpatient beds dropped
     ~12%) — the immortal-seed saturation (item for **EP-21**) now bites harder: at
     seed 42 a day-2 report window can hold zero bed admissions/discharges.
     Recalibrate in EP-21/EP-17, not piecemeal.~~ **Resolved by EP-21**
     (2026-08-13): backdated seed stamps + one-shot `BacklogDischarges`
     residual-LOS exits; `DIURNAL_BASE_ADMITS_PER_HR` recalibrated 6.0 → 3.5
     against the 944-inpatient-bed roster (~144 h effective mean LOS) — 12-day
     runs breathe at ~85% weekdays with a weekend dip (pinned by
     `census_regime_smoke`); the report_smoke strict churn corroboration is
     restored. Threads left open by EP-21 (small, cosmetic/deferred):
     ~~seeded ED occupants are still unmanaged — `EdPressure` never adopts
     them, so seeded boarders' waits grow without bound over multi-day runs
     (hand to EP-17/EP-18 if it bothers demos)~~ **resolved by EP-17**
     (2026-08-13): `EdPressure` adopts standing ED occupants on its first
     step (one-shot, reset-rearmed) — boarders place, walk-ins release on
     residual episode clocks; the report's overnight-window *label*
     clamps at t0 while the day-1 count correctly reaches back to pre-t0
     backdated stamps (report format is EP-16-owned; change deferred).

## Provenance discipline (applies to every brief)

Every node/edge/unit carries `confidence` ∈ {verified, reported, inferred, unverified};
units carry `source`/`era`. Any data edit keeps or improves this tagging. New geometry
from the KML is `source: "Google Earth Pro hand-mapping (owner), 2026-08"`,
`confidence: verified` for footprint shape/position, `inferred` for anything estimated
(bridge levels, podium height).
