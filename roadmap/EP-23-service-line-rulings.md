# EP-23 — Service-line rulings for the 11 unsourced units

> **☑ COMPLETE (2026-08-13).** Ran as an interactive owner Q&A session with a
> parallel OSINT sweep (8-cluster multi-agent web hunt, adversarially
> verified citations, ~650 searches) plus owner hand-searches of bot-blocked
> sources (Instagram, careers portal, fellowship pages). Outcome per unit —
> note the sweep *refuted* several worksheet hypotheses, and the owner
> escalated four rulings beyond the brief's (a)-(d) menu into roster
> corrections:
>
> | unit | ruling |
> |---|---|
> | Silverstein 7 | **(a) verified** → Women's Health antepartum/postpartum (DAISY Sept 2024; Feb-2023 split) — medicine hypothesis refuted |
> | Silverstein 8 | **(c) inferred** → Postpartum/Mother-Baby, women's line (2018 DAISY + live mother-baby reqs) |
> | Rhoads 2 | **retired** to archive — no 2021-2026 corroboration |
> | Rhoads 5 | **(a) verified** → the 24-bed SICU, `unit_type: icu`, Critical Care (Beacon req + CCM fellowship page) |
> | Rhoads 10 | **retired** to archive + floating floor removed — ruled a directory artifact (stable since ≥2019; Penn's own PDF internally inconsistent) |
> | Pavilion 7 Center | **HVICU**, `icu`, line.cardiac (CCM fellowship page names Pavilion 7; wing inferred; possible duplicate of 9 City — cross-noted) |
> | Pavilion 7 Campus | **(c) inferred** → HV/thoracic ward, line.cardiac |
> | Pavilion 8 Campus | **retired** to archive — no evidence of a distinct unit in 5 years of postings |
> | Pavilion 12 Center | **Oncology MICU, 12 beds** (`slots: "13-24"`), `icu`, line.oncology; 12 Campus grew to its verified 36 beds (`slots: "25-60"`) |
> | Pavilion 12 City | **(a) verified** → Solid Oncology (2022 req "12 City Oncology") |
> | Pavilion 14 City | **retired** to archive — liquid onc stays 14 Campus + Center |
>
> Bonus opportunistic rulings: **Ravdin 6** re-binned to Gyn/Gyn-Onc +
> overflow (DAISY, Feb-2023 split); **Founders 8 = the MICU** (opened March
> 2023 ex-Cedar; Founders 9 retired); **Founders 5 SICU entry retired**
> (superseded by Rhoads 5); 11 Campus cardiology-pairing conflict noted, not
> re-binned. Roster: 48 → 42 units, 1,263 → 1,115 rooms, NP 90 → 79,
> `line.other` = CHPS only, ⚠ chip = 0. Flag test re-pinned as
> `ep23_rulings_leave_no_units_awaiting_ruling` (empty set) + new
> `ep23_retired_units_are_absent`; retirements in
> `assets/data/archive/ep23_roster_rulings.json`. Two eyes-open smoke
> re-pins (compare flex ratio 2.0; report log-corroboration spans ED
> decisions). New open items recorded in the roadmap README (MICU capacity,
> HVICU 7-vs-9, 11 Campus, stale sim tuning → EP-21).

**Size:** S (data edits + doc updates; M only if a ruling changes a unit's
`unit_type` — see Pavilion 12 Center) · **Depends on:** EP-8 (met) ·
**Blocks:** nothing hard, but **run before EP-21/EP-17 if possible** — see
the sequencing note at the end.

## Context

EP-8 mapped every unit to a service line and deliberately refused to
silently bin the 11 units whose `service` string is literally
`"Unassigned (no public source)"`. Each carries `needs_owner_ruling: true`
in `assets/data/service_lines.json`; the compiler surfaces them as
`meta.data_warnings` (`"service line needs owner ruling: <id> — <service>"`,
`compile.rs:214-219`), visible at the status-bar ⚠ chip and on stderr at
load. The exact list is pinned by the test
`needs_ruling_warnings_match_the_eleven_unassigned_units`
(`hupsim-data/src/compile.rs:1030`).

This is not cosmetic: the 11 units hold **260 of the 1,263 modeled rooms
(~21% of the house)**, all currently rolled into `line.other`. What that
costs today:

- **Sim realism (EP-10, and EP-21's backdating):** LOS draws key off the
  service line (`params.rs::los_for`, table `LOS_BY_SERVICE_LINE` at
  `params.rs:134`). `line.other` draws median 96 h / σ 0.60. If, e.g., the
  three unassigned Pavilion oncology-block wings are really oncology
  (median 150 h), 72 beds are simulating with materially wrong turnover.
- **Placement engine (EP-14):** the service-line score term has weight 1.0
  (of ~2.8 total). Any query naming a line scores flagged units 0 on that
  term. Worse: `stage_from_room` (`query_panel.rs:126-140`) sets a staged
  patient's requested line to **their current unit's line** — a patient
  staged from Silverstein 7 requests `line.other` and gets the other ten
  unsourced units *plus the CHPS research beds* ranked as service matches.
- **Dashboards & report (EP-13/EP-16):** the service-line drawer tab and the
  report's admits/pressure tables carry a fat `line.other` row wearing the
  `[Unverified]` marker; ~21% of the house is unexplained in every demo.
- **NOT affected by a line change:** RN ratios and roster physics (those key
  off `units.json` staffing blocks / level-of-care fallback, not the line).
  Only a `unit_type` change would touch sim physics beyond LOS.

CHPS (`unit.ravdin.6ne.chps`) also lives in `line.other` but is **not part
of this EP** — research beds belong to no clinical line by design, its
identity is verified, and it is deliberately unflagged.

## Decision framework

Each of the 11 resolves into exactly ONE labeled end-state. Every ruling is
an edit to `service_lines.json` (and `units.json` only where noted), always
with a dated note. The EP-8 principle stands: nothing is binned *silently* —
but an explicit, dated, owner-approved inference is not silent.

- **(a) Verified — a public source was found.** Update `units.json`
  (`service` string, `source` with citation + date, `confidence:
  "verified"`, fix `era`), then `service_lines.json` (line, `confidence:
  "verified"` since the string now names the line, drop the flag).
- **(b) Reported — firsthand knowledge.** Reserved for a contributor who has
  direct, current knowledge of the campus (the author does not — every EP-23
  ruling landed in (a), (c), or (d) and this tier was not exercised). Same
  two-file edit; `units.json` gets `confidence: "reported"`, `source:
  "contributor ruling (firsthand knowledge), YYYY-MM-DD"`.
- **(c) Inferred — floor-block adjacency, owner-approved.** The Pavilion
  organizes floors by service block; HUP Main has building-level patterns.
  `units.json` stays honest (service string remains "Unassigned…",
  confidence stays `unverified` — the *identity* is still unsourced);
  `service_lines.json` re-bins with `confidence: "inferred"`, a note naming
  the adjacency argument, `"owner-approved YYYY-MM-DD"`, and drops the flag.
- **(d) Deliberately open.** Stays `line.other`; drop `needs_owner_ruling`
  and add `note: "owner ruling YYYY-MM-DD: deliberately open — no public
  source, no institutional knowledge"`. Rationale: the flag means *awaiting
  ruling*, and "open" IS a ruling; the honesty lives on in the unit's
  `unverified` provenance and the `line.other` `[Unverified]` marker.
  (Alternative: keep the flag so the ⚠ chip persists — defensible, but a
  permanent warning reads as *unfinished* rather than *deliberate*. Pick one
  policy for all (d) units and say why in the commit.)

Mixed outcomes are expected and fine — e.g. three (b), five (c), three (d).
Partial completion is NOT the goal, though: every unit leaves this session
with one of the four labels, even if the label is (d).

**Source hunt before ruling (timebox 30–60 min, evidence beats adjacency):**
Penn Medicine RN/CNA/CNS job requisitions (unit names + floors leak into
postings), pennmedicine.org patient-guide floor directories, the HUP
wayfinding PDFs already cited in provenance, UPHS press coverage of the
Pavilion move-in (floor assignments were published), nursing school
clinical-placement unit lists. A hit upgrades a (c) candidate to (a). After
the timebox, proceed — do not let the hunt stall the session.

## Per-unit worksheets

HUP Main side (all `era: unknown`, telemetry `none`, conservative):

1. **`unit.silverstein.7`** — 22 beds (701–722), med_surg. Adjacency:
   Silverstein 9/11/12 are a medicine block; Silverstein 10 ("former Cardiac
   Surgery") was already ruled medicine-backfill in EP-8 (inferred, unflagged)
   on the vacated-by-Pavilion-move argument. Candidate: `line.medicine` (c).
2. **`unit.silverstein.8`** — 26 beds (801–826), med_surg. Same block logic.
   Candidate: `line.medicine` (c).
3. **`unit.rhoads.2`** — 22 beds (2001–2022), med_surg. Rhoads is documented
   all-surgical post-2021 (3 ENT/plastics, 4 GI/bariatric, 6 general,
   7 uro/colorectal). Candidate: `line.surgery` (c). **Caveat:** low floors
   in older towers are often perioperative/procedural, not inpatient — if
   you believe Rhoads 2 isn't an inpatient unit at all, that is a
   `units.json` roster question (bed count and `kind`), bigger than a line
   edit: take (d) here and file the roster question in the roadmap open
   items rather than guessing.
4. **`unit.rhoads.5`** — 24 beds (5001–5024), med_surg. Same all-surgical
   argument (the JSON already carries this note). Candidate:
   `line.surgery` (c).
5. **`unit.rhoads.10`** — 22 beds (1035–1056), **the level-10 anomaly**:
   the building is documented G–7, yet the wayfinding directory rides these
   rooms on the Rhoads core; the renderer deliberately floats the slab at
   story 10, and `route.rs` already excludes it under the
   unverified-exclusion policy. **Recommend (d) regardless of any service
   guess** — assigning a line asserts the unit is *real*, which is the very
   thing in question. Couple its fate to the physical-anomaly open item: if
   an on-the-ground check says "directory error", the EP-1 archive pattern
   retires the unit entirely; if real, rule its line then.

Pavilion side (all `era: post_pavilion`, telemetry `all`, 24 beds each —
wings are 24-bed thirds of 72-bed floors):

6. **`unit.pavilion.7.center`** — floor 7's City wing is Vascular Surgery
   PCU (verified). Candidate: `line.cardiac` (c) under the heart-&-vascular
   umbrella (matching the EP-8 vascular→cardiac convention).
7. **`unit.pavilion.7.campus`** — same floor block. Candidate:
   `line.cardiac` (c).
8. **`unit.pavilion.8.campus`** — floor 8 City + Center are both Cardiac
   Medicine/Surgery (verified). Strongest adjacency case in the set.
   Candidate: `line.cardiac` (c).
9. **`unit.pavilion.12.center`** — `units.json` notes a dedicated
   **Oncology ICU** exists somewhere in the Pavilion oncology block,
   possibly here. TWO decisions, not one: the line (oncology) AND whether to
   assert `unit_type: "icu"`. The ICU assertion is **the only edit in this
   EP that changes sim physics beyond LOS**: level-of-care hard band in
   placement (EP-14), RN ratio fallback → 2 (roster sizes, workload),
   acuity baselines/volatility (EP-10), the EP-13 level-of-care tab, and
   census seeding. If no source/knowledge settles the ICU question, prefer
   the conservative split: `line.oncology` (c) + `unit_type` left
   `med_surg` + a note that the ICU question stays open.
10. **`unit.pavilion.12.city`** — floor 12 Campus is Medical Oncology
    (verified). Candidate: `line.oncology` (c).
11. **`unit.pavilion.14.city`** — floor 14 Campus + Center are Hematologic
    Oncology (verified). Candidate: `line.oncology` (c).

## In scope

1. **The ruling pass** — walk the 11 worksheets, assign each an end-state,
   apply the edits per the framework above. Every touched entry gets a dated
   note naming its end-state class and rationale (the JSON is the record;
   don't let the reasoning live only in the commit message).
2. **Re-pin the flag test** — `needs_ruling_warnings_match_the_eleven_
   unassigned_units` becomes a pin of the *post-ruling* open set (rename
   accordingly; an empty expected list is a fine and desirable pin). Keep
   pinning exact unit ids, not counts.
3. **Downstream expectation churn** — `cargo test --workspace`; expect
   possible churn ONLY in pinned *expectations* (EP-16 `report_smoke`
   groupings, EP-14 `query_smoke` orderings if scores shift). Re-pin those
   with eyes open; any *behavioral* failure beyond expectation drift means a
   bug — investigate, don't re-pin blindly. If Pavilion 12 Center becomes an
   ICU, additionally re-run the seeded-demo sanity pass (occupancy, roster
   counts in unit view) and check the EP-13 `constrained_icu` test still
   passes untouched.
4. **Docs** — update the three places that state "11": `hupsim/README.md`
   (data-files table, line ~126), `DESIGN.md` (line ~211), and the roadmap
   README "Risks / open items" unsourced-units entry. The ⚠ chip count in
   the running app should match the remaining open set.

## Out of scope

- Taxonomy changes (e.g. splitting a Transplant line out of Surgery — a
  separate one-file decision the EP-8 notes already anticipate).
- Re-binning CHPS, or any already-assigned unit (the EP-8 inferred
  judgments — Dulles 6, Silverstein 10, Founders 8, vascular→cardiac — may
  be *revisited opportunistically* if a ruling here contradicts one, but
  they are not this EP's checklist).
- Resolving the Rhoads level-10 physical anomaly (on-the-ground item).
- Staffing-ratio or capability edits beyond what an explicit ICU ruling
  requires.
- Any UI change — the ⚠ chip, `[Unverified]` marker, and warning plumbing
  already behave correctly for whatever state this EP leaves.

## Verification / acceptance

- `cargo test --workspace` green with the re-pinned open-set test.
- `grep -c needs_owner_ruling assets/data/service_lines.json` matches the
  chosen policy (0 if all (d)-units drop the flag; otherwise exactly the
  deliberately-open count), and the running app's ⚠ chip agrees.
- Every formerly-flagged entry carries a dated ruling note; a cold reader of
  `service_lines.json` alone can reconstruct what was decided and why.
- EP-18 can cite the file as the provenance-discipline exhibit with no
  further work.

## Sequencing note

Run this **after EP-22 and before EP-21**. EP-21's backdating draws each
seeded patient's total LOS from the unit's service line — rulings here
change those draws, so backdating stamps/tests pinned before EP-23 would
need re-pinning after it. EP-17's committed calibration table inherits the
same LOS regime, and EP-18 quotes headline numbers from both. Running EP-23
later is *allowed* (nothing breaks), but it converts one data session into
three re-pinning/recalibration chores. If the owner needs more time to rule,
prefer running the session anyway and taking (d) on the undecided units —
(d) is cheap to upgrade later, and the sequencing cost of *waiting* is
higher than the cost of a provisional "open" ruling.
