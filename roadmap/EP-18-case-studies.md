# EP-18 — Case studies + documentation refresh

**Size:** S/M · **Depends on:** EP-6 (transport analytics), EP-13, EP-15, EP-17 · **Blocks:** —

## Context

The written artifact layer: analyses that demonstrate the model answering questions an
hospitalist-informaticist with triage/capacity-management experience would
actually pose — the "documented insight" endpoint EP-6's brief already gestures at
("Document at least one such analysis in the workspace README when done"). Everything
here is writing + running existing tools; no new engine code beyond tiny glue.

## In scope

1. **Case studies** (a `docs/analyses/` directory in the workspace, one markdown file
   each, every figure/number reproducible from a stated seed + commit):
   - **ED-to-bed transport, HUP Main vs Clifton** (EP-6): route times from ED to
     representative units in both towers; what the skybridge topology implies for
     placement when minutes matter.
   - **Skybridge constraints on PCAM/CHPS access** (EP-6's validation question,
     including the deliberate CHPS-at-Ravdin-6NE modeling note — PCAM access and CHPS
     access are distinct destinations).
   - **Surge findings** (EP-15): the ICU-closure and arrival-wave experiments, A/B
     deltas, and the operational read (where boarding pools, which levers relieved it).
   - **Forecast calibration** (EP-17): the calibration table with commentary on where a
     simple model is honest vs where it breaks.
   Each study states data provenance (synthetic-but-shaped parameters, tagged
   confidence) so nothing masquerades as real HUP operational data.
2. **README/DESIGN refresh**: workspace `README.md` gains a "What this demonstrates"
   section linking the studies; `DESIGN.md` gains the phase-2 architecture additions
   (matrix, rolling stats, RN roster, query engine) at its current level of detail;
   the parent directory `readme.md` project-status block updated; roadmap README status
   boxes checked through this EP.
3. **Reproducibility check**: each study's stated command/seed re-produces its headline
   numbers (spot-check at least one number per study on a fresh run).

## Out of scope

- New features. If a study reveals a bug, file it in the roadmap README's open items
  (fixing it may be worth a session of its own).

## Verification / acceptance

- All four studies committed with reproduction commands; spot-checks pass.
- Docs mention no pre-phase-2 architecture as if current (grep for "8 m podium",
  "OSM plate", stale process names).
- A cold reader of `docs/analyses/` alone can tell what the model is for and what it
  deliberately does not claim.
