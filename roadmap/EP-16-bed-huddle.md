# EP-16 — Bed-huddle morning report

**Size:** M · **Depends on:** EP-13 (rollups), EP-10 (dynamics make it meaningful) · **Blocks:** —

## Context

A daily-snapshot view mirroring a real bed meeting — the artifact a capacity-management
group actually runs its morning on: what came in overnight, what should leave today,
where the pressure is. Exportable, so it reads as an operational deliverable rather than
a screen.

## In scope

1. **Snapshot assembly** (pure function over hospital + matrix + rolling stats + sim
   log, testable headlessly): for a given sim "now" —
   - **Overnight admits**: admissions since the previous 19:00 (or a 12 h window),
     grouped by service line, with acuity tier counts.
   - **Anticipated discharges**: occupied rooms whose patient is stable (tier +
     instability threshold) and past a service-typical LOS fraction (EP-10's LOS table
     is the reference; label the heuristic honestly on the report).
   - **Boarding**: current boarders with location and wait context.
   - **Pressure table**: per service line and per level of care — census/capacity,
     occupancy %, z badge, expected-net (anticipated discharges − recent arrival rate).
2. **Report view**: an egui window/modal ("Morning report — day 3, 07:00") laid out for
   reading top-to-bottom in a minute; EP-12 widget chips reused, but this surface leans
   text/table (it mirrors a printed huddle sheet).
3. **Export**: "Save report…" via the existing `rfd` dialog — markdown file (tables as
   pipe tables, header with sim day/time + scenario meta + seed for reproducibility).
   Markdown keeps it diffable and paste-able; no PDF machinery.
4. **Trigger**: toolbar button anytime, plus an optional auto-open at 07:00 sim time
   (toggle, default off — don't steal the screen).
5. **Tests**: snapshot assembly on a crafted world (known admits/discharge candidates/
   boarders in, exact expected report model out); export round-trip parses as valid
   markdown table content; determinism (same world ⇒ identical report text).

## Out of scope

- Editing/annotating the report in-app; historical report archives; email/printing.
- New clinical logic (discharge-readiness stays the labeled heuristic above).

## Verification / acceptance

- `cargo test --workspace` green; assembly tests as above.
- Run to sim-day 07:00 with EP-10 on: the report populates all four sections sanely;
  export writes a markdown file whose numbers match the on-screen drawer at the same
  instant; the seed line makes the snapshot reproducible.
